//! Port of the LENS-YIELD region: one row per planned lens label, separating a
//! lens that found nothing from one that never answered.

use serde::Serialize;
use serde_json::Value;

use super::js_ext::{js_key_order, same_value_zero};
use crate::js;

/// One row of `lensYield`, in the key order the JavaScript object literal has.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LensYield {
    /// The plan's label, or `lens<i>` when the entry has none. It is whatever
    /// value the plan carried, so it stays a JSON value.
    pub lens: Value,
    pub dispatches: u64,
    pub files: Option<u64>,
    pub candidates: u64,
    pub survived_to_report: u64,
    pub crashed: u64,
    pub not_reached: u64,
}

fn as_slice(v: &Value) -> &[Value] {
    match v {
        Value::Array(a) => a,
        _ => &[],
    }
}

/// Ports `buildLensYield`. Rows are grouped by label with `Map` key equality:
/// an object label is its own key and matches no candidate.
pub fn build_lens_yield(
    plan: &Value,
    raw_results: &Value,
    raw_candidates: &Value,
    report_findings: &Value,
) -> Vec<LensYield> {
    let results = as_slice(raw_results);
    let mut rows: Vec<LensYield> = Vec::new();
    let find =
        |rows: &[LensYield], lens: &Value| rows.iter().position(|r| same_value_zero(&r.lens, lens));
    for (i, entry) in as_slice(plan).iter().enumerate() {
        let label = js::get(entry, "lens").filter(|l| js::truthy(Some(l)));
        let lens = label
            .cloned()
            .unwrap_or_else(|| Value::String(format!("lens{}", i + 1)));
        let at = match find(&rows, &lens) {
            Some(at) => at,
            None => {
                rows.push(LensYield {
                    lens: js_key_order(&lens),
                    dispatches: 0,
                    files: None,
                    candidates: 0,
                    survived_to_report: 0,
                    crashed: 0,
                    not_reached: 0,
                });
                rows.len() - 1
            }
        };
        let agg = &mut rows[at];
        agg.dispatches += 1;
        if let Some(Value::Array(files)) = js::get(entry, "files") {
            agg.files = Some(agg.files.unwrap_or(0) + files.len() as u64);
        }
        match results.get(i) {
            Some(r) if js::truthy(Some(r)) => {
                if let Some(Value::Array(nr)) = js::get(r, "notReached") {
                    agg.not_reached += nr.len() as u64;
                }
            }
            _ => agg.crashed += 1,
        }
    }
    for c in as_slice(raw_candidates) {
        if !js::truthy(Some(c)) {
            continue;
        }
        if let Some(at) = js::get(c, "lens").and_then(|l| find(&rows, l)) {
            rows[at].candidates += 1;
        }
    }
    for f in as_slice(report_findings) {
        if !js::truthy(Some(f)) || !js::truthy(js::get(f, "survived")) {
            continue;
        }
        let mut credited: Vec<&Value> = Vec::new();
        let lens = js::get(f, "lens").filter(|l| js::truthy(Some(l)));
        let merged = match js::get(f, "mergedFrom") {
            Some(Value::Array(m)) => m.as_slice(),
            _ => &[],
        };
        for l in lens.into_iter().chain(merged) {
            if !credited.iter().any(|c| same_value_zero(c, l)) {
                credited.push(l);
            }
        }
        for l in credited {
            if let Some(at) = find(&rows, l) {
                rows[at].survived_to_report += 1;
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_repeated_focus_is_one_row() {
        let rows = build_lens_yield(
            &json!([{"lens": "c", "files": ["a", "b"]}, {"lens": "c", "files": ["d"]}]),
            &json!([{"findings": []}, null]),
            &json!([{"lens": "c"}, {"lens": "c"}]),
            &json!([{"lens": "c", "mergedFrom": ["c"], "survived": true}]),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (
                rows[0].dispatches,
                rows[0].files,
                rows[0].crashed,
                rows[0].survived_to_report
            ),
            (2, Some(3), 1, 1)
        );
    }

    #[test]
    fn an_object_label_is_its_own_row() {
        let rows = build_lens_yield(
            &json!([{"lens": {"a": 1}}, {"lens": {"a": 1}}]),
            &json!([]),
            &json!([{"lens": {"a": 1}}]),
            &json!([]),
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].candidates, 0);
    }
}
