//! JavaScript semantics the lens regions need beyond `crate::js`: `String()`
//! and template-literal conversion, `Number()`, strict and SameValueZero
//! equality, and slicing by UTF-16 code units.

use serde_json::Value;

pub use crate::js::{number_to_string, string_to_number};

/// `String(v)`, which is also what a template literal does with `${v}`.
/// `None` is `undefined`.
pub fn to_js_string(v: Option<&Value>) -> String {
    match v {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => number_to_string(n.as_f64().unwrap_or(f64::NAN)),
        Some(Value::String(s)) => s.clone(),
        // Array.prototype.join: null and undefined elements become "".
        Some(Value::Array(a)) => a
            .iter()
            .map(|e| match e {
                Value::Null => String::new(),
                e => to_js_string(Some(e)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
    }
}

/// `Number(v)`. Also what a relational operator does to a value compared
/// with a number.
pub fn to_number(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => string_to_number(s),
        Some(other) => string_to_number(&to_js_string(Some(other))),
    }
}

/// `a === b` for values parsed from JSON. Two objects or arrays are never the
/// same reference, so they are never strictly equal. `None` is `undefined`.
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

/// SameValueZero, the key equality of `Map` and `Set`, under the same rule
/// for objects as `strict_eq` (JSON has no NaN, where the two differ).
pub fn same_value_zero(a: &Value, b: &Value) -> bool {
    strict_eq(Some(a), Some(b))
}

/// An array index as a property key: the canonical decimal form of an integer
/// below 2^32 - 1.
fn array_index(key: &str) -> Option<u32> {
    let n: u32 = key.parse().ok()?;
    (n != u32::MAX && n.to_string() == key).then_some(n)
}

/// The key order a JavaScript object enumerates in, applied recursively:
/// array-index keys first in ascending order, then the rest in insertion
/// order. `JSON.parse` and object spread both produce it, so an object a
/// region returns from its input has it whatever order the JSON text had.
pub fn js_key_order(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut indexed: Vec<(u32, &String, &Value)> = Vec::new();
            let mut named: Vec<(&String, &Value)> = Vec::new();
            for (k, x) in m {
                match array_index(k) {
                    Some(n) => indexed.push((n, k, x)),
                    None => named.push((k, x)),
                }
            }
            indexed.sort_by_key(|&(n, _, _)| n);
            let out = indexed
                .into_iter()
                .map(|(_, k, x)| (k, x))
                .chain(named)
                .map(|(k, x)| (k.clone(), js_key_order(x)))
                .collect();
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(js_key_order).collect()),
        other => other.clone(),
    }
}

/// `s.slice(0, n)` by UTF-16 code units. A Rust string cannot hold the lone
/// high surrogate JavaScript leaves when the cut splits a pair; it becomes
/// U+FFFD, which is what `String.prototype.toWellFormed` makes of it.
pub fn utf16_prefix_lossy(s: &str, n: usize) -> String {
    let mut out = String::new();
    let mut units = 0;
    for c in s.chars() {
        let w = c.len_utf16();
        if units + w > n {
            if units < n {
                out.push('\u{FFFD}');
            }
            break;
        }
        out.push(c);
        units += w;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_print_the_javascript_way() {
        assert_eq!(number_to_string(1.0), "1");
        assert_eq!(number_to_string(-0.0), "0");
        assert_eq!(number_to_string(1.5e-7), "1.5e-7");
        assert_eq!(number_to_string(0.000001), "0.000001");
        assert_eq!(number_to_string(1e21), "1e+21");
        assert_eq!(
            number_to_string(123456789012345680000.0),
            "123456789012345680000"
        );
        assert_eq!(number_to_string(0.1 + 0.2), "0.30000000000000004");
    }

    #[test]
    fn string_conversion_of_compound_values() {
        assert_eq!(to_js_string(Some(&json!([1, null, [2, "a"]]))), "1,,2,a");
        assert_eq!(to_js_string(Some(&json!({"a": 1}))), "[object Object]");
        assert_eq!(to_js_string(None), "undefined");
    }

    #[test]
    fn number_conversion() {
        assert_eq!(to_number(Some(&json!(" 12 "))), 12.0);
        assert_eq!(to_number(Some(&json!("0x10"))), 16.0);
        assert_eq!(to_number(Some(&json!(""))), 0.0);
        assert!(to_number(Some(&json!("inf"))).is_nan());
        assert!(to_number(Some(&json!("-0x10"))).is_nan());
        assert_eq!(to_number(Some(&json!([5]))), 5.0);
        assert_eq!(to_number(Some(&json!(".5"))), 0.5);
    }

    #[test]
    fn utf16_cut_inside_a_pair() {
        assert_eq!(utf16_prefix_lossy("a😀b", 2), "a\u{FFFD}");
        assert_eq!(utf16_prefix_lossy("a😀b", 3), "a😀");
    }
}
