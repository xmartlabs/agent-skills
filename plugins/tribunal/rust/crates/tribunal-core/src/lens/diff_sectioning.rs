//! Port of the DIFF-SECTIONING region: the per-file diff splitter, the files a
//! finding widens a refuter's scope to, and the added-line veto.
//!
//! The regexes here are matched by hand rather than with the `regex` crate:
//! JavaScript's `\w` and `\b` are ASCII-only without the `u` flag, `.` stops at
//! `\r`, U+2028 and U+2029, and the cited-file pattern relies on backtracking
//! order (longest prefix first, then the extensions in their listed order).

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};

use super::js_ext;
use crate::js;

pub const VERIFIED_FILES_CAP: usize = 4;

/// Ports `stripCodeFence`. The text is an agent's free-text return.
pub fn strip_code_fence(text: Option<&str>) -> String {
    let s = js::trim(text.unwrap_or(""));
    if !s.starts_with("```") {
        return s.to_owned();
    }
    // `s.slice(s.indexOf("\n") + 1)`: with no newline that is the whole string.
    let without_open = match s.find('\n') {
        Some(i) => &s[i + 1..],
        None => s,
    };
    let body = match without_open.rfind("```") {
        Some(i) => &without_open[..i],
        None => without_open,
    };
    js::trim(body).to_owned()
}

/// What `splitDiffByFile` returns: its `Map` as ordered pairs, keyed by the
/// post-image path in first-seen order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiffSections {
    pub sections: Vec<(String, String)>,
    pub unparsed: Vec<String>,
}

impl DiffSections {
    pub fn get(&self, path: &str) -> Option<&str> {
        self.sections
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, s)| s.as_str())
    }
}

/// `/^diff --git a\/(.+) b\/(.+)$/` without flags. `.` matches no line
/// terminator and `$` is the end of the line, so a `\r` anywhere refuses the
/// header. The first group is greedy: the last ` b/` that leaves both groups
/// non-empty splits it.
fn header_path(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("diff --git a/")?;
    if rest.chars().any(js::is_line_terminator) {
        return None;
    }
    rest.match_indices(" b/")
        .map(|(i, _)| i)
        .filter(|&i| i >= 1 && i + 3 < rest.len())
        .last()
        .map(|i| &rest[i + 3..])
}

/// Ports `splitDiffByFile`.
pub fn split_diff_by_file(diff: Option<&str>) -> DiffSections {
    let mut out = DiffSections::default();
    let Some(diff) = diff.filter(|d| !d.is_empty()) else {
        return out;
    };
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut path: Option<&str> = None;
    let mut buf: Vec<&str> = Vec::new();
    let mut flush = |path: Option<&str>, buf: &[&str], out: &mut DiffSections| {
        let Some(path) = path else { return };
        let text = buf.join("\n");
        match index.get(path) {
            Some(&i) => {
                let prev = &mut out.sections[i].1;
                if prev.is_empty() {
                    *prev = text;
                } else {
                    prev.push('\n');
                    prev.push_str(&text);
                }
            }
            None => {
                index.insert(path.to_owned(), out.sections.len());
                out.sections.push((path.to_owned(), text));
            }
        }
    };
    for line in diff.split('\n') {
        if line.starts_with("diff --git ") {
            flush(path, &buf, &mut out);
            buf = vec![line];
            path = header_path(line).filter(|_| !line.contains('"'));
            if path.is_none() {
                out.unparsed.push(js_ext::utf16_prefix_lossy(line, 120));
            }
        } else if path.is_some() {
            buf.push(line);
        }
    }
    flush(path, &buf, &mut out);
    out
}

fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-')
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

const SOURCE_EXTENSIONS: [&str; 9] = [
    "ts", "tsx", "js", "jsx", "mts", "cts", "prisma", "sql", "json",
];

/// The matches of `/[\w./-]+\.(ts|tsx|js|jsx|mts|cts|prisma|sql|json)\b/g`.
fn source_file_mentions(text: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let byte_at = |i: usize| chars.get(i).map_or(text.len(), |&(b, _)| b);
    // Where a match starting at `start` ends, if one does. Within a run of
    // path characters a later start only loses candidates, so a failed start
    // fails the rest of its run.
    let match_at = |start: usize, run_end: usize| -> Option<usize> {
        for k in (start + 1..run_end).rev() {
            if chars[k].1 != '.' {
                continue;
            }
            for ext in SOURCE_EXTENSIONS {
                let e = k + 1 + ext.len();
                if e > chars.len() || !chars[k + 1..e].iter().map(|&(_, c)| c).eq(ext.chars()) {
                    continue;
                }
                if chars.get(e).is_none_or(|&(_, c)| !is_word_char(c)) {
                    return Some(e);
                }
            }
        }
        None
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !is_path_char(chars[i].1) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && is_path_char(chars[j].1) {
            j += 1;
        }
        match match_at(i, j) {
            Some(e) => {
                out.push(&text[byte_at(i)..byte_at(e)]);
                i = e;
            }
            None => i = j,
        }
    }
    out
}

