//! Port of `skills/review-pr/scripts/incremental-scope.mjs`: given the PR's
//! diff at a previously reviewed commit and at the current head, the files a
//! re-review has to read. A caller that cannot narrow safely gets
//! `usable: false` and reviews everything.
//!
//! The script sorts paths with `String.prototype.localeCompare`, which is ICU
//! collation, not code-unit order: `a_b` sorts before `a-b`, `a` before `A`,
//! and both before `B`. [`locale_compare`] reproduces it for ASCII.

use std::cmp::Ordering;
use std::path::Path;

use serde_json::{Map, Value};
use tribunal_core::js::is_line_terminator;

/// What `splitDiffByFile` returns: its `Map` as ordered pairs, keyed by the
/// post-image path in first-seen order, and the headers it could not parse.
///
/// `tribunal_core::lens::diff_sectioning::split_diff_by_file` is the
/// workflow's version of the same splitter, and it is not reused here: it
/// also refuses a header holding `"`, which this script accepts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Split {
    pub sections: Vec<(String, String)>,
    pub unparsed: Vec<String>,
}

impl Split {
    fn get(&self, path: &str) -> Option<&str> {
        self.sections
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, s)| s.as_str())
    }
}

/// `/^diff --git a\/(.+) b\/(.+)$/`: `.` stops at `\r`, U+2028 and U+2029,
/// and the greedy first group splits at the last ` b/` that leaves the second
/// group non-empty.
fn header_path(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("diff --git a/")?;
    if rest.chars().any(is_line_terminator) {
        return None;
    }
    rest.match_indices(" b/")
        .map(|(i, _)| i)
        .filter(|&i| i >= 1 && i + 3 < rest.len())
        .last()
        .map(|i| &rest[i + 3..])
}

/// Ports `splitDiffByFile`.
pub fn split_diff_by_file(diff: &str) -> Split {
    let mut out = Split::default();
    if diff.is_empty() {
        return out;
    }
    let mut path: Option<&str> = None;
    let mut buf: Vec<&str> = Vec::new();
    let flush = |path: Option<&str>, buf: &[&str], out: &mut Split| {
        let Some(path) = path else { return };
        let text = buf.join("\n");
        match out.sections.iter_mut().find(|(p, _)| p == path) {
            Some((_, prev)) if !prev.is_empty() => {
                prev.push('\n');
                prev.push_str(&text);
            }
            Some((_, prev)) => *prev = text,
            None => out.sections.push((path.to_owned(), text)),
        }
    };
    for line in diff.split('\n') {
        if line.starts_with("diff --git ") {
            flush(path, &buf, &mut out);
            buf = vec![line];
            path = header_path(line);
            if path.is_none() {
                out.unparsed.push(line.to_owned());
            }
        } else if path.is_some() {
            buf.push(line);
        }
    }
    flush(path, &buf, &mut out);
    out
}

/// ASCII in ICU root collation order (Node's default `en-US`): whitespace,
/// punctuation and symbols, digits, then letters with case folded together.
/// The other ASCII controls are ignorable.
const PRIMARY_ORDER: &str =
    "\t\n\u{b}\u{c}\r _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$0123456789abcdefghijklmnopqrstuvwxyz";

/// A character's (primary, tertiary) weight, `None` when ignorable. A non-ASCII
/// character sorts after all of ASCII by code point, which is not what ICU
/// does for accented Latin letters (a declared difference).
fn collation_element(c: char) -> Option<(u32, u8)> {
    if !c.is_ascii() {
        return Some((0x100 + u32::from(c), 0));
    }
    let lower = c.to_ascii_lowercase();
    PRIMARY_ORDER
        .find(lower)
        .map(|i| (i as u32, u8::from(c.is_ascii_uppercase())))
}

/// `a.localeCompare(b)` at tertiary strength: all primary weights first, then
/// case (lower before upper), ignorable characters skipped.
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    let ka: Vec<(u32, u8)> = a.chars().filter_map(collation_element).collect();
    let kb: Vec<(u32, u8)> = b.chars().filter_map(collation_element).collect();
    ka.iter()
        .map(|k| k.0)
        .cmp(kb.iter().map(|k| k.0))
        .then_with(|| ka.iter().map(|k| k.1).cmp(kb.iter().map(|k| k.1)))
}

/// What the script prints.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    pub usable: bool,
    pub reason: Option<String>,
    pub changed: Vec<String>,
    pub added: Vec<String>,
    pub unchanged: Vec<String>,
    pub gone: Vec<String>,
    pub in_scope: Vec<String>,
    /// `None` only for the usage error, which the script prints without it.
    pub in_scope_churn: Option<usize>,
}

