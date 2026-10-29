//! JavaScript value semantics the report regions need beyond `crate::js`:
//! `ToString`, `ToNumber`, `===` on JSON values, and the key order a plain
//! object gets in JavaScript.
//!
//! Every value these helpers see came out of `JSON.parse` on the JavaScript
//! side, so two objects or arrays are never the same reference and `===`
//! between them is always false.

use serde_json::{Map, Value};

pub use crate::js::{number_to_string, string_to_number};

/// `String(v)`, with `None` standing for `undefined`. An array joins its
/// elements with `,`, writing `null` elements as empty.
pub fn to_js_string(v: Option<&Value>) -> String {
    match v {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => number_to_string(n.as_f64().unwrap_or(f64::NAN)),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|e| match e {
                Value::Null => String::new(),
                other => to_js_string(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
    }
}

/// `Number(v)`. An array goes through its joined string, so `[20]` is 20 and
/// `[1,2]` is NaN.
pub fn to_number(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => string_to_number(s),
        Some(a @ Value::Array(_)) => string_to_number(&to_js_string(Some(a))),
        Some(Value::Object(_)) => f64::NAN,
    }
}

/// `a === b`, with `None` as `undefined`. Objects and arrays are never equal:
/// see the module note on references.
pub fn strict_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Value::Null), Some(Value::Null)) => true,
        (Some(Value::Bool(x)), Some(Value::Bool(y))) => x == y,
        (Some(Value::Number(x)), Some(Value::Number(y))) => x.as_f64() == y.as_f64(),
        (Some(Value::String(x)), Some(Value::String(y))) => x == y,
        _ => false,
    }
}

/// A canonical array index (`"0"` to `"4294967294"`, no leading zero). A plain
/// object lists these keys first, in numeric order, whatever order they were
/// written in.
fn is_array_index(k: &str) -> bool {
    !k.is_empty()
        && k.bytes().all(|c| c.is_ascii_digit())
        && (k == "0" || !k.starts_with('0'))
        && k.len() <= 10
        && k.parse::<u64>().is_ok_and(|n| n <= 4_294_967_294)
}

/// The key order JavaScript gives this value's objects, recursively: what
/// `JSON.parse` would build from it and what spreading it would keep.
pub fn js_order(v: Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut index: Vec<(u64, String, Value)> = Vec::new();
            let mut rest: Vec<(String, Value)> = Vec::new();
            for (k, v) in m {
                if is_array_index(&k) {
                    index.push((k.parse().unwrap(), k, js_order(v)));
                } else {
                    rest.push((k, js_order(v)));
                }
            }
            index.sort_by_key(|(n, _, _)| *n);
            let mut out = Map::new();
            for (_, k, v) in index {
                out.insert(k, v);
            }
            for (k, v) in rest {
                out.insert(k, v);
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.into_iter().map(js_order).collect()),
        other => other,
    }
}

/// `{ ...v }` for any JSON value. A string spreads one key per UTF-16 unit;
/// this models strings inside the Basic Multilingual Plane only, since a
/// character outside it would spread into lone surrogates, which a Rust
/// `String` cannot hold.
pub fn spread(v: &Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m.clone(),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .map(|(i, e)| (i.to_string(), e.clone()))
            .collect(),
        Value::String(s) => s
            .chars()
            .enumerate()
            .map(|(i, c)| (i.to_string(), Value::String(c.to_string())))
            .collect(),
        _ => Map::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_print_like_javascript() {
        let cases = [
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (1e-7, "1e-7"),
            (1.23e-18, "1.23e-18"),
            (0.1, "0.1"),
            (3.0, "3"),
            (-1.5e-10, "-1.5e-10"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
        ];
        for (x, s) in cases {
            assert_eq!(number_to_string(x), s);
        }
    }

    #[test]
    fn number_conversion_rejects_rust_only_spellings() {
        assert!(string_to_number("inf").is_nan());
        assert!(string_to_number("1_000").is_nan());
        assert!(string_to_number("-0x10").is_nan());
        assert!(string_to_number("\u{0085}20").is_nan());
        assert_eq!(string_to_number("\u{FEFF}20"), 20.0);
        assert_eq!(string_to_number("0X1e"), 30.0);
        assert_eq!(string_to_number("5."), 5.0);
        assert_eq!(to_number(Some(&json!([[20]]))), 20.0);
    }

    #[test]
    fn index_keys_move_first() {
        let v: Value =
            serde_json::from_str(r#"{"b":1,"10":2,"2":3,"01":4,"4294967295":5}"#).unwrap();
        let keys: Vec<String> = js_order(v).as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["2", "10", "b", "01", "4294967295"]);
    }
}
