//! Port of `lib/blast-radius.mjs`: the unchanged code that depends on what
//! the PR changed, two hops out (changed declaration, a declaration that uses
//! it, call sites of that one in files outside the PR). Its output feeds
//! DEPENDENTS-BLOCK (`tribunal_core::lens::dependents_block`).
//!
//! The script has two resolvers. With the reviewed repo's `typescript`
//! package it binds names through the TypeScript AST and adds attention facts;
//! without it, it scans lines by word. Production always finds `typescript`,
//! so [`run`] takes the binding resolver, parsing with oxc
//! ([`crate::ast_bindings`], [`crate::attention_facts`]); [`run_with`] with
//! [`Resolver::Words`] is what the script prints when it finds none.
//!
//! Git runs through [`Tools`], in the same order and with the same arguments
//! as the script's `execFileSync` calls.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{json, Value};
use tribunal_core::js;

use crate::ast_bindings::Ref;
use crate::attention_facts;
use crate::exec::Tools;
use crate::import_adjacency::{
    import_specifiers, posix_dirname, posix_join, posix_normalize, source_ext,
};
use crate::typescript_fallback::TypeScriptFallback;

/// How many callers the output lists.
pub const MAX_CALLERS: usize = 25;
const TEXT_CHARS: usize = 100;
/// `execFileSync`'s `maxBuffer`: a larger answer is a failed call.
const MAX_BUFFER: usize = 64 << 20;
const GREP_GLOBS: [&str; 6] = ["*.ts", "*.tsx", "*.js", "*.jsx", "*.mjs", "*.cjs"];

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_ident(b: u8) -> bool {
    is_word(b) || b == b'$'
}

fn is_source(p: &str) -> bool {
    source_ext(p).is_some()
}

fn no_ext(p: &str) -> &str {
    source_ext(p).map_or(p, |e| &p[..p.len() - e.len()])
}

/// A segment before the last one named one of `names`.
fn has_dir_segment(p: &str, names: &[&str]) -> bool {
    let segs: Vec<&str> = p.split('/').collect();
    segs[..segs.len() - 1].iter().any(|s| names.contains(s))
}

/// TEST_RE: a `__tests__`, `__mocks__`, `test` or `tests` directory, or a
/// `.test.`/`.spec.` file with something after it.
pub fn is_test(p: &str) -> bool {
    if has_dir_segment(p, &["__tests__", "__mocks__", "test", "tests"]) {
        return true;
    }
    let last = p.rsplit('/').next().unwrap_or(p);
    [".test.", ".spec."]
        .iter()
        .any(|m| last.match_indices(m).any(|(i, _)| i + m.len() < last.len()))
}

/// HANDLER_PATH: where a request or a job breaks.
fn on_handler_path(p: &str) -> bool {
    has_dir_segment(
        p,
        &[
            "controller",
            "controllers",
            "route",
            "routes",
            "job",
            "jobs",
            "worker",
            "workers",
            "webhook",
            "webhooks",
            "middleware",
            "middlewares",
            "_middleware",
        ],
    )
}

fn skip_ws(s: &str, mut i: usize) -> usize {
    while let Some(c) = s[i..].chars().next() {
        if !js::is_js_whitespace(c) {
            break;
        }
        i += c.len_utf8();
    }
    i
}

fn trim_start(s: &str) -> &str {
    s.trim_start_matches(js::is_js_whitespace)
}

fn trim_end(s: &str) -> &str {
    s.trim_end_matches(js::is_js_whitespace)
}

/// `[A-Za-z_$][\w$]*` at `i`: the index after it.
fn ident_end(s: &str, i: usize) -> Option<usize> {
    let b = s.as_bytes();
    match b.get(i) {
        Some(&c) if c.is_ascii_alphabetic() || c == b'_' || c == b'$' => {}
        _ => return None,
    }
    let mut j = i + 1;
    while j < b.len() && is_ident(b[j]) {
        j += 1;
    }
    Some(j)
}

/// `kw\s+` at `i`: the index after the whitespace.
fn keyword_ws(s: &str, i: usize, kw: &str) -> Option<usize> {
    if !s[i..].starts_with(kw) {
        return None;
    }
    let j = skip_ws(s, i + kw.len());
    (j > i + kw.len()).then_some(j)
}

/// `(^|[^\w$])name([^\w$]|$)`: `name` as a whole identifier.
pub fn has_word(s: &str, name: &str) -> bool {
    let b = s.as_bytes();
    s.match_indices(name).any(|(i, _)| {
        let end = i + name.len();
        (i == 0 || !is_ident(b[i - 1])) && (end == b.len() || !is_ident(b[end]))
    })
}

/// `\bw\b` with JavaScript's `\w` (no `$`).
fn has_bounded(s: &str, w: &str) -> bool {
    let b = s.as_bytes();
    s.match_indices(w).any(|(i, _)| {
        let end = i + w.len();
        (i == 0 || !is_word(b[i - 1])) && (end == b.len() || !is_word(b[end]))
    })
}

/// `(^|[^\w$.])name\s*(\?\.)?\(`: a call of `name`.
fn is_call_of(s: &str, name: &str) -> bool {
    let b = s.as_bytes();
    s.match_indices(name).any(|(i, _)| {
        if i > 0 && (is_ident(b[i - 1]) || b[i - 1] == b'.') {
            return false;
        }
        let mut j = skip_ws(s, i + name.len());
        if s[j..].starts_with("?.") {
            j += 2;
        }
        b.get(j) == Some(&b'(')
    })
}

/// COMMENT_LINE: `//`, `/*` or `*` after leading whitespace.
fn is_comment_line(l: &str) -> bool {
    let t = trim_start(l);
    t.starts_with("//") || t.starts_with("/*") || t.starts_with('*')
}

/// `String.prototype.slice(0, n)` over UTF-16 units. A cut through a
/// surrogate pair keeps the lead surrogate in JavaScript; a Rust string cannot
/// hold it, so the pair is dropped (a declared difference).
#[must_use]
pub fn utf16_prefix(s: &str, n: usize) -> &str {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        units += c.len_utf16();
        if units > n {
            return &s[..i];
        }
    }
    s
}

/// `String.prototype.slice(n)` over UTF-16 units.
fn utf16_suffix(s: &str, n: usize) -> &str {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        if units >= n {
            return &s[i..];
        }
        units += c.len_utf16();
    }
    ""
}

