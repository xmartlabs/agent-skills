//! `tribunal corpus collect`: candidate corpus entries from a repository's
//! inline review comments whose point the author acted on.
//!
//! Two kinds of source, any number of each:
//!
//! - a marker source ([`CommentSource`]): a reviewer that writes into its own
//!   comment which commit addressed it ("Addressed in commit 3a0af6f");
//! - [`LINE_CHANGED`]: any inline comment, by a person or a bot, whose
//!   commented lines a later commit of the pull request changed. The fix is
//!   the first such commit, found by following the lines through each commit's
//!   diff.
//!
//! It lists the comments with `gh api`, resolves the commits in a local
//! checkout, and writes one candidate entry per pull request, pinned at the
//! commit where most of its items hold. No model runs. A candidate is a lead:
//! a person confirms the defect before it is a label.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Value};

use super::source::{is_comment_line, CommentSource};
use crate::exec::Tools;

/// Comments read when `--max-comments` is not given.
pub const DEFAULT_MAX_COMMENTS: usize = 1000;
/// The source that needs no marker.
pub const LINE_CHANGED: &str = "line-changed";
/// The sources `tribunal corpus collect` uses when `--source` is not given.
pub const DEFAULT_SOURCES: &[&str] = &["coderabbit", LINE_CHANGED];
const PAGE: usize = 100;
/// A quote shorter than this matches too many lines to anchor anything.
const MIN_QUOTE: usize = 12;
const DESCRIPTION_CAP: usize = 700;
const MARKED: &str = "reviewer-addressed";

#[derive(Debug, Clone)]
pub struct CollectArgs {
    pub repo: String,
    pub checkout: PathBuf,
    pub out: PathBuf,
    /// Marker sources, tried in order.
    pub markers: Vec<CommentSource>,
    /// Also keep comments whose lines a later commit changed.
    pub line_changed: bool,
    /// Comments created before this ISO date or instant are not read.
    pub since: Option<String>,
    pub max_comments: usize,
}

/// One kept comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub id: u64,
    pub pr: u64,
    pub path: String,
    pub start_line: Option<u64>,
    pub line: u64,
    /// The commit the comment was made on.
    pub commit: String,
    /// The fix commit a marker names, often abbreviated. `None` for a
    /// [`LINE_CHANGED`] comment, whose fix is found in the history.
    pub fix: Option<String>,
    pub author: String,
    pub bot: bool,
    /// [`MARKED`] or [`LINE_CHANGED`].
    pub source: &'static str,
    pub title: String,
    pub text: String,
    pub reviewer_severity: Option<String>,
    pub url: String,
    pub created_at: String,
}

/// What the listing read and why each comment it did not keep was left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    pub comments: Vec<Comment>,
    pub read: usize,
    pub dropped: BTreeMap<String, usize>,
}

fn count(map: &mut BTreeMap<String, usize>, reason: &str) {
    *map.entry(reason.to_owned()).or_default() += 1;
}

fn pr_of(url: &str) -> Option<u64> {
    url.rsplit('/').next()?.parse().ok()
}

/// Reads a comment of no marker source: its first line is its title.
fn plain_reader() -> CommentSource {
    CommentSource {
        name: LINE_CHANGED.to_owned(),
        reviewer: String::new(),
        marker: "\u{0}".to_owned(),
    }
}

fn read_comment(
    c: &Value,
    markers: &[CommentSource],
    line_changed: bool,
) -> Result<Comment, &'static str> {
    if !c["in_reply_to_id"].is_null() {
        return Err("reply");
    }
    let author = c["user"]["login"].as_str().unwrap_or("");
    let bot = c["user"]["type"] == "Bot" || author.ends_with("[bot]");
    let body = c["body"].as_str().unwrap_or("");
    let own = markers.iter().find(|m| m.reviewer == author);
    let marked = own.and_then(|m| m.addressed_fix(body).map(|f| (f, m)));
    let (fix, source, reader) = match marked {
        Some((f, m)) => (Some(f), MARKED, m.clone()),
        None if line_changed => (
            None,
            LINE_CHANGED,
            own.cloned().unwrap_or_else(plain_reader),
        ),
        None => {
            return Err(match own {
                Some(m) if body.contains(&m.marker) => "no-fix-commit",
                Some(_) => "not-addressed",
                None => "other-author",
            })
        }
    };
    let pr = c["pull_request_url"]
        .as_str()
        .and_then(pr_of)
        .ok_or("no-pull-request")?;
    let line = c["original_line"]
        .as_u64()
        .or_else(|| c["line"].as_u64())
        .ok_or("no-line")?;
    let commit = c["original_commit_id"]
        .as_str()
        .or_else(|| c["commit_id"].as_str())
        .ok_or("no-commit")?;
    let path = c["path"].as_str().ok_or("no-path")?;
    let parsed = reader.parse_comment(body);
    Ok(Comment {
        id: c["id"].as_u64().unwrap_or(0),
        pr,
        path: path.to_owned(),
        start_line: c["original_start_line"]
            .as_u64()
            .or_else(|| c["start_line"].as_u64()),
        line,
        commit: commit.to_owned(),
        fix,
        author: author.to_owned(),
        bot,
        source,
        title: parsed.title,
        text: parsed.text,
        reviewer_severity: parsed.reviewer_severity,
        url: c["html_url"].as_str().unwrap_or("").to_owned(),
        created_at: c["created_at"].as_str().unwrap_or("").to_owned(),
    })
}

