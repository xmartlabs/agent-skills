//! Port of `skills/review-pr/scripts/resolve-evidence-line.mjs`: where each
//! candidate's `evidence` quote sits in the diff, and the line correction for
//! a lens that cited the patch's own line number instead of the file's.
//!
//! JavaScript semantics kept: lengths count UTF-16 units, `trim` and `\s` use
//! the ECMAScript whitespace set, line numbers are doubles (`parseInt` of the
//! hunk header, `Number(finding.line)`), and `endsWith` converts a non-string
//! argument with `String()`. A candidate whose `file` is not a string makes
//! the script throw inside its per-candidate `try`; the port returns the same
//! `error` result with V8's message.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{json, Map, Value};
use tribunal_core::evidence::js_ext::number_value;
use tribunal_core::js;
use tribunal_core::lens::js_ext::{js_key_order, to_number};

use crate::incremental_scope::read_file_utf8;
use crate::operator::json_parse;

const SENTINEL: &str = "No relevant quotes found";
const WINDOW: f64 = 10.0;
const SPECIFICITY_MIN_CHARS: usize = 8;
const GENERIC_QUOTES: [&str; 10] = [
    "}",
    "{",
    ");",
    ";",
    "return;",
    "return null;",
    "else {",
    "} else {",
    "});",
    "})",
];
const PATCH_LINE_NEAR: f64 = 12.0;
const NOT_IN_DIFF: &str = "not-in-diff not distinguished in v1";

/// Ports the script's own `diffIsUsable`: DIFF-USABLE without the churn check.
pub fn diff_is_usable(diff: &str) -> bool {
    tribunal_core::lens::diff_usable::diff_is_usable(Some(diff), None, None)
}