// ---------------------------------------------------------------------------
// localeCompare

/// The ICU root collation's primary weight of an ASCII character, as V8's
/// `localeCompare` ranks it (en-US, tertiary strength): whitespace, then
/// punctuation and symbols, then digits, then letters with case folded.
/// Control characters are ignorable (`None`). A non-ASCII character is ranked
/// after every letter by code point, which ICU does not do (declared).
fn primary(c: char) -> Option<u32> {
    const ORDER: &[u8] = b"\t\n\x0b\x0c\r _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$0123456789";
    if !c.is_ascii() {
        return Some(1000 + c as u32);
    }
    let b = c as u8;
    if let Some(p) = ORDER.iter().position(|&x| x == b) {
        return Some(p as u32);
    }
    b.is_ascii_alphabetic()
        .then(|| 100 + u32::from(b.to_ascii_lowercase() - b'a'))
}

/// `a.localeCompare(b)` for the paths the caller sort compares: primary
/// weights first, then case (lower before upper).
pub fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    let keys = |s: &str| -> (Vec<u32>, Vec<u8>) {
        s.chars()
            .filter_map(|c| primary(c).map(|p| (p, u8::from(c.is_ascii_uppercase()))))
            .unzip()
    };
    let (pa, ta) = keys(a);
    let (pb, tb) = keys(b);
    pa.cmp(&pb).then_with(|| ta.cmp(&tb))
}

// ---------------------------------------------------------------------------
// The diff

/// `changedLines(diff)`: each file the diff writes to, in first-seen order,
/// with the new-side lines it adds or deletes at. Line numbers are doubles, as
/// `Number(h[1])` and `line++` make them.
pub fn changed_lines(diff: &str) -> Vec<(String, Vec<f64>)> {
    let mut out: Vec<(String, Vec<f64>)> = Vec::new();
    let mut file: Option<usize> = None;
    let mut line = 0f64;
    for raw in diff.split('\n') {
        if raw.starts_with("diff --git ") {
            file = None;
            continue;
        }
        if let Some(rest) = raw.strip_prefix("+++ ") {
            let p = js::trim(rest);
            let name = p.strip_prefix("b/").unwrap_or(p);
            // `/dev/null` and an empty name are both a falsy `file`.
            file = if p == "/dev/null" || name.is_empty() {
                None
            } else {
                Some(match out.iter().position(|(f, _)| f == name) {
                    Some(i) => i,
                    None => {
                        out.push((name.to_owned(), Vec::new()));
                        out.len() - 1
                    }
                })
            };
            continue;
        }
        if let Some(start) = hunk_new_start(raw) {
            line = js::string_to_number(start);
            continue;
        }
        let Some(f) = file else { continue };
        if raw.starts_with("--- ") {
            continue;
        }
        if raw.starts_with('+') {
            out[f].1.push(line);
            line += 1.0;
        } else if raw.starts_with('-') {
            // A pure deletion still changes the declaration around it.
            out[f].1.push(line);
        } else if raw.starts_with(' ') {
            line += 1.0;
        }
    }
    out
}

fn digits_end(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    while j < b.len() && b[j].is_ascii_digit() {
        j += 1;
    }
    (j > i).then_some(j)
}

fn opt_count(b: &[u8], i: usize) -> usize {
    if b.get(i) == Some(&b',') {
        if let Some(j) = digits_end(b, i + 1) {
            return j;
        }
    }
    i
}

/// `^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@`: the new-side start digits.
fn hunk_new_start(raw: &str) -> Option<&str> {
    let b = raw.as_bytes();
    if !raw.starts_with("@@ -") {
        return None;
    }
    let i = opt_count(b, digits_end(b, 4)?);
    if !raw[i..].starts_with(" +") {
        return None;
    }
    let s = i + 2;
    let e = digits_end(b, s)?;
    raw[opt_count(b, e)..]
        .starts_with(" @@")
        .then(|| &raw[s..e])
}

// ---------------------------------------------------------------------------
// Declarations

/// One top-level declaration found by the column-0 heuristic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decl {
    pub name: String,
    /// `function`, `function*`, `const`, ..., or `cjs` for `exports.x =`.
    pub kind: String,
    pub exported: bool,
    /// First line, including the comment block right above it (1-based).
    pub start: usize,
    /// The declaration's own line.
    pub at: usize,
    pub end: usize,
    pub body: String,
    /// The name importers use: differs only for `module.exports = { a: b }`.
    pub export_as: String,
}

impl Decl {
    fn is_type(&self) -> bool {
        self.kind == "type" || self.kind == "interface"
    }
}

/// DECL_RE: `(exported, kind, name)`.
fn match_decl(l: &str) -> Option<(bool, String, String)> {
    let mut i = 0;
    let exported = match keyword_ws(l, i, "export") {
        Some(j) => {
            i = j;
            true
        }
        None => false,
    };
    for kw in ["default", "declare", "async"] {
        if let Some(j) = keyword_ws(l, i, kw) {
            i = j;
        }
    }
    let kinds = [
        "function*",
        "function",
        "const",
        "let",
        "var",
        "class",
        "enum",
        "type",
        "interface",
    ];
    for kind in kinds {
        let Some(j) = keyword_ws(l, i, kind) else {
            continue;
        };
        let end = ident_end(l, j)?;
        return Some((exported, kind.to_owned(), l[j..end].to_owned()));
    }
    None
}

/// CJS_DECL_RE: `exports.name =` or `module.exports.name =`.
fn match_cjs_decl(l: &str) -> Option<String> {
    let i = if l.starts_with("module.exports.") {
        15
    } else if l.starts_with("exports.") {
        8
    } else {
        return None;
    };
    let end = ident_end(l, i)?;
    let j = skip_ws(l, end);
    (l.as_bytes().get(j) == Some(&b'=')).then(|| l[i..end].to_owned())
}

/// CJS_OBJECT_START: `module.exports = {`, returning what follows the brace.
fn cjs_object_rest(l: &str) -> Option<&str> {
    let rest = l.strip_prefix("module.exports")?;
    let i = skip_ws(rest, 0);
    let rest = rest[i..].strip_prefix('=')?;
    let i = skip_ws(rest, 0);
    rest[i..].strip_prefix('{')
}

