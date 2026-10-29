//! Port of `lib/classify-artifacts.sh`: every saved external-head answer in
//! a vote directory, classified by [`crate::classify_verdict`], as the one
//! JSON line the workflow's ARTIFACT-TRUTH reads.
//!
//! The script quotes each entry with Python's `json.dumps`, so the printed
//! line keeps its spacing (`", "`, `": "`), escapes everything outside
//! printable ASCII, and writes a confidence with `repr(float)`. Entries come
//! in byte order of their file names; the script's bash sorts its glob by the
//! locale's collation (see the README's declared differences).

use std::path::Path;

use serde_json::{json, Map, Value};

use crate::classify_verdict::{classify_file, Classification};

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub failclass: String,
    pub verdict: Option<String>,
    pub confidence: Option<f64>,
    pub confidence_declared: Option<bool>,
    pub reason: String,
}

impl Entry {
    /// What the script derives from the classifier's lines.
    pub fn from_classification(c: &Classification) -> Self {
        let reason = c
            .reason
            .as_deref()
            .unwrap_or("")
            .trim_start_matches(' ')
            .chars()
            .take(100)
            .collect();
        Self {
            failclass: c.failclass.as_str().to_owned(),
            verdict: c.verdict.map(|v| v.as_str().to_owned()),
            confidence: c.confidence.as_deref().and_then(py_float),
            confidence_declared: c.confidence_declared,
            reason,
        }
    }

    pub fn to_value(&self) -> Value {
        json!({
            "failclass": self.failclass,
            "verdict": self.verdict,
            "confidence": self.confidence,
            "confidenceDeclared": self.confidence_declared,
            "reason": self.reason,
        })
    }

    fn to_python(&self) -> String {
        let opt_str = |s: &Option<String>| s.as_deref().map_or("null".to_owned(), py_str);
        format!(
            "{{\"failclass\": {}, \"verdict\": {}, \"confidence\": {}, \"confidenceDeclared\": {}, \"reason\": {}}}",
            py_str(&self.failclass),
            opt_str(&self.verdict),
            self.confidence.map_or("null".to_owned(), py_float_repr),
            self.confidence_declared
                .map_or("null", |d| if d { "true" } else { "false" }),
            py_str(&self.reason)
        )
    }
}

/// The classified directory: `total` is the number of matching artifacts,
/// `entries` is keyed by artifact basename without `.out`.
#[derive(Debug, Clone, PartialEq)]
pub struct Artifacts {
    pub total: usize,
    pub entries: Vec<(String, Entry)>,
}

/// The script's result: `Skipped` is its `{}` for a path that is not a
/// directory.
#[derive(Debug, Clone, PartialEq)]
pub enum ArtifactScan {
    Skipped,
    Scanned(Artifacts),
}

impl ArtifactScan {
    pub fn to_value(&self) -> Value {
        match self {
            Self::Skipped => json!({}),
            Self::Scanned(a) => json!({"total": a.total, "entries": self.truth()}),
        }
    }

    /// The `entries` map ARTIFACT-TRUTH reads, or `None` for `{}`.
    pub fn truth(&self) -> Option<Value> {
        match self {
            Self::Skipped => None,
            Self::Scanned(a) => Some(Value::Object(
                a.entries
                    .iter()
                    .map(|(k, e)| (k.clone(), e.to_value()))
                    .collect::<Map<_, _>>(),
            )),
        }
    }

    /// The printed line, byte for byte, without its newline.
    pub fn to_line(&self) -> String {
        match self {
            Self::Skipped => "{}".to_owned(),
            Self::Scanned(a) => format!(
                "{{\"total\":{},\"entries\":{{{}}}}}",
                a.total,
                a.entries
                    .iter()
                    .map(|(k, e)| format!("{}:{}", py_str(k), e.to_python()))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }
}

/// `float(s)` for the digits-and-dots text the classifier prints.
fn py_float(s: &str) -> Option<f64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    s.parse().ok()
}

/// `json.dumps` of a string: ASCII output, short escapes for the five
/// controls Python names, `\uXXXX` (surrogate pairs above the BMP) for the
/// rest outside `' '..='~'`.
pub(crate) fn py_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{C}' => out.push_str("\\f"),
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
    out
}

/// `repr(float)`: the shortest round-trip digits, in positional form for a
/// decimal exponent from -4 to 15 and `d.ddde±XX` otherwise.
pub(crate) fn py_float_repr(x: f64) -> String {
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0" } else { "0.0" }.to_owned();
    }
    let sci = format!("{:e}", x.abs());
    let (mantissa, exp) = sci.split_once('e').expect("{:e} has an exponent");
    let exp: i32 = exp.parse().expect("an integer exponent");
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let sign = if x < 0.0 { "-" } else { "" };
    let n = digits.len() as i32;
    if (-4..16).contains(&exp) {
        let body = if exp < 0 {
            format!("0.{}{digits}", "0".repeat((-exp - 1) as usize))
        } else if exp + 1 >= n {
            format!("{digits}{}.0", "0".repeat((exp + 1 - n) as usize))
        } else {
            let (int, frac) = digits.split_at((exp + 1) as usize);
            format!("{int}.{frac}")
        };
        format!("{sign}{body}")
    } else {
        let (head, tail) = digits.split_at(1);
        let frac = if tail.is_empty() {
            String::new()
        } else {
            format!(".{tail}")
        };
        let esign = if exp < 0 { '-' } else { '+' };
        format!("{sign}{head}{frac}e{esign}{:02}", exp.abs())
    }
}

/// Classifies every `*.out` in `dir` whose basename contains `filter`.
pub fn classify_artifacts(dir: &Path, filter: &str) -> ArtifactScan {
    if !dir.is_dir() {
        return ArtifactScan::Skipped;
    }
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|it| {
            it.filter_map(Result::ok)
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| n.ends_with(".out") && !n.starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let entries: Vec<(String, Entry)> = names
        .into_iter()
        .filter(|n| dir.join(n).exists())
        .filter_map(|n| {
            let base = n[..n.len() - ".out".len()].to_owned();
            base.contains(filter).then(|| {
                let c = classify_file(&dir.join(&n));
                (base, Entry::from_classification(&c))
            })
        })
        .collect();
    ArtifactScan::Scanned(Artifacts {
        total: entries.len(),
        entries,
    })
}

/// The script's entry: `classify-artifacts.sh <artifact-dir> [suffix-filter]`.
///
/// # Errors
/// The usage error the script exits 1 with when `dir` is empty.
pub fn run(dir: &str, filter: Option<&str>) -> Result<ArtifactScan, String> {
    if dir.is_empty() {
        return Err("usage: classify-artifacts.sh <artifact-dir> [suffix-filter]".to_owned());
    }
    Ok(classify_artifacts(Path::new(dir), filter.unwrap_or("")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_print_like_python() {
        let cases = [
            (0.5, "0.5"),
            (1.0, "1.0"),
            (0.0, "0.0"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (0.000015, "1.5e-05"),
            (1.05, "1.05"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1e16, "1e+16"),
            (123.0, "123.0"),
        ];
        for (x, want) in cases {
            assert_eq!(py_float_repr(x), want, "{x}");
        }
    }

    #[test]
    fn strings_escape_like_python() {
        assert_eq!(
            py_str("a\"b\\c\r\u{7f}é😀\u{1}"),
            "\"a\\\"b\\\\c\\r\\u007f\\u00e9\\ud83d\\ude00\\u0001\""
        );
    }
}
