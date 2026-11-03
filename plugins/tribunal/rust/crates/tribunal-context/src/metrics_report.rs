//! `tribunal metrics`: the refutation panel's scorecard over the rows in
//! `metrics.jsonl`, as `references/metrics-schema.md` ("Measurement modes")
//! defines it, and the row selection `tribunal audit` shares.
//!
//! Rows with no `schemaVersion` (the legacy log) and re-runs (a non-null
//! `rerunOf`) are counted and left out. The rest are grouped by the engine
//! that wrote them and by `schemaVersion`, and each group is reported on its
//! own: two groups are never averaged together. A figure whose field is
//! missing on most of the items it applies to carries a note saying so, and a
//! figure with no field at all is printed as not recorded rather than as 0.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

/// Which rows to read: one repo (`owner/name`) and rows whose `ts` is at or
/// after `since` (an ISO date or instant, compared as text).
#[derive(Debug, Clone, Default)]
pub struct RowFilter {
    pub repo: Option<String>,
    pub since: Option<String>,
}

impl RowFilter {
    #[must_use]
    pub fn keep(&self, row: &Value) -> bool {
        let repo_ok = self
            .repo
            .as_deref()
            .is_none_or(|r| row.get("repo").and_then(Value::as_str) == Some(r));
        let since_ok = self.since.as_deref().is_none_or(|s| {
            row.get("ts")
                .and_then(Value::as_str)
                .is_some_and(|ts| ts >= s)
        });
        repo_ok && since_ok
    }
}

/// The rows of a ledger, and how many non-empty lines were not a JSON object.
#[must_use]
pub fn parse_ledger(text: &str) -> (Vec<Value>, usize) {
    let mut rows = Vec::new();
    let mut unreadable = 0;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match serde_json::from_str::<Value>(line) {
            Ok(v) if v.is_object() => rows.push(v),
            _ => unreadable += 1,
        }
    }
    (rows, unreadable)
}

/// The engines in report order.
pub const ENGINES: [&str; 3] = ["rust", "workflow", "in-context"];

/// Which engine wrote a row. Rust rows say `engine: "rust"`. The Workflow and
/// the in-context path wrote no `engine`, and only `mode` tells them apart.
#[must_use]
pub fn engine_of(row: &Value) -> &'static str {
    if row.get("engine").and_then(Value::as_str) == Some("rust") {
        "rust"
    } else if row.get("mode").and_then(Value::as_str) == Some("in-context") {
        "in-context"
    } else {
        "workflow"
    }
}

/// `schemaVersion` when the row is canonical (2 or later), `None` for a
/// legacy row.
#[must_use]
pub fn schema_version(row: &Value) -> Option<u64> {
    row.get("schemaVersion")
        .and_then(Value::as_u64)
        .filter(|v| *v >= 2)
}

#[must_use]
pub fn is_rerun(row: &Value) -> bool {
    row.get("rerunOf").is_some_and(|v| !v.is_null())
}

#[derive(Debug)]
pub struct Group<'a> {
    pub engine: &'static str,
    pub schema_version: u64,
    pub rows: Vec<&'a Value>,
}

#[derive(Debug, Default)]
pub struct Selection<'a> {
    pub read: usize,
    pub unreadable: usize,
    pub filtered_out: usize,
    pub legacy: usize,
    /// Canonical rows left out for a non-null `rerunOf`.
    pub reruns: Vec<&'a Value>,
    /// Aggregated rows with no `rerunOf` key at all, counted as first runs.
    pub rerun_of_absent: usize,
    pub groups: Vec<Group<'a>>,
}

impl Selection<'_> {
    #[must_use]
    pub fn aggregated(&self) -> usize {
        self.groups.iter().map(|g| g.rows.len()).sum()
    }

    #[must_use]
    pub fn counts(&self) -> Value {
        json!({
            "read": self.read,
            "unreadableLines": self.unreadable,
            "filteredOut": self.filtered_out,
            "legacySkipped": self.legacy,
            "rerunsExcluded": self.reruns.len(),
            "aggregated": self.aggregated(),
            "rerunOfAbsent": self.rerun_of_absent,
        })
    }

    /// `N rows aggregated, M legacy skipped, K re-runs excluded`, with what
    /// else was left out.
    #[must_use]
    pub fn headline(&self) -> String {
        let mut s = format!(
            "{} rows aggregated, {} legacy skipped, {} re-runs excluded (of {} rows read",
            self.aggregated(),
            self.legacy,
            self.reruns.len(),
            self.read
        );
        if self.filtered_out > 0 {
            s += &format!(", {} outside --repo/--since", self.filtered_out);
        }
        if self.unreadable > 0 {
            s += &format!(", plus {} unreadable lines", self.unreadable);
        }
        s += ")";
        s
    }
}