/// `/\}\s*;?\s*$/`: where the closing brace starts, if the text ends with one.
fn closing_brace(text: &str) -> Option<usize> {
    let t = trim_end(text);
    let t = trim_end(t.strip_suffix(';').unwrap_or(t));
    t.ends_with('}').then(|| t.len() - 1)
}

/// `.replace(/\/\/[^\n]*/g, "")`.
fn strip_line_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("//") {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        rest = rest.find('\n').map_or("", |n| &rest[n..]);
    }
    out.push_str(rest);
    out
}

/// CJS_ENTRY on a trimmed entry: `(exported, local)`.
fn cjs_entry(e: &str) -> Option<(String, String)> {
    let a = ident_end(e, 0)?;
    let j = skip_ws(e, a);
    if j == e.len() {
        return Some((e[..a].to_owned(), e[..a].to_owned()));
    }
    if e.as_bytes()[j] != b':' {
        return None;
    }
    let k = skip_ws(e, j + 1);
    let b = ident_end(e, k)?;
    (b == e.len()).then(|| (e[..a].to_owned(), e[k..b].to_owned()))
}

/// `cjsExportNames(lines)`: local name to the name an importer uses, from
/// every `module.exports = {...}`.
pub fn cjs_export_names(lines: &[&str]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(first) = cjs_object_rest(lines[i]) else {
            i += 1;
            continue;
        };
        let mut text = first.to_owned();
        let mut j = i;
        while !trim_start(lines[j]).starts_with('}')
            && closing_brace(&text).is_none()
            && j + 1 < lines.len()
            && j - i < 500
        {
            j += 1;
            text.push('\n');
            text.push_str(lines[j]);
        }
        if let Some(b) = closing_brace(&text) {
            text.truncate(b);
        }
        for entry in strip_line_comments(&text).split(',') {
            if let Some((exported, local)) = cjs_entry(js::trim(entry)) {
                out.insert(local, exported);
            }
        }
        i = j + 1;
    }
    out
}

/// `declarations(content)`: every top-level declaration, each owning the
/// comment block right above it and running to the next one's start.
pub fn declarations(content: &str) -> Vec<Decl> {
    let lines: Vec<&str> = content.split('\n').collect();
    let mut decls: Vec<Decl> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let (exported, kind, name) = match match_decl(l) {
            Some(m) => m,
            None => match match_cjs_decl(l) {
                Some(name) => (true, "cjs".to_owned(), name),
                None => continue,
            },
        };
        let mut lead = i;
        while lead > 0 && is_comment_line(lines[lead - 1]) {
            lead -= 1;
        }
        decls.push(Decl {
            name,
            kind,
            exported,
            start: lead + 1,
            at: i + 1,
            end: 0,
            body: String::new(),
            export_as: String::new(),
        });
    }
    let cjs = cjs_export_names(&lines);
    let starts: Vec<usize> = decls.iter().map(|d| d.start).collect();
    for (i, d) in decls.iter_mut().enumerate() {
        d.end = starts.get(i + 1).map_or(lines.len(), |s| s - 1);
        d.body = lines[(d.at - 1).min(lines.len())..d.end.max(d.at - 1)].join("\n");
        if cjs.contains_key(&d.name) && !d.is_type() {
            d.exported = true;
        }
        d.export_as = cjs.get(&d.name).cloned().unwrap_or_else(|| d.name.clone());
    }
    decls
}

/// `\brequire\(\s*['"]`.
fn requires_literal(l: &str) -> bool {
    let b = l.as_bytes();
    l.match_indices("require(").any(|(i, _)| {
        (i == 0 || !is_word(b[i - 1])) && matches!(b.get(skip_ws(l, i + 8)), Some(b'\'' | b'"'))
    })
}

/// `^\s*(import\b|export\s+(\*|\{))`.
fn starts_import(l: &str) -> bool {
    let t = trim_start(l);
    if let Some(rest) = t.strip_prefix("import") {
        return rest.bytes().next().is_none_or(|c| !is_word(c));
    }
    match keyword_ws(t, 0, "export") {
        Some(j) => matches!(t.as_bytes().get(j), Some(b'*' | b'{')),
        None => false,
    }
}

/// `\bfrom\s*['"]|^\s*import\s*['"]|require\(`.
fn ends_import(l: &str) -> bool {
    let b = l.as_bytes();
    let from = l.match_indices("from").any(|(i, _)| {
        (i == 0 || !is_word(b[i - 1])) && matches!(b.get(skip_ws(l, i + 4)), Some(b'\'' | b'"'))
    });
    let t = trim_start(l);
    let import = t
        .strip_prefix("import")
        .is_some_and(|r| matches!(r.as_bytes().get(skip_ws(r, 0)), Some(b'\'' | b'"')));
    from || import || l.contains("require(")
}

/// `importLines(lines)`: the 1-based lines that belong to an import, a
/// require or a re-export, which can span lines.
fn import_lines(lines: &[&str]) -> HashSet<usize> {
    let mut out = HashSet::new();
    let mut i = 0;
    while i < lines.len() {
        if requires_literal(lines[i]) {
            out.insert(i + 1);
            let t = trim_start(lines[i]);
            let closes = t.strip_prefix('}').is_some_and(|r| {
                let r = trim_start(r);
                r.strip_prefix('=')
                    .is_some_and(|r| trim_start(r).starts_with("require("))
            });
            if closes {
                let mut k = i;
                while k > 0 && i - (k - 1) <= 200 {
                    k -= 1;
                    out.insert(k + 1);
                    if opens_destructure(lines[k]) {
                        break;
                    }
                }
            }
            i += 1;
            continue;
        }
        if !starts_import(lines[i]) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < lines.len() && !ends_import(lines[j]) && j - i < 200 {
            j += 1;
        }
        if j >= lines.len() {
            j = i;
        }
        for k in i..=j {
            out.insert(k + 1);
        }
        i = j + 1;
    }
    out
}

/// `^\s*(const|let|var)\s*\{`.
fn opens_destructure(l: &str) -> bool {
    let t = trim_start(l);
    ["const", "let", "var"].iter().any(|kw| {
        t.strip_prefix(kw)
            .is_some_and(|r| trim_start(r).starts_with('{'))
    })
}

// ---------------------------------------------------------------------------
// tsconfig aliases and specifiers

