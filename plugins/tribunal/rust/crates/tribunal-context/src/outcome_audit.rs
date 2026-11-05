//! `tribunal audit`: what happened to posted findings after the review, as
//! true- and false-positive proxies per mechanism
//! (`references/metrics-schema.md`, "Measurement modes").
//!
//! Rows are selected as `tribunal metrics` selects them, except that a re-run
//! row that posted findings is kept: what it posted is what the PR's author
//! saw. Per PR it asks `gh` two things: `gh pr view` for the state, the merge
//! time, the head and the reviews, and one `gh api repos/<repo>/compare/
//! <reviewed head>...<current head>` per reviewed head, which lists every file
//! changed after that review with its hunks. A finding whose file is among
//! them counts as fixed. One whose file is not, on a merged PR or under a
//! dismissed review at that commit, counts as a false-positive proxy.
//!
//! With `candidates` set it also lists each posted finding on its own: the PR,
//! title, file, line, outcome and the first commit after the review that
//! touches the file, for a maintainer to confirm before the finding becomes a
//! labelled corpus entry. That asks `gh` for one commit at a time, up to
//! [`COMMIT_SCAN_CAP`] per reviewed head.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::exec::Tools;
use crate::metrics_report::{engine_of, of, rate, select, RowFilter, ENGINES};

/// PRs queried when `--limit` is not given.
pub const DEFAULT_LIMIT: usize = 20;

/// PRs queried at once.
const CONCURRENCY: usize = 6;

/// PRs over the limit the text names; `--json` lists them all.
const SKIPPED_SHOWN: usize = 10;

/// Commits per reviewed head that `candidates` fetches one by one.
const COMMIT_SCAN_CAP: usize = 40;

/// GitHub's compare API lists at most this many files.
const COMPARE_FILE_CAP: usize = 300;

pub const CANDIDATES_NOTE: &str = "A candidate is a lead, not a label: a person has to confirm the finding was real before it joins a corpus. The commit is the first non-merge commit after the review that touches the finding's file, and its hunk is compared with the line only when no merge commit changed the file before it and every earlier commit could be read. A finding with no commit was not located within the first 40 commits, or gh failed.";

pub const PROXY_NOTE: &str = "These are proxies, not ground truth: a commit after the review that touches a finding's file is not proof the finding was fixed, and a merge that leaves the file alone is not proof the finding was wrong.";

#[derive(Debug, Clone)]
pub struct AuditArgs {
    pub filter: RowFilter,
    pub limit: usize,
    /// List each posted finding with the commit that first touched its file.
    pub candidates: bool,
}

impl Default for AuditArgs {
    fn default() -> Self {
        Self {
            filter: RowFilter::default(),
            limit: DEFAULT_LIMIT,
            candidates: false,
        }
    }
}

/// The files changed after a reviewed head. Each path maps to the line
/// ranges its hunks cover on the reviewed side, or `None` when GitHub gave no
/// patch (a binary file, or one too large to show).
#[derive(Debug, Clone, Default)]
struct Changed {
    files: HashMap<String, Option<Vec<(u64, u64)>>>,
    /// The commits of the range, oldest first.
    commits: Vec<RangeCommit>,
    /// Per file, the first commit of the range that touched it.
    first_touch: HashMap<String, FirstTouch>,
}

#[derive(Debug, Clone)]
struct RangeCommit {
    sha: String,
    merge: bool,
}

#[derive(Debug, Clone)]
struct FirstTouch {
    sha: String,
    ranges: Option<Vec<(u64, u64)>>,
    /// No earlier commit changed the file and every earlier commit was read,
    /// so its hunks are in the coordinates of the reviewed head.
    exact: bool,
}

#[derive(Debug, Clone, Default)]
struct PrState {
    state: String,
    merged_at: Option<String>,
    head: String,
    /// Commits a review was dismissed at.
    dismissed: BTreeSet<String>,
    after: HashMap<String, Result<Changed, String>>,
}

/// `@@ -a,b +c,d @@` lines of a patch, as `(a, a + b - 1)`. A hunk that only
/// adds lines (`b` is 0) covers the line it follows.
fn hunks(patch: &str) -> Vec<(u64, u64)> {
    patch
        .lines()
        .filter_map(|l| {
            let old = l.strip_prefix("@@ -")?.split(' ').next()?;
            let (start, len) = match old.split_once(',') {
                Some((s, n)) => (s.parse::<u64>().ok()?, n.parse::<u64>().ok()?),
                None => (old.parse::<u64>().ok()?, 1),
            };
            Some((start, start + len.max(1) - 1))
        })
        .collect()
}

