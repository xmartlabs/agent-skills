//! Port of the REPORT-FIDELITY-CHECKS region: three facts the synthesis
//! prompt tells the model to print, checked as substrings of the posted body,
//! and one marker it must not print on a gate-demoted finding.
//!
//! The inputs are fields of a finding's metrics row, read as JSON. A
//! non-string value is searched for as JavaScript's `String(value)` and comes
//! back unchanged in the result.

use serde_json::Value;

use super::js_ext;
use crate::js;

fn missing_if_absent(needle: &Value, report_text: &str) -> Vec<Value> {
    if report_text.contains(&js_ext::to_js_string(Some(needle))) {
        Vec::new()
    } else {
        vec![js_ext::js_order(needle.clone())]
    }
}

/// Ports `killReasonMissingFromReportText`. A falsy kill reason is reported as
/// `uncategorized`.
pub fn kill_reason_missing_from_report_text(
    kill_reason: Option<&Value>,
    report_text: &str,
) -> Vec<Value> {
    let expected = match kill_reason {
        Some(v) if js::truthy(Some(v)) => v.clone(),
        _ => Value::String("uncategorized".into()),
    };
    missing_if_absent(&expected, report_text)
}

/// Ports `mergedFromAnnotationMissingFromReportText`. Applies only to a
/// finding merged from two or more lenses.
pub fn merged_from_annotation_missing_from_report_text(
    merged_from: Option<&Value>,
    report_text: &str,
) -> Vec<Value> {
    let lenses: &[Value] = match merged_from {
        Some(Value::Array(a)) => a,
        _ => &[],
    };
    if lenses.len() < 2 {
        return Vec::new();
    }
    let mut missing = Vec::new();
    if !report_text.contains("(found by:") {
        missing.push(Value::String("(found by:".into()));
    }
    for lens in lenses {
        missing.extend(missing_if_absent(lens, report_text));
    }
    missing
}

/// Ports `groundedDiagnosticMissingFromReportText`. Applies only to a
/// typecheck-confirmed finding with a diagnostic.
pub fn grounded_diagnostic_missing_from_report_text(
    grounded: Option<&Value>,
    report_text: &str,
) -> Vec<Value> {
    let Some(g) = grounded.filter(|g| js::truthy(Some(g))) else {
        return Vec::new();
    };
    if js::get(g, "verdict") != Some(&Value::String("confirmed".into())) {
        return Vec::new();
    }
    match js::get(g, "diagnostic") {
        Some(d) if js::truthy(Some(d)) => missing_if_absent(d, report_text),
        _ => Vec::new(),
    }
}

/// A line that starts a block: a heading, a bold-only title or a numbered item.
fn starts_block(line: &str) -> bool {
    let t = line.trim_end_matches([' ', '\t', '\r']);
    let hashes = line.len() - line.trim_start_matches('#').len();
    let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    ((1..=6).contains(&hashes) && line.as_bytes().get(hashes) == Some(&b' '))
        || (js::utf16_len(t) >= 4 && t.starts_with("**") && t.ends_with("**"))
        || (digits >= 1 && line[digits..].starts_with(". "))
}

