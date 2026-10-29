//! Port of the PANEL-GATES region (both blocks): the panel thresholds, the
//! confidence-weighted vote tally, and the gates that decide whether a finding
//! escalates, refines, debates, or is placed outside the verdict.
//!
//! Votes, findings and harvested decisions are model output, so they are read
//! as JSON values with the JavaScript runtime checks. Two classes of input are
//! out of scope because a schema keeps them out: a non-string `file` (on a
//! finding, a decision's `files`, or the PR file list), where `samePath` would
//! throw on `.endsWith`; and a severity named after an `Object.prototype`
//! member such as `"toString"`, which `RANK[...]` would resolve to a function.

use serde::Serialize;
use serde_json::Value;

use super::js_ext::{self, is_str, is_string, or, string_or_empty};
use crate::js;

pub const T_KILL: f64 = 0.45;
pub const DELTA: f64 = 0.1;
pub const GEMINI_CONFIDENCE_CAP: f64 = 0.7;
pub const ESCALATE_CONFIDENCE_FLOOR: f64 = 0.6;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FailedSlot {
    pub source: Value,
    pub reason: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscardedVote {
    pub source: Value,
    pub reason: &'static str,
    pub decision_questioned: Value,
}

/// What `tallyVotes` returns, field for field and in its key order.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tally {
    pub valid: Vec<Value>,
    pub failed_slots: Vec<Value>,
    pub failed_slots_detail: Vec<FailedSlot>,
    pub discarded_votes: Vec<DiscardedVote>,
    pub unverified: bool,
    pub had_independent_voter: bool,
    pub had_external_voter: bool,
    pub refute_fraction: f64,
    pub survived: bool,
    pub borderline: bool,
    pub pre_decided: bool,
    pub core_stands: bool,
}

/// `new Set(values).size` under SameValueZero: an object or array is only
/// ever equal to itself, and no two JSON values share an identity.
fn distinct_count(values: &[Option<&Value>]) -> usize {
    let same = |a: Option<&Value>, b: Option<&Value>| match (a, b) {
        (None, None) => true,
        (Some(Value::Number(x)), Some(Value::Number(y))) => x.as_f64() == y.as_f64(),
        (Some(Value::Array(_) | Value::Object(_)), _) => false,
        (Some(x), Some(y)) => x == y,
        _ => false,
    };
    let mut seen: Vec<Option<&Value>> = Vec::new();
    for &v in values {
        if !seen.iter().any(|&s| same(s, v)) {
            seen.push(v);
        }
    }
    seen.len()
}

/// `VERDICT_SPLIT`.
pub fn verdict_split(tally: &Tally) -> bool {
    let verdicts: Vec<Option<&Value>> = tally.valid.iter().map(|v| js::get(v, "verdict")).collect();
    distinct_count(&verdicts) > 1
}

/// `UNDER_CONFIDENT`.
pub fn under_confident(tally: &Tally) -> bool {
    tally.valid.iter().any(|v| {
        matches!(js::get(v, "confidence"), Some(Value::Number(n))
            if n.as_f64().is_some_and(|c| c < ESCALATE_CONFIDENCE_FLOOR))
    })
}

/// `shouldEscalate`. The JavaScript `&&`/`||` chain returns an operand, so a
/// falsy `touchesHighRiskSurface` comes back as itself; `None` is `undefined`.
pub fn should_escalate(tally: &Tally, finding: &Value, claude_votes_r0: usize) -> Option<Value> {
    if claude_votes_r0 != 1 || tally.unverified {
        return Some(Value::Bool(false));
    }
    if verdict_split(tally) {
        return Some(Value::Bool(true));
    }
    let high_risk = js::get(finding, "touchesHighRiskSurface");
    if !js::truthy(high_risk) {
        return high_risk.cloned();
    }
    Some(Value::Bool(tally.borderline || under_confident(tally)))
}

/// `needsRefine`.
pub fn needs_refine(tally: &Tally) -> bool {
    !tally.survived && !tally.pre_decided && (tally.core_stands || tally.borderline)
}

