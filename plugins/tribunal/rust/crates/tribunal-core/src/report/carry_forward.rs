//! Port of the CARRY-FORWARD region: what happens to the previous review's
//! findings when an incremental run re-reads only part of the PR, how a
//! prior-finding verification is graded, and the verdict that has to survive
//! a scoped run that finds nothing new.
//!
//! Prior findings come from the baseline metrics row and verifier output from
//! a model, so both are `Value`s and every field is read the way JavaScript
//! reads it: truthiness, `String(x)`, `Number(x)` and `===` from `js_ext`.
//! Objects these functions return are put in JavaScript key order, since the
//! region builds them by spreading the input.

use serde_json::{json, Map, Value};

use super::js_ext;
use crate::js;

pub const CF_TITLE_OVERLAP_MIN: f64 = 0.4;
pub const CF_LINE_WINDOW: f64 = 40.0;
pub const PRIOR_QUOTE_MIN: usize = 12;

const CF_STOP: &[&str] = &[
    "the", "a", "an", "of", "in", "on", "to", "for", "is", "are", "and", "or", "not", "with",
    "this", "that", "its", "it", "at", "by", "from", "as", "be", "can", "when", "if", "into",
];

fn s(v: &str) -> Value {
    Value::String(v.into())
}

/// Ports `cfTokens`: the distinct title tokens in first-seen order (the
/// region's `Set`). `toLowerCase` is the full Unicode mapping, which matters
/// only where it yields ASCII: U+212A becomes `k`, U+0130 becomes `i` plus a
/// combining dot. Everything outside `[a-z0-9_]` splits.
pub fn cf_tokens(title: Option<&Value>) -> Vec<String> {
    let text = if js::truthy(title) {
        js_ext::to_js_string(title).to_lowercase()
    } else {
        String::new()
    };
    let mut out: Vec<String> = Vec::new();
    for t in text.split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')) {
        if t.len() > 2 && !CF_STOP.contains(&t) && !out.iter().any(|o| o == t) {
            out.push(t.to_owned());
        }
    }
    out
}

fn file_or_empty(f: &Value) -> Value {
    match js::get(f, "file") {
        Some(v) if js::truthy(Some(v)) => v.clone(),
        _ => s(""),
    }
}

/// Ports `cfSameFinding`: same file, and either lines within the window or
/// enough shared title tokens.
pub fn cf_same_finding(a: &Value, b: &Value) -> bool {
    if !js::truthy(Some(a)) || !js::truthy(Some(b)) {
        return false;
    }
    if !js_ext::strict_eq(Some(&file_or_empty(a)), Some(&file_or_empty(b))) {
        return false;
    }
    let la = js_ext::to_number(js::get(a, "line"));
    let lb = js_ext::to_number(js::get(b, "line"));
    if la.is_finite() && lb.is_finite() && la > 0.0 && lb > 0.0 && (la - lb).abs() <= CF_LINE_WINDOW
    {
        return true;
    }
    let ta = cf_tokens(js::get(a, "title"));
    let tb = cf_tokens(js::get(b, "title"));
    if ta.is_empty() || tb.is_empty() {
        return false;
    }
    let shared = ta.iter().filter(|t| tb.contains(t)).count();
    shared as f64 / ta.len().min(tb.len()) as f64 >= CF_TITLE_OVERLAP_MIN
}

/// Ports `incrementalFallbackFor`. `None` when there was no baseline or the
/// scope was usable; otherwise the record of a scoped re-review that was
/// expected and not delivered. Arguments are `None` for `undefined`.
pub fn incremental_fallback_for(
    prior_sha: Option<&Value>,
    scope_usable: Option<&Value>,
    unknown_path_count: Option<&Value>,
    probe_reason: Option<&Value>,
    baseline_source: Option<&Value>,
) -> Option<Value> {
    if !js::truthy(prior_sha) || js::truthy(scope_usable) {
        return None;
    }
    let or = |v: Option<&Value>, fallback: Value| match v {
        Some(v) if js::truthy(Some(v)) => v.clone(),
        _ => fallback,
    };
    let reason = if js::truthy(unknown_path_count) {
        s(&format!(
            "{} in-scope path(s) absent from the PR's file list",
            js_ext::to_js_string(unknown_path_count)
        ))
    } else {
        or(probe_reason, s("scope unusable, no reason given"))
    };
    Some(js_ext::js_order(json!({
        "expected": true,
        "baseline": prior_sha.cloned().unwrap_or(Value::Null),
        "source": or(baseline_source, Value::Null),
        "reason": reason,
    })))
}