fn or_empty(v: Option<&Value>) -> String {
    if js::truthy(v) {
        js_ext::to_js_string(v)
    } else {
        String::new()
    }
}

/// Ports `citedFiles`. `finding` is a candidate, always an object; only paths
/// in `known_files` (the PR's files) are returned.
pub fn cited_files(finding: &Value, known_files: Option<&[String]>) -> Vec<String> {
    let text = format!(
        "{} {}",
        or_empty(js::get(finding, "description")),
        or_empty(js::get(finding, "evidence"))
    );
    let known = known_files.unwrap_or(&[]);
    let own = js::get(finding, "file");
    let mut out: Vec<String> = Vec::new();
    for h in source_file_mentions(&text) {
        let suffix = format!("/{h}");
        let Some(f) = known.iter().find(|f| *f == h || f.ends_with(&suffix)) else {
            continue;
        };
        if f.is_empty() || js_ext::strict_eq(Some(&Value::String(f.clone())), own) {
            continue;
        }
        if !out.contains(f) {
            out.push(f.clone());
        }
    }
    out
}

/// Ports `verifiedFiles`.
pub fn verified_files(finding: &Value) -> Vec<String> {
    let anchors = match js::get(finding, "verifiedBy") {
        Some(Value::Array(a)) => a.as_slice(),
        _ => &[],
    };
    let own = js::get(finding, "file");
    let mut out: Vec<String> = Vec::new();
    for a in anchors {
        let file = match (js::truthy(Some(a)), js::get(a, "file")) {
            (true, Some(Value::String(f))) => {
                let t = js::trim(f);
                t.strip_prefix("./").unwrap_or(t).to_owned()
            }
            _ => String::new(),
        };
        if file.is_empty()
            || js_ext::strict_eq(Some(&Value::String(file.clone())), own)
            || out.contains(&file)
        {
            continue;
        }
        if !file.chars().all(is_path_char) || file.starts_with('/') {
            continue;
        }
        if file.split('/').any(|s| s == "..") {
            continue;
        }
        out.push(file);
        if out.len() == VERIFIED_FILES_CAP {
            break;
        }
    }
    out
}

/// Ports `externalScopeLines`.
pub fn external_scope_lines(finding: &Value, known_files: Option<&[String]>) -> Vec<String> {
    let cited = cited_files(finding, known_files);
    let anchored: Vec<String> = verified_files(finding)
        .into_iter()
        .filter(|f| !cited.contains(f))
        .collect();
    let mut lines = Vec::new();
    if !cited.is_empty() {
        lines.push(format!(
            "The finding cites {} — those ARE in scope. Open them. A claim whose mechanism spans files cannot be settled from one of them.",
            cited.join(", ")
        ));
    }
    if !anchored.is_empty() {
        lines.push(format!(
            "The finding's evidence anchors point at {} — those ARE in scope too, including any this PR did not touch. Open them: they are the code the claim says it depends on.",
            anchored.join(", ")
        ));
    }
    lines.push("If the claim is that something is ABSENT (a function nobody calls, a column nobody reads or writes, an event nobody listens to), a search settles it and a file does not. You may run `git grep -n <symbol>` in your working directory for each symbol the claim names, up to 3 searches, and open what they return that bears on the claim. One real call site, read or write you can quote refutes a claim of absence.".to_owned());
    lines
}

fn ascii_digits(s: &str) -> Option<(&str, &str)> {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    (n > 0).then(|| s.split_at(n))
}

/// `/^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@/`, as `Number(hunk[1])`.
fn hunk_new_start(line: &str) -> Option<f64> {
    let (_, r) = ascii_digits(line.strip_prefix("@@ -")?)?;
    let r = match r.strip_prefix(',') {
        Some(x) => ascii_digits(x)?.1,
        None => r,
    };
    let (start, r) = ascii_digits(r.strip_prefix(" +")?)?;
    let r = match r.strip_prefix(',') {
        Some(x) => ascii_digits(x)?.1,
        None => r,
    };
    r.starts_with(" @@")
        .then(|| start.parse().unwrap_or(f64::INFINITY))
}

/// Ports `addedLines`: new-side line numbers of the lines a section adds, in
/// `Set` insertion order. Numbers are JavaScript doubles, so a line number
/// past 2^53 stops advancing exactly as it does there.
pub fn added_lines(section: Option<&str>) -> Vec<f64> {
    let mut out = Vec::new();
    let mut seen: HashSet<u64> = HashSet::new();
    let mut n: Option<f64> = None;
    for line in section.unwrap_or("").split('\n') {
        if let Some(start) = hunk_new_start(line) {
            n = Some(start);
            continue;
        }
        let Some(cur) = n else { continue };
        if line.starts_with('-') || line.starts_with('\\') {
            continue;
        }
        if line.starts_with('+') && seen.insert(cur.to_bits()) {
            out.push(cur);
        }
        n = Some(cur + 1.0);
    }
    out
}

