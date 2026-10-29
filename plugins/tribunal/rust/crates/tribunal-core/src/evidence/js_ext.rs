//! JavaScript semantics the evidence regions need beyond `crate::js`:
//! `String(v)` and `Number(v)` on JSON values, number-to-string, UTF-16 string
//! order, property order of a plain object, and the inherited properties of
//! `Object.prototype` that a `by[key]` lookup can land on.

use std::cmp::Ordering;

use serde_json::{Map, Number, Value};

use crate::js;
pub use crate::js::{number_to_string, string_to_number};

/// A number as `JSON.stringify` writes it: whole values without a fraction,
/// and `null` for NaN and the infinities.
pub fn number_value(x: f64) -> Value {
    if x.fract() == 0.0 && x.abs() < 9.007_199_254_740_992e15 {
        Value::from(x as i64)
    } else {
        Number::from_f64(x).map_or(Value::Null, Value::Number)
    }
}

/// `String(v)`. An array joins its elements with `,`, writing `null` ones as
/// empty, and an object is `[object Object]`.
pub fn to_js_string(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => number_to_string(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .map(|e| match e {
                Value::Null => String::new(),
                other => to_js_string(other),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// `String(v || "")`: a falsy value, a missing one included, becomes empty.
pub fn string_or_empty(v: Option<&Value>) -> String {
    match v {
        Some(v) if js::truthy(Some(v)) => to_js_string(v),
        _ => String::new(),
    }
}

/// The property key a `obj[v]` read uses: `String(v)`, with a missing value
/// spelled `undefined`.
pub fn to_property_key(v: Option<&Value>) -> String {
    v.map_or_else(|| "undefined".into(), to_js_string)
}

/// `Number(v)`.
pub fn to_number(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => string_to_number(s),
        Some(a @ Value::Array(_)) => string_to_number(&to_js_string(a)),
        Some(Value::Object(_)) => f64::NAN,
    }
}

/// `a < b` on strings compares UTF-16 code units, not code points: U+FF5E
/// sorts after U+1F600, whose first unit is a surrogate.
pub fn utf16_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// A key a plain object lists first, in ascending numeric order: the canonical
/// decimal form of an integer up to 2^32 - 2.
pub fn is_array_index(key: &str) -> bool {
    !key.is_empty()
        && key.bytes().all(|b| b.is_ascii_digit())
        && (key == "0" || !key.starts_with('0'))
        && key.len() <= 10
        && key.parse::<u64>().is_ok_and(|n| n <= 4_294_967_294)
}

/// The order a plain object lists its own keys in: array indices ascending,
/// then the rest in insertion order.
pub fn property_order(map: Map<String, Value>) -> Map<String, Value> {
    let (mut index, rest): (Vec<_>, Vec<_>) = map.into_iter().partition(|(k, _)| is_array_index(k));
    index.sort_by_key(|(k, _)| k.parse::<u64>().unwrap());
    index.into_iter().chain(rest).collect()
}

/// A JSON value with every object in the key order `JSON.parse` gives it.
pub fn property_order_deep(v: Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(property_order(
            m.into_iter()
                .map(|(k, v)| (k, property_order_deep(v)))
                .collect(),
        )),
        Value::Array(a) => Value::Array(a.into_iter().map(property_order_deep).collect()),
        other => other,
    }
}

/// What `({})[key]` finds on `Object.prototype` when the object has no own
/// `key`. `Some(Some(n))` is a method whose `.length` is `n`; `Some(None)` is
/// `__proto__`, an object with no `length`. Both are truthy, so `obj[key] ||
/// []` keeps them.
pub fn object_prototype_property(key: &str) -> Option<Option<u32>> {
    match key {
        "__defineGetter__" | "__defineSetter__" => Some(Some(2)),
        "constructor"
        | "hasOwnProperty"
        | "__lookupGetter__"
        | "__lookupSetter__"
        | "isPrototypeOf"
        | "propertyIsEnumerable" => Some(Some(1)),
        "toString" | "valueOf" | "toLocaleString" => Some(Some(0)),
        "__proto__" => Some(None),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_print_as_javascript_does() {
        assert_eq!(number_to_string(42.0), "42");
        assert_eq!(number_to_string(1e21), "1e+21");
        assert_eq!(number_to_string(1.5e-7), "1.5e-7");
        assert_eq!(number_to_string(0.000001), "0.000001");
        assert_eq!(number_to_string(-0.0), "0");
        assert_eq!(number_to_string(123.456), "123.456");
    }

    #[test]
    fn number_of_a_string_follows_the_js_grammar() {
        assert_eq!(string_to_number(" 17 "), 17.0);
        assert_eq!(string_to_number(""), 0.0);
        assert_eq!(string_to_number("0x1A"), 26.0);
        assert!(string_to_number("inf").is_nan());
        assert!(string_to_number("1_0").is_nan());
        assert_eq!(string_to_number("-Infinity"), f64::NEG_INFINITY);
        assert_eq!(to_number(Some(&json!([5]))), 5.0);
        assert_eq!(to_number(Some(&Value::Null)), 0.0);
    }

    #[test]
    fn index_keys_come_first() {
        let m: Map<String, Value> = serde_json::from_str(r#"{"b":1,"10":2,"2":3,"01":4}"#).unwrap();
        let keys: Vec<String> = property_order(m).keys().cloned().collect();
        assert_eq!(keys, ["2", "10", "b", "01"]);
    }

    #[test]
    fn strings_compare_by_utf16_units() {
        assert_eq!(utf16_cmp("\u{FF5E}", "\u{1F600}"), Ordering::Greater);
    }
}