/// Ports `gradePriorVerification`: a FIXED or STILL_OPEN claim keeps its
/// verdict only with a quote of at least `PRIOR_QUOTE_MIN` UTF-16 units after
/// a JavaScript `trim`. Returns `{verdict, quote, reason}`.
pub fn grade_prior_verification(raw: Option<&Value>) -> Value {
    let field = |k: &str| -> String {
        match raw
            .filter(|r| js::truthy(Some(r)))
            .and_then(|r| js::get(r, k))
        {
            Some(v) if js::truthy(Some(v)) => js::trim(&js_ext::to_js_string(Some(v))).to_owned(),
            _ => String::new(),
        }
    };
    let verdict = match raw
        .filter(|r| js::truthy(Some(r)))
        .and_then(|r| js::get(r, "verdict"))
    {
        Some(Value::String(v)) if v == "FIXED" || v == "STILL_OPEN" => Some(v.as_str()),
        _ => None,
    };
    let quote = field("quote");
    let reason = field("reason");
    match verdict {
        Some(v) if js::utf16_len(&quote) >= PRIOR_QUOTE_MIN => {
            json!({"verdict": v, "quote": quote, "reason": reason})
        }
        Some(v) => {
            let suffix = if reason.is_empty() {
                String::new()
            } else {
                format!(" — {reason}")
            };
            json!({
                "verdict": "CANNOT_TELL",
                "quote": "",
                "reason": format!("claimed {v} with no quote to support it{suffix}"),
            })
        }
        None => json!({
            "verdict": "CANNOT_TELL",
            "quote": quote,
            "reason": if reason.is_empty() { "the verifier returned no usable verdict".to_owned() } else { reason },
        }),
    }
}

fn override_direction(f: &Value) -> Option<&str> {
    let o = js::get(f, "operatorOverride").filter(|o| js::truthy(Some(o)))?;
    js::get(o, "direction")?.as_str()
}

/// The severity a person lowered the finding to with `tribunal override
/// --severity`. Rust only: the Workflow does not read `operatorSeverity`.
fn operator_severity(f: &Value) -> Option<&Value> {
    let o = js::get(f, "operatorSeverity").filter(|o| js::truthy(Some(o)))?;
    js::get(o, "severity").filter(|v| v.is_string())
}

/// Ports `countsAsBlocker`: a human's override outranks the panel's flags. A
/// finding a person lowered with `operatorSeverity` no longer blocks.
pub fn counts_as_blocker(f: &Value) -> bool {
    match override_direction(f) {
        Some("rejected" | "withdrawn" | "dropped") => false,
        Some("reinstated") => true,
        _ if operator_severity(f).is_some() => false,
        _ => {
            js::truthy(Some(f))
                && (js::truthy(js::get(f, "blocking"))
                    || js::get(f, "severity") == Some(&s("critical")))
        }
    }
}

/// Ports `priorGrade`: a reinstated finding grades as critical, one a person
/// lowered grades at the severity they gave it; otherwise the finding's own
/// severity (`null` when absent), or the finding itself when it is falsy, as
/// `f && f.severity` does.
pub fn prior_grade(f: &Value) -> Value {
    if override_direction(f) == Some("reinstated") {
        return s("critical");
    }
    if let Some(sev) = operator_severity(f) {
        return sev.clone();
    }
    if !js::truthy(Some(f)) {
        return f.clone();
    }
    js_ext::js_order(js::get(f, "severity").cloned().unwrap_or(Value::Null))
}

