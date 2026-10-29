//! JavaScript semantics the profile regions need beyond `crate::js`: value
//! `ToString`, number formatting, `JSON.stringify`, the key order of a
//! JavaScript object, strict equality, and `new RegExp(pattern, "i")`.
//!
//! Risk-path patterns are written by a profile's author and compiled by the
//! JavaScript engine, so they go to `regress`, an ECMAScript regex engine, in
//! its non-Unicode mode and over UTF-16 code units, as V8 runs them.

pub use crate::js::number_to_string;
use serde_json::{Map, Value};

/// `ToString` of a JSON value; `None` is `undefined`.
pub fn to_string(v: Option<&Value>) -> String {
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
                e => to_string(Some(e)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
    }
}

/// A canonical array index ("0", "7", not "07"): JavaScript lists these keys
/// first, in numeric order, whatever order they were inserted in.
fn array_index(key: &str) -> Option<u64> {
    if key.is_empty() || key.len() > 10 || !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if key.len() > 1 && key.starts_with('0') {
        return None;
    }
    key.parse::<u64>().ok().filter(|&n| n < u32::MAX as u64)
}

/// The keys of an object in the order `Object.keys` and `JSON.stringify`
/// visit them.
pub fn ordered_entries(m: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut idx: Vec<(u64, &String, &Value)> = Vec::new();
    let mut rest = Vec::new();
    for (k, v) in m {
        match array_index(k) {
            Some(n) => idx.push((n, k, v)),
            None => rest.push((k, v)),
        }
    }
    idx.sort_by_key(|&(n, _, _)| n);
    idx.into_iter()
        .map(|(_, k, v)| (k, v))
        .chain(rest)
        .collect()
}

