//! JavaScript string and value semantics the ported regions depend on.
//!
//! The workflow these functions come from is JavaScript, and several of its
//! checks differ from the obvious Rust spelling: `.length` counts UTF-16 code
//! units, `trim()` uses a different whitespace set from `str::trim`, and a
//! multiline regex treats `\r`, U+2028 and U+2029 as line ends.

use serde_json::Value;

/// `String.prototype.length`: UTF-16 code units, not chars or bytes.
pub fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// The WhiteSpace and LineTerminator sets of ECMA-262. Unlike
/// `char::is_whitespace`, this includes U+FEFF and excludes U+0085.
pub fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// `String.prototype.trim`.
pub fn trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// A LineTerminator: where `.` stops and where `^`/`$` match in a multiline
/// JavaScript regex.
pub fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// The lines a multiline JavaScript regex sees, as byte ranges. A `^` can match
/// at each start; the range ends before the terminator. `"a\r\nb"` yields
/// `a`, an empty line between `\r` and `\n`, and `b`.
pub fn line_spans(s: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if is_line_terminator(c) {
            spans.push((start, i));
            start = i + c.len_utf8();
        }
    }
    spans.push((start, s.len()));
    spans
}

/// JavaScript truthiness of a JSON value.
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0 && !x.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(_)) | Some(Value::Object(_)) => true,
    }
}

/// `typeof v === "object" && v !== null`. Arrays count, as they do in
/// JavaScript.
pub fn is_object_like(v: &Value) -> bool {
    matches!(v, Value::Object(_) | Value::Array(_))
}

/// Property read on a JSON value: only an object has named properties here.
pub fn get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(m) => m.get(key),
        _ => None,
    }
}

/// `Number.isInteger`.
pub fn is_integer(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Number(n)) => n
            .as_f64()
            .is_some_and(|x| x.is_finite() && x.fract() == 0.0),
        _ => false,
    }
}

/// Case-insensitive match of an ASCII literal, as a JavaScript regex with the
/// `i` flag and without `u` does it: only ASCII letters fold, so U+017F never
/// matches `s` and U+212A never matches `k`.
pub fn starts_with_ascii_ci(s: &str, lit: &str) -> bool {
    debug_assert!(lit.is_ascii());
    s.len() >= lit.len()
        && s.is_char_boundary(lit.len())
        && s.as_bytes()[..lit.len()].eq_ignore_ascii_case(lit.as_bytes())
}

/// `/lit/i.test(s)` for an ASCII literal, with the same folding as
/// `starts_with_ascii_ci`.
pub fn contains_ascii_ci(s: &str, lit: &str) -> bool {
    debug_assert!(lit.is_ascii());
    lit.is_empty()
        || s.as_bytes()
            .windows(lit.len())
            .any(|w| w.eq_ignore_ascii_case(lit.as_bytes()))
}

/// `Number.prototype.toString()` (radix 10), as template literals and
/// `String(n)` print a number: `1e+21`, `1e-7`, `0.30000000000000004`, and
/// `-0` as `0`.
pub fn number_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_owned();
    }
    if x == 0.0 {
        return "0".to_owned();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    if x < 0.0 {
        return format!("-{}", number_to_string(-x));
    }
    // `{:e}` without a precision prints the fewest digits that round-trip, but
    // when several such digit strings exist it does not always pick the one
    // closest to `x`, which ECMA-262 Number::toString requires
    // (1658206780088562.2, not .3). The correctly rounded string of that same
    // length is the closest one; it is used whenever it round-trips.
    let shortest = format!("{x:e}");
    let len = shortest
        .split_once('e')
        .map_or(0, |(m, _)| m.chars().filter(char::is_ascii_digit).count());
    let closest = format!("{:.*e}", len.saturating_sub(1), x);
    let sci = if closest.parse::<f64>() == Ok(x) {
        closest
    } else {
        shortest
    };
    let (mantissa, exp) = sci
        .split_once('e')
        .expect("LowerExp always has an exponent");
    let mut digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
    }
    let k = digits.len() as i32;
    let n = exp.parse::<i32>().expect("integer exponent") + 1;
    if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        let (int, frac) = digits.split_at(n as usize);
        format!("{int}.{frac}")
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let sign = if n - 1 < 0 { '-' } else { '+' };
        let m = if k == 1 {
            digits.clone()
        } else {
            format!("{}.{}", &digits[..1], &digits[1..])
        };
        format!("{m}e{sign}{}", (n - 1).abs())
    }
}
/// StringToNumber: the JS trim set, an empty string is 0, `0x`/`0o`/`0b`
/// literals without a sign, `Infinity`, and decimal literals. Anything else is
/// NaN, including the `inf`/`nan` spellings Rust's parser accepts.
pub fn string_to_number(s: &str) -> f64 {
    let t = trim(s);
    if t.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(rest) = t.strip_prefix(prefix) {
            if rest.is_empty() || !rest.chars().all(|c| c.is_digit(radix)) {
                return f64::NAN;
            }
            return radix_literal_to_f64(rest, radix);
        }
    }
    let (negative, body) = match t.as_bytes()[0] {
        b'+' => (false, &t[1..]),
        b'-' => (true, &t[1..]),
        _ => (false, t),
    };
    if body == "Infinity" {
        return if negative {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    if !is_decimal_literal(body) {
        return f64::NAN;
    }
    let v: f64 = body.parse().unwrap_or(f64::NAN);
    if negative {
        -v
    } else {
        v
    }
}
/// A `0x`/`0o`/`0b` literal's digits, rounded once to nearest-even as V8 does.
/// Folding digit by digit rounds at every step past 2^53 and can land an ulp
/// off once the literal is wider than 128 bits.
fn radix_literal_to_f64(digits: &str, radix: u32) -> f64 {
    let width = radix.trailing_zeros();
    let mut bits: Vec<bool> = Vec::new();
    for c in digits.chars() {
        let d = c.to_digit(radix).unwrap_or(0);
        for k in (0..width).rev() {
            let b = (d >> k) & 1 == 1;
            if b || !bits.is_empty() {
                bits.push(b);
            }
        }
    }
    let to_int = |b: &[bool]| b.iter().fold(0u64, |a, &x| (a << 1) | u64::from(x));
    if bits.len() <= 53 {
        return to_int(&bits) as f64;
    }
    let mut mantissa = to_int(&bits[..53]);
    let sticky = bits[54..].iter().any(|&b| b);
    if bits[53] && (sticky || mantissa & 1 == 1) {
        mantissa += 1;
    }
    let exp = i32::try_from(bits.len() - 53).unwrap_or(i32::MAX);
    mantissa as f64 * 2f64.powi(exp)
}
fn is_decimal_literal(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut digits = i - int_start;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        digits += i - frac_start;
    }
    if digits == 0 {
        return false;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let exp_start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == exp_start {
            return false;
        }
    }
    i == b.len()
}