fn parse_compare(text: &str) -> Result<Changed, String> {
    let v: Value =
        serde_json::from_str(text).map_err(|e| format!("unreadable compare answer: {e}"))?;
    let status = v.get("status").and_then(Value::as_str).unwrap_or("");
    if matches!(status, "diverged" | "behind") {
        return Err(format!(
            "the branch was rewritten after the review (compare status {status})"
        ));
    }
    let files = v
        .get("files")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    if files.len() >= COMPARE_FILE_CAP {
        return Err(format!(
            "{} or more files changed after the review, and GitHub lists only {COMPARE_FILE_CAP}",
            files.len()
        ));
    }
    let commits = v
        .get("commits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| {
            Some(RangeCommit {
                sha: c.get("sha").and_then(Value::as_str)?.to_owned(),
                merge: c
                    .get("parents")
                    .and_then(Value::as_array)
                    .is_some_and(|p| p.len() > 1),
            })
        })
        .collect();
    let mut changed = Changed {
        commits,
        ..Changed::default()
    };
    for f in files {
        let ranges = f.get("patch").and_then(Value::as_str).map(hunks);
        for key in ["filename", "previous_filename"] {
            if let Some(name) = f.get(key).and_then(Value::as_str) {
                changed.files.insert(name.to_owned(), ranges.clone());
            }
        }
    }
    Ok(changed)
}

fn parse_pr(text: &str) -> Result<PrState, String> {
    let v: Value =
        serde_json::from_str(text).map_err(|e| format!("unreadable gh pr view answer: {e}"))?;
    let head = v
        .get("headRefOid")
        .and_then(Value::as_str)
        .filter(|h| !h.is_empty())
        .ok_or("gh pr view gave no headRefOid")?;
    let dismissed = v
        .get("reviews")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|r| r.get("state").and_then(Value::as_str) == Some("DISMISSED"))
        .filter_map(|r| r.pointer("/commit/oid").and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    Ok(PrState {
        state: v
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        merged_at: v.get("mergedAt").and_then(Value::as_str).map(str::to_owned),
        head: head.to_owned(),
        dismissed,
        after: HashMap::new(),
    })
}

fn failure(what: &str, out: &crate::exec::Output) -> String {
    let why = out.stderr.lines().next().unwrap_or("").trim();
    format!("{what} exited {:?}: {why}", out.code)
}

/// Fills `first_touch` for the `wanted` files the range changed, reading the
/// commits oldest first until each has one. Returns the calls made. A merge
/// commit is read but never taken as the first touch, since what it brings in
/// is not the author's change; if it changed a wanted file, that file's first
/// touch is inexact, and so is every one after a commit gh could not answer
/// for or whose file list GitHub cut short.
async fn locate_first_touch(
    tools: &Tools,
    repo: &str,
    changed: &mut Changed,
    wanted: &BTreeSet<String>,
) -> usize {
    let mut pending: BTreeSet<String> = wanted
        .iter()
        .filter(|f| changed.files.contains_key(*f))
        .cloned()
        .collect();
    let commits = changed.commits.clone();
    let (mut calls, mut unsure) = (0, false);
    let mut by_merge: BTreeSet<String> = BTreeSet::new();
    for c in commits.iter().take(COMMIT_SCAN_CAP) {
        if pending.is_empty() {
            break;
        }
        calls += 1;
        let path = format!("repos/{repo}/commits/{}", c.sha);
        let answer = match tools.gh(&["api", &path], None).await {
            Ok(out) if out.ok() => serde_json::from_str::<Value>(&out.stdout).ok(),
            _ => None,
        };
        let Some(v) = answer else {
            unsure = true;
            continue;
        };
        let files = v
            .get("files")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        unsure |= files.len() >= COMPARE_FILE_CAP;
        for f in files {
            let ranges = f.get("patch").and_then(Value::as_str).map(hunks);
            for key in ["filename", "previous_filename"] {
                let Some(name) = f.get(key).and_then(Value::as_str) else {
                    continue;
                };
                if !pending.contains(name) {
                    continue;
                }
                if c.merge {
                    by_merge.insert(name.to_owned());
                    continue;
                }
                pending.remove(name);
                changed.first_touch.insert(
                    name.to_owned(),
                    FirstTouch {
                        sha: c.sha.clone(),
                        ranges: ranges.clone(),
                        exact: !unsure && !by_merge.contains(name),
                    },
                );
            }
        }
    }
    calls
}

