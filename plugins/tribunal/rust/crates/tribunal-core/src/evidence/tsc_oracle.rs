//! Port of the TSC-ORACLE region (both blocks): parse and classify the
//! grounded typecheck's output, narrow it to the PR's files, and decide a
//! finding against it before the panel sees it.
//!
//! The text parsed here is tsc's own output and the typecheck shell's fixed
//! markers, not a model's prose, so it stays a function of the binary.

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use super::js_ext;
use crate::js;

pub const GROUNDED_LINE_WINDOW: f64 = 5.0;

#[derive(Clone, Debug, PartialEq)]
pub struct TscDiagnostic {
    pub file: String,
    pub line: f64,
    pub code: String,
    pub message: String,
}

impl TscDiagnostic {
    pub fn to_value(&self) -> Value {
        json!({
            "file": self.file,
            "line": js_ext::number_value(self.line),
            "code": self.code,
            "message": self.message,
        })
    }
}

fn diagnostic_line() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // The JavaScript pattern spelled with explicit classes: `.` stops at
        // every JS line terminator, `\s` is the JS whitespace set (U+FEFF in,
        // U+0085 out) and `\d` is ASCII.
        let dot = r"[^\n\r\u{2028}\u{2029}]";
        let ws = r"[\t\n\x0B\x0C\r \u{A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}]";
        Regex::new(&format!(
            r"^({dot}+?)\(([0-9]+),([0-9]+)\):{ws}+error{ws}+(TS[0-9]+):{ws}+({dot}*)$"
        ))
        .unwrap()
    })
}

/// Ports `parseTscDiagnostics`. Lines split on `\n` only, so a `\r` left at
/// the end of a CRLF line fails the anchored match, as it does in JavaScript.
pub fn parse_tsc_diagnostics(text: Option<&str>) -> Vec<TscDiagnostic> {
    text.unwrap_or("")
        .split('\n')
        .filter_map(|raw| {
            let m = diagnostic_line().captures(raw)?;
            Some(TscDiagnostic {
                file: m[1].to_owned(),
                line: m[2].parse::<f64>().unwrap(),
                code: m[4].to_owned(),
                message: js::trim(&m[5]).to_owned(),
            })
        })
        .collect()
}

/// `/^\s*WORD\s*$/m`: some occurrence of `word` with only whitespace between
/// it and a line start before, and a line end after. `\s` includes the line
/// terminators, so the whitespace may span lines.
fn whole_line_marker(t: &str, word: &str) -> bool {
    t.match_indices(word).any(|(at, _)| {
        let before = t[..at]
            .chars()
            .rev()
            .take_while(|c| js::is_js_whitespace(*c));
        let mut back_ok = at == 0;
        let mut walked = 0;
        for c in before {
            walked += c.len_utf8();
            if js::is_line_terminator(c) {
                back_ok = true;
                break;
            }
        }
        if walked == at {
            back_ok = true;
        }
        let rest = &t[at + word.len()..];
        let mut fwd_ok = rest.is_empty();
        let mut walked = 0;
        for c in rest.chars().take_while(|c| js::is_js_whitespace(*c)) {
            walked += c.len_utf8();
            if js::is_line_terminator(c) {
                fwd_ok = true;
                break;
            }
        }
        if walked == rest.len() {
            fwd_ok = true;
        }
        back_ok && fwd_ok
    })
}

/// Ports `classifyTypecheckText`: `"no-tsconfig"`, `"degraded"`, `"clean"` or
/// `"diagnostics"`. Unrecognized output degrades rather than reading as clean.
pub fn classify_typecheck_text(text: Option<&str>) -> &'static str {
    let t = text.unwrap_or("");
    if whole_line_marker(t, "NO_TSCONFIG") {
        return "no-tsconfig";
    }
    if t.contains("DEGRADED_") {
        return "degraded";
    }
    let has_diagnostic = t.match_indices("): error TS").any(|(at, m)| {
        t.as_bytes()
            .get(at + m.len())
            .is_some_and(u8::is_ascii_digit)
    });
    if has_diagnostic {
        return "diagnostics";
    }
    if whole_line_marker(t, "NO_DIAGNOSTICS") {
        return "clean";
    }
    "degraded"
}