/// Lists the repository's inline review comments, newest first, until
/// `since` or `max` is reached, and keeps those of the sources.
///
/// # Errors
/// When `gh` cannot be run, fails, or answers with something that is not a
/// list of comments.
pub async fn list_comments(
    tools: &Tools,
    repo: &str,
    markers: &[CommentSource],
    line_changed: bool,
    since: Option<&str>,
    max: usize,
) -> Result<Listing, String> {
    let mut listing = Listing::default();
    let path = format!("repos/{repo}/pulls/comments");
    'pages: for page in 1.. {
        let page = page.to_string();
        let per_page = PAGE.to_string();
        let out = tools
            .gh(
                &[
                    "api",
                    "-X",
                    "GET",
                    &path,
                    "-f",
                    "sort=created",
                    "-f",
                    "direction=desc",
                    "-f",
                    &format!("per_page={per_page}"),
                    "-f",
                    &format!("page={page}"),
                ],
                None,
            )
            .await
            .map_err(|e| format!("cannot run gh: {e}"))?;
        if !out.ok() {
            return Err(format!(
                "gh api {path} page {page} failed: {}",
                out.stderr.trim()
            ));
        }
        let items: Vec<Value> = serde_json::from_str(&out.stdout)
            .map_err(|e| format!("gh api {path} page {page} is not a list of comments: {e}"))?;
        if items.is_empty() {
            break;
        }
        for c in &items {
            if listing.read >= max {
                break 'pages;
            }
            let created = c["created_at"].as_str().unwrap_or("");
            if since.is_some_and(|s| created < s) {
                break 'pages;
            }
            listing.read += 1;
            match read_comment(c, markers, line_changed) {
                Ok(comment) => listing.comments.push(comment),
                Err(reason) => count(&mut listing.dropped, reason),
            }
        }
        if items.len() < PAGE {
            break;
        }
    }
    Ok(listing)
}

/// `(old start, old count, new count)` of each hunk of a `-U0` diff.
fn hunks(diff: &str) -> Vec<(i64, i64, i64)> {
    static HUNK: OnceLock<Regex> = OnceLock::new();
    let re = HUNK.get_or_init(|| {
        Regex::new(r"(?m)^@@ -(\d+)(?:,(\d+))? \+\d+(?:,(\d+))? @@").expect("a fixed pattern")
    });
    let n = |m: Option<regex::Match>| m.map_or(1, |m| m.as_str().parse().unwrap_or(1));
    re.captures_iter(diff)
        .map(|c| (n(c.get(1)), n(c.get(2)), n(c.get(3))))
        .collect()
}

/// Whether a hunk changes lines `a..=b` of the old file and, when it does
/// not, how far it moves them. A hunk with no old lines inserts after its
/// start line.
fn hit_or_shift(h: (i64, i64, i64), a: i64, b: i64) -> Result<i64, ()> {
    let (s, old, new) = h;
    let hit = if old == 0 {
        a <= s && s < b
    } else {
        s <= b && s + old > a
    };
    if hit {
        return Err(());
    }
    let before = if old == 0 { s < a } else { s + old - 1 < a };
    Ok(if before { new - old } else { 0 })
}

/// The checkout, through `git`.
struct Repo<'a> {
    tools: &'a Tools,
    dir: &'a Path,
}