/// Splits `rows` as every measurement mode reads them.
#[must_use]
pub fn select<'a>(rows: &'a [Value], unreadable: usize, f: &RowFilter) -> Selection<'a> {
    let mut sel = Selection {
        read: rows.len(),
        unreadable,
        ..Selection::default()
    };
    let mut groups: BTreeMap<(usize, std::cmp::Reverse<u64>), Vec<&Value>> = BTreeMap::new();
    for row in rows {
        if !f.keep(row) {
            sel.filtered_out += 1;
            continue;
        }
        let Some(v) = schema_version(row) else {
            sel.legacy += 1;
            continue;
        };
        if is_rerun(row) {
            sel.reruns.push(row);
            continue;
        }
        if row.get("rerunOf").is_none() {
            sel.rerun_of_absent += 1;
        }
        let engine = engine_of(row);
        let rank = ENGINES.iter().position(|e| *e == engine).unwrap_or(0);
        groups
            .entry((rank, std::cmp::Reverse(v)))
            .or_default()
            .push(row);
    }
    sel.groups = groups
        .into_iter()
        .map(|((rank, v), rows)| Group {
            engine: ENGINES[rank],
            schema_version: v.0,
            rows,
        })
        .collect();
    sel
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

#[allow(clippy::cast_precision_loss)]
pub(crate) fn rate(n: usize, d: usize) -> Value {
    if d == 0 {
        Value::Null
    } else {
        json!(round4(n as f64 / d as f64))
    }
}

fn is_true(v: &Value, key: &str) -> bool {
    v.get(key) == Some(&Value::Bool(true))
}

fn is_bool(v: &Value, key: &str) -> bool {
    v.get(key).is_some_and(Value::is_boolean)
}

fn array<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn coverage_note(notes: &mut Vec<String>, what: &str, has: usize, of: usize) {
    if of == 0 || has * 2 >= of {
        return;
    }
    if has == 0 {
        notes.push(format!(
            "{what}: not recorded on any of the {of} it applies to, so no figure"
        ));
    } else {
        notes.push(format!(
            "{what}: recorded on only {has} of {of}; the figure covers those {has}"
        ));
    }
}

/// The verdict a family's votes add up to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    /// No vote of the family carries a verdict.
    Absent,
    /// As many REFUTED as NOT_REFUTED votes.
    Split,
    Refuted(bool),
}

fn side(votes: &[Value], source: &str) -> Side {
    let (mut refuted, mut kept) = (0, 0);
    for v in votes
        .iter()
        .filter(|v| v.get("source").and_then(Value::as_str) == Some(source))
    {
        match v.get("verdict").and_then(Value::as_str) {
            Some("REFUTED") => refuted += 1,
            Some("NOT_REFUTED") => kept += 1,
            _ => {}
        }
    }
    match (refuted, kept) {
        (0, 0) => Side::Absent,
        (r, k) if r == k => Side::Split,
        (r, k) => Side::Refuted(r > k),
    }
}

/// The pairs `crossModelDisagreement` names, as `(key, first, second)`.
const PAIRS: [(&str, &str, &str); 3] = [
    ("codexVsClaudeMajority", "codex", "claude"),
    ("geminiVsClaudeMajority", "gemini", "claude"),
    ("codexVsGemini", "codex", "gemini"),
];

/// Bins of `refuteFraction - threshold`, as `[from, to)`.
pub const BINS: [(f64, f64); 8] = [
    (-1.0, -0.25),
    (-0.25, -0.1),
    (-0.1, -0.05),
    (-0.05, 0.0),
    (0.0, 0.05),
    (0.05, 0.1),
    (0.1, 0.25),
    (0.25, 1.0),
];

/// What a killed finding was killed for: the `killReason` most of its
/// REFUTED votes gave, `mixed` on a tie, `uncategorized` when none gave one.
fn kill_category(f: &Value) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for v in array(f, "votes") {
        if v.get("verdict").and_then(Value::as_str) == Some("REFUTED") {
            if let Some(r) = v.get("killReason").and_then(Value::as_str) {
                *counts.entry(r).or_default() += 1;
            }
        }
    }
    let Some(top) = counts.values().max().copied() else {
        return "uncategorized".into();
    };
    let mut leaders = counts.iter().filter(|(_, n)| **n == top);
    match (leaders.next(), leaders.next()) {
        (Some((r, _)), None) => (*r).to_owned(),
        _ => "mixed".into(),
    }
}

fn sorted_counts(m: BTreeMap<String, usize>) -> Value {
    let mut v: Vec<(String, usize)> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Value::Array(
        v.into_iter()
            .map(|(k, n)| json!({"key": k, "count": n}))
            .collect(),
    )
}

