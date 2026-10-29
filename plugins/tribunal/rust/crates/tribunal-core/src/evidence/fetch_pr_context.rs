//! Port of the FETCH-PR-CONTEXT region: merge the harvest's decisions and
//! CHANGES_REQUESTED summaries into the verified PR context.
//!
//! Objects are expected in JavaScript property order, the order `JSON.parse`
//! gives them (`js_ext::property_order_deep` converts a serde-parsed value).
//! With that, every key this function adds lands where JavaScript puts it.

use serde_json::{Map, Value};

use super::js_ext;
use crate::js;

/// Ports `buildPrContext(data, answer)`. `data` is the relayed context and
/// `answer` the harvest's structured output; pass `Value::Null` for a missing
/// one. `Err` where the JavaScript throws: a null `data`, a null summary or
/// review entry, a truthy non-iterable `priorReviewSummaries`, or a truthy
/// non-array `priorReviews`.
pub fn build_pr_context(data: &Value, answer: &Value) -> Result<Value, String> {
    let mut summaries: Vec<(f64, String)> = Vec::new();
    let raw = if js::truthy(Some(answer)) {
        js::get(answer, "priorReviewSummaries")
    } else {
        None
    };
    match raw {
        Some(Value::Array(items)) => {
            for s in items {
                if s.is_null() {
                    return Err("Cannot read properties of null (reading 'index')".into());
                }
                let index = js::get(s, "index");
                if js::is_integer(index) {
                    let i = index.and_then(Value::as_f64).unwrap();
                    let summary = js_ext::string_or_empty(js::get(s, "summary"));
                    match summaries.iter_mut().find(|(k, _)| *k == i) {
                        Some(entry) => entry.1 = summary,
                        None => summaries.push((i, summary)),
                    }
                }
            }
        }
        // A string iterates as one-character strings, none of which has an
        // `index`.
        Some(Value::String(_)) => {}
        Some(other) if js::truthy(Some(other)) => {
            return Err("answer.priorReviewSummaries is not iterable".into());
        }
        _ => {}
    }

    let mut out: Map<String, Value> = match data {
        Value::Null => return Err("Cannot read properties of null (reading 'priorReviews')".into()),
        Value::Object(m) => m.clone(),
        // A spread copies a string's characters and an array's elements under
        // their indices. JavaScript indexes UTF-16 units, so this agrees for
        // BMP text only; `data` is the relayed context object and is never a
        // string in production.
        Value::String(s) => s
            .chars()
            .enumerate()
            .map(|(i, c)| (i.to_string(), Value::String(c.to_string())))
            .collect(),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .map(|(i, v)| (i.to_string(), v.clone()))
            .collect(),
        Value::Bool(_) | Value::Number(_) => Map::new(),
    };

    let decisions = match js::truthy(Some(answer)).then(|| js::get(answer, "inThreadDecisions")) {
        Some(Some(d @ Value::Array(_))) => d.clone(),
        _ => Value::Array(Vec::new()),
    };
    out.insert("inThreadDecisions".into(), decisions);

    let reviews = match js::get(data, "priorReviews") {
        Some(Value::Array(list)) => list
            .iter()
            .enumerate()
            .map(|(i, r)| {
                if r.is_null() {
                    return Err("Cannot read properties of null (reading 'state')".to_owned());
                }
                let requested =
                    js::get(r, "state") == Some(&Value::String("CHANGES_REQUESTED".into()));
                let summary = summaries.iter().find(|(k, _)| *k == i as f64);
                Ok(match (requested, summary, r) {
                    (true, Some((_, s)), Value::Object(m)) => {
                        let mut m = m.clone();
                        m.insert("summary".into(), Value::String(s.clone()));
                        Value::Object(m)
                    }
                    _ => r.clone(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(other) if js::truthy(Some(other)) => {
            return Err("data.priorReviews.map is not a function".into());
        }
        _ => Vec::new(),
    };
    out.insert("priorReviews".into(), Value::Array(reviews));
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn data() -> Value {
        json!({"files": ["src/f1.ts"], "priorReviews": [
            {"author": "a", "state": "APPROVED", "summary": ""},
            {"author": "b", "state": "CHANGES_REQUESTED", "summary": ""},
            {"author": "c", "state": "COMMENTED", "summary": ""},
        ]})
    }

    #[test]
    fn a_summary_lands_on_its_change_request_by_index() {
        let answer = json!({
            "inThreadDecisions": [{"category": "BY-DESIGN"}],
            "priorReviewSummaries": [
                {"index": 1, "summary": "racy"},
                {"index": 2, "summary": "not a change request"},
                {"index": "1", "summary": "string index"},
            ],
        });
        let ctx = build_pr_context(&data(), &answer).unwrap();
        assert_eq!(ctx["priorReviews"][1]["summary"], "racy");
        assert_eq!(ctx["priorReviews"][2]["summary"], "");
        assert_eq!(ctx["inThreadDecisions"], answer["inThreadDecisions"]);
    }

    #[test]
    fn no_harvest_means_no_decisions() {
        let ctx = build_pr_context(&data(), &Value::Null).unwrap();
        assert_eq!(ctx["inThreadDecisions"], json!([]));
        let keys: Vec<&String> = ctx.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["files", "priorReviews", "inThreadDecisions"]);
    }

    #[test]
    fn a_null_context_is_an_error() {
        assert!(build_pr_context(&Value::Null, &Value::Null).is_err());
    }
}