/// Ports `unexaminedWordingInReportText`. Applies only to a gate-demoted
/// finding whose panel voted, and searches only that finding's own block.
pub fn unexamined_wording_in_report_text(finding: &Value, report_text: &str) -> Vec<Value> {
    let gated = js::truthy(js::get(finding, "verifiedByGateNote"));
    let voted = matches!(js::get(finding, "votes"), Some(Value::Array(v)) if !v.is_empty());
    if !gated || !voted {
        return Vec::new();
    }
    let file = match js::get(finding, "file") {
        Some(Value::String(f)) if !f.is_empty() => f,
        _ => return Vec::new(),
    };
    let line = js::get(finding, "line");
    if !js::is_integer(line) {
        return Vec::new();
    }
    let location = format!("{file}:{}", js_ext::to_js_string(line));
    let lines: Vec<&str> = report_text.split('\n').collect();
    let mut starts = vec![0];
    let mut fenced = false;
    for (i, line) in lines.iter().enumerate() {
        if line.trim_start_matches(' ').starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if !fenced && i != 0 && starts_block(line) {
            starts.push(i);
        }
    }
    let (mut not_adjudicated, mut no_refuter) = (false, false);
    for (k, &s) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(lines.len());
        let block = &lines[s..end];
        if !block.iter().take(3).any(|l| l.contains(&location)) {
            continue;
        }
        let text = block.join("\n");
        not_adjudicated |= text.contains("NOT ADJUDICATED");
        no_refuter |= js::contains_ascii_ci(&text, "no refuter examined");
    }
    [
        (not_adjudicated, "NOT ADJUDICATED"),
        (no_refuter, "no refuter examined"),
    ]
    .into_iter()
    .filter(|(hit, _)| *hit)
    .map(|(_, p)| Value::String(p.into()))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_missing_kill_reason_defaults_to_uncategorized() {
        assert_eq!(
            kill_reason_missing_from_report_text(None, "x"),
            [json!("uncategorized")]
        );
        assert!(kill_reason_missing_from_report_text(Some(&json!(7)), "killed: 7").is_empty());
    }

    #[test]
    fn one_lens_needs_no_annotation() {
        assert!(
            merged_from_annotation_missing_from_report_text(Some(&json!(["a"])), "").is_empty()
        );
        assert_eq!(
            merged_from_annotation_missing_from_report_text(Some(&json!(["a", "b"])), "a"),
            [json!("(found by:"), json!("b")]
        );
    }

    #[test]
    fn only_a_confirmed_diagnostic_is_checked() {
        let g = json!({"verdict": "confirmed", "diagnostic": "TS2345: x"});
        assert_eq!(
            grounded_diagnostic_missing_from_report_text(Some(&g), ""),
            [json!("TS2345: x")]
        );
        let g = json!({"verdict": "refuted", "diagnostic": "TS2345: x"});
        assert!(grounded_diagnostic_missing_from_report_text(Some(&g), "").is_empty());
    }

    fn demoted() -> Value {
        json!({"file": "a.ts", "line": 62, "votes": [{}], "verifiedByGateNote": "Downgraded"})
    }

    #[test]
    fn the_marker_is_found_only_in_the_findings_own_block() {
        let report = "### Nits\n\n**Other**\n`b.ts:9`\n\n— NOT ADJUDICATED: no refuter examined this claim\n\n**Demoted**\n`a.ts:62`\n\nPanel: 2 NOT_REFUTED votes.\n";
        assert!(unexamined_wording_in_report_text(&demoted(), report).is_empty());
        let report = report.replace("2 NOT_REFUTED votes.", "No Refuter Examined it.");
        assert_eq!(
            unexamined_wording_in_report_text(&demoted(), &report),
            [json!("no refuter examined")]
        );
    }

    #[test]
    fn a_numbered_item_is_a_block_and_a_fence_is_not() {
        let report = "1. Demoted — a.ts:62 — why\n```\n# not a heading\n```\nNOT ADJUDICATED\n2. Next — b.ts:9\n";
        assert_eq!(
            unexamined_wording_in_report_text(&demoted(), report),
            [json!("NOT ADJUDICATED")]
        );
    }

    #[test]
    fn a_finding_without_votes_or_gate_note_is_not_checked() {
        let report = "**Demoted**\n`a.ts:62`\nNOT ADJUDICATED: no refuter examined this claim\n";
        let mut f = demoted();
        f["votes"] = json!([]);
        assert!(unexamined_wording_in_report_text(&f, report).is_empty());
        let mut f = demoted();
        f["verifiedByGateNote"] = Value::Null;
        assert!(unexamined_wording_in_report_text(&f, report).is_empty());
        assert_eq!(
            unexamined_wording_in_report_text(&demoted(), report).len(),
            2
        );
    }
}