/// The calls for one PR, and how many were made. `wanted` is the files whose
/// first touching commit is wanted, when candidates were asked for.
async fn fetch_pr(
    tools: Tools,
    repo: String,
    pr: u64,
    shas: Vec<String>,
    wanted: Option<BTreeSet<String>>,
) -> (Result<PrState, String>, usize) {
    let n = pr.to_string();
    let view = [
        "pr",
        "view",
        &n,
        "--repo",
        &repo,
        "--json",
        "state,mergedAt,headRefOid,reviews",
    ];
    let mut st = match tools.gh(&view, None).await {
        Ok(o) if o.ok() => match parse_pr(&o.stdout) {
            Ok(s) => s,
            Err(e) => return (Err(e), 1),
        },
        Ok(o) => {
            return (
                Err(failure(&format!("gh pr view {n} --repo {repo}"), &o)),
                1,
            )
        }
        Err(e) => return (Err(format!("cannot run gh: {e}")), 1),
    };
    let mut calls = 1;
    for sha in shas {
        if sha == st.head {
            st.after.insert(sha, Ok(Changed::default()));
            continue;
        }
        let path = format!("repos/{repo}/compare/{sha}...{}", st.head);
        calls += 1;
        let r = match tools.gh(&["api", &path], None).await {
            Ok(o) if o.ok() => parse_compare(&o.stdout),
            Ok(o) => Err(failure(&format!("gh api {path}"), &o)),
            Err(e) => Err(format!("cannot run gh: {e}")),
        };
        let r = match (r, &wanted) {
            (Ok(mut changed), Some(files)) => {
                calls += locate_first_touch(&tools, &repo, &mut changed, files).await;
                Ok(changed)
            }
            (r, _) => r,
        };
        st.after.insert(sha, r);
    }
    (Ok(st), calls)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    Fixed,
    Dismissed,
    MergedUntouched,
    OpenUntouched,
    ClosedUnmerged,
    Unknown,
}

const OUTCOMES: [(Outcome, &str); 6] = [
    (Outcome::Fixed, "fixed"),
    (Outcome::Dismissed, "dismissed"),
    (Outcome::MergedUntouched, "mergedUntouched"),
    (Outcome::OpenUntouched, "openUntouched"),
    (Outcome::ClosedUnmerged, "closedUnmerged"),
    (Outcome::Unknown, "unknown"),
];

/// Whether the finding's file changed after the review at `sha`, and whether
/// its line sits inside a changed hunk (`None` when that cannot be told).
fn touched(f: &Value, sha: &str, st: &PrState) -> Result<(bool, Option<bool>), String> {
    let file = f
        .get("file")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("the finding names no file")?;
    let changed = st
        .after
        .get(sha)
        .ok_or("the reviewed head was not compared")?
        .as_ref()
        .map_err(Clone::clone)?;
    let Some(ranges) = changed.files.get(file) else {
        return Ok((false, Some(false)));
    };
    let line = f.get("line").and_then(Value::as_u64);
    let hit = match (line, ranges) {
        (Some(l), Some(r)) => Some(r.iter().any(|(a, b)| (*a..=*b).contains(&l))),
        _ => None,
    };
    Ok((true, hit))
}

fn classify(f: &Value, sha: &str, st: &PrState) -> (Outcome, Option<bool>) {
    match touched(f, sha, st) {
        Err(_) => (Outcome::Unknown, None),
        Ok((true, hit)) => (Outcome::Fixed, hit),
        Ok((false, _)) if st.dismissed.contains(sha) => (Outcome::Dismissed, None),
        Ok((false, _)) => match st.state.as_str() {
            "MERGED" => (Outcome::MergedUntouched, None),
            "OPEN" => (Outcome::OpenUntouched, None),
            "CLOSED" => (Outcome::ClosedUnmerged, None),
            _ => (Outcome::Unknown, None),
        },
    }
}