/// `shouldDebate`.
pub fn should_debate(tally: &Tally) -> bool {
    tally.borderline
}

fn as_str(v: &Value) -> Option<&str> {
    v.as_str()
}

/// `samePath`. `None` and `""` are the falsy paths.
pub fn same_path(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() => {
            a == b || a.ends_with(&format!("/{b}")) || b.ends_with(&format!("/{a}"))
        }
        _ => false,
    }
}

/// The distinct tokens of `s.split(/\W+/).filter(Boolean)`, in first-seen
/// order. `\W` without the `u` flag is everything but `[A-Za-z0-9_]`.
fn word_tokens(s: &str) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for t in s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// `decisionMatches`.
fn decision_matches(decision_questioned: Option<&Value>, summary: Option<&Value>) -> bool {
    let dq_lower = string_or_empty(decision_questioned).to_lowercase();
    let dq = js::trim(&dq_lower);
    let sum = string_or_empty(summary).to_lowercase();
    if dq.is_empty() || sum.is_empty() {
        return false;
    }
    let dq_tokens = word_tokens(dq);
    let s_tokens = word_tokens(&sum);
    let overlap = dq_tokens.iter().filter(|t| s_tokens.contains(t)).count();
    let ratio = overlap as f64 / dq_tokens.len().min(s_tokens.len()).max(1) as f64;
    ratio >= 0.4 || sum.contains(dq) || dq.contains(sum.as_str())
}

/// `preDecidedCoversFile`. A `decisions` that is not a non-empty array allows
/// any citation, as the JavaScript default `[]` does.
pub fn pre_decided_covers_file(
    decision_questioned: Option<&Value>,
    finding_file: Option<&str>,
    decisions: &Value,
) -> bool {
    let items = match decisions {
        Value::Array(a) if !a.is_empty() => a,
        _ => return true,
    };
    if js::trim(&string_or_empty(decision_questioned)).is_empty() {
        return false;
    }
    let finding_file = finding_file.filter(|f| !f.is_empty());
    for item in items {
        if !decision_matches(decision_questioned, js::get(item, "summary")) {
            continue;
        }
        let files = match js::get(item, "files") {
            Some(Value::Array(f)) => f.as_slice(),
            _ => &[],
        };
        if files.is_empty() {
            return true;
        }
        if finding_file.is_some() && files.iter().any(|f| same_path(as_str(f), finding_file)) {
            return true;
        }
    }
    false
}

/// `topicWords`: distinctive words, camelCase split, in first-seen order.
fn topic_words(text: &str) -> Vec<String> {
    const STOPWORDS: &str = "this that with from when then than into does have been will only also added each which their there these those because should would could file files code function returns return value null true false async await const finding";
    // `.replace(/([a-z0-9])([A-Z])/g, "$1 $2")`: matches never overlap, so the
    // upper-case letter of one match cannot start the next.
    let chars: Vec<char> = text.chars().collect();
    let mut split = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let lower_or_digit = c.is_ascii_lowercase() || c.is_ascii_digit();
        if lower_or_digit && chars.get(i + 1).is_some_and(|n| n.is_ascii_uppercase()) {
            split.push(c);
            split.push(' ');
            split.push(chars[i + 1]);
            i += 2;
        } else {
            split.push(c);
            i += 1;
        }
    }
    let lower = split.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    for w in lower.split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit())) {
        if w.len() >= 4 && !STOPWORDS.split(' ').any(|s| s == w) && !out.iter().any(|o| o == w) {
            out.push(w.to_owned());
        }
    }
    out
}

/// What `contestedPreDecision` returns when a discarded citation still names
/// a decision about this PR's files.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Contested {
    pub vote_source: Value,
    pub decision_source: Value,
    pub summary: Value,
    pub files: Vec<Value>,
}

