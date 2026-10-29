//! The Python semantics `persist-run.sh`'s heredocs rely on, over
//! `serde_json` values: `json.dumps` with its default arguments, truthiness,
//! `==`, `len`, and the `strptime` of an inflight marker's `ts`.
//!
//! A JSON integer is a Python `int` and any other number a `float`, which is
//! what `json.loads` gives; `float_roundtrip` keeps the float's value exact.

use std::cmp::Ordering;

use serde_json::{Number, Value};

/// `json.dumps(v)`: `", "` and `": "` separators, `ensure_ascii`, and floats
/// as `repr` writes them.
#[must_use]
pub fn dumps(v: &Value) -> String {
    let mut out = String::new();
    write(v, &mut out);
    out
}

fn write(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number(n)),
        Value::String(s) => string(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write(x, out);
            }
            out.push(']');
        }
        Value::Object(m) => {
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                string(k, out);
                out.push_str(": ");
                write(x, out);
            }
            out.push('}');
        }
    }
}

fn string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
        }
    }
    out.push('"');
}

fn number(n: &Number) -> String {
    if n.is_i64() || n.is_u64() {
        return n.to_string();
    }
    float_repr(n.as_f64().unwrap_or(f64::NAN))
}

/// `repr(float)`: the shortest round-trip digits, positional when the
/// decimal point falls in `(-4, 16]`, else `d.ddde±XX`.
#[must_use]
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    let sci = format!("{x:e}");
    let (sign, rest) = match sci.strip_prefix('-') {
        Some(r) => ("-", r),
        None => ("", sci.as_str()),
    };
    let (mantissa, exp) = rest
        .split_once('e')
        .expect("LowerExp always has an exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let exp: i32 = exp.parse().expect("LowerExp exponent is an integer");
    let decpt = exp + 1;
    let len = digits.len() as i32;
    let body = if decpt <= -4 || decpt > 16 {
        let mut m = digits[..1].to_owned();
        if len > 1 {
            m.push('.');
            m.push_str(&digits[1..]);
        }
        let e = decpt - 1;
        format!("{m}e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs())
    } else if decpt <= 0 {
        format!("0.{}{digits}", "0".repeat((-decpt) as usize))
    } else if decpt >= len {
        format!("{digits}{}.0", "0".repeat((decpt - len) as usize))
    } else {
        format!(
            "{}.{}",
            &digits[..decpt as usize],
            &digits[decpt as usize..]
        )
    };
    format!("{sign}{body}")
}

/// Python truthiness of a JSON value; `None` is a missing key.
#[must_use]
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(m)) => !m.is_empty(),
    }
}

fn numeric(v: &Value) -> Option<f64> {
    match v {
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

/// Python `a == b`, where a missing key reads as `None` and `bool` is an
/// `int`.
#[must_use]
pub fn eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    let null = Value::Null;
    let (a, b) = (a.unwrap_or(&null), b.unwrap_or(&null));
    if let (Some(x), Some(y)) = (numeric(a), numeric(b)) {
        return match (a, b) {
            (Value::Number(m), Value::Number(n)) => match (m.as_i64(), n.as_i64()) {
                (Some(i), Some(j)) => i == j,
                _ => match (m.as_u64(), n.as_u64()) {
                    (Some(i), Some(j)) => i == j,
                    _ => x.partial_cmp(&y) == Some(Ordering::Equal),
                },
            },
            _ => x == y,
        };
    }
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| eq(Some(p), Some(q)))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, p)| y.get(k).is_some_and(|q| eq(Some(p), Some(q))))
        }
        _ => false,
    }
}

/// `len(v)`; `None` where Python raises `TypeError`.
#[must_use]
pub fn len(v: &Value) -> Option<usize> {
    match v {
        Value::String(s) => Some(s.chars().count()),
        Value::Array(a) => Some(a.len()),
        Value::Object(m) => Some(m.len()),
        _ => None,
    }
}

/// `v >= 0`; `None` where Python raises `TypeError`.
#[must_use]
pub fn ge_zero(v: &Value) -> Option<bool> {
    numeric(v).map(|x| x >= 0.0)
}

/// `type(v).__name__`, for the `AttributeError` an `.get` on it raises.
#[must_use]
pub fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_i64() || n.is_u64() => "int",
        Value::Number(_) => "float",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `isinstance(v, int) and not isinstance(v, bool)`, as a wide integer.
#[must_use]
pub fn int(v: Option<&Value>) -> Option<i128> {
    match v {
        Some(Value::Number(n)) => n
            .as_i64()
            .map(i128::from)
            .or_else(|| n.as_u64().map(i128::from)),
        _ => None,
    }
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `datetime.strptime(ts, '%Y-%m-%dT%H:%M:%SZ')` as a UTC epoch: the
/// directive patterns of `_strptime`, matched case-insensitively, then the
/// `datetime` range checks.
#[must_use]
pub fn strptime_epoch(ts: &str) -> Option<i64> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(
            r"(?i-u)^([0-9]{4})-(1[0-2]|0[1-9]|[1-9])-(3[01]|[12][0-9]|0[1-9]|[1-9]| [1-9])T(2[0-3]|[0-1][0-9]|[0-9]):([0-5][0-9]|[0-9]):(6[0-1]|[0-5][0-9]|[0-9])Z$",
        )
        .expect("fixed pattern")
    });
    let c = re.captures(ts)?;
    let n = |i: usize| c[i].trim().parse::<i64>().ok();
    let (y, mo, d, h, mi, s) = (n(1)?, n(2)?, n(3)?, n(4)?, n(5)?, n(6)?);
    if y < 1 || d > days_in_month(y, mo) || s > 59 {
        return None;
    }
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn floats_print_like_repr() {
        let cases = [
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (0.1, "0.1"),
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (1.5e-5, "1.5e-05"),
            (0.0001, "0.0001"),
            (123.456, "123.456"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
        ];
        for (x, want) in cases {
            assert_eq!(float_repr(x), want, "{x}");
        }
    }

    #[test]
    fn dumps_escapes_like_ensure_ascii() {
        let v = json!({"a": "é\u{7f}\u{1f600}\"\n", "b": [1, 2.5, null, true]});
        let want = concat!(
            "{\"a\": \"\\u00e9\\u007f\\ud83d\\ude00\\\"\\n\", ",
            "\"b\": [1, 2.5, null, true]}"
        );
        assert_eq!(dumps(&v), want);
    }

    #[test]
    fn equality_crosses_int_float_and_bool() {
        assert!(eq(Some(&json!(1)), Some(&json!(1.0))));
        assert!(eq(Some(&json!(true)), Some(&json!(1))));
        assert!(eq(None, Some(&json!(null))));
        assert!(!eq(Some(&json!("1")), Some(&json!(1))));
        assert!(eq(Some(&json!({"a": [1]})), Some(&json!({"a": [1.0]}))));
    }

    #[test]
    fn strptime_takes_python_shapes() {
        assert_eq!(strptime_epoch("1970-01-01T00:00:05Z"), Some(5));
        assert_eq!(strptime_epoch("1970-1-1t0:0:5z"), Some(5));
        assert_eq!(strptime_epoch("1970-01- 1T00:00:05Z"), Some(5));
        assert_eq!(strptime_epoch("2023-02-29T00:00:00Z"), None);
        assert_eq!(strptime_epoch("1970-01-01T00:00:60Z"), None);
        assert_eq!(strptime_epoch("0000-01-01T00:00:00Z"), None);
    }
}
