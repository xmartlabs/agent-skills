//! Port of the MERGED-LOCATIONS-CHECK region: every extra location a merged
//! finding carries must appear in the printed report as `file:line`.
//!
//! `mergedLocations` is read off a finding as JSON, so it stays a `Value`:
//! JavaScript renders whatever `file` and `line` hold through a template
//! literal, and the missing keys come back as the raw `file` values.

use serde_json::Value;

use super::js_ext;
use crate::js;

/// Ports `mergedLocationsMissingFromReportText`. The first location is the
/// finding's own and is skipped. `Err` where JavaScript throws: a truthy
/// non-array list, or a `null` location after the first.
pub fn merged_locations_missing_from_report_text(
    merged_locations: Option<&Value>,
    report_text: &str,
) -> Result<Vec<Value>, String> {
    let locations: &[Value] = match merged_locations {
        Some(Value::Array(a)) => a,
        v if !js::truthy(v) => &[],
        _ => return Err("(mergedLocations || []).slice(...).map is not a function".into()),
    };
    let mut missing = Vec::new();
    for loc in locations.iter().skip(1) {
        if loc.is_null() {
            return Err("Cannot read properties of null (reading 'line')".into());
        }
        let file = js::get(loc, "file");
        let line = js::get(loc, "line");
        let key = if js::truthy(line) {
            Some(Value::String(format!(
                "{}:{}",
                js_ext::to_js_string(file),
                js_ext::to_js_string(line)
            )))
        } else {
            file.cloned()
        };
        if !report_text.contains(&js_ext::to_js_string(key.as_ref())) {
            // An undefined key serializes as `null` inside an array.
            missing.push(js_ext::js_order(key.unwrap_or(Value::Null)));
        }
    }
    Ok(missing)
}

/// The placement the ranking gives a finding it moves to "Questions to
/// review". The report prints those one line each, with no "also at" or
/// "found by", and the posted body leaves them out.
pub const QUESTION_TO_REVIEW: &str = "lower-priority-question";

/// Whether `finding` (a result or a metrics row's finding) was moved to
/// "Questions to review".
#[must_use]
pub fn is_question_to_review(finding: &Value) -> bool {
    js::get(finding, "placement").and_then(|p| js::get(p, "placement"))
        == Some(&Value::String(QUESTION_TO_REVIEW.into()))
}

/// The extra locations of a merged finding the report does not print, as
/// [`merged_locations_missing_from_report_text`] finds them, less three kinds
/// of false alarm seen on CI reviews (agent-skills #182): a location equal to
/// the finding's own one, the same location twice, and a location the report
/// prints with a shortened path (`widgets.ts:203` for
/// `src/components/widgets.ts:203`).
///
/// # Errors
/// As [`merged_locations_missing_from_report_text`].
pub fn merged_locations_missing_from_report(
    merged_locations: Option<&Value>,
    report_text: &str,
) -> Result<Vec<Value>, String> {
    let missing = merged_locations_missing_from_report_text(merged_locations, report_text)?;
    let own = match merged_locations {
        Some(Value::Array(a)) => a.first().and_then(location_key),
        _ => None,
    };
    let mut seen: Vec<String> = own.into_iter().collect();
    Ok(missing
        .into_iter()
        .filter(|key| {
            let Value::String(k) = key else {
                return true;
            };
            if seen.contains(k) {
                return false;
            }
            seen.push(k.clone());
            !printed_with_short_path(k, report_text)
        })
        .collect())
}

/// The `file:line` (or bare file) key the check looks for, for a location it
/// would print as a string.
fn location_key(loc: &Value) -> Option<String> {
    let file = js::get(loc, "file");
    let line = js::get(loc, "line");
    if js::truthy(line) {
        Some(format!(
            "{}:{}",
            js_ext::to_js_string(file),
            js_ext::to_js_string(line)
        ))
    } else {
        file.and_then(Value::as_str).map(str::to_owned)
    }
}

