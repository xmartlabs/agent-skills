//! JavaScript value semantics the vote regions need beyond `crate::js`:
//! `String(v)`, `Number(v)`, number-to-string, `===` on JSON values, and
//! UTF-16 slicing.

use serde_json::{Number, Value};

use crate::js;
pub use crate::js::{number_to_string, string_to_number};

/// `String(v)` for a JSON value; `None` is `undefined`. An array joins its
/// elements with commas, writing null and undefined as empty.
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
                other => to_js_string(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
    }
}

/// `Number(v)` for a JSON value; `None` is `undefined`.
pub fn to_number(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => string_to_number(s),
        Some(Value::Array(_)) => string_to_number(&to_js_string(v)),
        Some(Value::Object(_)) => f64::NAN,
    }
}

/// A JavaScript number as `JSON.stringify` writes it: whole values as
/// integers, NaN and the infinities as null.
pub fn number_value(x: f64) -> Value {
    if !x.is_finite() {
        return Value::Null;
    }
    if x.fract() == 0.0 && x.abs() < 9_007_199_254_740_992.0 {
        // -0 writes as 0 in JSON.
        return Value::from(x as i64);
    }
    Number::from_f64(x).map_or(Value::Null, Value::Number)
}

/// `a === b` on JSON values, `None` being `undefined`. Two objects or arrays
/// are never the same reference here: each came from its own parse. This is
/// also `SameValueZero`, as `Set.has` uses it, since JSON has no NaN.
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

/// `s.slice(0, n)` in UTF-16 code units. A cut through a surrogate pair leaves
/// JavaScript a lone high surrogate, which a Rust string cannot hold; it
/// becomes U+FFFD, which is what Node writes for it as UTF-8.
pub fn utf16_prefix(s: &str, n: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.len_utf16();
        if used + w > n {
            if used < n {
                out.push('\u{FFFD}');
            }
            break;
        }
        out.push(c);
        used += w;
    }
    out
}

/// The first truthy value of `a || b || ...`, or the last operand.
pub fn first_truthy<'a>(vals: &[Option<&'a Value>]) -> Option<&'a Value> {
    vals.iter()
        .copied()
        .find(|v| js::truthy(*v))
        .unwrap_or_else(|| vals.last().copied().flatten())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_print_like_javascript() {
        assert_eq!(number_to_string(1e21), "1e+21");
        assert_eq!(number_to_string(1e20), "100000000000000000000");
        assert_eq!(number_to_string(1.5e-7), "1.5e-7");
        assert_eq!(number_to_string(0.000001), "0.000001");
        assert_eq!(number_to_string(-0.0), "0");
        assert_eq!(number_to_string(123.456), "123.456");
    }

    #[test]
    fn string_coercion() {
        assert_eq!(to_js_string(Some(&json!([1, null, [2, "a"]]))), "1,,2,a");
        assert_eq!(to_js_string(Some(&json!({}))), "[object Object]");
        assert_eq!(to_js_string(None), "undefined");
    }

    #[test]
    fn number_coercion() {
        assert_eq!(to_number(Some(&json!(" 12 "))), 12.0);
        assert_eq!(to_number(Some(&json!("0x1F"))), 31.0);
        assert!(to_number(Some(&json!("-0x1F"))).is_nan());
        assert!(to_number(Some(&json!("inf"))).is_nan());
        assert_eq!(to_number(Some(&json!("-Infinity"))), f64::NEG_INFINITY);
        assert_eq!(to_number(Some(&json!(""))), 0.0);
        assert_eq!(to_number(Some(&json!([7]))), 7.0);
        assert_eq!(to_number(Some(&json!(".5"))), 0.5);
        assert!(to_number(Some(&json!("1e"))).is_nan());
        assert!(to_number(None).is_nan());
    }

    #[test]
    fn utf16_slicing() {
        assert_eq!(utf16_prefix("ab😀c", 3), "ab\u{FFFD}");
        assert_eq!(utf16_prefix("ab😀c", 4), "ab😀");
    }
}