/// `JSON.stringify(v, null, indent)`: keys in JavaScript order, numbers as
/// JavaScript prints them, and the layout `indent` gives (compact when empty).
pub fn stringify(v: &Value, indent: &str) -> String {
    let mut out = String::new();
    write_json(&mut out, v, indent, 0);
    out
}

fn write_json(out: &mut String, v: &Value, indent: &str, depth: usize) {
    let (open_sep, item_sep, close_sep, colon) = if indent.is_empty() {
        (String::new(), ",".to_owned(), String::new(), ":")
    } else {
        (
            format!("\n{}", indent.repeat(depth + 1)),
            format!(",\n{}", indent.repeat(depth + 1)),
            format!("\n{}", indent.repeat(depth)),
            ": ",
        )
    };
    match v {
        Value::Number(n) => out.push_str(&number_to_string(n.as_f64().unwrap_or(f64::NAN))),
        Value::Array(a) if a.is_empty() => out.push_str("[]"),
        Value::Array(a) => {
            out.push('[');
            out.push_str(&open_sep);
            for (i, e) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(&item_sep);
                }
                write_json(out, e, indent, depth + 1);
            }
            out.push_str(&close_sep);
            out.push(']');
        }
        Value::Object(m) if m.is_empty() => out.push_str("{}"),
        Value::Object(m) => {
            out.push('{');
            out.push_str(&open_sep);
            for (i, (k, e)) in crate::profile::js_ext::ordered_entries(m)
                .into_iter()
                .enumerate()
            {
                if i > 0 {
                    out.push_str(&item_sep);
                }
                out.push_str(&serde_json::to_string(k).expect("string"));
                out.push_str(colon);
                write_json(out, e, indent, depth + 1);
            }
            out.push_str(&close_sep);
            out.push('}');
        }
        other => out.push_str(&other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn length_counts_utf16_units() {
        assert_eq!(utf16_len("abc"), 3);
        assert_eq!(utf16_len("é"), 1);
        assert_eq!(utf16_len("😀"), 2);
    }

    #[test]
    fn trim_uses_the_js_whitespace_set() {
        assert_eq!(trim("\u{FEFF} x \u{FEFF}"), "x");
        assert_eq!(trim("\u{0085}x\u{0085}"), "\u{0085}x\u{0085}");
    }

    #[test]
    fn crlf_has_an_empty_line_between_cr_and_lf() {
        let s = "a\r\nb";
        let lines: Vec<&str> = line_spans(s).iter().map(|&(a, b)| &s[a..b]).collect();
        assert_eq!(lines, ["a", "", "b"]);
    }

    #[test]
    fn truthiness() {
        assert!(!truthy(Some(&json!(0))));
        assert!(!truthy(Some(&json!(""))));
        assert!(truthy(Some(&json!([]))));
        assert!(!truthy(None));
    }

    #[test]
    fn integer_check_accepts_whole_floats() {
        assert!(is_integer(Some(&json!(3.0))));
        assert!(!is_integer(Some(&json!(3.5))));
        assert!(!is_integer(Some(&json!("3"))));
    }

    #[test]
    fn ascii_case_fold_only() {
        assert!(starts_with_ascii_ci("VERDICT x", "verdict"));
        assert!(!starts_with_ascii_ci("\u{212A}ey", "key"));
    }
}