/// Ports `normalizeLine`: one leading `+`, `-` or space off, one trailing
/// `\r` off, trimmed, and every whitespace run collapsed to one space.
fn normalize_line(line: &str) -> String {
    let s = line.strip_prefix(['+', '-', ' ']).unwrap_or(line);
    let s = s.strip_suffix('\r').unwrap_or(s);
    let mut out = String::with_capacity(s.len());
    let mut in_space = false;
    for c in js::trim(s).chars() {
        if js::is_js_whitespace(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

fn positive(n: f64) -> Option<f64> {
    (n.is_finite() && n > 0.0).then_some(n)
}

/// `finding && finding.<key>`: only an object has the properties read here.
fn field<'a>(finding: &'a Value, key: &str) -> Option<&'a Value> {
    js::get(finding, key)
}

/// Ports `normalizePositiveInt(finding && finding.line)`. Despite the name, a
/// fractional line passes. A falsy `finding` converts to 0 or NaN, so reading
/// only objects gives the same answer.
fn claimed_line(finding: &Value) -> Option<f64> {
    positive(to_number(field(finding, "line")))
}

fn first_non_blank_line(text: &str) -> &str {
    text.split('\n')
        .map(js::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

fn is_low_specificity(normalized: &str) -> bool {
    js::utf16_len(normalized) < SPECIFICITY_MIN_CHARS || GENERIC_QUOTES.contains(&normalized)
}

#[derive(Debug, Clone)]
struct Record {
    new_line: Option<f64>,
    kind: &'static str,
    normalized: String,
    diff_line: f64,
}

/// `parseDiff`'s `Map`: files in first-seen order.
#[derive(Debug, Default)]
struct Files {
    order: Vec<(String, Vec<Record>)>,
    index: HashMap<String, usize>,
}

/// `/^diff --git a\/(.+?) b\/(.+)$/`, the second group. The first group is
/// lazy, so the first ` b/` that leaves both groups non-empty splits it, and
/// `.` refuses every line terminator.
fn git_header_path(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("diff --git a/")?;
    if rest.chars().any(js::is_line_terminator) {
        return None;
    }
    rest.match_indices(" b/")
        .map(|(i, _)| i)
        .find(|&i| i >= 1 && i + 3 < rest.len())
        .map(|i| &rest[i + 3..])
}

fn ascii_digits(s: &str) -> Option<(&str, &str)> {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    (n > 0).then(|| s.split_at(n))
}

fn skip_count(r: &str) -> Option<&str> {
    match r.strip_prefix(',') {
        Some(x) => ascii_digits(x).map(|(_, rest)| rest),
        None => Some(r),
    }
}

/// `/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/` as `parseInt` of both groups.
fn hunk_starts(line: &str) -> Option<(f64, f64)> {
    let (old, r) = ascii_digits(line.strip_prefix("@@ -")?)?;
    let r = skip_count(r)?;
    let (new, r) = ascii_digits(r.strip_prefix(" +")?)?;
    let r = skip_count(r)?;
    r.starts_with(" @@").then(|| {
        (
            old.parse().unwrap_or(f64::INFINITY),
            new.parse().unwrap_or(f64::INFINITY),
        )
    })
}

/// Ports `parseDiff`.
fn parse_diff(diff: &str) -> Files {
    let mut files = Files::default();
    let mut current: Option<usize> = None;
    let mut old_n: Option<f64> = None;
    let mut new_n: Option<f64> = None;
    for (i, raw) in diff.split('\n').enumerate() {
        let diff_line = (i + 1) as f64;
        if let Some(path) = git_header_path(raw) {
            let at = match files.index.get(path) {
                Some(&at) => at,
                None => {
                    files.index.insert(path.to_owned(), files.order.len());
                    files.order.push((path.to_owned(), Vec::new()));
                    files.order.len() - 1
                }
            };
            current = Some(at);
            old_n = None;
            new_n = None;
            continue;
        }
        let Some(at) = current else { continue };
        if let Some((o, n)) = hunk_starts(raw) {
            old_n = Some(o);
            new_n = Some(n);
            continue;
        }
        let (Some(o), Some(n)) = (old_n, new_n) else {
            continue;
        };
        let (kind, new_line) = if raw.starts_with('+') {
            new_n = Some(n + 1.0);
            ("added", Some(n))
        } else if raw.starts_with('-') {
            old_n = Some(o + 1.0);
            ("removed", None)
        } else if raw.starts_with(' ') || raw.is_empty() {
            new_n = Some(n + 1.0);
            old_n = Some(o + 1.0);
            ("context", Some(n))
        } else {
            continue;
        };
        files.order[at].1.push(Record {
            new_line,
            kind,
            normalized: normalize_line(raw),
            diff_line,
        });
    }
    files
}

/// `String(v)` as `endsWith` applies it to its argument. An object with an
/// own `toString` (never callable in JSON) has no primitive value and throws.
pub(crate) fn to_js_string(v: Option<&Value>) -> Result<String, String> {
    match v {
        None => Ok("undefined".into()),
        Some(Value::Null) => Ok("null".into()),
        Some(Value::Bool(b)) => Ok(b.to_string()),
        Some(Value::Number(n)) => Ok(js::number_to_string(n.as_f64().unwrap_or(f64::NAN))),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(Value::Array(a)) => a
            .iter()
            .map(|e| match e {
                Value::Null => Ok(String::new()),
                e => to_js_string(Some(e)),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|parts| parts.join(",")),
        Some(Value::Object(m)) if m.contains_key("toString") => {
            Err("Cannot convert object to primitive value".into())
        }
        Some(Value::Object(_)) => Ok("[object Object]".into()),
    }
}

/// Ports `lookupFile`: the exact path, else the first file where either path
/// ends with the other. `file` is `finding.file`, any JSON value or absent.
fn lookup_file<'a>(files: &'a Files, file: Option<&Value>) -> Result<Option<&'a [Record]>, String> {
    if let Some(Value::String(f)) = file {
        if let Some(&at) = files.index.get(f) {
            return Ok(Some(&files.order[at].1));
        }
    }
    for (key, records) in &files.order {
        if key.ends_with(to_js_string(file)?.as_str()) {
            return Ok(Some(records));
        }
        match file {
            Some(Value::String(f)) if f.ends_with(key.as_str()) => return Ok(Some(records)),
            Some(Value::String(_)) => {}
            None => return Err("Cannot read properties of undefined (reading 'endsWith')".into()),
            Some(Value::Null) => {
                return Err("Cannot read properties of null (reading 'endsWith')".into())
            }
            Some(_) => return Err("file.endsWith is not a function".into()),
        }
    }
    Ok(None)
}

/// One resolution, in the script's key order. `resolved_file: None` is
/// `undefined`, which `JSON.stringify` leaves out.
#[derive(Debug, Clone)]
struct Resolution {
    match_kind: &'static str,
    evidence_located: bool,
    claimed_line: Option<f64>,
    resolved_line: Option<f64>,
    resolved_file: Option<Value>,
    matched_line_kind: Option<&'static str>,
    ambiguous_count: usize,
    delta: Option<f64>,
    low_confidence: bool,
    note: Option<&'static str>,
}

fn opt_num(x: Option<f64>) -> Value {
    x.map_or(Value::Null, number_value)
}

impl Resolution {
    fn empty(match_kind: &'static str, note: Option<&'static str>) -> Self {
        Self {
            match_kind,
            evidence_located: matches!(match_kind, "exact" | "shifted" | "elsewhere"),
            claimed_line: None,
            resolved_line: None,
            resolved_file: Some(Value::Null),
            matched_line_kind: None,
            ambiguous_count: 0,
            delta: None,
            low_confidence: false,
            note,
        }
    }

    fn write(&self, out: &mut Map<String, Value>) {
        out.insert("matchKind".into(), self.match_kind.into());
        out.insert("evidenceLocated".into(), self.evidence_located.into());
        out.insert("claimedLine".into(), opt_num(self.claimed_line));
        out.insert("resolvedLine".into(), opt_num(self.resolved_line));
        if let Some(f) = &self.resolved_file {
            out.insert("resolvedFile".into(), f.clone());
        }
        out.insert("matchedLineKind".into(), self.matched_line_kind.into());
        out.insert("ambiguousCount".into(), self.ambiguous_count.into());
        out.insert("delta".into(), opt_num(self.delta));
        out.insert("lowConfidence".into(), self.low_confidence.into());
        out.insert("note".into(), self.note.into());
    }
}

/// Ports `buildMatch`.
fn build_match(
    kind: &'static str,
    m: &Record,
    claimed: Option<f64>,
    file: Option<&Value>,
    ambiguous_count: usize,
    low_confidence: bool,
) -> Resolution {
    Resolution {
        match_kind: kind,
        evidence_located: true,
        claimed_line: claimed,
        resolved_line: m.new_line,
        resolved_file: match m.new_line {
            Some(_) => file.map(js_key_order),
            None => Some(Value::Null),
        },
        matched_line_kind: Some(m.kind),
        ambiguous_count,
        delta: m.new_line.zip(claimed).map(|(n, c)| n - c),
        low_confidence,
        note: m
            .new_line
            .is_none()
            .then_some("matched a removed line — no new-file line to resolve to"),
    }
}

/// Ports `resolveEvidence`. `line` replaces `finding.line` when the batch
/// resolves at a corrected line.
fn resolve(files: &Files, finding: &Value, line: Option<f64>) -> Result<Resolution, String> {
    let Some(Value::String(raw)) = field(finding, "evidence") else {
        return Ok(Resolution::empty("skipped", None));
    };
    let trimmed = js::trim(raw);
    if trimmed.is_empty() || trimmed == SENTINEL {
        return Ok(Resolution::empty("skipped", None));
    }
    let multi = raw.contains('\n');
    let evidence_line = if multi {
        first_non_blank_line(raw)
    } else {
        raw.as_str()
    };
    let evidence = normalize_line(evidence_line);
    let claimed = match line {
        Some(l) => positive(l),
        None => claimed_line(finding),
    };
    let unlocated = |kind, note| Resolution {
        claimed_line: claimed,
        low_confidence: multi,
        ..Resolution::empty(kind, note)
    };
    if evidence.is_empty() || is_low_specificity(&evidence) {
        return Ok(unlocated("low-specificity", None));
    }
    let file = field(finding, "file");
    let records = match lookup_file(files, file)? {
        Some(r) if !r.is_empty() => r,
        _ => return Ok(unlocated("not-found", Some(NOT_IN_DIFF))),
    };
    let all: Vec<&Record> = records
        .iter()
        .filter(|r| r.normalized == evidence)
        .collect();
    if all.is_empty() {
        return Ok(unlocated("not-found", Some(NOT_IN_DIFF)));
    }
    let in_window: Vec<&Record> = match claimed {
        Some(c) => all
            .iter()
            .copied()
            .filter(|m| m.new_line.is_some_and(|n| (n - c).abs() <= WINDOW))
            .collect(),
        None => Vec::new(),
    };
    let pool = if in_window.is_empty() {
        &all
    } else {
        &in_window
    };
    if pool.len() > 1 {
        let at_claim: Vec<&&Record> = match claimed {
            Some(c) => pool.iter().filter(|m| m.new_line == Some(c)).collect(),
            None => Vec::new(),
        };
        if let [m] = at_claim.as_slice() {
            return Ok(build_match("exact", m, claimed, file, all.len(), multi));
        }
        return Ok(Resolution {
            evidence_located: true,
            ambiguous_count: all.len(),
            ..unlocated("ambiguous", None)
        });
    }
    let m = pool[0];
    let kind = if claimed.is_some() && m.new_line == claimed {
        "exact"
    } else if !in_window.is_empty() {
        "shifted"
    } else {
        "elsewhere"
    };
    Ok(build_match(kind, m, claimed, file, all.len(), multi))
}

/// Ports `patchLineCorrection`: `Some((correctedLine, patchLine))`.
fn patch_line_correction(files: &Files, finding: &Value) -> Result<Option<(f64, f64)>, String> {
    let Some(claimed) = claimed_line(finding) else {
        return Ok(None);
    };
    let Some(Value::String(evidence)) = field(finding, "evidence") else {
        return Ok(None);
    };
    let quote: HashSet<String> = evidence
        .split('\n')
        .map(normalize_line)
        .filter(|l| !l.is_empty() && !is_low_specificity(l))
        .collect();
    if quote.is_empty() {
        return Ok(None);
    }
    let Some(records) = lookup_file(files, field(finding, "file"))? else {
        return Ok(None);
    };
    let Some(at) = records.iter().find(|r| r.diff_line == claimed) else {
        return Ok(None);
    };
    let Some(at_new) = at.new_line.filter(|&n| n != claimed) else {
        return Ok(None);
    };
    let quoted: Vec<&Record> = records
        .iter()
        .filter(|r| quote.contains(&r.normalized))
        .collect();
    if quoted.iter().any(|r| r.new_line == Some(claimed)) {
        return Ok(None);
    }
    if quote.contains(&at.normalized) {
        return Ok(Some((at_new, claimed)));
    }
    let near_as_patch = quoted
        .iter()
        .any(|r| (r.diff_line - claimed).abs() <= PATCH_LINE_NEAR);
    let near_as_file = quoted.iter().any(|r| {
        r.new_line
            .is_some_and(|n| (n - claimed).abs() <= PATCH_LINE_NEAR)
    });
    if !near_as_patch || near_as_file {
        return Ok(None);
    }
    Ok(Some((at_new, claimed)))
}

fn with_id(c: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(id) = field(c, "id") {
        out.insert("id".into(), js_key_order(id));
    }
    out
}

/// Ports `resolveEvidenceBatch`.
///
/// # Errors
/// Where the script throws out of the batch: `findings` is not an array, or a
/// candidate is `null` (its `c.id` throws again inside the `catch`).
pub fn resolve_evidence_batch(diff: &str, findings: &Value) -> Result<Value, String> {
    if !diff_is_usable(diff) {
        return Ok(json!({"ran": false, "reason": "invalid-diff"}));
    }
    let Value::Array(list) = findings else {
        return Err("TypeError: findings.map is not a function".into());
    };
    let files = parse_diff(diff);
    let mut results = Vec::with_capacity(list.len());
    for c in list {
        if c.is_null() {
            return Err("TypeError: Cannot read properties of null (reading 'id')".into());
        }
        let mut out = with_id(c);
        let resolved = patch_line_correction(&files, c).and_then(|fix| match fix {
            None => resolve(&files, c, None).map(|r| (r, None)),
            Some((corrected, patch)) => {
                resolve(&files, c, Some(corrected)).map(|r| (r, Some((corrected, patch))))
            }
        });
        match resolved {
            Ok((r, fix)) => {
                r.write(&mut out);
                if let Some((corrected, patch)) = fix {
                    out.insert("correctedLine".into(), number_value(corrected));
                    out.insert("patchLine".into(), number_value(patch));
                }
            }
            Err(message) => {
                out.insert("matchKind".into(), "error".into());
                out.insert("error".into(), message.into());
            }
        }
        results.push(Value::Object(out));
    }
    Ok(json!({"ran": true, "results": results}))
}

/// The script's main path without `--relay`: `node resolve-evidence-line.mjs
/// <diff-path> <candidates-json-path|->`, with the candidates file (or stdin)
/// already read into `candidates_text`.
///
/// # Errors
/// As [`resolve_evidence_batch`].
pub fn run(diff_path: &Path, candidates_text: &str) -> Result<Value, String> {
    let Some(diff) = read_file_utf8(diff_path) else {
        return Ok(json!({"ran": false, "reason": "diff-unreadable"}));
    };
    let Ok(candidates) = json_parse(candidates_text) else {
        return Ok(json!({"ran": false, "reason": "candidates-unparseable"}));
    };
    resolve_evidence_batch(&diff, &candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizing_strips_one_marker_and_collapses_whitespace() {
        assert_eq!(normalize_line("+  a \u{FEFF}\t b\r"), "a b");
        assert_eq!(normalize_line("++x"), "+x");
        assert_eq!(normalize_line("\u{0085}x"), "\u{0085}x");
    }

    #[test]
    fn the_header_split_is_lazy() {
        assert_eq!(git_header_path("diff --git a/a b/c b/d"), Some("c b/d"));
        assert_eq!(git_header_path("diff --git a/ b/x b/y"), Some("y"));
        assert_eq!(git_header_path("diff --git a/x b/y\r"), None);
    }

    #[test]
    fn a_non_string_file_throws_like_v8() {
        let files = parse_diff("diff --git a/x.ts b/x.ts\n@@ -1 +1 @@\n+a");
        assert_eq!(
            lookup_file(&files, Some(&json!(5))).unwrap_err(),
            "file.endsWith is not a function"
        );
        assert!(lookup_file(&files, Some(&json!(["x.ts"])))
            .unwrap()
            .is_some());
        assert!(lookup_file(&Files::default(), None).unwrap().is_none());
    }
}
