//! The corpus entry schema (`templates/corpus-entry.schema.json`) and a check
//! of an entry against it. The check covers what that schema uses: `type`
//! (one or a list), `required`, `enum`, `properties` and `items`.

use serde_json::Value;

pub const SCHEMA_TEXT: &str = include_str!("../../../../../templates/corpus-entry.schema.json");

/// # Panics
/// Never: the schema ships with the binary and a unit test parses it.
#[must_use]
pub fn schema() -> Value {
    serde_json::from_str(SCHEMA_TEXT).expect("the shipped corpus schema is JSON")
}

/// Every place `entry` breaks the schema, as `<path>: <problem>`. Empty when
/// it conforms.
#[must_use]
pub fn violations(entry: &Value) -> Vec<String> {
    let mut out = Vec::new();
    check(&schema(), entry, "$", &mut out);
    out
}

fn type_ok(want: &str, v: &Value) -> bool {
    match want {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        _ => true,
    }
}

fn check(schema: &Value, v: &Value, path: &str, out: &mut Vec<String>) {
    let types: Vec<&str> = match &schema["type"] {
        Value::String(t) => vec![t.as_str()],
        Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    if !types.is_empty() && !types.iter().any(|t| type_ok(t, v)) {
        out.push(format!("{path}: expected {}, got {v}", types.join(" or ")));
        return;
    }
    if let Some(allowed) = schema["enum"].as_array() {
        if !allowed.contains(v) {
            out.push(format!(
                "{path}: {v} is not one of {}",
                Value::Array(allowed.clone())
            ));
        }
    }
    if let Some(obj) = v.as_object() {
        for key in schema["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !obj.contains_key(key) {
                out.push(format!("{path}: missing {key}"));
            }
        }
        if let Some(props) = schema["properties"].as_object() {
            for (k, sub) in props {
                if let Some(child) = obj.get(k) {
                    check(sub, child, &format!("{path}.{k}"), out);
                }
            }
        }
    }
    if let (Some(items), Some(arr)) = (schema.get("items"), v.as_array()) {
        for (i, child) in arr.iter().enumerate() {
            check(items, child, &format!("{path}[{i}]"), out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn good() -> Value {
        json!({
            "repo": "acme/widgets", "pr": 7, "reviewedSha": "a".repeat(40), "status": "candidate",
            "knownReal": [{"id": "x", "file": "src/widgets.py", "description": "d", "expectedSeverity": null,
                "proposal": {"verdict": "defect", "severity": "nit", "description": "d", "model": "m", "costUsd": 0.1, "date": "2026-11-27"}}],
            "skipped": [{"id": "y", "file": "src/widgets.py", "reason": "fix-already-in-pin"}]
        })
    }

    #[test]
    fn the_shipped_schema_parses_and_a_good_entry_passes() {
        assert_eq!(schema()["title"], "Tribunal corpus entry");
        assert_eq!(violations(&good()), Vec::<String>::new());
    }

    #[test]
    fn each_kind_of_break_is_reported() {
        let mut e = good();
        e.as_object_mut().unwrap().remove("reviewedSha");
        assert_eq!(violations(&e), vec!["$: missing reviewedSha"]);
        let mut e = good();
        e["status"] = json!("accepted");
        assert!(violations(&e)[0].starts_with("$.status: \"accepted\" is not one of"));
        let mut e = good();
        e["knownReal"][0].as_object_mut().unwrap().remove("file");
        assert_eq!(violations(&e), vec!["$.knownReal[0]: missing file"]);
        let mut e = good();
        e["pr"] = json!("7");
        assert_eq!(violations(&e), vec!["$.pr: expected integer, got \"7\""]);
        let mut e = good();
        e["knownReal"][0]["proposal"]["verdict"] = json!("maybe");
        assert_eq!(violations(&e).len(), 1);
    }
}