/// `stripJsonc`: comments removed outside strings, then trailing commas.
pub fn strip_jsonc(text: &str) -> String {
    let c: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] == '"' {
            let mut j = i + 1;
            while j < c.len() && c[j] != '"' {
                j += if c[j] == '\\' { 2 } else { 1 };
            }
            out.extend(&c[i..(j + 1).min(c.len())]);
            i = j;
        } else if c[i] == '/' && c.get(i + 1) == Some(&'/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
            out.push('\n');
        } else if c[i] == '/' && c.get(i + 1) == Some(&'*') {
            let end = (i + 2..c.len().saturating_sub(1)).find(|&k| c[k] == '*' && c[k + 1] == '/');
            i = end.map_or(c.len(), |e| e + 1);
        } else {
            out.push(c[i]);
        }
        i += 1;
    }
    strip_trailing_commas(&out)
}

/// `.replace(/,(\s*[}\]])/g, "$1")`.
fn strip_trailing_commas(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s.as_bytes()[i] == b',' {
            let j = skip_ws(s, i + 1);
            if matches!(s.as_bytes().get(j), Some(b'}' | b']')) {
                out.push_str(&s[i + 1..=j]);
                i = j + 1;
                continue;
            }
        }
        let c = s[i..].chars().next().expect("char boundary");
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// `readAliases(tsconfigText)`: `compilerOptions.paths` keys to their first
/// target joined to `baseUrl`, in the key order JavaScript keeps. Anything
/// that throws in the script (bad JSON, a non-string base or target) is no
/// aliases at all.
pub fn read_aliases(tsconfig: Option<&str>) -> Vec<(String, String)> {
    let text = strip_jsonc(tsconfig.unwrap_or(""));
    let Ok(parsed) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let Value::Object(root) = &parsed else {
        return Vec::new();
    };
    let opts = match root.get("compilerOptions") {
        Some(Value::Object(o)) => o,
        _ => return Vec::new(),
    };
    let base = opts.get("baseUrl").filter(|b| js::truthy(Some(b)));
    let entries: Vec<(String, Value)> = match opts.get("paths") {
        Some(Value::Object(m)) => tribunal_core::profile::js_ext::ordered_entries(m)
            .into_iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        Some(Value::Array(a)) => a
            .iter()
            .enumerate()
            .map(|(i, v)| (i.to_string(), v.clone()))
            .collect(),
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for (k, v) in entries {
        let Value::Array(targets) = v else { continue };
        let Some(first) = targets.first().filter(|t| js::truthy(Some(t))) else {
            continue;
        };
        let base = match base {
            None => ".",
            Some(Value::String(s)) => s.as_str(),
            Some(_) => return Vec::new(),
        };
        let Value::String(first) = first else {
            return Vec::new();
        };
        // `aliases["__proto__"] = string` sets nothing.
        if k != "__proto__" {
            out.push((k, posix_normalize(&posix_join(&[base, first]))));
        }
    }
    out
}

/// `v.replace(/\*$/, repl)` with a string replacement: `$$`, `$&`, `` $` ``
/// and `$'` expand; with no capture groups every other `$` stays literal.
fn replace_trailing_star(v: &str, repl: &str) -> String {
    let Some(before) = v.strip_suffix('*') else {
        return v.to_owned();
    };
    let mut out = before.to_owned();
    let mut chars = repl.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('$') => out.push('$'),
            Some('&') => out.push('*'),
            Some('`') => out.push_str(before),
            Some('\'') => {}
            _ => {
                out.push('$');
                continue;
            }
        }
        chars.next();
    }
    out
}

/// `specPointsAt(spec, importer, target, aliases)`: does a relative or
/// aliased specifier written in `importer` name `target`?
pub fn spec_points_at(
    spec: &str,
    importer: &str,
    target: &str,
    aliases: &[(String, String)],
) -> bool {
    let resolved = if spec.starts_with('.') {
        Some(posix_normalize(&posix_join(&[
            &posix_dirname(importer),
            spec,
        ])))
    } else {
        aliases.iter().find_map(|(k, v)| {
            if let Some(prefix) = k.strip_suffix("/*") {
                if let Some(rest) = spec.strip_prefix(&k[..prefix.len() + 1]) {
                    return Some(posix_normalize(&replace_trailing_star(v, rest)));
                }
            }
            (k == spec).then(|| posix_normalize(v))
        })
    };
    let Some(resolved) = resolved else {
        return false;
    };
    let t = no_ext(target);
    let r = no_ext(&resolved);
    r == t || format!("{r}/index") == t
}

fn imports_from(content: &str, importer: &str, target: &str, aliases: &[(String, String)]) -> bool {
    import_specifiers(content)
        .iter()
        .any(|s| spec_points_at(s, importer, target, aliases))
}

fn awaited_at(lines: &[&str], n: usize) -> bool {
    let here = lines.get(n - 1).copied().unwrap_or("");
    let before = if n >= 2 {
        lines.get(n - 2).copied().unwrap_or("")
    } else {
        ""
    };
    let before = trim_end(before);
    let b = before.as_bytes();
    has_bounded(here, "await")
        || has_bounded(here, "return")
        || has_bounded(here, "yield")
        || here.contains(".then(")
        || here.contains(".catch(")
        || before.ends_with('=')
        || before.ends_with("=>")
        || (before.ends_with("await") && (b.len() == 5 || !is_word(b[b.len() - 6])))
}

// ---------------------------------------------------------------------------
// The repository

/// `makeRepo(repoDir, rev)`: files as checked out, or at `rev` through
/// `git show`, read once each.
pub struct Repo<'a> {
    tools: &'a Tools,
    dir: String,
    rev: Option<String>,
    cache: HashMap<String, Option<String>>,
    /// The reviewed repo's `typescript`, for files oxc cannot parse cleanly.
    pub ts: TypeScriptFallback,
}

impl<'a> Repo<'a> {
    pub fn new(tools: &'a Tools, dir: &str, rev: Option<&str>) -> Self {
        Self {
            tools,
            dir: dir.to_owned(),
            // `--rev ""` is falsy in the script: the checkout is read.
            rev: rev.filter(|r| !r.is_empty()).map(str::to_owned),
            cache: HashMap::new(),
            ts: TypeScriptFallback::for_repo(dir),
        }
    }

