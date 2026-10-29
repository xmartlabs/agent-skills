//! Port of `lib/check-report-fidelity.mjs`: the report-fidelity gate. Every
//! finding gets its four `*MissingFromReport` arrays and
//! `unexaminedWordingInReport`, computed against the
//! body GitHub will receive (the `### Draft comments` section stripped).
//!
//! The comparisons are the workflow regions the script slices out of
//! `workflows/tribunal.js` (MERGED-LOCATIONS-CHECK, REPORT-FIDELITY-CHECKS,
//! DRAFT-COMMENTS-STRIP), reused from `tribunal_core::report`. What those
//! ports leave out is the throws, which this script does not catch: they are
//! reproduced here in evaluation order, before each region runs.

use serde_json::Value;
use tribunal_core::js;
use tribunal_core::report::draft_comments_strip::strip_draft_comments_from_body;
use tribunal_core::report::merged_locations_check::merged_locations_missing_from_report_text;
use tribunal_core::report::report_fidelity_checks::{
    grounded_diagnostic_missing_from_report_text, kill_reason_missing_from_report_text,
    merged_from_annotation_missing_from_report_text, unexamined_wording_in_report_text,
};

use crate::operator::{json_parse, read_file_utf8, CliOutput};
use crate::resolve_evidence_line::to_js_string;

const USAGE: &str = "usage: check-report-fidelity.mjs <report-file> < metrics.json > metrics.json";

fn type_error(msg: &str) -> String {
    format!("TypeError: {msg}")
}

/// `String(v)` where the script only needs it not to throw.
fn stringable(v: Option<&Value>) -> Result<(), String> {
    to_js_string(v).map(drop).map_err(|e| type_error(&e))
}

fn or_falsy(v: Option<&Value>) -> Option<&Value> {
    v.filter(|x| js::truthy(Some(x)))
}

/// The throws of `mergedLocationsMissingFromReportText`: its `map` builds
/// every key before its `filter` searches for any.
fn merged_locations_throw(locations: Option<&Value>) -> Result<(), String> {
    let list = match locations {
        None => return Ok(()),
        Some(Value::Array(a)) => a,
        Some(Value::String(_)) => {
            return Err(type_error(
                "(mergedLocations || []).slice(...).map is not a function",
            ))
        }
        Some(_) => {
            return Err(type_error(
                "(mergedLocations || []).slice is not a function",
            ))
        }
    };
    let mut keys = Vec::new();
    for loc in list.iter().skip(1) {
        if loc.is_null() {
            return Err(type_error(
                "Cannot read properties of null (reading 'line')",
            ));
        }
        let line = js::get(loc, "line");
        let file = js::get(loc, "file");
        if js::truthy(line) {
            stringable(file)?;
            stringable(line)?;
        } else {
            keys.push(file);
        }
    }
    keys.into_iter().try_for_each(stringable)
}

/// The vote whose `killReason` the report must cite.
fn refuted_vote(f: &Value) -> Option<&Value> {
    match js::get(f, "votes") {
        Some(Value::Array(votes)) => votes.iter().find(|v| {
            js::truthy(Some(v)) && js::get(v, "verdict") == Some(&Value::String("REFUTED".into()))
        }),
        _ => None,
    }
}