#[allow(clippy::too_many_lines)]
fn group_metrics(g: &Group) -> Value {
    let mut notes = Vec::new();
    let mut findings: Vec<&Value> = Vec::new();
    let mut rows_without_findings = 0;
    for r in &g.rows {
        match r.get("findings").and_then(Value::as_array) {
            Some(a) => findings.extend(a.iter()),
            None => rows_without_findings += 1,
        }
    }
    let pinned = g.rows.iter().filter(|r| is_true(r, "pinned")).count();
    let panel_ran_known = findings.iter().filter(|f| is_bool(f, "panelRan")).count();
    coverage_note(
        &mut notes,
        "findings[].panelRan",
        panel_ran_known,
        findings.len(),
    );
    let paneled: Vec<&Value> = findings
        .iter()
        .copied()
        .filter(|f| is_true(f, "panelRan"))
        .collect();

    // Refutation kill rate.
    let survived_known = paneled.iter().filter(|f| is_bool(f, "survived")).count();
    let killed: Vec<&Value> = paneled
        .iter()
        .copied()
        .filter(|f| f.get("survived") == Some(&Value::Bool(false)))
        .collect();
    coverage_note(
        &mut notes,
        "findings[].survived on paneled findings",
        survived_known,
        paneled.len(),
    );

    // Candidates reaching a panel.
    let (mut reach_rows, mut candidates, mut reach_paneled) = (0, 0, 0);
    for r in &g.rows {
        if let (Some(c), Some(p)) = (
            r.get("candidates").and_then(Value::as_u64),
            r.get("paneled").and_then(Value::as_u64),
        ) {
            reach_rows += 1;
            candidates += usize::try_from(c).unwrap_or(0);
            reach_paneled += usize::try_from(p).unwrap_or(0);
        }
    }
    coverage_note(
        &mut notes,
        "candidates and paneled",
        reach_rows,
        g.rows.len(),
    );

    // Debate.
    let (mut debate_known, mut fired) = (0, 0);
    let mut outcomes: BTreeMap<String, usize> = BTreeMap::new();
    for f in &paneled {
        let Some(d) = f.get("debate").filter(|d| is_bool(d, "fired")) else {
            continue;
        };
        debate_known += 1;
        if is_true(d, "fired") {
            fired += 1;
            let o = d
                .get("outcome")
                .and_then(Value::as_str)
                .unwrap_or("none")
                .to_owned();
            *outcomes.entry(o).or_default() += 1;
        } else if let Some(o) = d.get("outcome").and_then(Value::as_str) {
            *outcomes.entry(format!("{o} (not fired)")).or_default() += 1;
        }
    }
    coverage_note(
        &mut notes,
        "findings[].debate on paneled findings",
        debate_known,
        paneled.len(),
    );
    let flipped = outcomes.get("flipped").copied().unwrap_or(0);

    // Cross-model disagreement, over findings an external head voted on.
    let voter_known = paneled
        .iter()
        .filter(|f| is_bool(f, "hadExternalVoter"))
        .count();
    coverage_note(
        &mut notes,
        "findings[].hadExternalVoter on paneled findings",
        voter_known,
        paneled.len(),
    );
    let external: Vec<&Value> = paneled
        .iter()
        .copied()
        .filter(|f| is_true(f, "hadExternalVoter"))
        .collect();
    let mut pairs = Map::new();
    for (key, a, b) in PAIRS {
        let (mut compared, mut disagreed, mut a_missing, mut b_missing) = (0, 0, 0, 0);
        for f in &external {
            let votes = array(f, "votes");
            match (side(votes, a), side(votes, b)) {
                (Side::Refuted(x), Side::Refuted(y)) => {
                    compared += 1;
                    if x != y {
                        disagreed += 1;
                    }
                }
                (Side::Refuted(_), _) => b_missing += 1,
                _ => a_missing += 1,
            }
        }
        pairs.insert(
            key.into(),
            json!({
                "first": a, "second": b,
                "compared": compared, "disagreed": disagreed,
                "rate": rate(disagreed, compared),
                "firstNoVerdict": a_missing, "secondNoVerdict": b_missing,
            }),
        );
    }

    // Panel degradation.
    let (mut slots_known, mut with_failed, mut unverified_known, mut unverified) = (0, 0, 0, 0);
    let (mut detail_known, mut slots) = (0, 0);
    let mut by_reason: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for f in &paneled {
        if let Some(fs) = f.get("failedSlots").and_then(Value::as_array) {
            slots_known += 1;
            if !fs.is_empty() {
                with_failed += 1;
                if f.get("failedSlotsDetail").is_some_and(Value::is_array) {
                    detail_known += 1;
                }
            }
        }
        if is_bool(f, "unverified") {
            unverified_known += 1;
            if is_true(f, "unverified") {
                unverified += 1;
            }
        }
        for d in array(f, "failedSlotsDetail") {
            slots += 1;
            let reason = d.get("reason").and_then(Value::as_str).unwrap_or("unknown");
            let source = d.get("source").and_then(Value::as_str).unwrap_or("unknown");
            *by_reason
                .entry(reason.to_owned())
                .or_default()
                .entry(source.to_owned())
                .or_default() += 1;
        }
    }
    coverage_note(
        &mut notes,
        "findings[].failedSlots on paneled findings",
        slots_known,
        paneled.len(),
    );
    coverage_note(
        &mut notes,
        "findings[].unverified on paneled findings",
        unverified_known,
        paneled.len(),
    );
    coverage_note(
        &mut notes,
        "findings[].failedSlotsDetail on findings with a failed slot",
        detail_known,
        with_failed,
    );
    let mut reasons: Vec<(String, BTreeMap<String, usize>)> = by_reason.into_iter().collect();
    reasons.sort_by(|a, b| {
        let n = |m: &BTreeMap<String, usize>| m.values().sum::<usize>();
        n(&b.1).cmp(&n(&a.1)).then_with(|| a.0.cmp(&b.0))
    });

    // Kills by killReason.
    let (mut refuted_votes, mut reason_known) = (0, 0);
    let mut vote_reasons: BTreeMap<String, usize> = BTreeMap::new();
    for f in &paneled {
        for v in array(f, "votes") {
            if v.get("verdict").and_then(Value::as_str) != Some("REFUTED") {
                continue;
            }
            refuted_votes += 1;
            if v.get("killReason").is_some() {
                reason_known += 1;
            }
            let r = v
                .get("killReason")
                .and_then(Value::as_str)
                .unwrap_or("uncategorized");
            *vote_reasons.entry(r.to_owned()).or_default() += 1;
        }
    }
    coverage_note(
        &mut notes,
        "votes[].killReason on REFUTED votes",
        reason_known,
        refuted_votes,
    );
    let mut kill_reasons: BTreeMap<String, usize> = BTreeMap::new();
    for f in &killed {
        *kill_reasons.entry(kill_category(f)).or_default() += 1;
    }
    let mut kill_reasons: Vec<(String, usize)> = kill_reasons.into_iter().collect();
    kill_reasons.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let killed_by_reason: Vec<Value> = kill_reasons
        .into_iter()
        .map(|(k, n)| json!({"key": k, "count": n, "shareOfPaneled": rate(n, paneled.len())}))
        .collect();

    // refuteFraction around the threshold.
    let (mut no_fraction, mut no_threshold, mut near) = (0, 0, 0);
    let mut bins = [0usize; BINS.len()];
    for f in &paneled {
        let Some(rf) = f.get("refuteFraction").and_then(Value::as_f64) else {
            no_fraction += 1;
            continue;
        };
        let Some(t) = f.get("threshold").and_then(Value::as_f64) else {
            no_threshold += 1;
            continue;
        };
        let delta = ((rf - t) * 1e9).round() / 1e9;
        if delta.abs() < 0.05 {
            near += 1;
        }
        let i = BINS
            .iter()
            .position(|(lo, hi)| delta >= *lo && delta < *hi)
            .unwrap_or(if delta < 0.0 { 0 } else { BINS.len() - 1 });
        bins[i] += 1;
    }
    let with_values = paneled.len() - no_fraction - no_threshold;
    coverage_note(
        &mut notes,
        "findings[].refuteFraction and threshold on paneled findings",
        with_values,
        paneled.len(),
    );

    json!({
        "engine": g.engine,
        "schemaVersion": g.schema_version,
        "rows": g.rows.len(),
        "pinnedRows": pinned,
        "rowsWithoutFindings": rows_without_findings,
        "findings": findings.len(),
        "paneled": paneled.len(),
        "killRate": {
            "killed": killed.len(),
            "paneled": paneled.len(),
            "rate": rate(killed.len(), paneled.len()),
            "survivedRecorded": survived_known,
        },
        "panelReach": {
            "paneled": reach_paneled,
            "candidates": candidates,
            "rate": rate(reach_paneled, candidates),
            "rowsWithBoth": reach_rows,
        },
        "debate": {
            "paneled": paneled.len(),
            "recorded": debate_known,
            "fired": fired,
            "fireRate": rate(fired, debate_known),
            "flipped": flipped,
            "flipRate": rate(flipped, fired),
            "outcomes": sorted_counts(outcomes),
        },
        "crossModel": {
            "paneled": paneled.len(),
            "hadExternalVoterRecorded": voter_known,
            "withExternalVoter": external.len(),
            "pairs": pairs,
        },
        "degradation": {
            "paneled": paneled.len(),
            "failedSlotsRecorded": slots_known,
            "withFailedSlots": with_failed,
            "failedSlotsRate": rate(with_failed, slots_known),
            "unverifiedRecorded": unverified_known,
            "unverified": unverified,
            "unverifiedRate": rate(unverified, unverified_known),
            "failedSlots": slots,
            "byReason": reasons.into_iter().map(|(reason, by_source)| json!({
                "reason": reason,
                "count": by_source.values().sum::<usize>(),
                "bySource": by_source,
            })).collect::<Vec<_>>(),
        },
        "killReasons": {
            "refutedVotes": refuted_votes,
            "killReasonRecorded": reason_known,
            "votesByReason": sorted_counts(vote_reasons),
            "killedByReason": killed_by_reason,
        },
        "refuteFraction": {
            "paneled": paneled.len(),
            "withValues": with_values,
            "noFraction": no_fraction,
            "noThreshold": no_threshold,
            "nearThreshold": near,
            "nearThresholdRate": rate(near, with_values),
            "bins": BINS.iter().zip(bins).map(|((lo, hi), n)| json!({"from": lo, "to": hi, "count": n})).collect::<Vec<_>>(),
        },
        "notes": notes,
    })
}