impl Scope {
    fn unusable(reason: String) -> Self {
        Self {
            reason: Some(reason),
            in_scope_churn: Some(0),
            ..Self::default()
        }
    }

    /// The script's JSON line, keys in its order: a usable scope puts
    /// `inScopeChurn` third, an unusable one last.
    pub fn to_json(&self) -> String {
        let list = |v: &[String]| Value::from(v.to_vec());
        let mut m = Map::new();
        m.insert("usable".into(), self.usable.into());
        m.insert("reason".into(), self.reason.clone().into());
        if self.usable {
            m.insert(
                "inScopeChurn".into(),
                self.in_scope_churn.unwrap_or(0).into(),
            );
        }
        m.insert("changed".into(), list(&self.changed));
        m.insert("added".into(), list(&self.added));
        m.insert("unchanged".into(), list(&self.unchanged));
        m.insert("gone".into(), list(&self.gone));
        m.insert("inScope".into(), list(&self.in_scope));
        if let (false, Some(n)) = (self.usable, self.in_scope_churn) {
            m.insert("inScopeChurn".into(), n.into());
        }
        Value::Object(m).to_string()
    }
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort_by(|x, y| locale_compare(x, y));
    v
}

/// Changed lines of a section: `+` or `-`, but not a `+++` or `---` header.
fn churn(body: &str) -> usize {
    body.split('\n')
        .filter(|l| {
            (l.starts_with('+') || l.starts_with('-'))
                && !(l.starts_with("+++") || l.starts_with("---"))
        })
        .count()
}

/// Ports `scopeIncremental`.
pub fn scope_incremental(baseline: &str, current: &str) -> Scope {
    if baseline.is_empty() || current.is_empty() {
        return Scope::unusable("a diff was empty".into());
    }
    let prev = split_diff_by_file(baseline);
    let curr = split_diff_by_file(current);
    if !prev.unparsed.is_empty() || !curr.unparsed.is_empty() {
        return Scope::unusable(format!(
            "unparsed diff header(s): {} baseline, {} current",
            prev.unparsed.len(),
            curr.unparsed.len()
        ));
    }
    if prev.sections.is_empty() || curr.sections.is_empty() {
        return Scope::unusable("a diff produced no sections".into());
    }
    let (mut changed, mut added, mut unchanged) = (Vec::new(), Vec::new(), Vec::new());
    for (path, text) in &curr.sections {
        match prev.get(path) {
            None => added.push(path.clone()),
            Some(old) if old != text => changed.push(path.clone()),
            Some(_) => unchanged.push(path.clone()),
        }
    }
    let gone: Vec<String> = prev
        .sections
        .iter()
        .filter(|(p, _)| curr.get(p).is_none())
        .map(|(p, _)| p.clone())
        .collect();
    // Sorted from `[...changed, ...added]` before either is sorted: the sort
    // is stable, so paths that collate equal keep this order.
    let in_scope = sorted(changed.iter().chain(&added).cloned().collect());
    let in_scope_churn = in_scope
        .iter()
        .map(|p| churn(curr.get(p).unwrap_or("")))
        .sum();
    Scope {
        usable: true,
        reason: None,
        changed: sorted(changed),
        added: sorted(added),
        unchanged: sorted(unchanged),
        gone: sorted(gone),
        in_scope,
        in_scope_churn: Some(in_scope_churn),
    }
}

/// V8's longest string in UTF-16 units. Node's `readFileSync(p, "utf8")`
/// throws `ERR_STRING_TOO_LONG` for a file of this many bytes or more, even
/// when its text would decode to fewer units.
pub const NODE_STRING_LIMIT: usize = 0x1fff_ffe8;

/// `readFileSync(path, "utf8")`: `None` where it throws, which includes a
/// file too large for a V8 string.
pub fn read_file_utf8(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    (bytes.len() < NODE_STRING_LIMIT).then(|| match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}

/// A patch as the script reads it, or empty when it cannot be read.
fn read_patch(path: &str) -> String {
    read_file_utf8(Path::new(path)).unwrap_or_default()
}

/// The script's main path. `None` or an empty string is a missing argument,
/// which yields the usage error; an unreadable file reads as empty.
pub fn scope_files(baseline: Option<&str>, current: Option<&str>) -> Scope {
    match (
        baseline.filter(|p| !p.is_empty()),
        current.filter(|p| !p.is_empty()),
    ) {
        (Some(b), Some(c)) => scope_incremental(&read_patch(b), &read_patch(c)),
        _ => Scope {
            reason: Some("usage: incremental-scope.mjs <baseline.patch> <current.patch>".into()),
            ..Scope::default()
        },
    }
}