    /// `execFileSync("git", ["-C", dir, ...args])`: stdout, or `None` where
    /// it would throw.
    async fn git(&self, args: &[&str]) -> Option<String> {
        let mut full = vec!["-C", self.dir.as_str()];
        full.extend_from_slice(args);
        let out = self.tools.git(&full, None).await.ok()?;
        (out.ok() && out.stdout.len() <= MAX_BUFFER && out.stderr.len() <= MAX_BUFFER)
            .then_some(out.stdout)
    }

    /// A file's text, `None` when it cannot be read.
    pub async fn read(&mut self, file: &str) -> Option<String> {
        if let Some(hit) = self.cache.get(file) {
            return hit.clone();
        }
        let text = match &self.rev {
            Some(rev) => self.git(&["show", &format!("{rev}:{file}")]).await,
            None => std::fs::read(posix_join(&[&self.dir, file]))
                .ok()
                .map(|b| String::from_utf8_lossy(&b).into_owned()),
        };
        self.cache.insert(file.to_owned(), text.clone());
        text
    }

    /// `readBlob(sha)`: `git cat-file -p <sha>`, `None` when it fails.
    pub async fn read_blob(&self, sha: &str) -> Option<String> {
        self.git(&["cat-file", "-p", sha]).await
    }

    /// The source files that mention `name` as a whole word (`git grep`).
    pub async fn files_mentioning(&self, name: &str) -> Vec<String> {
        let mut args = vec!["grep", "-l", "-w", "-I", "-F", "-e", name];
        if let Some(rev) = &self.rev {
            args.push(rev);
        }
        args.push("--");
        args.extend(GREP_GLOBS);
        let Some(out) = self.git(&args).await else {
            return Vec::new();
        };
        let cut = self.rev.as_deref().map_or(0, |r| js::utf16_len(r) + 1);
        out.split('\n')
            .filter(|l| !l.is_empty())
            .map(|l| utf16_suffix(l, cut).to_owned())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// blastRadius

/// Which resolver finds the callers: the script's with `typescript`
/// (`bindings`), or its line-by-word scan without it (`words`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolver {
    Bindings,
    Words,
}

#[derive(Debug, Clone)]
struct Caller {
    file: String,
    line: usize,
    via: String,
    hop: u8,
    call: bool,
    awaited: Option<bool>,
    /// Absent for the word resolver, which has no try verdict.
    in_try: Option<Option<bool>>,
    text: String,
}

impl Caller {
    fn rank(&self) -> u8 {
        if self.call {
            let base = if self.awaited == Some(false) { 0 } else { 2 };
            base + u8::from(self.in_try == Some(Some(true)))
        } else {
            4
        }
    }
}

/// An importer that binds the name, with its reference sites.
struct BoundImporter {
    file: String,
    content: String,
    refs: Vec<Ref>,
}

struct Importer {
    file: String,
    content: String,
}

struct Second {
    file: String,
    name: String,
    via: String,
}

/// `Map.set`: a new key goes last, an existing one keeps its place.
fn set_second(second: &mut Vec<(String, Second)>, key: String, val: Second) {
    match second.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 = val,
        None => second.push((key, val)),
    }
}

/// Every file outside `file` (source, not a test) that imports `name` from
/// it, read in `git grep`'s order.
async fn importers(
    repo: &mut Repo<'_>,
    name: &str,
    file: &str,
    aliases: &[(String, String)],
) -> Vec<Importer> {
    let mut read = Vec::new();
    for g in repo.files_mentioning(name).await {
        if g != file && is_source(&g) && !is_test(&g) {
            let content = repo.read(&g).await;
            read.push((g, content));
        }
    }
    read.into_iter()
        .filter_map(|(g, content)| {
            let content = content.filter(|c| !c.is_empty())?;
            imports_from(&content, &g, file, aliases).then_some(Importer { file: g, content })
        })
        .collect()
}

struct Collector<'a> {
    in_pr: HashSet<&'a str>,
    seen: HashSet<(String, usize)>,
    callers: Vec<Caller>,
}

impl Collector<'_> {
    /// `record(g, name, via, hop)`: every non-import, non-comment line of `g`
    /// that names `name`.
    fn record(&mut self, g: &Importer, name: &str, via: &str, hop: u8) {
        if self.in_pr.contains(g.file.as_str()) {
            return;
        }
        let lines: Vec<&str> = g.content.split('\n').collect();
        let skip = import_lines(&lines);
        for (i, l) in lines.iter().enumerate() {
            if skip.contains(&(i + 1)) || !has_word(l, name) || is_comment_line(l) {
                continue;
            }
            if !self.seen.insert((g.file.clone(), i + 1)) {
                continue;
            }
            let call = is_call_of(l, name);
            self.callers.push(Caller {
                file: g.file.clone(),
                line: i + 1,
                via: via.to_owned(),
                hop,
                call,
                awaited: call.then(|| awaited_at(&lines, i + 1)),
                in_try: None,
                text: utf16_prefix(js::trim(l), TEXT_CHARS).to_owned(),
            });
        }
    }

    /// `recordRefs(g, via, hop, sync)`: the reference sites the parser found.
    /// `sync`: the changed declaration returns no promise, so a call to it
    /// carries no await verdict.
    fn record_refs(&mut self, g: &BoundImporter, via: &str, hop: u8, sync: bool) {
        if self.in_pr.contains(g.file.as_str()) {
            return;
        }
        for r in &g.refs {
            if !self.seen.insert((g.file.clone(), r.line)) {
                continue;
            }
            self.callers.push(Caller {
                file: g.file.clone(),
                line: r.line,
                via: via.to_owned(),
                hop,
                call: r.call,
                awaited: if r.call && sync { None } else { r.awaited },
                in_try: Some(r.in_try),
                text: r.text.clone(),
            });
        }
    }
}

/// `boundImporters(name, file)`: the importers that bind `name` from `file`,
/// following a re-exporting barrel, four targets at most.
async fn bound_importers(
    repo: &mut Repo<'_>,
    name: &str,
    file: &str,
    aliases: &[(String, String)],
) -> Vec<BoundImporter> {
    let mut out = Vec::new();
    let mut targets = vec![file.to_owned()];
    let mut i = 0;
    while i < targets.len() && i < 4 {
        for g in repo.files_mentioning(name).await {
            if g == targets[i] || !is_source(&g) || is_test(&g) {
                continue;
            }
            let Some(content) = repo.read(&g).await.filter(|c| !c.is_empty()) else {
                continue;
            };
            let Some(r) = repo
                .ts
                .binding_refs(&g, &content, &targets[i], name, aliases)
                .await
            else {
                continue;
            };
            if r.reexports && !targets.contains(&g) {
                targets.push(g.clone());
            }
            if !r.refs.is_empty() {
                out.push(BoundImporter {
                    file: g,
                    content,
                    refs: r.refs,
                });
            }
        }
        i += 1;
    }
    out
}