/// The scorecard: the selection's counts and one entry per engine and
/// `schemaVersion`.
#[must_use]
pub fn metrics_report(rows: &[Value], unreadable: usize, f: &RowFilter) -> Value {
    let sel = select(rows, unreadable, f);
    let mut notes = Vec::new();
    if sel.rerun_of_absent > 0 {
        notes.push(format!(
            "{} aggregated rows have no rerunOf field and are counted as first runs",
            sel.rerun_of_absent
        ));
    }
    json!({
        "rows": sel.counts(),
        "headline": sel.headline(),
        "groups": sel.groups.iter().map(group_metrics).collect::<Vec<_>>(),
        "notes": notes,
    })
}

fn n(v: &Value) -> usize {
    v.as_u64()
        .and_then(|x| usize::try_from(x).ok())
        .unwrap_or(0)
}

/// `n of d (x.x%)`.
#[must_use]
pub fn of(num: usize, den: usize) -> String {
    if den == 0 {
        return format!("{num} of 0");
    }
    #[allow(clippy::cast_precision_loss)]
    let pct = num as f64 * 100.0 / den as f64;
    format!("{num} of {den} ({pct:.1}%)")
}

fn render_group(s: &mut String, g: &Value) {
    let paneled = n(&g["paneled"]);
    s.push_str(&format!(
        "\n{}, schemaVersion {}: {} rows ({} pinned), {} findings, {} paneled\n",
        g["engine"].as_str().unwrap_or("?"),
        g["schemaVersion"],
        g["rows"],
        g["pinnedRows"],
        g["findings"],
        paneled
    ));
    let k = &g["killRate"];
    s.push_str(&format!(
        "  Refutation kill rate: {} paneled findings killed\n",
        of(n(&k["killed"]), paneled)
    ));
    let r = &g["panelReach"];
    if n(&r["rowsWithBoth"]) == 0 {
        s.push_str("  Candidates reaching a panel: not recorded\n");
    } else {
        s.push_str(&format!(
            "  Candidates reaching a panel: {} candidates (over {} rows)\n",
            of(n(&r["paneled"]), n(&r["candidates"])),
            r["rowsWithBoth"]
        ));
    }
    let d = &g["debate"];
    if n(&d["recorded"]) == 0 {
        s.push_str("  Debate: not recorded\n");
    } else {
        let fired = n(&d["fired"]);
        let flip = if fired == 0 {
            "no debate fired".to_owned()
        } else {
            format!("flipped {}", of(n(&d["flipped"]), fired))
        };
        s.push_str(&format!(
            "  Debate: fired on {} paneled findings; {flip}\n",
            of(fired, n(&d["recorded"]))
        ));
        let outcomes: Vec<String> = d["outcomes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|o| format!("{} {}", o["key"].as_str().unwrap_or("?"), o["count"]))
            .collect();
        if !outcomes.is_empty() {
            s.push_str(&format!("    outcomes: {}\n", outcomes.join(", ")));
        }
    }
    let c = &g["crossModel"];
    let ext = n(&c["withExternalVoter"]);
    if n(&c["hadExternalVoterRecorded"]) == 0 {
        s.push_str("  Cross-model disagreement: hadExternalVoter not recorded\n");
    } else {
        s.push_str(&format!(
            "  Cross-model disagreement, over the {ext} paneled findings with an external voter:\n"
        ));
        for (key, _, _) in PAIRS {
            let p = &c["pairs"][key];
            let (a, b) = (
                p["first"].as_str().unwrap_or("?"),
                p["second"].as_str().unwrap_or("?"),
            );
            let label = if b == "claude" {
                format!("{a} vs Claude majority")
            } else {
                format!("{a} vs {b}")
            };
            let compared = n(&p["compared"]);
            let second = if b == "claude" {
                format!("no Claude majority on {}", p["secondNoVerdict"])
            } else {
                format!("{b} gave no verdict on {}", p["secondNoVerdict"])
            };
            let missing = format!("{a} gave no verdict on {}, {second}", p["firstNoVerdict"]);
            if compared == 0 {
                s.push_str(&format!("    {label}: nothing to compare ({missing})\n"));
            } else {
                s.push_str(&format!(
                    "    {label}: disagreed on {} compared ({missing})\n",
                    of(n(&p["disagreed"]), compared)
                ));
            }
        }
    }
    let dg = &g["degradation"];
    let failed = if n(&dg["failedSlotsRecorded"]) == 0 {
        "failedSlots not recorded".to_owned()
    } else {
        format!(
            "{} paneled findings with a failed slot",
            of(n(&dg["withFailedSlots"]), n(&dg["failedSlotsRecorded"]))
        )
    };
    let unverified = if n(&dg["unverifiedRecorded"]) == 0 {
        "unverified not recorded".to_owned()
    } else {
        format!(
            "{} unverified (no head voted)",
            of(n(&dg["unverified"]), n(&dg["unverifiedRecorded"]))
        )
    };
    s.push_str(&format!("  Panel degradation: {failed}; {unverified}\n"));
    let reasons: Vec<String> = dg["byReason"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            let by: Vec<String> = r["bySource"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, v)| format!("{k} {v}"))
                .collect();
            format!(
                "{} {} ({})",
                r["reason"].as_str().unwrap_or("?"),
                r["count"],
                by.join(", ")
            )
        })
        .collect();
    if !reasons.is_empty() {
        s.push_str(&format!(
            "    failed slots by reason: {}\n",
            reasons.join("; ")
        ));
    }
    let kr = &g["killReasons"];
    let killed_by: Vec<String> = kr["killedByReason"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            format!(
                "{} {}",
                e["key"].as_str().unwrap_or("?"),
                of(n(&e["count"]), paneled)
            )
        })
        .collect();
    if killed_by.is_empty() {
        s.push_str("  Kills by killReason: no paneled finding was killed\n");
    } else {
        s.push_str(&format!(
            "  Kills by killReason (share of paneled findings): {}\n",
            killed_by.join(", ")
        ));
    }
    let votes: Vec<String> = kr["votesByReason"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| format!("{} {}", e["key"].as_str().unwrap_or("?"), e["count"]))
        .collect();
    if !votes.is_empty() {
        s.push_str(&format!(
            "    REFUTED votes by killReason: {}\n",
            votes.join(", ")
        ));
    }
    let rf = &g["refuteFraction"];
    let with = n(&rf["withValues"]);
    if with == 0 {
        s.push_str("  refuteFraction around the threshold: not recorded\n");
    } else {
        s.push_str(&format!(
            "  refuteFraction minus threshold, over {with} paneled findings ({} with no fraction, {} with no threshold):\n",
            rf["noFraction"], rf["noThreshold"]
        ));
        for b in rf["bins"].as_array().into_iter().flatten() {
            s.push_str(&format!(
                "    {:>6} to {:>5}  {}\n",
                format!("{:+.2}", b["from"].as_f64().unwrap_or(0.0)),
                format!("{:+.2}", b["to"].as_f64().unwrap_or(0.0)),
                b["count"]
            ));
        }
        s.push_str(&format!(
            "    within 0.05 of the threshold: {}\n",
            of(n(&rf["nearThreshold"]), with)
        ));
    }
    for note in g["notes"].as_array().into_iter().flatten() {
        s.push_str(&format!("  note: {}\n", note.as_str().unwrap_or("")));
    }
}