impl Repo<'_> {
    async fn git(&self, args: &[&str]) -> Option<String> {
        let out = self.tools.git(args, Some(self.dir)).await.ok()?;
        out.ok().then_some(out.stdout)
    }

    async fn resolve(&self, sha: &str) -> Option<String> {
        self.git(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{sha}^{{commit}}"),
        ])
        .await
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
    }

    /// The pull request's head, fetched into a private ref so its commits
    /// (the ones commented on and the fixes) are in the checkout.
    async fn fetch_pr(&self, pr: u64) -> Option<String> {
        let local = format!("refs/tribunal-corpus/pr-{pr}");
        self.git(&[
            "fetch",
            "--quiet",
            "--no-tags",
            "origin",
            &format!("+refs/pull/{pr}/head:{local}"),
        ])
        .await?;
        self.resolve(&local).await
    }

    /// A commit by full or abbreviated sha, fetched by full sha when missing.
    async fn commit(&self, sha: &str) -> Option<String> {
        if let Some(full) = self.resolve(sha).await {
            return Some(full);
        }
        if sha.len() == 40 {
            self.git(&["fetch", "--quiet", "--no-tags", "origin", sha])
                .await?;
            return self.resolve(sha).await;
        }
        None
    }

    async fn lines(&self, sha: &str, path: &str) -> Option<Vec<String>> {
        self.git(&["show", &format!("{sha}:{path}")])
            .await
            .map(|s| s.lines().map(str::to_owned).collect())
    }

    /// The first commit after `seen` on the way to `head` that changes lines
    /// `from..=to` of `path` as they were at `seen`, following them as earlier
    /// commits move them.
    async fn first_change(
        &self,
        seen: &str,
        head: &str,
        path: &str,
        from: u64,
        to: u64,
    ) -> Result<String, &'static str> {
        let log = self
            .git(&[
                "log",
                "--reverse",
                "--first-parent",
                "--format=%H %P",
                &format!("{seen}..{head}"),
                "--",
                path,
            ])
            .await
            .ok_or("history-not-readable")?;
        let (mut a, mut b) = (from as i64, to as i64);
        for line in log.lines() {
            let mut shas = line.split_whitespace();
            let (Some(commit), Some(parent)) = (shas.next(), shas.next()) else {
                continue;
            };
            let merge = shas.next().is_some();
            let diff = self
                .git(&[
                    "diff",
                    "-U0",
                    "--no-color",
                    "--no-ext-diff",
                    parent,
                    commit,
                    "--",
                    path,
                ])
                .await
                .ok_or("history-not-readable")?;
            let mut moved = 0;
            for h in hunks(&diff) {
                match hit_or_shift(h, a, b) {
                    Err(()) if merge => return Err("changed-by-merge"),
                    Err(()) => return Ok(commit.to_owned()),
                    Ok(d) => moved += d,
                }
            }
            a += moved;
            b += moved;
        }
        Err("lines-unchanged")
    }

    async fn is_ancestor(&self, a: &str, b: &str) -> bool {
        self.tools
            .git(&["merge-base", "--is-ancestor", a, b], Some(self.dir))
            .await
            .is_ok_and(|o| o.ok())
    }
}

fn quotable(line: &str) -> bool {
    let q = line.trim();
    q.chars().count() >= MIN_QUOTE
        && !q.contains('`')
        && !q.chars().all(|c| c.is_alphanumeric() || "._/-".contains(c))
        && !q.chars().all(|c| !c.is_alphanumeric())
}

/// The longest quotable line of the range, code before comments.
fn best_quote(lines: &[String], from: u64, to: u64) -> Option<(u64, String)> {
    (from.max(1)..=to)
        .filter_map(|n| {
            let l = lines.get(usize::try_from(n).ok()? - 1)?;
            quotable(l).then(|| (n, l.trim().to_owned()))
        })
        .max_by_key(|(n, q)| {
            (
                !is_comment_line(q),
                q.chars().count(),
                std::cmp::Reverse(*n),
            )
        })
}

/// An item ready to be placed at a pin.
#[derive(Debug, Clone)]
struct Item {
    c: Comment,
    seen_at: String,
    fix: String,
    line: u64,
    quote: Option<String>,
}