/// `usingDecls(g)`: the exported declarations of an importer that contain one
/// of its references.
fn using_decls(g: &BoundImporter) -> Vec<Decl> {
    declarations(&g.content)
        .into_iter()
        .filter(|u| {
            u.exported && !u.is_type() && g.refs.iter().any(|r| r.line >= u.at && r.line <= u.end)
        })
        .collect()
}

/// `blastRadius(diff, repo, { max })` with the word resolver:
/// `{changed: [{file, symbols}], callers: [{file, line, via, call, awaited,
/// text}], total, truncated}`.
pub async fn blast_radius(diff: &str, repo: &mut Repo<'_>, max: usize) -> Value {
    blast_radius_with(diff, repo, max, Resolver::Words).await
}

/// `blastRadius(diff, repo, { max, ts })`: with [`Resolver::Bindings`] each
/// caller also carries `inTry`, and a call outside any try ranks before one
/// inside it.
#[allow(clippy::too_many_lines)]
pub async fn blast_radius_with(
    diff: &str,
    repo: &mut Repo<'_>,
    max: usize,
    resolver: Resolver,
) -> Value {
    let changed = changed_lines(diff);
    let aliases = read_aliases(repo.read("tsconfig.json").await.as_deref());
    let mut col = Collector {
        in_pr: changed.iter().map(|(f, _)| f.as_str()).collect(),
        seen: HashSet::new(),
        callers: Vec::new(),
    };
    let mut out_changed: Vec<Value> = Vec::new();
    let mut second: Vec<(String, Second)> = Vec::new();

    for (file, lines) in &changed {
        if !is_source(file) || is_test(file) {
            continue;
        }
        let Some(content) = repo.read(file).await.filter(|c| !c.is_empty()) else {
            continue;
        };
        let decls = declarations(&content);
        let hit: Vec<usize> = (0..decls.len())
            .filter(|&i| {
                let d = &decls[i];
                !d.is_type()
                    && lines
                        .iter()
                        .any(|&n| n >= d.start as f64 && n <= d.end as f64)
            })
            .collect();
        if hit.is_empty() {
            continue;
        }
        out_changed.push(json!({
            "file": file,
            "symbols": hit.iter().map(|&i| decls[i].name.clone()).collect::<Vec<_>>(),
        }));
        for &di in &hit {
            let d = &decls[di];
            for (ui, u) in decls.iter().enumerate() {
                if ui != di
                    && u.exported
                    && !u.is_type()
                    && has_word(&u.body, &d.name)
                    && !hit.contains(&ui)
                {
                    set_second(
                        &mut second,
                        format!("{file}#{}", u.export_as),
                        Second {
                            file: file.clone(),
                            name: u.export_as.clone(),
                            via: format!("{} -> {}", d.name, u.export_as),
                        },
                    );
                }
            }
            if !d.exported {
                continue;
            }
            if resolver == Resolver::Bindings {
                let sync =
                    repo.ts.returns_promise(file, &content, &d.name).await == Some(Some(false));
                for g in bound_importers(repo, &d.export_as, file, &aliases).await {
                    col.record_refs(&g, &d.export_as, 1, sync);
                    for u in using_decls(&g) {
                        set_second(
                            &mut second,
                            format!("{}#{}", g.file, u.export_as),
                            Second {
                                file: g.file.clone(),
                                name: u.export_as.clone(),
                                via: format!("{} -> {}", d.export_as, u.export_as),
                            },
                        );
                    }
                }
                continue;
            }
            for g in importers(repo, &d.export_as, file, &aliases).await {
                col.record(&g, &d.export_as, &d.export_as, 1);
                for u in declarations(&g.content) {
                    if u.exported && !u.is_type() && has_word(&u.body, &d.export_as) {
                        set_second(
                            &mut second,
                            format!("{}#{}", g.file, u.export_as),
                            Second {
                                file: g.file.clone(),
                                name: u.export_as.clone(),
                                via: format!("{} -> {}", d.export_as, u.export_as),
                            },
                        );
                    }
                }
            }
        }
    }
    for (_, s) in &second {
        if resolver == Resolver::Bindings {
            let content = repo.read(&s.file).await.unwrap_or_default();
            let sync =
                repo.ts.returns_promise(&s.file, &content, &s.name).await == Some(Some(false));
            for g in bound_importers(repo, &s.name, &s.file, &aliases).await {
                col.record_refs(&g, &s.via, 2, sync);
            }
            continue;
        }
        for g in importers(repo, &s.name, &s.file, &aliases).await {
            col.record(&g, &s.name, &s.via, 2);
        }
    }

    // Unawaited calls first, then awaited ones, then plain references, a call
    // outside any try before one inside it; within a rank the first hop, then
    // handler paths, then file and line.
    let mut callers = col.callers;
    callers.sort_by(|a, b| {
        a.rank()
            .cmp(&b.rank())
            .then(a.hop.cmp(&b.hop))
            .then(on_handler_path(&b.file).cmp(&on_handler_path(&a.file)))
            .then_with(|| locale_compare(&a.file, &b.file))
            .then(a.line.cmp(&b.line))
    });
    // One caller of each changed symbol per rank before a second of any.
    let mut nth: HashMap<(u8, String), usize> = HashMap::new();
    let mut order: Vec<(u8, usize, usize, Caller)> = callers
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let t = nth.entry((c.rank(), c.via.clone())).or_insert(0);
            *t += 1;
            (c.rank(), *t, i, c)
        })
        .collect();
    order.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let total = order.len();
    let listed: Vec<Value> = order
        .into_iter()
        .take(max)
        .map(|(_, _, _, c)| match c.in_try {
            None => json!({
                "file": c.file,
                "line": c.line,
                "via": c.via,
                "call": c.call,
                "awaited": c.awaited,
                "text": c.text,
            }),
            Some(in_try) => json!({
                "file": c.file,
                "line": c.line,
                "via": c.via,
                "call": c.call,
                "awaited": c.awaited,
                "inTry": in_try,
                "text": c.text,
            }),
        })
        .collect();
    json!({
        "changed": out_changed,
        "callers": listed,
        "total": total,
        "truncated": total > max,
    })
}