/// The scorecard as text for a terminal.
#[must_use]
pub fn render(report: &Value) -> String {
    let mut s = format!(
        "tribunal metrics: {}\n",
        report["headline"].as_str().unwrap_or("")
    );
    s.push_str(
        "Each engine and schemaVersion is reported on its own; groups are never averaged together.\n",
    );
    for note in report["notes"].as_array().into_iter().flatten() {
        s.push_str(&format!("note: {}\n", note.as_str().unwrap_or("")));
    }
    let groups = report["groups"].as_array().cloned().unwrap_or_default();
    if groups.is_empty() {
        s.push_str("No rows to aggregate.\n");
    }
    for g in &groups {
        render_group(&mut s, g);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vote(source: &str, verdict: &str, reason: Option<&str>) -> Value {
        json!({"source": source, "verdict": verdict, "confidence": 0.8, "killReason": reason})
    }

    fn finding(survived: bool, rf: f64, votes: Vec<Value>) -> Value {
        let external = votes.iter().any(|v| v["source"] != "claude");
        json!({
            "title": "t", "file": "a.ts", "panelRan": true, "survived": survived,
            "refuteFraction": rf, "threshold": 0.45, "unverified": false,
            "hadExternalVoter": external, "failedSlots": [], "failedSlotsDetail": [],
            "votes": votes,
            "debate": {"fired": false, "outcome": null},
        })
    }

    fn row(engine: Option<&str>, schema: Option<u64>, rerun: Value, findings: Vec<Value>) -> Value {
        let mut r = json!({
            "ts": "2026-10-30T10:00:00Z", "repo": "o/r", "pr": 1, "headSha": "abc",
            "mode": "workflow", "candidates": 4, "paneled": findings.iter().filter(|f| f["panelRan"] == true).count(),
            "rerunOf": rerun, "findings": findings,
        });
        if let Some(e) = engine {
            r["engine"] = json!(e);
        }
        if let Some(v) = schema {
            r["schemaVersion"] = json!(v);
        }
        r
    }

    /// One ledger with every kind of row the selection has to tell apart.
    fn ledger() -> Vec<Value> {
        let rust_a = row(
            Some("rust"),
            Some(3),
            Value::Null,
            vec![
                // Killed: two REFUTED votes, both not-triggered.
                finding(
                    false,
                    0.8,
                    vec![
                        vote("claude", "REFUTED", Some("not-triggered")),
                        vote("codex", "REFUTED", Some("not-triggered")),
                    ],
                ),
                // Survived: codex disagrees with the Claude majority.
                finding(
                    true,
                    0.42,
                    vec![
                        vote("claude", "NOT_REFUTED", None),
                        vote("claude", "NOT_REFUTED", None),
                        vote("codex", "REFUTED", Some("incorrect-premise")),
                    ],
                ),
                // Never paneled.
                json!({"title": "nit", "file": "b.ts", "panelRan": false, "survived": true}),
            ],
        );
        let mut degraded = finding(true, 0.0, vec![vote("claude", "NOT_REFUTED", None)]);
        degraded["failedSlots"] = json!(["codex"]);
        degraded["failedSlotsDetail"] = json!([{"source": "codex", "reason": "timeout"}]);
        let mut unverified = finding(true, 0.0, vec![]);
        unverified["unverified"] = json!(true);
        unverified["refuteFraction"] = Value::Null;
        unverified["failedSlots"] = json!(["claude", "codex"]);
        unverified["failedSlotsDetail"] = json!([
            {"source": "claude", "reason": "crashed"},
            {"source": "codex", "reason": "timeout"},
        ]);
        let rust_b = row(
            Some("rust"),
            Some(3),
            Value::Null,
            vec![degraded, unverified],
        );
        let mut debated = finding(
            true,
            0.5,
            vec![
                vote("claude", "REFUTED", Some("pre-existing")),
                vote("gemini", "NOT_REFUTED", None),
            ],
        );
        debated["debate"] = json!({"fired": true, "outcome": "flipped"});
        // A Workflow row whose findings predate hadExternalVoter.
        let mut old = finding(false, 0.6, vec![vote("claude", "REFUTED", None)]);
        old.as_object_mut().unwrap().remove("hadExternalVoter");
        let workflow = row(None, Some(3), Value::Null, vec![debated, old]);
        let mut workflow_v2 = row(None, Some(2), Value::Null, vec![]);
        workflow_v2.as_object_mut().unwrap().remove("rerunOf");
        workflow_v2.as_object_mut().unwrap().remove("candidates");
        let rerun = row(
            Some("rust"),
            Some(3),
            json!(1),
            vec![finding(false, 0.9, vec![vote("claude", "REFUTED", None)])],
        );
        let legacy = json!({"ts": "2026-07-01", "repo": "o/r", "pr": null, "findings": [{"panelRan": true, "survived": false}]});
        let other_repo = row(Some("rust"), Some(3), Value::Null, vec![]);
        let mut other_repo = other_repo;
        other_repo["repo"] = json!("o/other");
        vec![
            rust_a,
            rust_b,
            workflow,
            workflow_v2,
            rerun,
            legacy,
            other_repo,
        ]
    }

    fn group<'a>(r: &'a Value, engine: &str, v: u64) -> &'a Value {
        r["groups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["engine"] == engine && g["schemaVersion"] == v)
            .unwrap_or_else(|| panic!("no {engine} v{v} group in {r}"))
    }

    #[test]
    fn splits_legacy_reruns_engines_and_versions() {
        let rows = ledger();
        let r = metrics_report(&rows, 2, &RowFilter::default());
        assert_eq!(
            r["rows"],
            json!({
                "read": 7, "unreadableLines": 2, "filteredOut": 0, "legacySkipped": 1,
                "rerunsExcluded": 1, "aggregated": 5, "rerunOfAbsent": 1,
            })
        );
        assert_eq!(
            r["headline"],
            "5 rows aggregated, 1 legacy skipped, 1 re-runs excluded (of 7 rows read, plus 2 unreadable lines)"
        );
        let keys: Vec<(String, u64)> = r["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| {
                (
                    g["engine"].as_str().unwrap().to_owned(),
                    g["schemaVersion"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            keys,
            [
                ("rust".to_owned(), 3),
                ("workflow".to_owned(), 3),
                ("workflow".to_owned(), 2)
            ]
        );
    }

    #[test]
    fn rust_group_numbers() {
        let rows = ledger();
        let r = metrics_report(&rows, 0, &RowFilter::default());
        let g = group(&r, "rust", 3);
        assert_eq!(g["rows"], 3);
        assert_eq!(g["findings"], 5);
        assert_eq!(g["paneled"], 4);
        assert_eq!(
            g["killRate"],
            json!({"killed": 1, "paneled": 4, "rate": 0.25, "survivedRecorded": 4})
        );
        // Three rows with candidates 4 each; 2 + 2 + 0 paneled.
        assert_eq!(
            g["panelReach"],
            json!({"paneled": 4, "candidates": 12, "rate": 0.3333, "rowsWithBoth": 3})
        );
        assert_eq!(g["debate"]["fired"], 0);
        assert_eq!(g["debate"]["fireRate"], 0.0);
        assert_eq!(g["debate"]["flipRate"], Value::Null);
        // Two findings had an external voter; the degraded one had only Claude.
        let c = &g["crossModel"];
        assert_eq!(c["withExternalVoter"], 2);
        assert_eq!(
            c["pairs"]["codexVsClaudeMajority"],
            json!({
                "first": "codex", "second": "claude", "compared": 2, "disagreed": 1,
                "rate": 0.5, "firstNoVerdict": 0, "secondNoVerdict": 0,
            })
        );
        assert_eq!(c["pairs"]["geminiVsClaudeMajority"]["compared"], 0);
        assert_eq!(c["pairs"]["geminiVsClaudeMajority"]["firstNoVerdict"], 2);
        let d = &g["degradation"];
        assert_eq!(d["withFailedSlots"], 2);
        assert_eq!(d["failedSlotsRate"], 0.5);
        assert_eq!(d["unverified"], 1);
        assert_eq!(d["unverifiedRate"], 0.25);
        assert_eq!(d["failedSlots"], 3);
        assert_eq!(
            d["byReason"],
            json!([
                {"reason": "timeout", "count": 2, "bySource": {"codex": 2}},
                {"reason": "crashed", "count": 1, "bySource": {"claude": 1}},
            ])
        );
        let k = &g["killReasons"];
        assert_eq!(k["refutedVotes"], 3);
        assert_eq!(
            k["votesByReason"],
            json!([
                {"key": "not-triggered", "count": 2},
                {"key": "incorrect-premise", "count": 1},
            ])
        );
        assert_eq!(
            k["killedByReason"],
            json!([{"key": "not-triggered", "count": 1, "shareOfPaneled": 0.25}])
        );
        let f = &g["refuteFraction"];
        assert_eq!(f["withValues"], 3);
        assert_eq!(f["noFraction"], 1);
        // 0.8 - 0.45 = +0.35; 0.42 - 0.45 = -0.03; 0.0 - 0.45 = -0.45.
        let counts: Vec<u64> = f["bins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["count"].as_u64().unwrap())
            .collect();
        assert_eq!(counts, [1, 0, 0, 1, 0, 0, 0, 1]);
        assert_eq!(f["nearThreshold"], 1);
        assert_eq!(g["notes"], json!([]));
    }

    #[test]
    fn workflow_group_numbers_and_missing_fields() {
        let rows = ledger();
        let r = metrics_report(&rows, 0, &RowFilter::default());
        let g = group(&r, "workflow", 3);
        assert_eq!(g["paneled"], 2);
        assert_eq!(g["killRate"]["rate"], 0.5);
        assert_eq!(g["debate"]["fired"], 1);
        assert_eq!(g["debate"]["flipped"], 1);
        assert_eq!(g["debate"]["flipRate"], 1.0);
        assert_eq!(
            g["crossModel"]["pairs"]["geminiVsClaudeMajority"]["disagreed"],
            1
        );
        // One killed finding with a REFUTED vote that gave no killReason.
        assert_eq!(
            g["killReasons"]["killedByReason"],
            json!([{"key": "uncategorized", "count": 1, "shareOfPaneled": 0.5}])
        );
        let v2 = group(&r, "workflow", 2);
        assert_eq!(v2["findings"], 0);
        assert_eq!(v2["panelReach"]["rowsWithBoth"], 0);
        assert_eq!(v2["panelReach"]["rate"], Value::Null);
        assert_eq!(
            v2["notes"],
            json!([
                "candidates and paneled: not recorded on any of the 1 it applies to, so no figure"
            ])
        );
        let text = render(&r);
        assert!(
            text.contains("  Candidates reaching a panel: not recorded\n"),
            "{text}"
        );
    }

    #[test]
    fn a_field_missing_on_most_findings_is_named() {
        let mut a = finding(true, 0.1, vec![vote("claude", "NOT_REFUTED", None)]);
        let mut b = a.clone();
        let c = a.clone();
        a.as_object_mut().unwrap().remove("hadExternalVoter");
        b.as_object_mut().unwrap().remove("hadExternalVoter");
        let rows = [row(Some("rust"), Some(3), Value::Null, vec![a, b, c])];
        let r = metrics_report(&rows, 0, &RowFilter::default());
        let g = group(&r, "rust", 3);
        assert_eq!(
            g["notes"],
            json!(["findings[].hadExternalVoter on paneled findings: recorded on only 1 of 3; the figure covers those 1"])
        );
    }

    #[test]
    fn filters_by_repo_and_date() {
        let rows = ledger();
        let f = RowFilter {
            repo: Some("o/other".into()),
            since: None,
        };
        let r = metrics_report(&rows, 0, &f);
        assert_eq!(r["rows"]["filteredOut"], 6);
        assert_eq!(r["rows"]["aggregated"], 1);
        let f = RowFilter {
            repo: None,
            since: Some("2026-08-01".into()),
        };
        let r = metrics_report(&rows, 0, &f);
        assert_eq!(r["rows"]["filteredOut"], 1);
        assert_eq!(r["rows"]["legacySkipped"], 0);
    }

    #[test]
    fn renders_the_headline_and_each_group() {
        let rows = ledger();
        let text = render(&metrics_report(&rows, 0, &RowFilter::default()));
        assert!(text.starts_with(
            "tribunal metrics: 5 rows aggregated, 1 legacy skipped, 1 re-runs excluded (of 7 rows read)\n"
        ), "{text}");
        assert!(
            text.contains("\nrust, schemaVersion 3: 3 rows (0 pinned), 5 findings, 4 paneled\n"),
            "{text}"
        );
        assert!(
            text.contains("  Refutation kill rate: 1 of 4 (25.0%) paneled findings killed\n"),
            "{text}"
        );
        assert!(
            text.contains("    codex vs Claude majority: disagreed on 1 of 2 (50.0%) compared"),
            "{text}"
        );
        assert!(
            text.contains(
                "    failed slots by reason: timeout 2 (codex 2); crashed 1 (claude 1)\n"
            ),
            "{text}"
        );
        assert_eq!(
            render(&metrics_report(&[], 0, &RowFilter::default())),
            "tribunal metrics: 0 rows aggregated, 0 legacy skipped, 0 re-runs excluded (of 0 rows read)\nEach engine and schemaVersion is reported on its own; groups are never averaged together.\nNo rows to aggregate.\n"
        );
    }

    #[test]
    fn parses_a_ledger_counting_bad_lines() {
        let (rows, bad) = parse_ledger("{\"a\":1}\n\nnot json\n[1]\n{\"b\":2}\n");
        assert_eq!(rows.len(), 2);
        assert_eq!(bad, 2);
    }
}