async fn holds(repo: &Repo<'_>, pin: &str, it: &Item) -> Result<(), &'static str> {
    let Some(lines) = repo.lines(pin, &it.c.path).await else {
        return Err("file-missing-at-pin");
    };
    if let Some(q) = &it.quote {
        if !lines.iter().any(|l| l.trim() == q) {
            return Err("quote-not-at-pin");
        }
    }
    if repo.is_ancestor(&it.fix, pin).await {
        return Err("fix-already-in-pin");
    }
    Ok(())
}

fn slugify(text: &str) -> String {
    let mut s = String::new();
    for ch in text.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            s.push(ch);
        } else if !s.ends_with('-') {
            s.push('-');
        }
    }
    let s: String = s.trim_matches('-').chars().take(70).collect();
    let s = s.trim_end_matches('-').to_owned();
    if s.is_empty() {
        "item".to_owned()
    } else {
        s
    }
}

fn file_name(repo: &str, pr: u64) -> String {
    format!("pr-{}-{pr}.json", repo.replace('/', "-"))
}

/// Refuses an output directory that holds an entry other than a candidate,
/// then removes this repository's earlier candidate files.
///
/// # Errors
/// The first non-candidate entry found, or an I/O failure.
pub fn prepare_out(out: &Path, repo: &str) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    let prefix = format!("pr-{}-", repo.replace('/', "-"));
    let mut ours = Vec::new();
    for entry in
        std::fs::read_dir(out).map_err(|e| format!("cannot read {}: {e}", out.display()))?
    {
        let path = entry.map_err(|e| e.to_string())?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_owned();
        if !(name.starts_with("pr-") && name.ends_with(".json")) {
            continue;
        }
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("cannot read {name}: {e}"))?;
        let status = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["status"].as_str().map(str::to_owned));
        if status.as_deref() != Some("candidate") {
            return Err(format!(
                "refusing to write into {}: {name} is not a candidate file (status {})",
                out.display(),
                status.as_deref().unwrap_or("missing")
            ));
        }
        if name.starts_with(&prefix) {
            ours.push(path);
        }
    }
    for p in ours {
        std::fs::remove_file(&p).map_err(|e| format!("cannot remove {}: {e}", p.display()))?;
    }
    Ok(())
}

fn severity_of(reviewer: Option<&str>) -> Value {
    match reviewer.map(str::to_lowercase).as_deref() {
        Some("critical" | "major") => json!("important"),
        Some("minor" | "trivial" | "nitpick") => json!("nit"),
        _ => Value::Null,
    }
}

fn sources(args: &CollectArgs) -> Vec<String> {
    let mut v: Vec<String> = args.markers.iter().map(|m| m.name.clone()).collect();
    if args.line_changed {
        v.push(LINE_CHANGED.to_owned());
    }
    v
}

/// The pull request's author, so a comment on one's own code is not a lead.
async fn pr_author(tools: &Tools, repo: &str, pr: u64) -> Option<String> {
    let out = tools
        .gh(&["api", &format!("repos/{repo}/pulls/{pr}")], None)
        .await
        .ok()?;
    if !out.ok() {
        return None;
    }
    let v: Value = serde_json::from_str(&out.stdout).ok()?;
    v["user"]["login"].as_str().map(str::to_owned)
}