/// The libuv name of an errno this platform reports.
fn errno_name(n: i32) -> Option<&'static str> {
    #[cfg(target_os = "macos")]
    const TABLE: &[(i32, &str)] = &[
        (1, "EPERM"),
        (2, "ENOENT"),
        (3, "ESRCH"),
        (4, "EINTR"),
        (5, "EIO"),
        (6, "ENXIO"),
        (7, "E2BIG"),
        (8, "ENOEXEC"),
        (9, "EBADF"),
        (12, "ENOMEM"),
        (13, "EACCES"),
        (14, "EFAULT"),
        (16, "EBUSY"),
        (17, "EEXIST"),
        (18, "EXDEV"),
        (19, "ENODEV"),
        (20, "ENOTDIR"),
        (21, "EISDIR"),
        (22, "EINVAL"),
        (23, "ENFILE"),
        (24, "EMFILE"),
        (25, "ENOTTY"),
        (26, "ETXTBSY"),
        (27, "EFBIG"),
        (28, "ENOSPC"),
        (29, "ESPIPE"),
        (30, "EROFS"),
        (31, "EMLINK"),
        (32, "EPIPE"),
        (34, "ERANGE"),
        (35, "EAGAIN"),
        (37, "EALREADY"),
        (38, "ENOTSOCK"),
        (39, "EDESTADDRREQ"),
        (40, "EMSGSIZE"),
        (41, "EPROTOTYPE"),
        (42, "ENOPROTOOPT"),
        (43, "EPROTONOSUPPORT"),
        (44, "ESOCKTNOSUPPORT"),
        (45, "ENOTSUP"),
        (47, "EAFNOSUPPORT"),
        (48, "EADDRINUSE"),
        (49, "EADDRNOTAVAIL"),
        (50, "ENETDOWN"),
        (51, "ENETUNREACH"),
        (53, "ECONNABORTED"),
        (54, "ECONNRESET"),
        (55, "ENOBUFS"),
        (56, "EISCONN"),
        (57, "ENOTCONN"),
        (58, "ESHUTDOWN"),
        (60, "ETIMEDOUT"),
        (61, "ECONNREFUSED"),
        (62, "ELOOP"),
        (63, "ENAMETOOLONG"),
        (64, "EHOSTDOWN"),
        (65, "EHOSTUNREACH"),
        (66, "ENOTEMPTY"),
        (78, "ENOSYS"),
        (79, "EFTYPE"),
        (84, "EOVERFLOW"),
        (89, "ECANCELED"),
        (92, "EILSEQ"),
        (96, "ENODATA"),
        (100, "EPROTO"),
    ];
    #[cfg(not(target_os = "macos"))]
    const TABLE: &[(i32, &str)] = &[
        (1, "EPERM"),
        (2, "ENOENT"),
        (3, "ESRCH"),
        (4, "EINTR"),
        (5, "EIO"),
        (6, "ENXIO"),
        (7, "E2BIG"),
        (8, "ENOEXEC"),
        (9, "EBADF"),
        (11, "EAGAIN"),
        (12, "ENOMEM"),
        (13, "EACCES"),
        (14, "EFAULT"),
        (16, "EBUSY"),
        (17, "EEXIST"),
        (18, "EXDEV"),
        (19, "ENODEV"),
        (20, "ENOTDIR"),
        (21, "EISDIR"),
        (22, "EINVAL"),
        (23, "ENFILE"),
        (24, "EMFILE"),
        (25, "ENOTTY"),
        (26, "ETXTBSY"),
        (27, "EFBIG"),
        (28, "ENOSPC"),
        (29, "ESPIPE"),
        (30, "EROFS"),
        (31, "EMLINK"),
        (32, "EPIPE"),
        (34, "ERANGE"),
        (36, "ENAMETOOLONG"),
        (38, "ENOSYS"),
        (39, "ENOTEMPTY"),
        (40, "ELOOP"),
        (49, "EUNATCH"),
        (61, "ENODATA"),
        (64, "ENONET"),
        (71, "EPROTO"),
        (75, "EOVERFLOW"),
        (84, "EILSEQ"),
        (88, "ENOTSOCK"),
        (89, "EDESTADDRREQ"),
        (90, "EMSGSIZE"),
        (91, "EPROTOTYPE"),
        (92, "ENOPROTOOPT"),
        (93, "EPROTONOSUPPORT"),
        (94, "ESOCKTNOSUPPORT"),
        (95, "ENOTSUP"),
        (97, "EAFNOSUPPORT"),
        (98, "EADDRINUSE"),
        (99, "EADDRNOTAVAIL"),
        (100, "ENETDOWN"),
        (101, "ENETUNREACH"),
        (103, "ECONNABORTED"),
        (104, "ECONNRESET"),
        (105, "ENOBUFS"),
        (106, "EISCONN"),
        (107, "ENOTCONN"),
        (108, "ESHUTDOWN"),
        (110, "ETIMEDOUT"),
        (111, "ECONNREFUSED"),
        (112, "EHOSTDOWN"),
        (113, "EHOSTUNREACH"),
        (114, "EALREADY"),
        (121, "EREMOTEIO"),
        (125, "ECANCELED"),
    ];
    TABLE.iter().find(|(k, _)| *k == n).map(|(_, name)| *name)
}