/// A value as JavaScript would hold it after `JSON.parse`: every object's keys
/// in JavaScript order. Applied to whatever a ported function passes through,
/// so its serialization matches the workflow's.
pub fn js_order(v: &Value) -> Value {
    match v {
        Value::Array(a) => Value::Array(a.iter().map(js_order).collect()),
        Value::Object(m) => Value::Object(
            ordered_entries(m)
                .into_iter()
                .map(|(k, v)| (k.clone(), js_order(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `JSON.stringify`, as a template literal shows it (`undefined` for a missing
/// value).
pub fn json_stringify(v: Option<&Value>) -> String {
    match v {
        None => "undefined".into(),
        Some(Value::Number(n)) => number_to_string(n.as_f64().unwrap_or(f64::NAN)),
        Some(Value::Array(a)) => format!(
            "[{}]",
            a.iter()
                .map(|e| json_stringify(Some(e)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Some(Value::Object(m)) => format!(
            "{{{}}}",
            ordered_entries(m)
                .into_iter()
                .map(|(k, v)| format!(
                    "{}:{}",
                    serde_json::to_string(k).expect("string"),
                    json_stringify(Some(v))
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Some(other) => other.to_string(),
    }
}

/// `a === b` for two values read from JSON. Two arrays or objects are never
/// the same object, whatever they hold.
pub fn strict_equals(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Value::Null), Some(Value::Null)) => true,
        (Some(Value::Bool(x)), Some(Value::Bool(y))) => x == y,
        (Some(Value::Number(x)), Some(Value::Number(y))) => x.as_f64() == y.as_f64(),
        (Some(Value::String(x)), Some(Value::String(y))) => x == y,
        _ => false,
    }
}

/// The message V8 gives `TypeError` for a property read on `null`.
pub fn null_property_error(prop: &str) -> String {
    format!("Cannot read properties of null (reading '{prop}')")
}

// ---------------------------------------------------------------------------
// new RegExp(pattern, "i")
//
// regress runs the pattern. Where it parts from V8 in non-Unicode mode, this
// wrapper rewrites the pattern on the way in and the path on the way to
// `test`. Each rule was found by a differential run against V8 and is pinned
// by `tests/diff_profile.rs`.
// ---------------------------------------------------------------------------

const NOTHING_TO_REPEAT: &str = "Nothing to repeat";
const INVALID_ESCAPE: &str = "Invalid escape";

const fn unit(c: u8) -> u16 {
    c as u16
}

/// Code units V8 matches only to themselves under `i` without `u`, which
/// regress folds onto another unit: `ı` and `ſ` (their uppercase is ASCII,
/// which non-Unicode Canonicalize refuses), the Kelvin, Ohm and Angstrom
/// signs, `ϴ` and `ẞ` (regress closes a character class under Unicode simple
/// folding), and one member of each pair whose uppercase is two units long
/// (regress pairs them by simple uppercase). Found by matching every BMP unit
/// against every other through `\uXXXX` and `[\uXXXX]` in both engines.
const FOLD_SINGLETONS: [u16; 37] = [
    0x0131, 0x017F, 0x03F4, 0x1E9E, 0x1F88, 0x1F89, 0x1F8A, 0x1F8B, 0x1F8C, 0x1F8D, 0x1F8E, 0x1F8F,
    0x1F98, 0x1F99, 0x1F9A, 0x1F9B, 0x1F9C, 0x1F9D, 0x1F9E, 0x1F9F, 0x1FA8, 0x1FA9, 0x1FAA, 0x1FAB,
    0x1FAC, 0x1FAD, 0x1FAE, 0x1FAF, 0x1FBC, 0x1FCC, 0x1FD3, 0x1FE3, 0x1FFC, 0x2126, 0x212A, 0x212B,
    0xFB05,
];

/// Each `FOLD_SINGLETONS` unit is matched as a stand-in that folds to
/// nothing: lead surrogates of planes 15 and 16, which a path holds only as
/// the first half of a supplementary private-use character.
const STAND_IN_BASE: u16 = 0xDB80;

fn stand_in(u: u16) -> Option<u16> {
    FOLD_SINGLETONS
        .iter()
        .position(|&s| s == u)
        .map(|i| STAND_IN_BASE + i as u16)
}

/// The units `\W` holds: everything but `[0-9A-Za-z_]`.
const NON_WORD: [(u16, u16); 5] = [
    (0x0000, 0x002F),
    (0x003A, 0x0040),
    (0x005B, 0x005E),
    (0x0060, 0x0060),
    (0x007B, 0xFFFF),
];

/// A compiled `new RegExp(pattern, "i")`. Only `test` is offered: the
/// workflow never reads `lastIndex` or match groups from these patterns, and
/// without the `g` flag `test` keeps no state between calls.
#[derive(Debug, Clone)]
pub struct JsRegex {
    re: regress::Regex,
}

impl JsRegex {
    /// `new RegExp(pattern, "i")`. The error reads `Invalid regular
    /// expression: /<pattern>/i: <reason>`; the reason is regress's wording,
    /// not V8's, except for the two refusals `prepare` adds.
    pub fn new_ignore_case(pattern: &str) -> Result<JsRegex, String> {
        let error = |reason: &str| format!("Invalid regular expression: /{pattern}/i: {reason}");
        let units: Vec<u16> = pattern.encode_utf16().collect();
        let source = prepare(&units).map_err(error)?;
        regress::Regex::from_unicode(source.into_iter(), "i")
            .map(|re| JsRegex { re })
            .map_err(|e| error(&e.text))
    }

    /// `RegExp.prototype.test` on a string, over its UTF-16 code units, with
    /// each `FOLD_SINGLETONS` unit replaced by its stand-in.
    pub fn test(&self, s: &str) -> bool {
        let input: Vec<u16> = s.encode_utf16().map(|u| stand_in(u).unwrap_or(u)).collect();
        self.re.find_from_ucs2(&input, 0).next().is_some()
    }
}

/// V8's `ScanForCaptures` answer to one question: whether the pattern has a
/// named group anywhere, skipping escapes and class bodies.
fn has_named_groups(units: &[u16]) -> bool {
    let mut i = 0;
    while i < units.len() {
        let c = units[i];
        i += 1;
        if c == unit(b'\\') {
            i += 1;
        } else if c == unit(b'[') {
            while i < units.len() {
                let d = units[i];
                i += 1;
                if d == unit(b'\\') {
                    i += 1;
                } else if d == unit(b']') {
                    break;
                }
            }
        } else if c == unit(b'(')
            && units.get(i) == Some(&unit(b'?'))
            && units.get(i + 1) == Some(&unit(b'<'))
            && !matches!(units.get(i + 2), Some(&d) if d == unit(b'=') || d == unit(b'!'))
        {
            return true;
        }
    }
    false
}

fn hex_digit(u: Option<&u16>) -> Option<u16> {
    let c = char::from_u32(u32::from(*u?))?;
    c.to_digit(16).map(|d| d as u16)
}

/// The value of `n` hex digits at `i`, if all `n` are there.
fn hex_at(units: &[u16], i: usize, n: usize) -> Option<u16> {
    (i..i + n).try_fold(0u16, |v, j| Some(v * 16 + hex_digit(units.get(j))?))
}

fn is_octal(u: Option<&u16>) -> bool {
    u.is_some_and(|d| (unit(b'0')..=unit(b'7')).contains(d))
}

/// Whether a quantifier starts at `i`: `*`, `+`, `?`, or a `{n}`, `{n,}` or
/// `{n,m}` brace. Annex B reads any other `{` as a literal.
fn quantifier_at(units: &[u16], i: usize) -> bool {
    let digits_end = |mut j: usize| {
        while units
            .get(j)
            .is_some_and(|d| (unit(b'0')..=unit(b'9')).contains(d))
        {
            j += 1;
        }
        j
    };
    match units.get(i) {
        Some(&c) if c == unit(b'*') || c == unit(b'+') || c == unit(b'?') => true,
        Some(&c) if c == unit(b'{') => {
            let j = digits_end(i + 1);
            if j == i + 1 {
                return false;
            }
            match units.get(j) {
                Some(&c) if c == unit(b'}') => true,
                Some(&c) if c == unit(b',') => units.get(digits_end(j + 1)) == Some(&unit(b'}')),
                _ => false,
            }
        }
        _ => false,
    }
}

/// One class atom, as V8 reads it without `u`.
#[derive(Clone, Copy)]
enum ClassAtom {
    Unit(u16),
    /// `\d`, `\D`, `\s`, `\S`, `\w` or `\W`, by its letter.
    Escape(u16),
}

/// One member of a class after its ranges are read.
enum ClassItem {
    Atom(ClassAtom),
    Range(u16, u16),
}

/// The pattern rewritten for regress. See `prepare`.
struct Prepared<'a> {
    units: &'a [u16],
    named: bool,
    i: usize,
    out: Vec<u32>,
}

impl Prepared<'_> {
    fn push(&mut self, u: u16) {
        self.out.push(u32::from(u));
    }

    /// A unit that means itself wherever it lands: `\uXXXX`, or the bare
    /// unit for a surrogate. regress joins `\uD83D\uDE00` into one character,
    /// which no single unit of the path then matches, and after a lead
    /// surrogate escape it misreads any `\u` that follows (`[\uDB80\u0132]`
    /// matches `0`); V8 without `u` reads both as two plain units.
    fn push_escaped(&mut self, u: u16) {
        if (0xD800..0xE000).contains(&u) {
            self.push(u);
            return;
        }
        self.out.extend("\\u".chars().map(u32::from));
        self.out.extend(format!("{u:04X}").chars().map(u32::from));
    }

    /// Copies a group name from its `<` through its `>`, joining each
    /// surrogate pair into one code point: V8 reads a name by code point even
    /// without the `u` flag, so `(?<𝒜>x)` is a valid name, and regress needs
    /// the joined character to see it.
    fn copy_name(&mut self) {
        let units = self.units;
        while self.i < units.len() {
            let c = units[self.i];
            let next = units.get(self.i + 1).copied();
            if (0xD800..0xDC00).contains(&c) && next.is_some_and(|d| (0xDC00..0xE000).contains(&d))
            {
                let d = next.expect("checked");
                self.out
                    .push(0x10000 + ((u32::from(c) - 0xD800) << 10) + (u32::from(d) - 0xDC00));
                self.i += 2;
                continue;
            }
            self.push(c);
            self.i += 1;
            if c == unit(b'>') {
                break;
            }
        }
    }

    /// An escape outside a class, from its backslash.
    fn escape(&mut self) -> Result<(), &'static str> {
        let units = self.units;
        let Some(&n) = units.get(self.i + 1) else {
            self.push(unit(b'\\'));
            self.i += 1;
            return Ok(());
        };
        if n == unit(b'u') && units.get(self.i + 2) == Some(&unit(b'{')) {
            self.push(n);
            self.i += 2;
            return Ok(());
        }
        if n == unit(b'u') {
            if let Some(v) = hex_at(units, self.i + 2, 4) {
                self.push_escaped(stand_in(v).unwrap_or(v));
                self.i += 6;
                return Ok(());
            }
        }
        if let Some(s) = stand_in(n) {
            self.push_escaped(s);
            self.i += 2;
            return Ok(());
        }
        self.out.extend([u32::from(unit(b'\\')), u32::from(n)]);
        self.i += 2;
        if (n == unit(b'b') || n == unit(b'B')) && quantifier_at(units, self.i) {
            return Err(NOTHING_TO_REPEAT);
        }
        if self.named && n == unit(b'k') && units.get(self.i) == Some(&unit(b'<')) {
            self.copy_name();
        }
        Ok(())
    }

    /// One class atom at `i`, or `None` at a backslash that ends the pattern.
    fn class_atom(&mut self) -> Result<Option<ClassAtom>, &'static str> {
        let units = self.units;
        let c = units[self.i];
        if c != unit(b'\\') {
            self.i += 1;
            return Ok(Some(ClassAtom::Unit(c)));
        }
        let Some(&n) = units.get(self.i + 1) else {
            return Ok(None);
        };
        let at = self.i + 2;
        let (atom, len) = match n {
            _ if b"dDsSwW".iter().any(|&e| unit(e) == n) => (ClassAtom::Escape(n), 2),
            _ if n == unit(b'b') => (ClassAtom::Unit(0x08), 2),
            _ if n == unit(b'f') => (ClassAtom::Unit(0x0C), 2),
            _ if n == unit(b'n') => (ClassAtom::Unit(0x0A), 2),
            _ if n == unit(b'r') => (ClassAtom::Unit(0x0D), 2),
            _ if n == unit(b't') => (ClassAtom::Unit(0x09), 2),
            _ if n == unit(b'v') => (ClassAtom::Unit(0x0B), 2),
            _ if n == unit(b'k') && self.named => return Err(INVALID_ESCAPE),
            _ if n == unit(b'c') => match units.get(at) {
                Some(&l)
                    if char::from_u32(u32::from(l))
                        .is_some_and(|l| l.is_ascii_alphanumeric() || l == '_') =>
                {
                    (ClassAtom::Unit(l % 32), 3)
                }
                // `\c` without a control letter is a backslash; the `c`
                // is read next as itself.
                _ => (ClassAtom::Unit(unit(b'\\')), 1),
            },
            _ if is_octal(Some(&n)) => {
                let mut v = n - unit(b'0');
                let mut len = 2;
                if is_octal(units.get(at)) {
                    v = v * 8 + units[at] - unit(b'0');
                    len = 3;
                    if v < 32 && is_octal(units.get(at + 1)) {
                        v = v * 8 + units[at + 1] - unit(b'0');
                        len = 4;
                    }
                }
                (ClassAtom::Unit(v), len)
            }
            _ if n == unit(b'x') => match hex_at(units, at, 2) {
                Some(v) => (ClassAtom::Unit(v), 4),
                None => (ClassAtom::Unit(n), 2),
            },
            _ if n == unit(b'u') => match hex_at(units, at, 4) {
                Some(v) => (ClassAtom::Unit(v), 6),
                None => (ClassAtom::Unit(n), 2),
            },
            _ => (ClassAtom::Unit(n), 2),
        };
        self.i += len;
        Ok(Some(atom))
    }

    /// A class from its `[`. It is copied as written unless it holds `\W` or
    /// a `FOLD_SINGLETONS` unit (alone or inside a range): then it is written
    /// out unit by unit, with each such unit as its stand-in and `\W` without
    /// them. regress closes a class under Unicode folding, so a class holding
    /// `ſ` (as `\W` does) would also match `s` and `S`.
    fn class(&mut self) -> Result<(), &'static str> {
        let units = self.units;
        let start = self.i;
        self.i += 1;
        let negated = units.get(self.i) == Some(&unit(b'^'));
        if negated {
            self.i += 1;
        }
        let mut items = Vec::new();
        let mut verbatim = false;
        let mut closed = false;
        while self.i < units.len() {
            if units[self.i] == unit(b']') {
                self.i += 1;
                closed = true;
                break;
            }
            let Some(a) = self.class_atom()? else {
                break;
            };
            let dash = units.get(self.i) == Some(&unit(b'-'));
            let next = units.get(self.i + 1);
            if !dash || next.is_none() || next == Some(&unit(b']')) {
                items.push(ClassItem::Atom(a));
                continue;
            }
            self.i += 1;
            let Some(b) = self.class_atom()? else {
                break;
            };
            match (a, b) {
                (ClassAtom::Unit(lo), ClassAtom::Unit(hi)) if lo <= hi => {
                    items.push(ClassItem::Range(lo, hi))
                }
                (ClassAtom::Unit(_), ClassAtom::Unit(_)) => verbatim = true,
                // Annex B: a range with a class escape at either end is the
                // union of both ends and the dash.
                _ => items.extend([
                    ClassItem::Atom(a),
                    ClassItem::Atom(ClassAtom::Unit(unit(b'-'))),
                    ClassItem::Atom(b),
                ]),
            }
        }
        let touched = items.iter().any(|item| match *item {
            ClassItem::Atom(ClassAtom::Unit(u)) => stand_in(u).is_some(),
            ClassItem::Atom(ClassAtom::Escape(e)) => e == unit(b'W'),
            ClassItem::Range(lo, hi) => FOLD_SINGLETONS.iter().any(|s| (lo..=hi).contains(s)),
        });
        if verbatim || !closed || !touched {
            self.copy_class(start);
            return Ok(());
        }
        self.push(unit(b'['));
        if negated {
            self.push(unit(b'^'));
        }
        for item in items {
            match item {
                ClassItem::Atom(ClassAtom::Escape(e)) if e == unit(b'W') => {
                    for (lo, hi) in NON_WORD {
                        self.push_range(lo, hi);
                    }
                }
                ClassItem::Atom(ClassAtom::Escape(e)) => {
                    self.push(unit(b'\\'));
                    self.push(e);
                }
                ClassItem::Atom(ClassAtom::Unit(u)) => self.push_range(u, u),
                ClassItem::Range(lo, hi) => self.push_range(lo, hi),
            }
        }
        self.push(unit(b']'));
        Ok(())
    }

    /// `lo-hi` without its `FOLD_SINGLETONS` units, plus their stand-ins.
    fn push_range(&mut self, lo: u16, hi: u16) {
        let mut from = u32::from(lo);
        for s in FOLD_SINGLETONS
            .iter()
            .copied()
            .filter(|s| (lo..=hi).contains(s))
        {
            if from < u32::from(s) {
                self.push_span(from as u16, s - 1);
            }
            self.push_escaped(stand_in(s).expect("a singleton"));
            from = u32::from(s) + 1;
        }
        if from <= u32::from(hi) {
            self.push_span(from as u16, hi);
        }
    }

    fn push_span(&mut self, lo: u16, hi: u16) {
        self.push_escaped(lo);
        if lo < hi {
            self.push(unit(b'-'));
            self.push_escaped(hi);
        }
    }

    /// The class from `start` to `i` as written, except that `\u{` loses its
    /// backslash (see `prepare`) and a surrogate escape becomes its unit (see
    /// `push_escaped`).
    fn copy_class(&mut self, start: usize) {
        let units = self.units;
        let mut j = start;
        while j < self.i {
            if units[j] == unit(b'\\') && j + 1 < self.i {
                let surrogate = (units[j + 1] == unit(b'u'))
                    .then(|| hex_at(units, j + 2, 4))
                    .flatten()
                    .filter(|v| (0xD800..0xE000).contains(v));
                if let Some(v) = surrogate {
                    self.push(v);
                    j += 6;
                    continue;
                }
                if !(units[j + 1] == unit(b'u') && units.get(j + 2) == Some(&unit(b'{'))) {
                    self.push(units[j]);
                }
                self.push(units[j + 1]);
                j += 2;
            } else {
                self.push(units[j]);
                j += 1;
            }
        }
    }
}

/// The pattern as regress should read it: UTF-16 code units (so a quantifier
/// after an astral character repeats its low surrogate only, as in V8), with
/// the places where regress and V8 part brought into line:
///
/// - `\b` or `\B` under a quantifier: V8 refuses it (`Nothing to repeat`),
///   regress accepts it.
/// - `\k` in a class of a pattern with a named group: V8 refuses it
///   (`Invalid escape`), regress reads `k`.
/// - `\u{`: regress reads a code point escape, V8 without `u` reads the
///   letter `u` and a brace, so `\u{61}` is sixty-one `u`s. The backslash is
///   dropped, except in a group name, where V8 reads the escape too.
/// - A group name holding an astral character: see `copy_name`.
/// - A `FOLD_SINGLETONS` unit the pattern names, written or escaped, becomes
///   its stand-in, and a class holding one is rewritten (see `class`).
fn prepare(units: &[u16]) -> Result<Vec<u32>, &'static str> {
    let mut p = Prepared {
        units,
        named: has_named_groups(units),
        i: 0,
        out: Vec::with_capacity(units.len()),
    };
    while p.i < units.len() {
        let c = units[p.i];
        if c == unit(b'\\') {
            p.escape()?;
        } else if c == unit(b'[') {
            p.class()?;
        } else if c == unit(b'(')
            && units.get(p.i + 1) == Some(&unit(b'?'))
            && units.get(p.i + 2) == Some(&unit(b'<'))
            && !matches!(units.get(p.i + 3), Some(&d) if d == unit(b'=') || d == unit(b'!'))
        {
            p.out.extend([u32::from(c), u32::from(b'?')]);
            p.i += 2;
            p.copy_name();
        } else if let Some(s) = stand_in(c) {
            p.push_escaped(s);
            p.i += 1;
        } else {
            p.push(c);
            p.i += 1;
        }
    }
    Ok(p.out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn re(p: &str) -> JsRegex {
        JsRegex::new_ignore_case(p).unwrap()
    }

    #[test]
    fn numbers_print_as_javascript_does() {
        assert_eq!(number_to_string(1e21), "1e+21");
        assert_eq!(number_to_string(1e-7), "1e-7");
        assert_eq!(number_to_string(0.000001), "0.000001");
        assert_eq!(number_to_string(-2.5), "-2.5");
        assert_eq!(number_to_string(123.0), "123");
    }

    #[test]
    fn index_keys_come_first() {
        let v: Value = serde_json::from_str(r#"{"b":1,"10":2,"a":3,"2":4,"01":5}"#).unwrap();
        assert_eq!(
            json_stringify(Some(&v)),
            r#"{"2":4,"10":2,"b":1,"a":3,"01":5}"#
        );
        assert_eq!(to_string(Some(&json!([1, null, "x"]))), "1,,x");
    }

    #[test]
    fn case_folding_is_the_non_unicode_kind() {
        assert!(!re("s").test("\u{017F}"));
        assert!(!re("k").test("\u{212A}"));
        assert!(re("\u{03C3}").test("\u{03C2}"));
        assert!(!re("(?-i:a)").test("A"));
    }

    #[test]
    fn a_pattern_that_does_not_compile_is_an_error() {
        assert!(JsRegex::new_ignore_case("a{2,1}").is_err());
        assert!(JsRegex::new_ignore_case("(?<a>x)|(?<a>y)").is_ok());
        assert!(JsRegex::new_ignore_case("{").is_ok());
    }

    #[test]
    fn lookbehind_and_backreferences() {
        assert!(re("(?<=(a)\\1)b").test("aab"));
        assert!(re("(\\2a|(b))+").test("ba"));
        assert!(!re(".").test("\r"));
        assert!(re("(?m:^b)").test("a\u{2028}b"));
    }
}