/// Collects candidates and writes them. Returns the summary.
///
/// # Errors
/// A failed listing or an output directory it must not write into. A comment
/// whose commits cannot be found is dropped and counted, not an error.
pub async fn collect(tools: &Tools, args: &CollectArgs) -> Result<Value, String> {
    let listing = list_comments(
        tools,
        &args.repo,
        &args.markers,
        args.line_changed,
        args.since.as_deref(),
        args.max_comments,
    )
    .await?;
    prepare_out(&args.out, &args.repo)?;
    let repo = Repo {
        tools,
        dir: &args.checkout,
    };
    let mut by_pr: BTreeMap<u64, Vec<Comment>> = BTreeMap::new();
    for c in &listing.comments {
        by_pr.entry(c.pr).or_default().push(c.clone());
    }
    let mut dropped_items: BTreeMap<String, usize> = BTreeMap::new();
    let mut dropped_list = Vec::new();
    let mut skipped_by: BTreeMap<String, usize> = BTreeMap::new();
    let mut held_by: BTreeMap<String, usize> = BTreeMap::new();
    let (mut held, mut skipped_n, mut written) = (0usize, 0usize, 0usize);
    for (pr, mut comments) in by_pr {
        comments.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        let head = repo.fetch_pr(pr).await;
        let pr_author = if comments.iter().any(|c| c.source == LINE_CHANGED) {
            pr_author(tools, &args.repo, pr).await
        } else {
            None
        };
        let mut items = Vec::new();
        for c in comments {
            let reason = async {
                if c.source == LINE_CHANGED && pr_author.as_deref() == Some(c.author.as_str()) {
                    return Err("pr-author");
                }
                let seen_at = repo.commit(&c.commit).await.ok_or("commit-not-found")?;
                let from = c.start_line.unwrap_or(c.line).min(c.line);
                let fix = match &c.fix {
                    Some(f) => repo.commit(f).await.ok_or("fix-not-found")?,
                    None => {
                        let head = head.as_deref().ok_or("no-head")?;
                        repo.first_change(&seen_at, head, &c.path, from, c.line)
                            .await?
                    }
                };
                let lines = repo
                    .lines(&seen_at, &c.path)
                    .await
                    .ok_or("file-not-readable")?;
                let quote = best_quote(&lines, from, c.line);
                Ok::<_, &'static str>(Item {
                    line: quote.as_ref().map_or(c.line, |q| q.0),
                    quote: quote.map(|q| q.1),
                    seen_at,
                    fix,
                    c: c.clone(),
                })
            }
            .await;
            match reason {
                Ok(it) => items.push(it),
                Err(r) => {
                    count(&mut dropped_items, r);
                    dropped_list.push(json!({"pr": pr, "comment": c.url, "reason": r}));
                }
            }
        }
        if items.is_empty() {
            continue;
        }
        let mut pins: Vec<String> = Vec::new();
        for p in items.iter().map(|i| i.seen_at.clone()).chain(head.clone()) {
            if !pins.contains(&p) {
                pins.push(p);
            }
        }
        let mut best: Option<(usize, usize)> = None;
        let mut verdicts: HashMap<(usize, usize), Result<(), &'static str>> = HashMap::new();
        for (pi, pin) in pins.iter().enumerate() {
            let mut n = 0;
            for (ii, it) in items.iter().enumerate() {
                let v = holds(&repo, pin, it).await;
                n += usize::from(v.is_ok());
                verdicts.insert((pi, ii), v);
            }
            if best.is_none_or(|(bn, _)| n > bn) {
                best = Some((n, pi));
            }
        }
        let (_, pi) = best.expect("at least one pin");
        let pin = &pins[pi];
        let mut real = Vec::new();
        let mut skipped = Vec::new();
        let mut ids: HashMap<String, usize> = HashMap::new();
        for (ii, it) in items.iter().enumerate() {
            let mut id = slugify(&it.c.title);
            let k = ids.entry(id.clone()).or_default();
            *k += 1;
            if *k > 1 {
                id = format!("{id}-{k}");
            }
            if let Err(why) = verdicts[&(pi, ii)] {
                count(&mut skipped_by, why);
                skipped_n += 1;
                skipped
                    .push(json!({"id": id, "file": it.c.path, "reason": why, "comment": it.c.url}));
                continue;
            }
            held += 1;
            count(&mut held_by, it.c.source);
            let at = format!("{}:{}", it.c.path, it.line);
            let at = match &it.quote {
                Some(q) => format!("{at} `{q}`"),
                None => at,
            };
            let kind = if it.c.bot { "bot" } else { "person" };
            let how = if it.c.source == MARKED {
                format!("{} marked the comment addressed", it.c.author)
            } else {
                "the first later commit of the pull request that changed the commented lines"
                    .to_owned()
            };
            let description: String =
                format!("{}. {}", it.c.title.trim_end_matches('.'), it.c.text)
                    .trim()
                    .chars()
                    .take(DESCRIPTION_CAP)
                    .collect();
            real.push(json!({
                "id": id,
                "file": it.c.path,
                "line": it.line,
                "quote": it.quote,
                "description": description,
                "evidence": format!("At {}, {at}. Fixed in {} ({}).", &pin[..12], &it.fix[..12], how),
                "raisedBy": format!("{} ({}) inline comment {}", it.c.author, kind, it.c.url),
                "resolution": format!("fixed in {}", &it.fix[..12]),
                "source": it.c.source,
                "reviewer": it.c.author,
                "reviewerKind": kind,
                "expectedSeverity": severity_of(it.c.reviewer_severity.as_deref()),
                "reviewerSeverity": it.c.reviewer_severity,
                "tier": "candidate",
                "comment": it.c.url,
                "seenAt": it.seen_at,
                "fix": it.fix,
            }));
        }
        let entry = json!({
            "repo": args.repo,
            "pr": pr,
            "headSha": head,
            "reviewedSha": pin,
            "status": "candidate",
            "groundTruthTier": "candidate",
            "candidateSource": {"kind": "review-comments", "sources": sources(args)},
            "knownReal": real,
            "skipped": skipped,
        });
        let path = args.out.join(file_name(&args.repo, pr));
        let mut text = serde_json::to_string_pretty(&entry).map_err(|e| e.to_string())?;
        text.push('\n');
        std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        written += 1;
    }
    Ok(json!({
        "repo": args.repo,
        "out": args.out.display().to_string(),
        "commentsRead": listing.read,
        "kept": listing.comments.len(),
        "droppedComments": listing.dropped,
        "droppedItems": dropped_items,
        "droppedItemList": dropped_list,
        "pullRequestsWritten": written,
        "itemsHeld": held,
        "itemsHeldBySource": held_by,
        "itemsSkipped": skipped_n,
        "skippedBy": skipped_by,
    }))
}