/// The five arrays of one finding, in the order the script assigns them.
fn missing_for(f: &Value, report_text: &str) -> Result<[(&'static str, Value); 5], String> {
    let locations = or_falsy(js::get(f, "mergedLocations"));
    merged_locations_throw(locations)?;
    let merged_locations = merged_locations_missing_from_report_text(locations, report_text)?;

    let kill_reason = match refuted_vote(f) {
        Some(vote) => {
            let reason = js::get(vote, "killReason");
            if let Some(r) = or_falsy(reason) {
                stringable(Some(r))?;
            }
            kill_reason_missing_from_report_text(reason, report_text)
        }
        None => Vec::new(),
    };

    let merged_from = or_falsy(js::get(f, "mergedFrom"));
    if let Some(Value::Array(lenses)) = merged_from {
        if lenses.len() >= 2 {
            lenses.iter().try_for_each(|l| stringable(Some(l)))?;
        }
    }
    let annotation = merged_from_annotation_missing_from_report_text(merged_from, report_text);

    let grounded = or_falsy(js::get(f, "grounded"));
    if let Some(g) = grounded {
        let diagnostic = or_falsy(js::get(g, "diagnostic"));
        if js::get(g, "verdict") == Some(&Value::String("confirmed".into())) && diagnostic.is_some()
        {
            stringable(diagnostic)?;
        }
    }
    let diagnostic = grounded_diagnostic_missing_from_report_text(grounded, report_text);

    Ok([
        (
            "mergedLocationsMissingFromReport",
            Value::Array(merged_locations),
        ),
        ("killReasonMissingFromReport", Value::Array(kill_reason)),
        (
            "mergedFromAnnotationMissingFromReport",
            Value::Array(annotation),
        ),
        (
            "groundedDiagnosticMissingFromReport",
            Value::Array(diagnostic),
        ),
        (
            "unexaminedWordingInReport",
            Value::Array(unexamined_wording_in_report_text(f, report_text)),
        ),
    ])
}

/// Ports `applyFidelityChecks`: writes the five arrays onto every object in
/// `findings`, in place. Anything that is not an array of findings is left
/// as it is, and so is a finding that is an array (`JSON.stringify` drops
/// the named properties the script sets on one).
///
/// # Errors
/// The TypeError the script dies on, as `TypeError: <message>`.
pub fn apply_fidelity_checks(findings: &mut Value, report_text: &str) -> Result<(), String> {
    let Value::Array(list) = findings else {
        return Ok(());
    };
    for f in list.iter_mut() {
        if !f.is_object() {
            continue;
        }
        let arrays = missing_for(f, report_text)?;
        let obj = f.as_object_mut().expect("checked above");
        for (key, value) in arrays {
            obj.insert(key.into(), value);
        }
    }
    Ok(())
}

/// The script past its argument and input handling: strips the drafts from
/// `report_text` and applies the checks to `metrics.findings`.
///
/// # Errors
/// As [`apply_fidelity_checks`], and the TypeError of `null` metrics.
pub fn check_report_fidelity(metrics: &mut Value, report_text: &str) -> Result<(), String> {
    let posted = strip_draft_comments_from_body(report_text);
    match metrics {
        Value::Null => Err(type_error(
            "Cannot read properties of null (reading 'findings')",
        )),
        Value::Object(m) => match m.get_mut("findings") {
            Some(findings) => apply_fidelity_checks(findings, &posted),
            None => Ok(()),
        },
        _ => Ok(()),
    }
}

fn failed(message: String) -> CliOutput {
    CliOutput {
        stdout: String::new(),
        stderr: format!("{message}\n"),
        code: 1,
    }
}

/// The script's entry: `check-report-fidelity.mjs <report-file> <
/// metrics.json`, with `args` the arguments after the script. On success
/// stdout is the patched metrics as `JSON.stringify` writes them, with no
/// newline. An uncaught throw leaves stderr holding its `TypeError: ...`
/// line alone, where Node prints a stack around it.
pub fn run(args: &[&str], stdin: &str) -> CliOutput {
    let report_path = args.first().copied().unwrap_or("");
    if report_path.is_empty() {
        return failed(USAGE.into());
    }
    let report = match read_file_utf8(report_path) {
        Ok(t) => t,
        Err(e) => {
            return failed(format!(
                "check-report-fidelity: could not read {report_path} — {e}"
            ))
        }
    };
    let mut metrics = match json_parse(stdin) {
        Ok(v) => v,
        Err(e) => {
            return failed(format!(
                "check-report-fidelity: metrics on stdin were not valid JSON — {e}"
            ))
        }
    };
    match check_report_fidelity(&mut metrics, &report) {
        Ok(()) => CliOutput {
            stdout: js::stringify(&metrics, ""),
            stderr: String::new(),
            code: 0,
        },
        Err(e) => CliOutput::crash(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_location_only_in_the_drafts_is_missing() {
        let mut m = json!({"findings": [{"mergedLocations": [{"file": "a.ts"}, {"file": "b.ts", "line": 4}]}]});
        let body = "### Findings\nx\n\n### Draft comments\nalso at: b.ts:4\n";
        check_report_fidelity(&mut m, body).unwrap();
        assert_eq!(
            m["findings"][0]["mergedLocationsMissingFromReport"],
            json!(["b.ts:4"])
        );
    }

    #[test]
    fn a_kill_reason_with_its_own_to_string_throws() {
        let mut m = json!({"findings": [{"votes": [{"verdict": "REFUTED", "killReason": {"toString": 1}}]}]});
        assert_eq!(
            check_report_fidelity(&mut m, "").unwrap_err(),
            "TypeError: Cannot convert object to primitive value"
        );
    }
}
