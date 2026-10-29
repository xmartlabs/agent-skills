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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
