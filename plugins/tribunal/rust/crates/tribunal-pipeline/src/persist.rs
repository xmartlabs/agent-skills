//! The end of the Synthesize phase: the payload files `persist-run` reads,
//! and what the workflow makes of the line it prints back (the vote
//! reconciliation, the self-audit, the wall clock) before it checks the row
//! landed.

use serde_json::{json, Map, Value};
use tribunal_core::js;
use tribunal_core::lens::js_ext::to_js_string;

/// `externalVotes`: every non-Claude vote of every result, as the
/// reconciliation reads them.
#[must_use]
pub fn external_votes(results: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for v in results {
        let refined = js::truthy(js::get(v, "refined"));
        let Some(Value::Array(votes)) = js::get(v, "allVotes") else {
            continue;
        };
        for vote in votes {
            let source = js::get(vote, "source");
            if !js::truthy(source) || source == Some(&json!("claude")) {
                continue;
            }
            let mut m = Map::new();
            for k in ["slotId", "source", "verdict", "confidence"] {
                if let Some(x) = js::get(vote, k) {
                    m.insert(k.into(), x.clone());
                }
            }
            m.insert(
                "confidenceDeclared".into(),
                js::get(vote, "confidenceDeclared")
                    .filter(|x| !x.is_null())
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            if let Some(x) = js::get(vote, "slotStatus") {
                m.insert("slotStatus".into(), x.clone());
            }
            m.insert("round".into(), json!(if refined { 1 } else { 0 }));
            out.push(Value::Object(m));
        }
    }
    out
}

/// `persistSafeString`: control characters other than tab, newline and
/// carriage return become U+FFFD, and a backslash that would start a `\uXXXX`
/// escape becomes U+29F5. (A Rust string holds no lone surrogate.)
#[must_use]
pub fn persist_safe_string(s: &str) -> String {
    let chars: Vec<char> = s
        .chars()
        .map(|c| {
            if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect();
    chars
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let escape = c == '\\'
                && chars.get(i + 1) == Some(&'u')
                && chars
                    .get(i + 2..i + 6)
                    .is_some_and(|h| h.iter().all(char::is_ascii_hexdigit));
            if escape {
                '\u{29f5}'
            } else {
                c
            }
        })
        .collect()
}

fn safe(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(persist_safe_string(s)),
        Value::Array(a) => Value::Array(a.iter().map(safe).collect()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), safe(x))).collect()),
        other => other.clone(),
    }
}

/// `persistJson`: `JSON.stringify` with every string value made safe (keys
/// are not passed through the replacer).
#[must_use]
pub fn persist_json(v: &Value) -> String {
    js::stringify(&safe(v), "")
}

/// RECONCILIATION-RELAY's `resolveVoteReconciliation`.
#[must_use]
pub fn resolve_vote_reconciliation(relayed: Option<&Value>, has_external_votes: bool) -> Value {
    if let Some(Value::Object(r)) = relayed {
        if matches!(r.get("checked"), Some(Value::Number(_) | Value::Null)) {
            let mut out = r.clone();
            let or = |k: &str, d: Value| {
                r.get(k)
                    .filter(|x| js::truthy(Some(x)))
                    .cloned()
                    .unwrap_or(d)
            };
            out.insert("counts".into(), or("counts", json!({})));
            out.insert("mismatches".into(), or("mismatches", json!([])));
            return Value::Object(out);
        }
    }
    if !has_external_votes {
        return Value::Null;
    }
    let error = match relayed.and_then(|r| js::get(r, "error")) {
        Some(Value::String(e)) => e.clone(),
        _ => "reconcile-audit-persist did not return a usable result".into(),
    };
    json!({"checked": null, "ok": null, "counts": {}, "mismatches": [], "error": error})
}

fn number(v: Option<&Value>) -> f64 {
    v.and_then(Value::as_f64).unwrap_or(0.0)
}