/// `contestedPreDecision`.
pub fn contested_pre_decision(
    discarded_votes: &[DiscardedVote],
    finding: &Value,
    decisions: &Value,
    pr_files: &[String],
) -> Option<Contested> {
    let finding_file = match js::get(finding, "file") {
        Some(Value::String(f)) if !f.is_empty() => f.as_str(),
        _ => return None,
    };
    let items = match decisions {
        Value::Array(a) if !a.is_empty() => a,
        _ => return None,
    };
    let own = topic_words(&format!(
        "{} {}",
        string_or_empty(js::get(finding, "title")),
        string_or_empty(js::get(finding, "description"))
    ));
    let in_pr = |f: Option<&str>| pr_files.iter().any(|p| same_path(Some(p), f));
    if !in_pr(Some(finding_file)) {
        return None;
    }
    for d in discarded_votes {
        for item in items {
            let files = match js::get(item, "files") {
                Some(Value::Array(f)) => f.as_slice(),
                _ => &[],
            };
            if files.is_empty() || !files.iter().all(|f| in_pr(as_str(f))) {
                continue;
            }
            let summary = js::get(item, "summary");
            if !decision_matches(Some(&d.decision_questioned), summary) {
                continue;
            }
            let shared = topic_words(&string_or_empty(summary))
                .iter()
                .filter(|w| own.contains(w))
                .count();
            if shared < 2 {
                continue;
            }
            return Some(Contested {
                vote_source: or(Some(&d.source), Value::from("unknown")),
                decision_source: or(js::get(item, "source"), Value::Null),
                summary: summary.cloned().unwrap_or(Value::Null),
                files: files.to_vec(),
            });
        }
    }
    None
}

fn citation_holds(vote: &Value, finding_file: Option<&str>, decisions: &Value) -> bool {
    let dq = js::get(vote, "decisionQuestioned");
    matches!(dq, Some(Value::String(s)) if !js::trim(s).is_empty())
        && pre_decided_covers_file(dq, finding_file, decisions)
}

/// `factualRefutationFails`: why a v2 REFUTED vote may not kill, or `None`.
pub fn factual_refutation_fails(vote: &Value) -> Option<&'static str> {
    if !is_string(js::get(vote, "introducedByPr")) {
        return None;
    }
    let kill_reason = js::get(vote, "killReason");
    if !is_str(kill_reason, "not-triggered") && !is_str(kill_reason, "incorrect-premise") {
        return Some("judgement-kill-reason");
    }
    let quote = js::get(vote, "contradiction").and_then(|c| js::get(c, "quote"));
    if js::trim(&string_or_empty(quote)).is_empty() {
        return Some("no-contradicting-quote");
    }
    if js::get(vote, "quoteFound") == Some(&Value::Bool(false)) {
        return Some("quote-not-at-commit");
    }
    None
}

fn weight(v: &Value) -> f64 {
    let raw = match js::get(v, "confidence") {
        // JSON has no NaN, so clamp is `Math.min(1, Math.max(0, c))`.
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.5).clamp(0.0, 1.0),
        _ => 0.5,
    };
    if is_str(js::get(v, "source"), "gemini") {
        GEMINI_CONFIDENCE_CAP.min(raw)
    } else {
        raw
    }
}

