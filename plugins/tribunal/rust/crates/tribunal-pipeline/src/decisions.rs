//! What the reviewers are told was already decided, the reviews still asking
//! for changes, and the claims the operator wants re-tested. Ports the glue
//! from `HARVESTED_MAX` to `isRetestTarget`, around the PRIOR-REVIEWS region.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;
use tribunal_core::evidence::prior_reviews::{open_change_requests, post_gate, PostGate, Review};
use tribunal_core::js;
use tribunal_core::lens::js_ext::to_js_string;

pub const HARVESTED_MAX: usize = 4000;
pub const SUBSTANTIVE_REVIEW_BODY_MIN: usize = 200;

pub const PRECEDENCE_RULE: &str = "AUTHORITY IS ORDERED: an item in the OPERATOR block WINS over any contradicting item in the THREAD block. If the OPERATOR block marks something REOPENED / BACK IN SCOPE, it is NOT pre-decided — even if the THREAD block lists the same item as DEFERRED or BY-DESIGN.";

// JavaScript's /i without the u flag folds ASCII only, hence the -u group.
static SCAFFOLDING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"</?(?i-u:invoke|function_calls|parameter|inThreadDecisions|antml:[a-z_]+)[^>]*>")
        .expect("static regex")
});

#[derive(Debug, Clone, PartialEq)]
pub struct Decisions {
    pub operator_decisions: String,
    pub thread_decisions: String,
    pub combined_decisions: String,
    /// The block every lens reads under `<context>`; empty when nothing was
    /// decided.
    pub context_block: String,
    pub prior_reviews: Vec<Review>,
    pub prior_review_gate: PostGate,
    pub retest_claims: Vec<String>,
    pub logs: Vec<String>,
}

fn truthy_text(v: Option<&Value>) -> String {
    if js::truthy(v) {
        to_js_string(v)
    } else {
        String::new()
    }
}

fn sanitize(v: Option<&Value>) -> String {
    let text = truthy_text(v);
    js::trim(&SCAFFOLDING.replace_all(&text, "")).to_owned()
}

/// `s.slice(0, n)` in UTF-16 units. A cut through a surrogate pair keeps
/// neither half: a Rust string cannot hold the lone high surrogate
/// JavaScript would keep.
fn slice_utf16(s: &str, n: usize) -> String {
    let mut used = 0;
    let mut out = String::new();
    for c in s.chars() {
        used += c.len_utf16();
        if used > n {
            break;
        }
        out.push(c);
    }
    out
}

/// Ports `serializeThreadDecisions`.
pub fn serialize_thread_decisions(items: Option<&Value>) -> String {
    let Some(Value::Array(items)) = items else {
        return String::new();
    };
    let lines: Vec<String> = items
        .iter()
        .filter_map(|item| {
            let category = sanitize(js::get(item, "category"));
            let summary = sanitize(js::get(item, "summary"));
            let source = sanitize(js::get(item, "source"));
            if category.is_empty() || summary.is_empty() {
                return None;
            }
            let files: Vec<String> = match js::get(item, "files") {
                Some(Value::Array(fs)) => fs
                    .iter()
                    .map(|f| sanitize(Some(f)))
                    .filter(|f| !f.is_empty())
                    .collect(),
                _ => Vec::new(),
            };
            let scope = if files.is_empty() {
                String::new()
            } else {
                format!(" [files: {}]", files.join(", "))
            };
            let source = if source.is_empty() {
                String::new()
            } else {
                format!(" ({source})")
            };
            Some(format!("{category}: {summary}{scope}{source}"))
        })
        .collect();
    slice_utf16(&lines.join("\n"), HARVESTED_MAX)
}

fn is_line_space(c: char) -> bool {
    js::is_js_whitespace(c) || js::is_line_terminator(c)
}

fn strip_retest(line: &str) -> Option<&str> {
    let head = line.get(..7)?;
    head.eq_ignore_ascii_case("RETEST:")
        .then(|| line[7..].trim_start_matches(is_line_space))
}

/// The retest claims, lowercased: `args.retest` first, then every
/// `RETEST:` line of the operator's context. Claims shorter than eight
/// characters are dropped.
fn retest_claims(retest_arg: Option<&Value>, operator: &str) -> Vec<String> {
    let from_args: Vec<String> = match retest_arg {
        Some(Value::Array(a)) => a.iter().map(|s| truthy_text(Some(s))).collect(),
        _ => Vec::new(),
    };
    let from_context = operator
        .split('\n')
        .map(js::trim)
        .filter_map(strip_retest)
        .map(str::to_owned);
    from_args
        .into_iter()
        .chain(from_context)
        .map(|s| js::trim(&s).to_lowercase())
        .filter(|s| js::utf16_len(s) >= 8)
        .collect()
}