/// libuv's `uv_strerror` for an error name.
fn uv_strerror(name: &str) -> &'static str {
    match name {
        "E2BIG" => "argument list too long",
        "EACCES" => "permission denied",
        "EADDRINUSE" => "address already in use",
        "EADDRNOTAVAIL" => "address not available",
        "EAFNOSUPPORT" => "address family not supported",
        "EAGAIN" => "resource temporarily unavailable",
        "EALREADY" => "connection already in progress",
        "EBADF" => "bad file descriptor",
        "EBUSY" => "resource busy or locked",
        "ECANCELED" => "operation canceled",
        "ECONNABORTED" => "software caused connection abort",
        "ECONNREFUSED" => "connection refused",
        "ECONNRESET" => "connection reset by peer",
        "EDESTADDRREQ" => "destination address required",
        "EEXIST" => "file already exists",
        "EFAULT" => "bad address in system call argument",
        "EFBIG" => "file too large",
        "EFTYPE" => "inappropriate file type or format",
        "EHOSTDOWN" => "host is down",
        "EHOSTUNREACH" => "host is unreachable",
        "EILSEQ" => "illegal byte sequence",
        "EINTR" => "interrupted system call",
        "EINVAL" => "invalid argument",
        "EIO" => "i/o error",
        "EISCONN" => "socket is already connected",
        "EISDIR" => "illegal operation on a directory",
        "ELOOP" => "too many symbolic links encountered",
        "EMFILE" => "too many open files",
        "EMLINK" => "too many links",
        "EMSGSIZE" => "message too long",
        "ENAMETOOLONG" => "name too long",
        "ENETDOWN" => "network is down",
        "ENETUNREACH" => "network is unreachable",
        "ENFILE" => "file table overflow",
        "ENOBUFS" => "no buffer space available",
        "ENODATA" => "no data available",
        "ENODEV" => "no such device",
        "ENOENT" => "no such file or directory",
        "ENOEXEC" => "exec format error",
        "ENOMEM" => "not enough memory",
        "ENONET" => "machine is not on the network",
        "ENOPROTOOPT" => "protocol not available",
        "ENOSPC" => "no space left on device",
        "ENOSYS" => "function not implemented",
        "ENOTCONN" => "socket is not connected",
        "ENOTDIR" => "not a directory",
        "ENOTEMPTY" => "directory not empty",
        "ENOTSOCK" => "socket operation on non-socket",
        "ENOTSUP" => "operation not supported on socket",
        "ENOTTY" => "inappropriate ioctl for device",
        "ENXIO" => "no such device or address",
        "EOVERFLOW" => "value too large for defined data type",
        "EPERM" => "operation not permitted",
        "EPIPE" => "broken pipe",
        "EPROTO" => "protocol error",
        "EPROTONOSUPPORT" => "protocol not supported",
        "EPROTOTYPE" => "protocol wrong type for socket",
        "ERANGE" => "result too large",
        "EREMOTEIO" => "remote I/O error",
        "EROFS" => "read-only file system",
        "ESHUTDOWN" => "cannot send after transport endpoint shutdown",
        "ESOCKTNOSUPPORT" => "socket type not supported",
        "ESPIPE" => "invalid seek",
        "ESRCH" => "no such process",
        "ETIMEDOUT" => "connection timed out",
        "ETXTBSY" => "text file is busy",
        "EUNATCH" => "protocol driver not attached",
        "EXDEV" => "cross-device link not permitted",
        _ => "",
    }
}

/// Node's `e.message` for a failed `readFileSync(path)`: `CODE: description,
/// open '<path>'` when opening fails, `CODE: description, read` when reading
/// does (a directory), and `Unknown system error -N` for both halves when
/// libuv has no name for the errno.
fn read_error_message(path: &str, e: &std::io::Error, syscall: &str) -> String {
    let (code, desc) = match e.raw_os_error() {
        Some(n) => match errno_name(n) {
            Some(name) => (name.to_owned(), uv_strerror(name).to_owned()),
            None => {
                let s = format!("Unknown system error -{n}");
                (s.clone(), s)
            }
        },
        None => return e.to_string(),
    };
    if syscall == "open" {
        format!("{code}: {desc}, open '{path}'")
    } else {
        format!("{code}: {desc}, {syscall}")
    }
}

/// `readFileSync(path, "utf8")`: open, then read; the error names the step.
fn read_diff(path: &str) -> Result<String, String> {
    use std::io::Read;
    let mut f =
        std::fs::File::open(Path::new(path)).map_err(|e| read_error_message(path, &e, "open"))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)
        .map_err(|e| read_error_message(path, &e, "read"))?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// The script's `main` for `<diff-file> <repo-dir> [--rev <sha>]` as
/// production runs it, with a parser: the binding resolver and attention
/// facts.
///
/// # Errors
/// The `FATAL: cannot read the diff ...` line the script prints (exit 1),
/// without its newline.
pub async fn run(
    diff_file: &str,
    repo_dir: &str,
    rev: Option<&str>,
    tools: &Tools,
) -> Result<Value, String> {
    run_with(diff_file, repo_dir, rev, tools, Resolver::Bindings).await
}

/// The script's `main` with the resolver it would pick: `Bindings` when it
/// loads `typescript`, `Words` (and no attention facts) when it cannot.
///
/// # Errors
/// As [`run`].
pub async fn run_with(
    diff_file: &str,
    repo_dir: &str,
    rev: Option<&str>,
    tools: &Tools,
    resolver: Resolver,
) -> Result<Value, String> {
    let ts = TypeScriptFallback::for_repo(repo_dir);
    run_in(diff_file, repo_dir, rev, tools, resolver, ts).await
}

/// [`run_with`], asking `ts` for the files oxc cannot parse cleanly, where
/// the script's `typescript` comes from `[repoDir, process.cwd()]`.
///
/// # Errors
/// As [`run`].
pub async fn run_in(
    diff_file: &str,
    repo_dir: &str,
    rev: Option<&str>,
    tools: &Tools,
    resolver: Resolver,
    ts: TypeScriptFallback,
) -> Result<Value, String> {
    let diff = read_diff(diff_file)
        .map_err(|e| format!("FATAL: cannot read the diff at {diff_file}: {e}"))?;
    let mut repo = Repo::new(tools, repo_dir, rev);
    repo.ts = ts;
    let mut out = blast_radius_with(&diff, &mut repo, MAX_CALLERS, resolver).await;
    let attention = match resolver {
        Resolver::Bindings => {
            attention_facts::attention_facts(&diff, &mut repo, attention_facts::MAX_FACTS).await
        }
        Resolver::Words => json!({"facts": [], "total": 0, "truncated": false, "parser": null}),
    };
    let obj = out.as_object_mut().expect("an object");
    let name = match resolver {
        Resolver::Bindings => "bindings",
        Resolver::Words => "words",
    };
    obj.insert("resolver".to_owned(), json!(name));
    obj.insert("attention".to_owned(), attention);
    if resolver == Resolver::Bindings {
        obj.insert("typescriptFallback".to_owned(), repo.ts.report());
    }
    Ok(out)
}