/// The mechanism buckets a posted finding falls in, in report order.
pub const BUCKETS: [&str; 10] = [
    "all posted",
    "blocking",
    "not blocking",
    "not paneled",
    "paneled, external voter",
    "paneled, Claude only",
    "paneled, no debate",
    "debate fired",
    "debate flipped",
    "debate held",
];

fn buckets_of(f: &Value) -> Vec<&'static str> {
    let yes = |v: &Value, k: &str| v.get(k) == Some(&Value::Bool(true));
    let mut b = vec![BUCKETS[0]];
    b.push(if yes(f, "blocking") {
        BUCKETS[1]
    } else {
        BUCKETS[2]
    });
    if !yes(f, "panelRan") {
        b.push(BUCKETS[3]);
        return b;
    }
    b.push(if yes(f, "hadExternalVoter") {
        BUCKETS[4]
    } else {
        BUCKETS[5]
    });
    match f.get("debate").filter(|d| yes(d, "fired")) {
        None => b.push(BUCKETS[6]),
        Some(d) => {
            b.push(BUCKETS[7]);
            match d.get("outcome").and_then(Value::as_str) {
                Some("flipped") => b.push(BUCKETS[8]),
                Some("held") => b.push(BUCKETS[9]),
                _ => {}
            }
        }
    }
    b
}

#[derive(Default)]
struct Tally {
    findings: usize,
    outcomes: BTreeMap<Outcome, usize>,
    line_known: usize,
    line_hit: usize,
}

impl Tally {
    fn add(&mut self, o: Outcome, hit: Option<bool>) {
        self.findings += 1;
        *self.outcomes.entry(o).or_default() += 1;
        if o == Outcome::Fixed {
            if let Some(h) = hit {
                self.line_known += 1;
                if h {
                    self.line_hit += 1;
                }
            }
        }
    }

    fn get(&self, o: Outcome) -> usize {
        self.outcomes.get(&o).copied().unwrap_or(0)
    }

    fn to_value(&self, bucket: &str) -> Value {
        let fixed = self.get(Outcome::Fixed);
        let decided = fixed + self.get(Outcome::Dismissed) + self.get(Outcome::MergedUntouched);
        let mut v = json!({"bucket": bucket, "findings": self.findings});
        for (o, k) in OUTCOMES {
            v[k] = json!(self.get(o));
        }
        v["fixedLineInHunk"] = json!(self.line_hit);
        v["fixedLineKnown"] = json!(self.line_known);
        v["decided"] = json!(decided);
        v["fixedShareOfDecided"] = rate(fixed, decided);
        v
    }
}

struct Pr<'a> {
    repo: String,
    pr: u64,
    latest: String,
    rows: Vec<&'a Value>,
}