/// The logs the workflow writes about the reconciliation.
pub fn reconciliation_logs(
    rec: &Value,
    has_external_votes: bool,
    vote_dir: &str,
    logs: &mut Vec<String>,
) {
    if !has_external_votes || rec.is_null() {
        return;
    }
    let checked = js::get(rec, "checked");
    if checked == Some(&Value::Null) {
        logs.push(
            "Vote reconciliation FAILED — treat every external vote on this run as unaudited"
                .into(),
        );
        return;
    }
    let counts = js::get(rec, "counts").cloned().unwrap_or(json!({}));
    let count = |k: &str| {
        js::get(&counts, k)
            .filter(|x| js::truthy(Some(x)))
            .cloned()
            .unwrap_or(json!(0))
    };
    let blocked = count("repeat-blocked");
    let classifier_failed = match js::get(rec, "classifierFailed") {
        Some(x) if !x.is_null() => x.clone(),
        _ => count("classifier-failed"),
    };
    let deviations = js::get(rec, "mismatches")
        .and_then(Value::as_array)
        .map_or(0, |m| {
            m.iter()
                .filter(|x| {
                    let kind = js::get(x, "kind");
                    kind != Some(&json!("repeat-blocked"))
                        && kind != Some(&json!("classifier-failed"))
                })
                .count()
        });
    let checked_s = to_js_string(checked);
    if deviations > 0 {
        logs.push(format!(
            "Vote reconciliation: {deviations} deviation(s) across {checked_s} external votes — {}. External votes on this run are NOT clean; read the artifacts in {vote_dir} before trusting a panel outcome that turned on an external vote.",
            js::stringify(&counts, "")
        ));
    } else if !js::truthy(Some(&classifier_failed)) {
        logs.push(format!(
            "Vote reconciliation: {checked_s} external votes match their CLI artifacts"
        ));
    }
    if js::truthy(Some(&classifier_failed)) {
        let audited = number(checked) - number(Some(&classifier_failed));
        logs.push(format!(
            "Vote reconciliation: {} of {checked_s} external vote(s) could NOT be audited — the verdict classifier failed to run. Treat them as unaudited: they are neither confirmed clean nor known deviant. Only the remaining {} were audited at all, and whatever this run says about vote integrity is a claim about those alone.",
            to_js_string(Some(&classifier_failed)),
            js::number_to_string(audited)
        ));
    }
    if js::truthy(Some(&blocked)) {
        logs.push(format!(
            "Driver protocol: {} repeat invocation(s) refused by the single-shot latch. The votes stand — this is a note about driver behaviour, not about the panel.",
            to_js_string(Some(&blocked))
        ));
    }
}

/// The row's `selfAudit`, and its log line.
pub fn self_audit(bookkeeping: Option<&Value>, logs: &mut Vec<String>) -> Value {
    let relayed = bookkeeping.and_then(|b| js::get(b, "selfAudit"));
    let audit = match relayed {
        Some(a) if js::truthy(Some(a)) && js::get(a, "passed").is_some() => a.clone(),
        _ => json!({
            "passed": null,
            "checked": 0,
            "violations": [{"id": "audit-unreadable", "detail": "the self-audit returned nothing usable"}],
        }),
    };
    if js::get(&audit, "passed") == Some(&json!(true)) {
        logs.push(format!(
            "Self-audit: {} invariant(s) hold on this run's own output",
            to_js_string(js::get(&audit, "checked"))
        ));
    } else {
        let violations = js::get(&audit, "violations")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let listed = violations
            .iter()
            .map(|x| {
                format!(
                    "{} ({})",
                    to_js_string(js::get(x, "id")),
                    to_js_string(js::get(x, "detail"))
                )
            })
            .collect::<Vec<_>>()
            .join(" | ");
        logs.push(format!(
            "SELF-AUDIT FAILED — {} invariant(s) violated: {listed}. Read this before trusting anything else in the report: these are defects in the HARNESS, not findings about the PR.",
            violations.len()
        ));
    }
    audit
}

/// `wallClockSeconds`, `wallClockNote` and `wallClockSource` as the workflow
/// copies them off the relayed line.
#[must_use]
pub fn wall_clock_fields(bookkeeping: Option<&Value>) -> [(&'static str, Value); 3] {
    let typed = |k: &str, number: bool| {
        bookkeeping
            .and_then(|b| js::get(b, k))
            .filter(|v| if number { v.is_number() } else { v.is_string() })
            .cloned()
            .unwrap_or(Value::Null)
    };
    [
        ("wallClockSeconds", typed("wallClockSeconds", true)),
        ("wallClockNote", typed("wallClockNote", false)),
        ("wallClockSource", typed("wallClockSource", false)),
    ]
}

/// `rowIsThisRun`.
#[must_use]
pub fn row_is_this_run(
    check: Option<&Value>,
    pr: &Value,
    repo: &str,
    head_sha: Option<&str>,
    findings: usize,
) -> bool {
    let Some(c) = check.filter(|c| js::truthy(Some(c))) else {
        return false;
    };
    let head = head_sha
        .filter(|s| !s.is_empty())
        .map_or(Value::Null, |s| json!(s));
    js::get(c, "found") != Some(&json!(false))
        && js::get(c, "pr").is_some_and(|p| p == pr)
        && js::get(c, "repo").is_none_or(|r| r.is_null() || r == &json!(repo))
        && js::get(c, "headSha").unwrap_or(&Value::Null) == &head
        && js::get(c, "findings").and_then(Value::as_f64) == Some(findings as f64)
}
