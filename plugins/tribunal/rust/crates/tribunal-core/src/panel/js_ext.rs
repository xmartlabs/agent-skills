//! JavaScript semantics the panel regions need beyond `crate::js`: number to
//! string conversion and `String(value)`.

use serde_json::Value;

use crate::js;
pub use crate::js::number_to_string;

/// `String(v)`, where `None` is `undefined`. An array joins its elements with
/// commas, printing `null` and `undefined` elements as empty.
pub fn to_js_string(v: Option<&Value>) -> String {
    match v {
        None => "undefined".to_owned(),
        Some(Value::Null) => "null".to_owned(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => number_to_string(n.as_f64().unwrap_or(f64::NAN)),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|e| match e {
                Value::Null => String::new(),
                e => to_js_string(Some(e)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
    }
}

/// `String(v || "")`.
pub fn string_or_empty(v: Option<&Value>) -> String {
    if js::truthy(v) {
        to_js_string(v)
    } else {
        String::new()
    }
}

/// `v === s` for a string literal `s`.
pub fn is_str(v: Option<&Value>, s: &str) -> bool {
    matches!(v, Some(Value::String(x)) if x == s)
}

/// `typeof v === "string"`.
pub fn is_string(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::String(_)))
}

/// `v || fallback`, with the fallback given as a JSON value.
pub fn or(v: Option<&Value>, fallback: Value) -> Value {
    match v {
        Some(x) if js::truthy(Some(x)) => x.clone(),
        _ => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_print_as_javascript_does() {
        let cases = [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (1_200_000.0, "1200000"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1e21, "1e+21"),
            (123e20, "1.23e+22"),
            (1e-7, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (0.000001, "0.000001"),
            (-2.5, "-2.5"),
            (f64::INFINITY, "Infinity"),
            (f64::from_bits(0x4317_9085_685d_83c9), "1658206780088562.2"),
            (f64::from_bits(0x42ea_8090_bb0f_6d84), "233115890514796.12"),
        ];
        for (x, s) in cases {
            assert_eq!(number_to_string(x), s, "{x}");
        }
    }

    #[test]
    fn string_of_values() {
        assert_eq!(
            to_js_string(Some(&json!([1, null, "a", [2, 3]]))),
            "1,,a,2,3"
        );
        assert_eq!(to_js_string(Some(&json!({"a": 1}))), "[object Object]");
        assert_eq!(string_or_empty(Some(&json!(0))), "");
        assert_eq!(string_or_empty(None), "");
    }
}