fn pr_number(row: &Value) -> Option<u64> {
    match row.get("pr")? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// One posted finding as a lead for a labelled corpus.
fn candidate(
    p: &Pr,
    st: &Result<PrState, String>,
    engine: &str,
    f: &Value,
    sha: &str,
    outcome: Outcome,
    hit: Option<bool>,
) -> Value {
    let s = st.as_ref().ok();
    let file = f.get("file").and_then(Value::as_str).unwrap_or("");
    let line = f.get("line").and_then(Value::as_u64);
    let touch = s
        .and_then(|s| s.after.get(sha))
        .and_then(|r| r.as_ref().ok())
        .and_then(|c| c.first_touch.get(file));
    let first = touch.map(|t| {
        let covers = match (line, &t.ranges) {
            (Some(l), Some(r)) if t.exact => Some(r.iter().any(|(a, b)| (*a..=*b).contains(&l))),
            _ => None,
        };
        json!({"commit": t.sha, "hunkCoversLine": covers, "exact": t.exact})
    });
    json!({
        "repo": p.repo,
        "pr": p.pr,
        "state": s.map(|s| s.state.clone()),
        "engine": engine,
        "reviewedHead": sha,
        "title": f.get("title"),
        "file": f.get("file"),
        "line": f.get("line"),
        "severity": f.get("severity"),
        "blocking": f.get("blocking") == Some(&Value::Bool(true)),
        "outcome": OUTCOMES.iter().find(|(o, _)| *o == outcome).map(|(_, k)| *k),
        "lineInChangedHunk": hit,
        "firstTouch": first,
    })
}

fn posted(row: &Value) -> impl Iterator<Item = &Value> {
    row.get("findings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|f| f.get("posted") == Some(&Value::Bool(true)))
}

fn ts(row: &Value) -> &str {
    row.get("ts").and_then(Value::as_str).unwrap_or("")
}

/// The audit over `rows`, asking `gh` through `tools`.
#[allow(clippy::too_many_lines)]
pub async fn audit(tools: &Tools, rows: &[Value], unreadable: usize, args: &AuditArgs) -> Value {
    let sel = select(rows, unreadable, &args.filter);
    let posted_reruns: Vec<&Value> = sel
        .reruns
        .iter()
        .copied()
        .filter(|r| posted(r).next().is_some())
        .collect();
    let mut no_identity = 0;
    let mut prs: BTreeMap<(String, u64), Pr> = BTreeMap::new();
    let candidates = sel
        .groups
        .iter()
        .flat_map(|g| g.rows.iter().copied())
        .chain(posted_reruns.iter().copied())
        .filter(|r| posted(r).next().is_some());
    for row in candidates {
        let repo = row
            .get("repo")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let sha = row
            .get("headSha")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let (Some(repo), Some(pr), Some(_)) = (repo, pr_number(row), sha) else {
            no_identity += 1;
            continue;
        };
        let e = prs.entry((repo.to_owned(), pr)).or_insert_with(|| Pr {
            repo: repo.to_owned(),
            pr,
            latest: String::new(),
            rows: Vec::new(),
        });
        if ts(row) > e.latest.as_str() {
            e.latest = ts(row).to_owned();
        }
        e.rows.push(row);
    }
    let mut prs: Vec<Pr> = prs.into_values().collect();
    prs.sort_by(|a, b| {
        b.latest
            .cmp(&a.latest)
            .then_with(|| (&a.repo, a.pr).cmp(&(&b.repo, b.pr)))
    });
    let with_posted = prs.len();
    let skipped: Vec<String> = prs
        .iter()
        .skip(args.limit)
        .map(|p| format!("{}#{}", p.repo, p.pr))
        .collect();
    prs.truncate(args.limit);
    for p in &mut prs {
        p.rows.sort_by(|a, b| ts(a).cmp(ts(b)));
    }

    let sem = Arc::new(tokio::sync::Semaphore::new(CONCURRENCY));
    let handles: Vec<_> = prs
        .iter()
        .map(|p| {
            let shas: Vec<String> = p
                .rows
                .iter()
                .filter_map(|r| r.get("headSha").and_then(Value::as_str))
                .map(str::to_owned)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let wanted = args.candidates.then(|| {
                p.rows
                    .iter()
                    .flat_map(|r| posted(r))
                    .filter_map(|f| f.get("file").and_then(Value::as_str))
                    .filter(|f| !f.is_empty())
                    .map(str::to_owned)
                    .collect::<BTreeSet<_>>()
            });
            let (tools, repo, pr, sem) = (tools.clone(), p.repo.clone(), p.pr, sem.clone());
            tokio::spawn(async move {
                let _permit = sem.acquire_owned().await;
                fetch_pr(tools, repo, pr, shas, wanted).await
            })
        })
        .collect();
    let mut states = Vec::with_capacity(handles.len());
    let mut gh_calls = 0;
    for h in handles {
        let (st, calls) = h
            .await
            .unwrap_or_else(|e| (Err(format!("the gh task failed: {e}")), 0));
        gh_calls += calls;
        states.push(st);
    }

    let mut per_engine: BTreeMap<usize, (BTreeMap<&str, Tally>, usize, Tally)> = BTreeMap::new();
    let mut per_pr = Vec::new();
    let mut leads = Vec::new();
    let mut gh_failures = 0;
    for (p, st) in prs.iter().zip(&states) {
        let mut seen = BTreeSet::new();
        let mut pr_tally = Tally::default();
        let mut engines = BTreeSet::new();
        let mut errors: BTreeSet<String> = BTreeSet::new();
        for row in &p.rows {
            let engine = engine_of(row);
            let rank = ENGINES.iter().position(|e| *e == engine).unwrap_or(0);
            engines.insert(rank);
            let sha = row.get("headSha").and_then(Value::as_str).unwrap_or("");
            let slot = per_engine.entry(rank).or_default();
            for f in row
                .get("findings")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let id = (
                    f.get("file").and_then(Value::as_str).unwrap_or(""),
                    f.get("title").and_then(Value::as_str).unwrap_or(""),
                );
                let (outcome, hit) = match st {
                    Ok(s) => {
                        if let Some(Err(e)) = s.after.get(sha) {
                            errors.insert(e.clone());
                        }
                        classify(f, sha, s)
                    }
                    Err(_) => (Outcome::Unknown, None),
                };
                let is_posted = f.get("posted") == Some(&Value::Bool(true));
                let killed = f.get("panelRan") == Some(&Value::Bool(true))
                    && f.get("survived") == Some(&Value::Bool(false));
                if !(is_posted || killed) || !seen.insert((is_posted, id)) {
                    if is_posted {
                        slot.1 += 1;
                    }
                    continue;
                }
                if is_posted {
                    if args.candidates {
                        leads.push(candidate(p, st, engine, f, sha, outcome, hit));
                    }
                    pr_tally.add(outcome, hit);
                    for b in buckets_of(f) {
                        slot.0.entry(b).or_default().add(outcome, hit);
                    }
                } else {
                    slot.2.add(outcome, hit);
                }
            }
        }
        if let Err(e) = st {
            gh_failures += 1;
            errors.insert(e.clone());
        }
        let mut entry = json!({
            "repo": p.repo,
            "pr": p.pr,
            "engines": engines.iter().map(|r| ENGINES[*r]).collect::<Vec<_>>(),
            "reviews": p.rows.len(),
            "posted": pr_tally.findings,
        });
        if let Ok(s) = st {
            entry["state"] = json!(s.state);
            entry["mergedAt"] = json!(s.merged_at);
        }
        for (o, k) in OUTCOMES {
            entry[k] = json!(pr_tally.get(o));
        }
        entry["errors"] = json!(errors);
        per_pr.push(entry);
    }

    let groups: Vec<Value> = per_engine
        .into_iter()
        .map(|(rank, (buckets, dupes, killed))| {
            json!({
                "engine": ENGINES[rank],
                "posted": buckets.get(BUCKETS[0]).map_or(0, |t| t.findings),
                "duplicatesMerged": dupes,
                "buckets": BUCKETS.iter().filter_map(|b| buckets.get(b).map(|t| t.to_value(b))).collect::<Vec<_>>(),
                "killed": {
                    "findings": killed.findings,
                    "fileTouchedAfter": killed.get(Outcome::Fixed),
                    "unknown": killed.get(Outcome::Unknown),
                    "untouched": killed.findings - killed.get(Outcome::Fixed) - killed.get(Outcome::Unknown),
                },
            })
        })
        .collect();

    let mut report = json!({
        "note": PROXY_NOTE,
        "rows": sel.counts(),
        "headline": sel.headline(),
        "postedReruns": posted_reruns.len(),
        "prs": {
            "withPostedFindings": with_posted,
            "queried": prs.len(),
            "limit": args.limit,
            "skippedOverLimit": skipped,
            "rowsWithoutIdentity": no_identity,
            "ghCalls": gh_calls,
            "ghFailures": gh_failures,
        },
        "perPr": per_pr,
        "groups": groups,
    });
    if args.candidates {
        report["candidatesNote"] = json!(CANDIDATES_NOTE);
        report["candidates"] = json!(leads);
    }
    report
}

fn u(v: &Value) -> usize {
    v.as_u64()
        .and_then(|x| usize::try_from(x).ok())
        .unwrap_or(0)
}

const LABELS: [(&str, &str); 6] = [
    ("fixed", "fixed"),
    ("dismissed", "dismissed"),
    ("mergedUntouched", "merged untouched"),
    ("openUntouched", "open untouched"),
    ("closedUnmerged", "closed unmerged"),
    ("unknown", "unknown"),
];

/// The audit as text for a terminal.
#[must_use]
pub fn render(report: &Value) -> String {
    let mut s =
        String::from("tribunal audit: what happened to posted findings after the review.\n");
    s += &format!("{}\n", report["note"].as_str().unwrap_or(""));
    s += &format!("Rows: {}", report["headline"].as_str().unwrap_or(""));
    let reruns = u(&report["postedReruns"]);
    if reruns > 0 {
        s += &format!("; {reruns} re-run rows kept because they posted findings");
    }
    s += ".\n";
    let p = &report["prs"];
    s += &format!(
        "PRs: {} with posted findings, {} queried (--limit {})",
        p["withPostedFindings"], p["queried"], p["limit"]
    );
    let skipped: Vec<&str> = p["skippedOverLimit"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !skipped.is_empty() {
        let more = skipped.len().saturating_sub(SKIPPED_SHOWN);
        s += &format!(
            ", {} not queried over the limit (older reviews): {}{}",
            skipped.len(),
            skipped[..skipped.len().min(SKIPPED_SHOWN)].join(", "),
            if more > 0 {
                format!(" and {more} more (--json lists them all)")
            } else {
                String::new()
            }
        );
    }
    s += ".\n";
    if u(&p["rowsWithoutIdentity"]) > 0 {
        s += &format!(
            "{} rows with posted findings were skipped: they lack repo, pr or headSha.\n",
            p["rowsWithoutIdentity"]
        );
    }
    s += &format!(
        "gh calls: {}; PRs gh could not answer: {}.\n",
        p["ghCalls"], p["ghFailures"]
    );
    let per_pr = report["perPr"].as_array().cloned().unwrap_or_default();
    if per_pr.is_empty() {
        s += "No PR with posted findings matches.\n";
        return s;
    }
    s += "\nPer PR (most recent review first):\n";
    for e in &per_pr {
        let state = match (e["state"].as_str(), e["mergedAt"].as_str()) {
            (Some(st), Some(at)) => format!("{st} {at}"),
            (Some(st), None) => st.to_owned(),
            _ => "state unknown".to_owned(),
        };
        let parts: Vec<String> = LABELS
            .iter()
            .filter(|(k, _)| u(&e[*k]) > 0)
            .map(|(k, l)| format!("{} {l}", e[*k]))
            .collect();
        s += &format!(
            "  {}#{} {state}: {} posted{}{}\n",
            e["repo"].as_str().unwrap_or("?"),
            e["pr"],
            e["posted"],
            if parts.is_empty() {
                String::new()
            } else {
                format!(": {}", parts.join(", "))
            },
            if e["reviews"].as_u64().unwrap_or(0) > 1 {
                format!(" ({} reviews)", e["reviews"])
            } else {
                String::new()
            }
        );
        for err in e["errors"].as_array().into_iter().flatten() {
            s += &format!("    {}\n", err.as_str().unwrap_or(""));
        }
    }
    for g in report["groups"].as_array().into_iter().flatten() {
        s += &format!(
            "\n{}: {} posted findings ({} repeats across re-reviews counted once)\n",
            g["engine"].as_str().unwrap_or("?"),
            g["posted"],
            g["duplicatesMerged"]
        );
        let line = |c: [String; 9]| {
            format!(
                "  {:<24} {:>8} {:>6} {:>9} {:>8} {:>6} {:>8} {:>7}  {}\n",
                c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7], c[8]
            )
        };
        s += &line(
            [
                "bucket",
                "findings",
                "fixed",
                "dismissed",
                "merged",
                "open",
                "closed",
                "unknown",
                "fixed of decided",
            ]
            .map(str::to_owned),
        );
        for b in g["buckets"].as_array().into_iter().flatten() {
            s += &line([
                b["bucket"].as_str().unwrap_or("?").to_owned(),
                b["findings"].to_string(),
                b["fixed"].to_string(),
                b["dismissed"].to_string(),
                b["mergedUntouched"].to_string(),
                b["openUntouched"].to_string(),
                b["closedUnmerged"].to_string(),
                b["unknown"].to_string(),
                if u(&b["decided"]) == 0 {
                    "-".to_owned()
                } else {
                    of(u(&b["fixed"]), u(&b["decided"]))
                },
            ]);
        }
        if let Some(all) = g["buckets"].as_array().and_then(|b| b.first()) {
            if u(&all["fixed"]) > 0 {
                s += &format!(
                    "  Fixed findings whose line sits in a hunk changed after the review: {} with a known line\n",
                    of(u(&all["fixedLineInHunk"]), u(&all["fixedLineKnown"]))
                );
            }
        }
        let k = &g["killed"];
        if u(&k["findings"]) > 0 {
            s += &format!(
                "  False-negative proxy: killed findings whose file changed after the review: {} ({} unknown)\n",
                of(u(&k["fileTouchedAfter"]), u(&k["findings"]) - u(&k["unknown"])),
                k["unknown"]
            );
        }
    }
    s += "\nfixed: a commit after the review touched the finding's file (true-positive proxy). dismissed: untouched, and a review at that commit was dismissed. merged: the PR merged without touching the file (false-positive proxy, as is dismissed). open and closed: untouched on a PR still open, or closed without merging. unknown: gh failed, the branch was rewritten after the review, or the finding names no file.\n";
    if let Some(list) = report["candidates"].as_array() {
        s += &render_candidates(list, report["candidatesNote"].as_str().unwrap_or(""));
    }
    s
}

/// How strongly a candidate points at a real finding, for ordering: a fix
/// whose first commit changed the finding's own line comes first.
fn lead_rank(c: &Value) -> u8 {
    match (
        c["outcome"].as_str(),
        c["firstTouch"]["hunkCoversLine"].as_bool(),
    ) {
        (Some("fixed"), Some(true)) => 0,
        (Some("fixed"), _) => 1,
        (Some("dismissed" | "mergedUntouched"), _) => 2,
        _ => 3,
    }
}

fn render_candidates(list: &[Value], note: &str) -> String {
    let mut s = format!(
        "
Candidates for a labelled corpus: {} posted findings.\n{note}\n",
        list.len()
    );
    let mut sorted: Vec<&Value> = list.iter().collect();
    sorted.sort_by_key(|c| lead_rank(c));
    for c in sorted {
        let outcome = c["outcome"].as_str().unwrap_or("unknown");
        let label = LABELS
            .iter()
            .find(|(k, _)| *k == outcome)
            .map_or(outcome, |(_, l)| *l);
        let how = match (outcome, c["firstTouch"]["commit"].as_str()) {
            ("fixed", Some(sha)) => {
                let sha = &sha[..sha.len().min(8)];
                match c["firstTouch"]["hunkCoversLine"].as_bool() {
                    Some(true) => format!("fixed by {sha}, its hunk covers the line"),
                    Some(false) => format!("fixed by {sha}, line outside its hunk"),
                    None => format!("fixed by {sha}, line not comparable"),
                }
            }
            ("fixed", None) => "fixed, first commit not located".to_owned(),
            _ => label.to_owned(),
        };
        s += &format!(
            "  {}#{} {}:{} [{}] {:?}: {how}\n",
            c["repo"].as_str().unwrap_or("?"),
            c["pr"],
            c["file"].as_str().unwrap_or("?"),
            c["line"]
                .as_u64()
                .map_or_else(|| "?".to_owned(), |l| l.to_string()),
            if c["blocking"] == true {
                "blocking"
            } else {
                "not blocking"
            },
            c["title"].as_str().unwrap_or(""),
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunk_ranges_on_the_reviewed_side() {
        assert_eq!(
            hunks("@@ -10,3 +10,4 @@ fn x\n ctx\n+add\n@@ -40 +41,2 @@\n@@ -7,0 +8,2 @@"),
            [(10, 12), (40, 40), (7, 7)]
        );
        assert_eq!(hunks("no header"), []);
    }

    #[test]
    fn a_rewritten_branch_is_unknown_not_fixed() {
        let e = parse_compare(r#"{"status": "diverged", "files": [{"filename": "a.ts"}]}"#)
            .unwrap_err();
        assert!(e.contains("rewritten"), "{e}");
        let c = parse_compare(
            r#"{"status": "ahead", "files": [{"filename": "b.ts", "previous_filename": "a.ts", "patch": "@@ -1,2 +1,2 @@"}]}"#,
        )
        .unwrap();
        assert_eq!(c.files.get("a.ts"), Some(&Some(vec![(1, 2)])));
        assert!(c.files.contains_key("b.ts"));
    }

    #[test]
    fn mechanism_buckets() {
        let f = json!({"blocking": true, "panelRan": true, "hadExternalVoter": true,
            "debate": {"fired": true, "outcome": "flipped"}});
        assert_eq!(
            buckets_of(&f),
            [
                "all posted",
                "blocking",
                "paneled, external voter",
                "debate fired",
                "debate flipped"
            ]
        );
        let nit = json!({"blocking": false, "panelRan": false});
        assert_eq!(
            buckets_of(&nit),
            ["all posted", "not blocking", "not paneled"]
        );
    }
}