/// Ports `addedRanges`: the added lines as `[start, end]` runs.
pub fn added_ranges(section: Option<&str>) -> Vec<[f64; 2]> {
    let mut lines = added_lines(section);
    lines.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut runs: Vec<[f64; 2]> = Vec::new();
    for n in lines {
        match runs.last_mut() {
            Some(last) if n == last[1] + 1.0 => last[1] = n,
            _ => runs.push([n, n]),
        }
    }
    runs
}

/// Ports `anyLocationAdded`. `sections` maps a file to its section-index
/// entry, whose `added` holds `[start, end]` runs; `finding` is a candidate.
///
/// Each run is destructured as `[a, b]`. section-diff.mjs writes them as
/// two-number arrays; a run that is not iterable at all would throw in
/// JavaScript and is treated here as matching nothing.
pub fn any_location_added(sections: &Map<String, Value>, finding: &Value) -> bool {
    let r = js::get(finding, "evidenceResolution");
    if let Some(r) = r.filter(|r| js::truthy(Some(r))) {
        let resolved = js::get(r, "resolvedFile");
        // `!(r.ambiguousCount > 1)`: an absent or non-numeric count is NaN,
        // which is not greater than 1.
        let ambiguous = js_ext::to_number(js::get(r, "ambiguousCount"));
        if js::truthy(js::get(r, "evidenceLocated"))
            && js::get(r, "matchedLineKind") == Some(&Value::String("added".into()))
            && (!js::truthy(resolved) || js_ext::strict_eq(resolved, js::get(finding, "file")))
            && (ambiguous.is_nan() || ambiguous <= 1.0)
        {
            return true;
        }
    }
    let merged = match js::get(finding, "mergedLocations") {
        Some(Value::Array(m)) => m.as_slice(),
        _ => &[],
    };
    std::iter::once(finding).chain(merged).any(|loc| {
        if !js::truthy(Some(loc)) {
            return false;
        }
        let section = match js::get(loc, "file") {
            Some(Value::String(f)) => sections.get(f),
            _ => None,
        };
        let Some(section) = section.filter(|s| js::truthy(Some(s))) else {
            return false;
        };
        let line = js_ext::to_number(js::get(loc, "line"));
        let Some(Value::Array(added)) = js::get(section, "added") else {
            return false;
        };
        line.is_finite()
            && added.iter().any(|run| {
                let (a, b) = destructure_pair(run);
                line >= js_ext::to_number(a.as_ref()) && line <= js_ext::to_number(b.as_ref())
            })
    })
}

/// `const [a, b] = run` for an array or a string (iterated by code point).
fn destructure_pair(run: &Value) -> (Option<Value>, Option<Value>) {
    match run {
        Value::Array(v) => (v.first().cloned(), v.get(1).cloned()),
        Value::String(s) => {
            let mut it = s.chars().map(|c| Value::String(c.to_string()));
            (it.next(), it.next())
        }
        _ => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_crlf_header_is_refused() {
        let d = split_diff_by_file(Some("diff --git a/x.ts b/x.ts\r\n+1"));
        assert!(d.sections.is_empty());
        assert_eq!(d.unparsed, ["diff --git a/x.ts b/x.ts\r"]);
    }

    #[test]
    fn the_last_b_side_marker_wins() {
        let d = split_diff_by_file(Some("diff --git a/a b/c b/d\n+1"));
        assert_eq!(d.get("d"), Some("diff --git a/a b/c b/d\n+1"));
    }

    #[test]
    fn mentions_follow_backtracking_order() {
        assert_eq!(
            source_file_mentions("a.tsx.b x.ts.bak y.json"),
            ["a.tsx", "x.ts", "y.json"]
        );
        assert_eq!(source_file_mentions("a.tsé"), ["a.ts"]);
        assert!(source_file_mentions("a.tsx_").is_empty());
    }

    #[test]
    fn hunk_lines_and_runs() {
        let s = "@@ -1,3 +1,4 @@\n a\n-b\n+c\n+d\n e\n@@ -10,2 +11,3 @@\n x\n+++y\n+z";
        assert_eq!(added_lines(Some(s)), [2.0, 3.0, 12.0, 13.0]);
        assert_eq!(added_ranges(Some(s)), [[2.0, 3.0], [12.0, 13.0]]);
    }

    #[test]
    fn a_resolved_quote_on_an_added_line_vetoes() {
        let sections: Map<String, Value> = json!({"src/a.ts": {"added": [[2, 2]]}})
            .as_object()
            .unwrap()
            .clone();
        let f = json!({"file": "src/a.ts", "line": 3, "evidenceResolution":
            {"evidenceLocated": true, "matchedLineKind": "added", "ambiguousCount": "1"}});
        assert!(any_location_added(&sections, &f));
    }
}