/// The summary as text.
#[must_use]
pub fn summary_text(s: &Value) -> String {
    let map = |v: &Value| {
        v.as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, n)| format!("{k} {n}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "none".to_owned())
    };
    format!(
        "{repo}: {read} comments read, {kept} kept (left out: {dc}).\n\
         Of the kept: {held} held at their pin ({hb}), {sk} skipped ({skb}), {di} dropped ({dib}).\n\
         {w} candidate file(s) in {out}. A candidate is a lead: confirm each defect at its pin before it becomes a label.\n",
        repo = s["repo"].as_str().unwrap_or(""),
        read = s["commentsRead"],
        kept = s["kept"],
        dc = map(&s["droppedComments"]),
        held = s["itemsHeld"],
        hb = map(&s["itemsHeldBySource"]),
        sk = s["itemsSkipped"],
        skb = map(&s["skippedBy"]),
        di = s["droppedItems"].as_object().map_or(0, |m| m.values().filter_map(Value::as_u64).sum::<u64>()),
        dib = map(&s["droppedItems"]),
        w = s["pullRequestsWritten"],
        out = s["out"].as_str().unwrap_or(""),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_prefer_code_then_length() {
        let lines: Vec<String> = [
            "def widget_count(items):",
            "    # count every widget in the list here",
            "    return len([i for i in items if i])",
            "x",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        assert_eq!(
            best_quote(&lines, 1, 4),
            Some((3, "return len([i for i in items if i])".to_owned()))
        );
        assert_eq!(best_quote(&lines, 4, 4), None);
        assert_eq!(best_quote(&lines, 9, 12), None);
    }

    #[test]
    fn hunks_hit_or_move_the_commented_lines() {
        let diff = "diff --git a/x b/x\n@@ -2,0 +3,2 @@\n+a\n+b\n@@ -10 +12 @@\n-c\n+d\n";
        assert_eq!(hunks(diff), vec![(2, 0, 2), (10, 1, 1)]);
        // Lines 5 to 7: an insertion after line 2 moves them by 2.
        assert_eq!(hit_or_shift((2, 0, 2), 5, 7), Ok(2));
        // An insertion after line 5, inside the range, changes them.
        assert_eq!(hit_or_shift((5, 0, 1), 5, 7), Err(()));
        // An insertion right after the last line does not.
        assert_eq!(hit_or_shift((7, 0, 1), 5, 7), Ok(0));
        // A change that overlaps the first or the last line hits.
        assert_eq!(hit_or_shift((3, 3, 1), 5, 7), Err(()));
        assert_eq!(hit_or_shift((7, 2, 0), 5, 7), Err(()));
        // A deletion before moves them up; one after leaves them.
        assert_eq!(hit_or_shift((1, 3, 0), 5, 7), Ok(-3));
        assert_eq!(hit_or_shift((9, 1, 4), 5, 7), Ok(0));
    }

    #[test]
    fn slugs_are_short_and_safe() {
        assert_eq!(
            slugify("Skip this test when `x` < 1.3!"),
            "skip-this-test-when-x-1-3"
        );
        assert_eq!(slugify("¿?"), "item");
    }

    #[test]
    fn reviewer_severity_maps_to_two_levels() {
        assert_eq!(severity_of(Some("Major")), json!("important"));
        assert_eq!(severity_of(Some("Minor")), json!("nit"));
        assert_eq!(severity_of(Some("Refactor suggestion")), Value::Null);
        assert_eq!(severity_of(None), Value::Null);
    }
}
