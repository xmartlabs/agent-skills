//! Port of the VERIFIED-GATE region: a finding about to be published as
//! Blocking must cite the code its claim rests on, or it is demoted.
//!
//! Findings are model output, and the gate's job is to judge their shape, so
//! it reads them as JSON values rather than a typed struct: a `line` that
//! arrives as a string has to produce the same problem text it did in
//! JavaScript, not a deserialization error.

use serde_json::{Map, Value};

use crate::js;

pub const MIN_QUOTE_CHARS: usize = 10;

pub fn is_gated_finding(f: &Value) -> bool {
    let not_nit = js::get(f, "severity") != Some(&Value::String("nit".into()));
    let adjudicated = js::get(f, "adjudicated") == Some(&Value::Bool(true));
    js::truthy(Some(f)) && not_nit && adjudicated
}

pub fn anchor_problems(anchor: &Value, i: usize) -> Vec<String> {
    let at = format!("verifiedBy[{i}]");
    if !js::is_object_like(anchor) {
        return vec![format!("{at} is not an object")];
    }
    let mut problems = Vec::new();
    let file_ok = matches!(js::get(anchor, "file"), Some(Value::String(s)) if !s.is_empty());
    if !file_ok {
        problems.push(format!("{at}.file is missing"));
    }
    let line = js::get(anchor, "line");
    let line_ok = js::is_integer(line) && line.and_then(Value::as_f64).is_some_and(|l| l >= 1.0);
    if !line_ok {
        problems.push(format!("{at}.line is not a positive integer"));
    }
    let quote = match js::get(anchor, "quote") {
        Some(Value::String(s)) => js::trim(s),
        _ => "",
    };
    if js::utf16_len(quote) < MIN_QUOTE_CHARS {
        problems.push(format!(
            "{at}.quote is shorter than {MIN_QUOTE_CHARS} characters (paste the code the claim rests on, not a label)"
        ));
    }
    problems
}

pub fn finding_problems(f: &Value) -> Vec<String> {
    if !is_gated_finding(f) {
        return Vec::new();
    }
    match js::get(f, "verifiedBy") {
        Some(Value::Array(anchors)) if !anchors.is_empty() => anchors
            .iter()
            .enumerate()
            .flat_map(|(i, a)| anchor_problems(a, i))
            .collect(),
        _ => vec![
            "verifiedBy is missing or empty — a blocking finding must cite the code its claim depends on"
                .to_owned(),
        ],
    }
}

/// Demotes each gated finding whose anchors fail, keeping every other field
/// and its position. A demoted finding is an object by construction: only an
/// object can carry `adjudicated: true`.
pub fn demote_unanchored(findings: &[Value]) -> Vec<Value> {
    findings
        .iter()
        .map(|f| {
            let problems = finding_problems(f);
            if problems.is_empty() {
                return f.clone();
            }
            let mut out: Map<String, Value> = f.as_object().cloned().unwrap_or_default();
            out.insert("adjudicated".into(), Value::Bool(false));
            out.insert(
                "verifiedByGateNote".into(),
                Value::String(format!("Downgraded from Blocking: {}", problems.join("; "))),
            );
            Value::Object(out)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn anchored() -> Value {
        json!({"file": "prisma/schema.prisma", "line": 1374, "quote": "enum upload_type {"})
    }

    #[test]
    fn a_nit_or_unadjudicated_finding_is_not_gated() {
        assert!(!is_gated_finding(
            &json!({"severity": "nit", "adjudicated": true})
        ));
        assert!(!is_gated_finding(&json!({"severity": "important"})));
        assert!(is_gated_finding(&json!({"adjudicated": true})));
    }

    #[test]
    fn a_cited_blocking_finding_passes_untouched() {
        let f = json!({"severity": "important", "adjudicated": true, "verifiedBy": [anchored()]});
        assert_eq!(demote_unanchored(std::slice::from_ref(&f)), vec![f]);
    }

    #[test]
    fn a_missing_anchor_demotes_and_keeps_field_order() {
        let f = json!({"title": "t", "adjudicated": true, "severity": "critical"});
        let out = demote_unanchored(&[f]);
        let keys: Vec<&str> = out[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["title", "adjudicated", "severity", "verifiedByGateNote"]
        );
        assert_eq!(out[0]["adjudicated"], json!(false));
    }

    #[test]
    fn a_string_line_and_a_label_quote_each_report() {
        let a = json!({"file": "a.ts", "line": "12", "quote": "label"});
        assert_eq!(
            anchor_problems(&a, 0),
            [
                "verifiedBy[0].line is not a positive integer",
                "verifiedBy[0].quote is shorter than 10 characters (paste the code the claim rests on, not a label)",
            ]
        );
    }
}