/// Ports `filterDiagnosticsToPR`.
pub fn filter_diagnostics_to_pr(
    diags: Option<&[TscDiagnostic]>,
    pr_files: Option<&[String]>,
) -> Vec<TscDiagnostic> {
    let set: HashSet<&str> = pr_files.unwrap_or(&[]).iter().map(String::as_str).collect();
    diags
        .unwrap_or(&[])
        .iter()
        .filter(|d| set.contains(d.file.as_str()))
        .cloned()
        .collect()
}

/// The `{ [file]: diagnostics[] }` object, kept in the order a JavaScript
/// object lists its keys.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiagsByFile {
    entries: Vec<(String, Vec<TscDiagnostic>)>,
}

impl DiagsByFile {
    /// Own entries, given in the object's key order.
    pub fn from_entries(entries: Vec<(String, Vec<TscDiagnostic>)>) -> Self {
        DiagsByFile { entries }
    }

    pub fn get(&self, file: &str) -> Option<&[TscDiagnostic]> {
        self.entries
            .iter()
            .find(|(k, _)| k == file)
            .map(|(_, v)| v.as_slice())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn to_value(&self) -> Value {
        Value::Object(
            self.entries
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        Value::Array(v.iter().map(TscDiagnostic::to_value).collect()),
                    )
                })
                .collect::<Map<String, Value>>(),
        )
    }
}