/// Whether the report prints `key` with its directories dropped: the last
/// path segment, plus `:line` when the key has one, not preceded by a
/// character that would make it part of a longer name.
fn printed_with_short_path(key: &str, report_text: &str) -> bool {
    let (path, line) = match key.rsplit_once(':') {
        Some((p, l)) if !l.is_empty() && l.bytes().all(|b| b.is_ascii_digit()) => (p, Some(l)),
        _ => (key, None),
    };
    let Some((_, base)) = path.rsplit_once('/') else {
        return false;
    };
    if base.is_empty() {
        return false;
    }
    let short = match line {
        Some(l) => format!("{base}:{l}"),
        None => base.to_owned(),
    };
    report_text.match_indices(&short).any(|(i, _)| {
        let before = report_text[..i].chars().next_back();
        let after = report_text[i + short.len()..].chars().next();
        let part_of_name = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/');
        !before.is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
            && !(line.is_some() && after.is_some_and(|c| c.is_ascii_digit()))
            && !(line.is_none() && after.is_some_and(part_of_name))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_short_path_in_the_report_counts_as_printed() {
        let locs = json!([
            {"file": "src/components/widgets.ts", "line": 212},
            {"file": "src/components/widgets.ts", "line": 203},
            {"file": "src/components/widgets.ts", "line": 407}
        ]);
        let report = "- Bug `widgets.ts:212` (important)\n  - also at: `widgets.ts:203`\n";
        assert_eq!(
            merged_locations_missing_from_report(Some(&locs), report).unwrap(),
            [json!("src/components/widgets.ts:407")]
        );
    }

    #[test]
    fn a_longer_name_or_line_is_not_the_short_path() {
        let locs = json!([
            {"file": "src/a.ts", "line": 1},
            {"file": "src/components/widgets.ts", "line": 20}
        ]);
        for report in ["my-widgets.ts:20", "widgets.ts:203", "oldwidgets.ts:20"] {
            assert_eq!(
                merged_locations_missing_from_report(Some(&locs), report).unwrap(),
                [json!("src/components/widgets.ts:20")],
                "{report}"
            );
        }
    }

    #[test]
    fn the_own_location_and_repeats_are_not_reported_missing() {
        let locs = json!([
            {"file": "src/hooks/save.ts", "line": 167},
            {"file": "src/hooks/save.ts", "line": 167},
            {"file": "src/hooks/save.ts", "line": 172},
            {"file": "src/hooks/save.ts", "line": 172}
        ]);
        assert_eq!(
            merged_locations_missing_from_report(Some(&locs), "nothing printed").unwrap(),
            [json!("src/hooks/save.ts:172")]
        );
    }

    #[test]
    fn errors_stay_those_of_the_ported_check() {
        let locs = json!([{"file": "a.ts", "line": 1}, null]);
        assert!(merged_locations_missing_from_report(Some(&locs), "").is_err());
    }

    #[test]
    fn a_question_to_review_is_told_by_its_placement() {
        assert!(is_question_to_review(
            &json!({"placement": {"placement": "lower-priority-question", "worth": 2}})
        ));
        assert!(!is_question_to_review(&json!({"placement": null})));
        assert!(!is_question_to_review(&json!({})));
    }

    #[test]
    fn reports_only_the_dropped_extra_location() {
        let locs = json!([{"file": "a.sh", "line": 5}, {"file": "b.sh", "line": 12}, {"file": "c.sh", "line": 9}]);
        assert_eq!(
            merged_locations_missing_from_report_text(Some(&locs), "also at: b.sh:12").unwrap(),
            [json!("c.sh:9")]
        );
    }

    #[test]
    fn a_float_line_prints_as_javascript_does() {
        let locs = json!([{}, {"file": "a.ts", "line": 3.0}]);
        assert!(
            merged_locations_missing_from_report_text(Some(&locs), "a.ts:3")
                .unwrap()
                .is_empty()
        );
    }
}