/// Ports `unresolvedPriorBlockersFrom`. A re-examined finding stops counting
/// only when its verification says FIXED. Both lists are built by spreading
/// prior findings, so their elements are objects; a `null` element (which
/// throws in JavaScript) cannot occur.
pub fn unresolved_prior_blockers_from(carried: &[Value], re_examined: &[Value]) -> Vec<Value> {
    let not_fixed = |f: &&Value| {
        let verdict = js::get(f, "verification").and_then(|v| js::get(v, "verdict"));
        verdict != Some(&s("FIXED"))
    };
    carried
        .iter()
        .chain(re_examined.iter().filter(not_fixed))
        .filter(|f| counts_as_blocker(f))
        .map(|f| js_ext::js_order(f.clone()))
        .collect()
}

/// Ports `verdictFor`. `None` stands for an `undefined` list.
pub fn verdict_for(
    surviving: Option<&[Value]>,
    unresolved_prior_severities: Option<&[Value]>,
) -> &'static str {
    let survivors = surviving.unwrap_or(&[]);
    let priors = unresolved_prior_severities.unwrap_or(&[]);
    let severity_is = |f: &Value, sev: &str| js::get(f, "severity") == Some(&s(sev));
    let blocking: Vec<&Value> = survivors
        .iter()
        .filter(|f| {
            js::truthy(Some(f))
                && !js::truthy(js::get(f, "verifiedByGateNote"))
                && js::get(f, "adjudicated") != Some(&Value::Bool(false))
        })
        .collect();
    let critical_survivor = blocking.iter().any(|f| severity_is(f, "critical"));
    let escalated_important = blocking.iter().any(|f| {
        severity_is(f, "important")
            && (js::truthy(js::get(f, "isBehaviorChanging"))
                || js::truthy(js::get(f, "touchesHighRiskSurface")))
    });
    let critical_prior = priors.contains(&s("critical"));
    if critical_survivor || escalated_important || critical_prior {
        return "REQUEST CHANGES";
    }
    let important = survivors.iter().any(|f| {
        js::truthy(Some(f)) && (severity_is(f, "important") || severity_is(f, "critical"))
    }) || !priors.is_empty();
    if important {
        "COMMENT"
    } else {
        "APPROVE"
    }
}

/// The four buckets `classifyPriorFindings` returns.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PriorClassification {
    pub carried: Vec<Value>,
    pub moot: Vec<Value>,
    pub still_open: Vec<Value>,
    pub not_resurfaced: Vec<Value>,
}

impl PriorClassification {
    /// The JavaScript object, in its key order.
    pub fn to_json(&self) -> Value {
        json!({
            "carried": self.carried,
            "moot": self.moot,
            "stillOpen": self.still_open,
            "notResurfaced": self.not_resurfaced,
        })
    }
}

/// `(list || []).includes(file)` with a truthy `file`: an array compares with
/// `===`, a non-empty string searches for `String(file)`, anything else
/// throws. The falsy check comes first: `""` becomes `[]`, so a file whose
/// `String()` is empty (`[]`) is not found in it.
fn in_list(list: Option<&Value>, file: &Value) -> Result<bool, String> {
    match list {
        v if !js::truthy(v) => Ok(false),
        Some(Value::Array(items)) => {
            Ok(items.iter().any(|i| js_ext::strict_eq(Some(i), Some(file))))
        }
        Some(Value::String(text)) => Ok(text.contains(&js_ext::to_js_string(Some(file)))),
        _ => Err("(list || []).includes is not a function".into()),
    }
}

/// `Object.prototype` members as seen through a plain object's lookup, with
/// their function `length`. `__proto__` resolves to `Object.prototype`
/// itself, whose `length` is undefined.
fn inherited_length(key: &str) -> Option<Option<u32>> {
    Some(match key {
        "constructor"
        | "hasOwnProperty"
        | "__lookupGetter__"
        | "__lookupSetter__"
        | "isPrototypeOf"
        | "propertyIsEnumerable" => Some(1),
        "__defineGetter__" | "__defineSetter__" => Some(2),
        "toString" | "valueOf" | "toLocaleString" => Some(0),
        "__proto__" => None,
        _ => return None,
    })
}

