//! Port of the LENS-BUDGET region: the surfaces lenses report they did not
//! reach under the tool-call budget, and the union of every kind of blind spot
//! the metrics row records as `notChecked`.

use serde_json::Value;

use super::js_ext;
use crate::js;

/// Ports `collectNotReached`. `results` is the review phase's result array as
/// returned (a crashed lens is null); only the first `lens_count` are lenses.
pub fn collect_not_reached(results: &Value, lens_count: usize) -> Vec<String> {
    let results = match results {
        Value::Array(r) => r.as_slice(),
        _ => &[],
    };
    let mut out: Vec<String> = Vec::new();
    for r in results.iter().take(lens_count) {
        if !js::truthy(Some(r)) {
            continue;
        }
        let Some(Value::Array(items)) = js::get(r, "notReached") else {
            continue;
        };
        for s in items {
            let s = match s {
                Value::Null => String::new(),
                s => js_ext::to_js_string(Some(s)),
            };
            let s = js::trim(&s);
            if !s.is_empty() && !out.iter().any(|o| o == s) {
                out.push(s.to_owned());
            }
        }
    }
    out
}

/// Ports `mergeNotChecked`, which is variadic: each list is one argument.
/// `flatMap` spreads an array and keeps any other truthy value whole, and
/// `String(null)` is `"null"`, which survives the blank filter.
pub fn merge_not_checked(lists: &[Value]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |v: &Value| {
        let s = js_ext::to_js_string(Some(v));
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    };
    for l in lists {
        match l {
            Value::Array(items) => items.iter().for_each(&mut add),
            l if js::truthy(Some(l)) => add(l),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn crashed_lenses_and_wrong_types_contribute_nothing() {
        let r = json!([null, {"notReached": "a.ts"}, {"notReached": ["  b.ts ", null, "b.ts"]}]);
        assert_eq!(collect_not_reached(&r, 3), ["b.ts"]);
        assert!(collect_not_reached(&r, 2).is_empty());
    }

    #[test]
    fn a_third_list_is_kept_and_null_entries_stringify() {
        let got = merge_not_checked(&[json!(["a"]), json!(null), json!(["b", null]), json!("c")]);
        assert_eq!(got, ["a", "b", "null", "c"]);
    }
}