/// Ports `isRetestTarget`: a claim matches when the finding's text contains
/// it, or when at least 60% of its words of five or more characters (three
/// at least) appear there.
#[must_use]
pub fn is_retest_target(claims: &[String], finding: &Value) -> bool {
    if claims.is_empty() {
        return false;
    }
    let hay = format!(
        "{} {} {}",
        truthy_text(js::get(finding, "title")),
        truthy_text(js::get(finding, "file")),
        truthy_text(js::get(finding, "description"))
    )
    .to_lowercase();
    claims.iter().any(|claim| {
        if hay.contains(claim.as_str()) {
            return true;
        }
        let words: Vec<&str> = claim
            .split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || "._/-".contains(c)))
            .filter(|w| w.len() >= 5)
            .collect();
        if words.len() < 3 {
            return false;
        }
        let hits = words.iter().filter(|w| hay.contains(*w)).count();
        hits as f64 / words.len() as f64 >= 0.6
    })
}

/// Ports the glue. `orchestrator_context` is `--context` as given;
/// `retest_arg` is `args.retest`.
#[must_use]
pub fn build_decisions(
    orchestrator_context: Option<&str>,
    pr_context: &Value,
    retest_arg: Option<&Value>,
) -> Decisions {
    let mut logs = Vec::new();
    let operator_decisions = js::trim(orchestrator_context.unwrap_or("")).to_owned();
    let thread_decisions = serialize_thread_decisions(js::get(pr_context, "inThreadDecisions"));

    let substantive = js::get(pr_context, "priorReviews")
        .and_then(Value::as_array)
        .map_or(0, |rs| {
            rs.iter()
                .filter(|r| {
                    truthy_text(js::get(r, "state")).to_uppercase() != "APPROVED"
                        && js::utf16_len(&truthy_text(js::get(r, "summary")))
                            > SUBSTANTIVE_REVIEW_BODY_MIN
                })
                .count()
        });
    if substantive > 0 && thread_decisions.is_empty() {
        logs.push(format!(
            "WARNING: fetch-pr found {substantive} substantive prior review(s) but zero in-thread decisions extracted — worth a manual check."
        ));
    }

    let combined_decisions = [
        (!operator_decisions.is_empty()).then(|| {
            format!("[OPERATOR — explicit --context for this run, highest authority]\n{operator_decisions}")
        }),
        (!thread_decisions.is_empty()).then(|| {
            format!("[THREAD — harvested from this PR's review conversation; a CLAIM about what was decided, not a fact]\n{thread_decisions}")
        }),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n\n");
    let context_block = if combined_decisions.is_empty() {
        String::new()
    } else {
        format!("\n\nDecisions already made (do NOT re-litigate). A finding that merely repeats a DEFERRED/BY-DESIGN/SPLIT-OUT item is pre-decided, not a blocker. A \"RESOLVED in <sha>\" item is a claim: verify it still holds at HEAD — flag only if the fix is incomplete there.\n{PRECEDENCE_RULE}\n\n{combined_decisions}")
    };

    let raw_reviews = js::get(pr_context, "priorReviews");
    let prior_reviews = open_change_requests(raw_reviews.unwrap_or(&Value::Null));
    let prior_review_gate = post_gate(
        Some(&prior_reviews),
        js::get(pr_context, "viewer"),
        raw_reviews,
    );
    if !prior_reviews.is_empty() {
        let who: Vec<String> = prior_reviews
            .iter()
            .map(|r| match r.at.as_deref() {
                Some(at) if !at.is_empty() => format!("{} ({})", r.author, slice_utf16(at, 10)),
                _ => r.author.clone(),
            })
            .collect();
        logs.push(format!(
            "{} prior review(s) still requesting changes: {}",
            prior_reviews.len(),
            who.join(", ")
        ));
    }
    if let Some(s) = prior_review_gate
        .own_standing_review
        .as_ref()
        .filter(|_| prior_review_gate.status == "own-prior-review")
    {
        logs.push(format!(
            "{} already has a standing {} review on this PR ({}) — Step 4 must ask before posting, and compare against it rather than stacking a second review.",
            s.author,
            s.state,
            slice_utf16(s.at.as_deref().unwrap_or(""), 10)
        ));
    }

    let retest_claims = retest_claims(retest_arg, &operator_decisions);
    if !retest_claims.is_empty() {
        logs.push(format!(
            "{} claim(s) forced to panel by --context RETEST",
            retest_claims.len()
        ));
    }
    Decisions {
        operator_decisions,
        thread_decisions,
        combined_decisions,
        context_block,
        prior_reviews,
        prior_review_gate,
        retest_claims,
        logs,
    }
}