/// `related && related.length ? related.map((r) => r.file) : <carry>` for
/// `(importAdjacency || {})[file]`. `Ok(None)` means carry the finding.
fn related_files(
    adjacency: Option<&Map<String, Value>>,
    file: &Value,
) -> Result<Option<Vec<Value>>, String> {
    let key = js_ext::to_js_string(Some(file));
    let not_a_function = || Err("related.map is not a function".to_owned());
    let Some(related) = adjacency.and_then(|m| m.get(&key)) else {
        return match inherited_length(&key) {
            Some(Some(n)) if n > 0 => not_a_function(),
            _ => Ok(None),
        };
    };
    let length = match related {
        Value::String(t) => Some(json!(js::utf16_len(t))),
        Value::Array(a) => Some(json!(a.len())),
        Value::Object(_) => js::get(related, "length").cloned(),
        _ => None,
    };
    if !js::truthy(Some(related)) || !js::truthy(length.as_ref()) {
        return Ok(None);
    }
    let Value::Array(items) = related else {
        return not_a_function();
    };
    items
        .iter()
        .map(|r| match r {
            Value::Null => Err("Cannot read properties of null (reading 'file')".to_owned()),
            r => Ok(js::get(r, "file").cloned().unwrap_or(Value::Null)),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// `for (const p of priorFindings || [])`: a string iterates by code point.
fn iterate(prior_findings: Option<&Value>) -> Result<Vec<Value>, String> {
    match prior_findings {
        Some(Value::Array(a)) => Ok(a.clone()),
        Some(Value::String(t)) => Ok(t.chars().map(|c| Value::String(c.to_string())).collect()),
        v if !js::truthy(v) => Ok(Vec::new()),
        v => Err(format!("{} is not iterable", js_ext::to_js_string(v))),
    }
}

fn with(p: &Value, extra: &[(&str, Value)]) -> Value {
    let mut m = js_ext::spread(p);
    for (k, v) in extra {
        m.insert((*k).to_owned(), v.clone());
    }
    js_ext::js_order(Value::Object(m))
}

/// Ports `classifyPriorFindings`. `scope` is `{unchanged, gone, changed,
/// added}`; `import_adjacency` maps a file to `[{file, ...}]`. `Err` where
/// JavaScript throws: a `null` prior finding, a findings list that is not
/// iterable, a scope list that is neither an array nor a string, an
/// adjacency entry that has a length but no `map`, or a `null` adjacency
/// item.
pub fn classify_prior_findings(
    scope: Option<&Value>,
    prior_findings: Option<&Value>,
    current_findings: &[Value],
    import_adjacency: Option<&Map<String, Value>>,
) -> Result<PriorClassification, String> {
    let mut out = PriorClassification::default();
    let Some(scope) = scope.filter(|s| js::truthy(Some(s))) else {
        return Ok(out);
    };
    let list = |k: &str| js::get(scope, k);
    for p in iterate(prior_findings)? {
        if p.is_null() {
            return Err("Cannot read properties of null (reading 'file')".into());
        }
        let file = match js::get(&p, "file") {
            Some(f) if js::truthy(Some(f)) => f,
            _ => {
                out.carried.push(with(
                    &p,
                    &[("carryReason", s("no file recorded on the prior finding"))],
                ));
                continue;
            }
        };
        if in_list(list("unchanged"), file)? {
            match related_files(import_adjacency, file)? {
                Some(related) => out.not_resurfaced.push(with(
                    &p,
                    &[
                        ("escalatedByImportHeuristic", Value::Bool(true)),
                        ("relatedFiles", Value::Array(related)),
                    ],
                )),
                None => out.carried.push(with(
                    &p,
                    &[("carryReason", s("file unchanged since that review"))],
                )),
            }
            continue;
        }
        if in_list(list("gone"), file)? {
            out.moot.push(with(
                &p,
                &[("carryReason", s("file is no longer part of this PR's diff"))],
            ));
            continue;
        }
        if in_list(list("changed"), file)? || in_list(list("added"), file)? {
            match current_findings.iter().find(|c| cf_same_finding(&p, c)) {
                Some(m) => {
                    let mut spread = js_ext::spread(&p);
                    // `matchedTitle: undefined` keeps the key, and JSON drops it.
                    match js::get(m, "title") {
                        Some(t) => {
                            spread.insert("matchedTitle".into(), t.clone());
                        }
                        None => {
                            spread.shift_remove("matchedTitle");
                        }
                    }
                    out.still_open.push(js_ext::js_order(Value::Object(spread)));
                }
                None => out.not_resurfaced.push(with(&p, &[])),
            }
            continue;
        }
        out.carried.push(with(
            &p,
            &[(
                "carryReason",
                s("file not present in this run's scope buckets"),
            )],
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_words_and_short_tokens_drop() {
        assert_eq!(
            cf_tokens(Some(&json!("The 'failed' listener is OK with x_y2"))),
            ["failed", "listener", "x_y2"]
        );
        assert_eq!(cf_tokens(Some(&json!("\u{212A}ey"))), ["key"]);
        assert_eq!(cf_tokens(Some(&json!({}))), ["object"]);
    }

    #[test]
    fn same_file_unrelated_titles_do_not_match() {
        let a = json!({"file": "w.ts", "title": "Worker's 'failed' listener flips the row"});
        let b = json!({"file": "w.ts", "title": "contactName ?? undefined triggers a lookup"});
        assert!(!cf_same_finding(&a, &b));
        let c = json!({"file": "w.ts", "line": "0x1E", "title": "x"});
        let d = json!({"file": "w.ts", "line": 60, "title": "y"});
        assert!(cf_same_finding(&c, &d));
    }

    #[test]
    fn a_fix_claim_needs_a_quote() {
        let g = grade_prior_verification(Some(
            &json!({"verdict": "FIXED", "reason": "looks handled"}),
        ));
        assert_eq!(
            g,
            json!({"verdict": "CANNOT_TELL", "quote": "", "reason": "claimed FIXED with no quote to support it — looks handled"})
        );
    }

    #[test]
    fn a_lowered_finding_stops_blocking_and_grades_at_its_new_severity() {
        let f = json!({"severity": "critical", "blocking": true, "operatorSeverity": {"severity": "nit", "from": "critical"}});
        assert!(!counts_as_blocker(&f));
        assert_eq!(prior_grade(&f), s("nit"));
        let reinstated = json!({"severity": "important", "blocking": false, "operatorSeverity": {"severity": "nit"}, "operatorOverride": {"direction": "reinstated"}});
        assert!(counts_as_blocker(&reinstated));
    }

    #[test]
    fn a_reinstated_finding_blocks_and_grades_critical() {
        let f = json!({"severity": "important", "blocking": false, "operatorOverride": {"direction": "reinstated"}});
        assert!(counts_as_blocker(&f));
        assert_eq!(prior_grade(&f), json!("critical"));
        assert_eq!(
            verdict_for(Some(&[]), Some(&[prior_grade(&f)])),
            "REQUEST CHANGES"
        );
    }

    #[test]
    fn a_changed_file_finding_that_did_not_resurface_is_not_dropped() {
        let scope = json!({"changed": ["w.ts"], "unchanged": ["u.ts"], "gone": [], "added": []});
        let prior = json!([
            {"file": "u.ts", "title": "a"},
            {"file": "w.ts", "title": "Worker flips the row", "severity": "critical"},
        ]);
        let out = classify_prior_findings(Some(&scope), Some(&prior), &[], None).unwrap();
        assert_eq!(out.carried.len(), 1);
        assert_eq!(out.not_resurfaced[0]["severity"], json!("critical"));
    }
}