/// Ports `groupDiagnosticsByFile`. A file named after an `Object.prototype`
/// member makes `(by[file] ||= []).push` throw in JavaScript, so it is an
/// error here too.
pub fn group_diagnostics_by_file(diags: Option<&[TscDiagnostic]>) -> Result<DiagsByFile, String> {
    let mut entries: Vec<(String, Vec<TscDiagnostic>)> = Vec::new();
    for d in diags.unwrap_or(&[]) {
        if let Some((_, list)) = entries.iter_mut().find(|(k, _)| *k == d.file) {
            list.push(d.clone());
        } else if js_ext::object_prototype_property(&d.file).is_some() {
            return Err(format!("by[{:?}].push is not a function", d.file));
        } else {
            entries.push((d.file.clone(), vec![d.clone()]));
        }
    }
    let (mut index, rest): (Vec<_>, Vec<_>) = entries
        .into_iter()
        .partition(|(k, _)| js_ext::is_array_index(k));
    index.sort_by_key(|(k, _)| k.parse::<u64>().unwrap());
    Ok(DiagsByFile::from_entries(
        index.into_iter().chain(rest).collect(),
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroundedVerdict {
    Confirmed,
    Refuted,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Adjudication {
    pub verdict: Option<GroundedVerdict>,
    pub diagnostic: Option<String>,
}

impl Adjudication {
    pub fn to_value(&self) -> Value {
        json!({
            "verdict": self.verdict.map(|v| match v {
                GroundedVerdict::Confirmed => "confirmed",
                GroundedVerdict::Refuted => "refuted",
            }),
            "diagnostic": self.diagnostic,
        })
    }
}

/// Ports `adjudicateFinding`. The finding is model output, read as JSON.
///
/// `diagsByFile[finding.file]` is a property read, so a file named after an
/// `Object.prototype` member finds a function: with a known line, `.find` on
/// it throws; without one, its `.length` decides the refute branch.
pub fn adjudicate_finding(
    finding: &Value,
    diags_by_file: &DiagsByFile,
    covered: Option<&HashSet<String>>,
) -> Result<Adjudication, String> {
    let file = js::get(finding, "file");
    let key = js_ext::to_property_key(file);
    let own = diags_by_file.get(&key);
    let inherited = match own {
        Some(_) => None,
        None => js_ext::object_prototype_property(&key),
    };
    let diags: &[TscDiagnostic] = own.unwrap_or(&[]);
    let line = match js::get(finding, "line") {
        Some(Value::Number(n)) => n.as_f64().filter(|l| l.is_finite() && *l > 0.0),
        _ => None,
    };
    let hit = match line {
        Some(_) if inherited.is_some() => {
            return Err(format!("diagsByFile[{key:?}].find is not a function"))
        }
        Some(l) => diags
            .iter()
            .find(|d| (d.line - l).abs() <= GROUNDED_LINE_WINDOW),
        None => None,
    };
    if let Some(hit) = hit {
        return Ok(Adjudication {
            verdict: Some(GroundedVerdict::Confirmed),
            diagnostic: Some(format!(
                "{}({}): {}: {}",
                hit.file,
                js_ext::number_to_string(hit.line),
                hit.code,
                hit.message
            )),
        });
    }
    let is_covered =
        covered.is_some_and(|set| matches!(file, Some(Value::String(f)) if set.contains(f)));
    let no_diags = match inherited {
        Some(length) => length == Some(0),
        None => diags.is_empty(),
    };
    if js::truthy(js::get(finding, "mechanicalClaim")) && no_diags && is_covered {
        return Ok(Adjudication {
            verdict: Some(GroundedVerdict::Refuted),
            diagnostic: None,
        });
    }
    Ok(Adjudication {
        verdict: None,
        diagnostic: None,
    })
}

/// Ports `resolveDiagFile`. `file` is a finding's `file`, a string in the
/// findings schema; `None` stands for a missing one and comes back as is.
pub fn resolve_diag_file(file: Option<&str>, pr_files: Option<&[String]>) -> Option<String> {
    let files = pr_files.unwrap_or(&[]);
    let file = file?;
    if file.is_empty() || files.iter().any(|f| f == file) {
        return Some(file.to_owned());
    }
    let matches: Vec<&String> = files
        .iter()
        .filter(|f| f.ends_with(file) || file.ends_with(f.as_str()) || f.contains(file))
        .collect();
    Some(match matches.as_slice() {
        [only] => (*only).clone(),
        _ => file.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TSC: &str = "src/a.ts(42,10): error TS2345: Argument of type 'string' is not assignable.\n    continuation that tsc indents\nsrc/b.ts(7,1): error TS2304: Cannot find name 'x'.";

    #[test]
    fn one_diagnostic_per_head_line() {
        let d = parse_tsc_diagnostics(Some(TSC));
        assert_eq!(d.len(), 2);
        assert_eq!(
            (d[0].file.as_str(), d[0].line, d[0].code.as_str()),
            ("src/a.ts", 42.0, "TS2345")
        );
        assert!(parse_tsc_diagnostics(None).is_empty());
    }

    #[test]
    fn a_crlf_line_does_not_parse() {
        assert!(parse_tsc_diagnostics(Some("a.ts(1,2): error TS1: x\r")).is_empty());
    }

    #[test]
    fn classification() {
        for (text, want) in [
            ("NO_TSCONFIG", "no-tsconfig"),
            ("DEGRADED_TIMEOUT", "degraded"),
            (TSC, "diagnostics"),
            ("NO_DIAGNOSTICS", "clean"),
            ("", "degraded"),
            ("x NO_DIAGNOSTICS", "degraded"),
            ("x\r  NO_TSCONFIG \u{2028}y", "no-tsconfig"),
        ] {
            assert_eq!(classify_typecheck_text(Some(text)), want, "{text:?}");
        }
    }

    #[test]
    fn a_window_confirms_and_line_zero_does_not() {
        let by = group_diagnostics_by_file(Some(&filter_diagnostics_to_pr(
            Some(&parse_tsc_diagnostics(Some(TSC))),
            Some(&["src/a.ts".to_owned(), "src/c.ts".to_owned()]),
        )))
        .unwrap();
        assert!(by.get("src/b.ts").is_none());
        let covered: HashSet<String> = ["src/a.ts", "src/c.ts"].map(String::from).into();
        let at = |f: Value| adjudicate_finding(&f, &by, Some(&covered)).unwrap().verdict;
        assert_eq!(
            at(json!({"file": "src/a.ts", "line": 47})),
            Some(GroundedVerdict::Confirmed)
        );
        assert_eq!(at(json!({"file": "src/a.ts", "line": 48})), None);
        assert_eq!(at(json!({"file": "src/a.ts", "line": 0})), None);
        assert_eq!(
            at(json!({"file": "src/c.ts", "line": 3, "mechanicalClaim": true})),
            Some(GroundedVerdict::Refuted)
        );
    }

    #[test]
    fn a_shared_basename_does_not_resolve() {
        let files = ["src/a/index.ts".to_owned(), "src/b/index.ts".to_owned()];
        assert_eq!(
            resolve_diag_file(Some("index.ts"), Some(&files)).as_deref(),
            Some("index.ts")
        );
        assert_eq!(
            resolve_diag_file(Some("a/index.ts"), Some(&files)).as_deref(),
            Some("src/a/index.ts")
        );
    }
}