/// `tallyVotes`. A vote JavaScript treats as falsy is a crashed slot, named by
/// its position in `panel`.
pub fn tally_votes(
    votes: &[Value],
    panel: &[Value],
    finding_file: Option<&str>,
    decisions: &Value,
    decisions_given: bool,
) -> Tally {
    let unknown = || Value::from("unknown");
    let crashed: Vec<Value> = votes
        .iter()
        .enumerate()
        .filter(|(_, v)| !js::truthy(Some(v)))
        .map(|(i, _)| or(panel.get(i).and_then(|p| js::get(p, "source")), unknown()))
        .collect();
    let present: Vec<&Value> = votes.iter().filter(|v| js::truthy(Some(v))).collect();
    let unavailable = |v: &Value| is_str(js::get(v, "slotStatus"), "UNAVAILABLE");
    let valid: Vec<&Value> = present
        .iter()
        .copied()
        .filter(|v| !unavailable(v))
        .collect();
    let unavailable_present: Vec<&Value> =
        present.iter().copied().filter(|v| unavailable(v)).collect();

    let mut failed_slots = crashed.clone();
    failed_slots.extend(
        unavailable_present
            .iter()
            .map(|v| or(js::get(v, "source"), unknown())),
    );
    let mut failed_slots_detail: Vec<FailedSlot> = crashed
        .into_iter()
        .map(|source| FailedSlot {
            source,
            reason: Value::from("crashed"),
        })
        .collect();
    failed_slots_detail.extend(unavailable_present.iter().map(|v| FailedSlot {
        source: or(js::get(v, "source"), unknown()),
        reason: or(js::get(v, "failureReason"), unknown()),
    }));

    let is_pre_decided = |v: &Value| is_str(js::get(v, "refutationType"), "PRE_DECIDED");
    let invented = |v: &Value| !decisions_given && is_string(js::get(v, "introducedByPr"));
    let discarded: Vec<bool> = valid
        .iter()
        .map(|v| {
            is_str(js::get(v, "verdict"), "REFUTED")
                && if is_pre_decided(v) {
                    !citation_holds(v, finding_file, decisions) || invented(v)
                } else {
                    factual_refutation_fails(v).is_some()
                }
        })
        .collect();
    let discarded_votes: Vec<DiscardedVote> = valid
        .iter()
        .zip(&discarded)
        .filter(|(_, d)| **d)
        .map(|(v, _)| {
            let dq = js::get(v, "decisionQuestioned");
            let reason = if !is_pre_decided(v) {
                factual_refutation_fails(v).unwrap_or_default()
            } else if invented(v) {
                "no-decisions-given"
            } else if matches!(dq, Some(Value::String(s)) if !js::trim(s).is_empty()) {
                "citation-does-not-cover-finding"
            } else {
                "no-decision-cited"
            };
            DiscardedVote {
                source: or(js::get(v, "source"), unknown()),
                reason,
                decision_questioned: or(dq, Value::Null),
            }
        })
        .collect();

    let mut refute_weight = 0.0;
    let mut keep_weight = 0.0;
    for (v, _) in valid.iter().zip(&discarded).filter(|(_, d)| !**d) {
        if is_str(js::get(v, "verdict"), "REFUTED") {
            refute_weight += weight(v);
        }
    }
    for (v, _) in valid.iter().zip(&discarded).filter(|(_, d)| !**d) {
        if is_str(js::get(v, "verdict"), "NOT_REFUTED") {
            keep_weight += weight(v);
        }
    }
    let total = refute_weight + keep_weight;
    let refute_fraction = if total > 0.0 {
        refute_weight / total
    } else {
        0.0
    };
    let unverified = valid.is_empty();

    let pre_decided = valid
        .iter()
        .zip(&discarded)
        .any(|(v, d)| is_pre_decided(v) && citation_holds(v, finding_file, decisions) && !*d);
    let core_stands = valid.iter().any(|v| {
        js::truthy(js::get(v, "coreStands"))
            || is_str(js::get(v, "refutationType"), "TECHNICALLY_WRONG")
    });
    let had_external_voter = valid.iter().any(|v| {
        let source = js::get(v, "source");
        js::truthy(source) && !is_str(source, "claude")
    });

    Tally {
        valid: valid.iter().map(|v| (*v).clone()).collect(),
        failed_slots,
        failed_slots_detail,
        discarded_votes,
        unverified,
        had_independent_voter: valid.len() >= 2,
        had_external_voter,
        refute_fraction,
        survived: unverified || refute_fraction < T_KILL,
        borderline: !unverified && (refute_fraction - T_KILL).abs() <= DELTA,
        pre_decided,
        core_stands,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IntendedBy {
    pub file: Value,
    pub rule: Value,
}

/// What `placeSurvivor` returns.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    pub placement: Option<&'static str>,
    pub intended_by: Option<IntendedBy>,
    pub proposed_severity: Option<&'static str>,
}

const SEVERITIES: [&str; 3] = ["nit", "important", "critical"];

/// `PLACEMENT_SEVERITY_RANK[v]`: the property key is `String(v)`, so `["nit"]`
/// ranks as `"nit"`.
fn severity_rank(v: Option<&Value>) -> Option<usize> {
    let key = js_ext::to_js_string(v);
    SEVERITIES.iter().position(|s| *s == key)
}

/// `placeSurvivor`.
pub fn place_survivor(votes: &[Value], finding: &Value, line_added: bool) -> Placement {
    let live: Vec<&Value> = votes
        .iter()
        .filter(|v| {
            js::truthy(Some(v))
                && !is_str(js::get(v, "slotStatus"), "UNAVAILABLE")
                && is_string(js::get(v, "introducedByPr"))
        })
        .collect();
    let mut kept = Placement::default();
    if live.is_empty() {
        return kept;
    }
    // `x > live.length / 2` on whole numbers.
    let majority = |n: usize| n * 2 > live.len();
    let own = severity_rank(js::get(finding, "severity"));
    let lower: Vec<usize> = live
        .iter()
        .filter_map(|v| severity_rank(js::get(v, "proposedSeverity")))
        .filter(|r| own.is_some_and(|o| *r < o))
        .collect();
    if majority(lower.len()) {
        kept.proposed_severity = lower.iter().max().map(|&top| SEVERITIES[top]);
    }
    if live.len() < 2 {
        return kept;
    }
    let intended_by = |v: &Value| js::get(v, "intendedBy").cloned();
    let intended: Vec<&Value> = live
        .iter()
        .copied()
        .filter(|v| {
            let rule = js::get(v, "intendedBy").and_then(|i| js::get(i, "rule"));
            !js::trim(&string_or_empty(rule)).is_empty()
        })
        .collect();
    if majority(intended.len()) {
        let first = intended_by(intended[0]).unwrap_or(Value::Null);
        return Placement {
            placement: Some("intended"),
            intended_by: Some(IntendedBy {
                file: or(js::get(&first, "file"), Value::Null),
                rule: js::get(&first, "rule").cloned().unwrap_or(Value::Null),
            }),
            ..kept
        };
    }
    let says_no = live
        .iter()
        .filter(|v| is_str(js::get(v, "introducedByPr"), "no"))
        .count();
    if !line_added && majority(says_no) {
        return Placement {
            placement: Some("pre-existing"),
            ..kept
        };
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn v(source: &str, verdict: &str, confidence: f64) -> Value {
        json!({"source": source, "verdict": verdict, "confidence": confidence, "slotStatus": "OK"})
    }

    #[test]
    fn a_one_one_tie_at_equal_confidence_kills_and_splits() {
        let votes = [
            v("claude", "NOT_REFUTED", 0.85),
            v("codex", "REFUTED", 0.85),
        ];
        let t = tally_votes(&votes, &[], None, &json!([]), true);
        assert!(!t.survived && t.borderline && verdict_split(&t));
        assert_eq!(
            should_escalate(&t, &json!({"touchesHighRiskSurface": false}), 1),
            Some(json!(true))
        );
    }

    #[test]
    fn gemini_is_capped() {
        let votes = [v("claude", "NOT_REFUTED", 0.8), v("gemini", "REFUTED", 1.0)];
        let t = tally_votes(&votes, &[], None, &json!([]), true);
        assert!((t.refute_fraction - 0.7 / 1.5).abs() < 1e-12);
    }

    #[test]
    fn paths_match_on_whole_segments() {
        assert!(same_path(Some("src/jobs/worker.ts"), Some("worker.ts")));
        assert!(!same_path(
            Some("src/jobs/accounts_worker.ts"),
            Some("worker.ts")
        ));
    }

    #[test]
    fn camel_case_matches_do_not_overlap() {
        assert_eq!(
            topic_words("retryDuringRedisStartup"),
            ["retry", "during", "redis", "startup"]
        );
        assert_eq!(topic_words("abcdEFGH"), ["abcd", "efgh"]);
    }
}
