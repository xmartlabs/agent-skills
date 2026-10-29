//! Port of `skills/review-pr/scripts/import-adjacency.mjs`: which carried
//! forward (unchanged-file) findings import, or are imported by, a file the PR
//! changed. A heuristic over the candidate set it is handed, never module
//! resolution.
//!
//! Also home to the pieces `blast_radius` shares with it: Node's
//! `path.posix` functions and the import-specifier scan both scripts run.
//!
//! The manifest is whatever `JSON.parse` made of stdin, so the port follows
//! JavaScript where the caller could send an odd shape: a non-string path or a
//! non-string alias target throws (`Err`), a string `carried` is iterated by
//! code point. A `files` that is truthy but not an object is read as `{}`;
//! JavaScript would index into the array, string or number instead.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Map, Value};
use tribunal_core::js;
use tribunal_core::profile::js_ext::{ordered_entries, to_string};

/// `Object.prototype` members: `files[name]` finds one of these on a plain
/// object even when the manifest has no such key.
const OBJECT_PROTO: [&str; 12] = [
    "constructor",
    "__defineGetter__",
    "__defineSetter__",
    "hasOwnProperty",
    "__lookupGetter__",
    "__lookupSetter__",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toString",
    "valueOf",
    "__proto__",
    "toLocaleString",
];

const EXTS: [&str; 7] = ["", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"];
const INDEX_SUFFIXES: [&str; 4] = ["/index.ts", "/index.tsx", "/index.js", "/index.jsx"];

// ---------------------------------------------------------------------------
// path.posix

/// Node's `normalizeString` for `/`, as `path.posix.normalize` calls it.
/// Works on bytes: every byte it compares is ASCII, and the UTF-16 lengths
/// Node compares lead to the same branch (a segment ending in `..` with
/// length 2 is `..` either way).
fn normalize_string(path: &str, allow_above_root: bool) -> String {
    let p = path.as_bytes();
    let mut res: Vec<u8> = Vec::new();
    let mut last_segment_length: usize = 0;
    let mut last_slash: isize = -1;
    let mut dots: i32 = 0;
    let mut code: u8 = 0;
    let mut i: usize = 0;
    while i <= p.len() {
        if i < p.len() {
            code = p[i];
        } else if code == b'/' {
            break;
        } else {
            code = b'/';
        }
        if code == b'/' {
            if last_slash == i as isize - 1 || dots == 1 {
                // no-op
            } else if dots == 2 {
                let mut handled = false;
                if res.len() < 2
                    || last_segment_length != 2
                    || res[res.len() - 1] != b'.'
                    || res[res.len() - 2] != b'.'
                {
                    if res.len() > 2 {
                        match res.iter().rposition(|&b| b == b'/') {
                            None => {
                                res.clear();
                                last_segment_length = 0;
                            }
                            Some(idx) => {
                                res.truncate(idx);
                                let slash = res
                                    .iter()
                                    .rposition(|&b| b == b'/')
                                    .map_or(-1, |x| x as isize);
                                last_segment_length = (res.len() as isize - 1 - slash) as usize;
                            }
                        }
                        handled = true;
                    } else if !res.is_empty() {
                        res.clear();
                        last_segment_length = 0;
                        handled = true;
                    }
                }
                if !handled && allow_above_root {
                    if res.is_empty() {
                        res.extend_from_slice(b"..");
                    } else {
                        res.extend_from_slice(b"/..");
                    }
                    last_segment_length = 2;
                }
            } else {
                let seg = &p[(last_slash + 1) as usize..i];
                if !res.is_empty() {
                    res.push(b'/');
                }
                res.extend_from_slice(seg);
                last_segment_length = (i as isize - last_slash - 1) as usize;
            }
            last_slash = i as isize;
            dots = 0;
        } else if code == b'.' && dots != -1 {
            dots += 1;
        } else {
            dots = -1;
        }
        i += 1;
    }
    String::from_utf8(res).expect("cut at ASCII bytes only")
}

/// `path.posix.normalize`.
pub fn posix_normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".to_owned();
    }
    let absolute = path.starts_with('/');
    let trailing = path.ends_with('/');
    let mut out = normalize_string(path, !absolute);
    if out.is_empty() {
        if absolute {
            return "/".to_owned();
        }
        return if trailing { "./" } else { "." }.to_owned();
    }
    if trailing {
        out.push('/');
    }
    if absolute {
        format!("/{out}")
    } else {
        out
    }
}

/// `path.posix.join`.
pub fn posix_join(parts: &[&str]) -> String {
    let joined: Vec<&str> = parts.iter().copied().filter(|p| !p.is_empty()).collect();
    if joined.is_empty() {
        return ".".to_owned();
    }
    posix_normalize(&joined.join("/"))
}

/// `path.posix.dirname`.
pub fn posix_dirname(path: &str) -> String {
    let p = path.as_bytes();
    if p.is_empty() {
        return ".".to_owned();
    }
    let has_root = p[0] == b'/';
    let mut end: Option<usize> = None;
    let mut matched_slash = true;
    let mut i = p.len() - 1;
    while i >= 1 {
        if p[i] == b'/' {
            if !matched_slash {
                end = Some(i);
                break;
            }
        } else {
            matched_slash = false;
        }
        i -= 1;
    }
    match end {
        None => if has_root { "/" } else { "." }.to_owned(),
        Some(1) if has_root => "//".to_owned(),
        Some(e) => path[..e].to_owned(),
    }
}

/// `path.posix.basename` without an extension argument.
pub fn posix_basename(path: &str) -> String {
    let p = path.as_bytes();
    let mut start = 0;
    let mut end: Option<usize> = None;
    let mut matched_slash = true;
    for i in (0..p.len()).rev() {
        if p[i] == b'/' {
            if !matched_slash {
                start = i + 1;
                break;
            }
        } else if end.is_none() {
            matched_slash = false;
            end = Some(i + 1);
        }
    }
    match end {
        None => String::new(),
        Some(e) => path[start..e].to_owned(),
    }
}

// ---------------------------------------------------------------------------
// The import scan

fn skip_js_ws(s: &str, mut i: usize) -> usize {
    while let Some(c) = s[i..].chars().next() {
        if !js::is_js_whitespace(c) {
            break;
        }
        i += c.len_utf8();
    }
    i
}

fn is_quote(b: Option<&u8>) -> bool {
    matches!(b, Some(b'\'') | Some(b'"'))
}

/// `['"]([^'"]+)['"]` at `i`: the specifier and the index after its closing
/// quote.
fn quoted(s: &str, i: usize) -> Option<(String, usize)> {
    let b = s.as_bytes();
    if !is_quote(b.get(i)) {
        return None;
    }
    let start = i + 1;
    let close = start + b[start..].iter().position(|&c| c == b'\'' || c == b'"')?;
    if close == start {
        return None;
    }
    Some((s[start..close].to_owned(), close + 1))
}

/// One attempt of IMPORT_RE at byte `i`.
fn import_at(s: &str, i: usize) -> Option<(String, usize)> {
    let rest = &s[i..];
    if rest.starts_with("from") {
        return quoted(s, skip_js_ws(s, i + 4));
    }
    let call = if rest.starts_with("require(") {
        8
    } else if rest.starts_with("import(") {
        7
    } else {
        return None;
    };
    let (spec, after) = quoted(s, skip_js_ws(s, i + call))?;
    let j = skip_js_ws(s, after);
    (s.as_bytes().get(j) == Some(&b')')).then_some((spec, j + 1))
}

/// Every specifier the global IMPORT_RE finds, in order: `from '...'`,
/// `require('...')` and `import('...')`, string literals only.
pub fn import_specifiers(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < content.len() {
        if let Some((spec, end)) = import_at(content, i) {
            out.push(spec);
            i = end;
        } else {
            i += content[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    out
}

/// The SOURCE_RE extension the path ends with, if any.
pub fn source_ext(p: &str) -> Option<&'static str> {
    [".tsx", ".ts", ".jsx", ".js", ".mjs", ".cjs"]
        .into_iter()
        .find(|e| p.ends_with(e))
}

fn stem(p: &str) -> String {
    let base = posix_basename(p);
    match source_ext(&base) {
        Some(e) => base[..base.len() - e.len()].to_owned(),
        None => base,
    }
}

// ---------------------------------------------------------------------------
// computeAdjacency

/// `[...new Set(v || [])]` where every element must then be a string, since
/// the script runs `path.posix.basename` over all of them.
fn path_list(v: Option<&Value>, name: &str) -> Result<Vec<String>, String> {
    let items: Vec<Value> = if !js::truthy(v) {
        Vec::new()
    } else {
        match v {
            Some(Value::Array(a)) => a.clone(),
            Some(Value::String(s)) => s.chars().map(|c| Value::String(c.to_string())).collect(),
            _ => return Err(format!("{name} is not iterable")),
        }
    };
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let Value::String(s) = item else {
            return Err("The \"path\" argument must be of type string".to_owned());
        };
        if !out.contains(&s) {
            out.push(s);
        }
    }
    Ok(out)
}

/// `Object.entries(aliases || {})`.
fn alias_entries(v: Option<&Value>) -> Vec<(String, Value)> {
    if !js::truthy(v) {
        return Vec::new();
    }
    match v {
        Some(Value::Object(m)) => ordered_entries(m)
            .into_iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        Some(Value::Array(a)) => a
            .iter()
            .enumerate()
            .map(|(i, v)| (i.to_string(), v.clone()))
            .collect(),
        Some(Value::String(s)) => s
            .chars()
            .enumerate()
            .map(|(i, c)| (i.to_string(), Value::String(c.to_string())))
            .collect(),
        _ => Vec::new(),
    }
}

struct Alias {
    key_prefix: String,
    target_prefix: String,
}

fn alias_prefixes(entries: Vec<(String, Value)>) -> Result<Vec<Alias>, String> {
    let mut out = Vec::new();
    for (key, targets) in entries {
        let keep = match &targets {
            Value::Array(a) => !a.is_empty(),
            other => js::truthy(Some(other)),
        };
        if !keep {
            continue;
        }
        let target = match &targets {
            Value::Array(a) => &a[0],
            other => other,
        };
        let Value::String(target) = target else {
            return Err("replace is not a function".to_owned());
        };
        out.push(Alias {
            key_prefix: key.strip_suffix('*').unwrap_or(&key).to_owned(),
            target_prefix: target.strip_suffix('*').unwrap_or(target).to_owned(),
        });
    }
    Ok(out)
}

/// `files[name]`, then `content || ""` as the scan coerces it. `None` is
/// `undefined`. An `Object.prototype` member is found on any plain object and
/// stringifies to text with no specifier in it. No `files` is `{}`, which
/// still has those members.
fn file_content(files: Option<&Map<String, Value>>, name: &str) -> Option<String> {
    match files.and_then(|f| f.get(name)) {
        Some(v) if js::truthy(Some(v)) => Some(to_string(Some(v))),
        Some(_) => Some(String::new()),
        None if OBJECT_PROTO.contains(&name) => Some(String::new()),
        None => None,
    }
}

struct Resolver<'a> {
    known: HashSet<&'a str>,
    stems: HashMap<String, Vec<&'a str>>,
    aliases: &'a [Alias],
}

impl Resolver<'_> {
    /// `resolveSpecifier`: the known file a specifier names, with "path" for a
    /// resolved match and "stem" for the same-basename fallback.
    fn resolve(&self, spec: &str, from: &str) -> Option<(String, &'static str)> {
        let resolved = if spec.starts_with('.') {
            posix_normalize(&posix_join(&[&posix_dirname(from), spec]))
        } else {
            let alias = self
                .aliases
                .iter()
                .find(|a| spec.starts_with(a.key_prefix.as_str()))?;
            format!("{}{}", alias.target_prefix, &spec[alias.key_prefix.len()..])
        };
        for ext in EXTS {
            let f = format!("{resolved}{ext}");
            if self.known.contains(f.as_str()) {
                return Some((f, "path"));
            }
        }
        for suf in INDEX_SUFFIXES {
            let f = format!("{resolved}{suf}");
            if self.known.contains(f.as_str()) {
                return Some((f, "path"));
            }
        }
        self.stems
            .get(&stem(&resolved))
            .and_then(|list| list.iter().find(|p| **p != from))
            .map(|p| ((*p).to_owned(), "stem"))
    }
}

/// `computeAdjacency(manifest)`: `{relatedFiles, scanned,
/// aliasResolutionAttempted}`, keys in the order `JSON.stringify` prints.
///
/// # Errors
/// Where the script throws: a `null` manifest, a non-iterable `carried` or
/// `changed`, a non-string path, or an alias whose target is not a string.
pub fn compute_adjacency(manifest: &Value) -> Result<Value, String> {
    if manifest.is_null() {
        return Err(
            "Cannot destructure property 'files' of 'object null' as it is null.".to_owned(),
        );
    }
    let files = match js::get(manifest, "files") {
        Some(Value::Object(m)) => Some(m),
        _ => None,
    };
    let carried = path_list(js::get(manifest, "carried"), "carried")?;
    let changed = path_list(js::get(manifest, "changed"), "changed")?;

    let mut known_order: Vec<&str> = Vec::new();
    for p in carried.iter().chain(changed.iter()) {
        if !known_order.contains(&p.as_str()) {
            known_order.push(p);
        }
    }
    let mut stems: HashMap<String, Vec<&str>> = HashMap::new();
    for p in &known_order {
        stems.entry(stem(p)).or_default().push(p);
    }
    let aliases = alias_prefixes(alias_entries(js::get(manifest, "aliases")))?;
    let resolver = Resolver {
        known: known_order.iter().copied().collect(),
        stems,
        aliases: &aliases,
    };

    let mut changed_targets: Vec<(&str, HashSet<String>)> = Vec::new();
    for cf in &changed {
        let Some(content) = file_content(files, cf) else {
            continue;
        };
        let targets = import_specifiers(&content)
            .iter()
            .filter_map(|s| resolver.resolve(s, cf).map(|(f, _)| f))
            .collect();
        changed_targets.push((cf, targets));
    }

    let mut related = Map::new();
    let mut scanned = 0u64;
    for carried_file in &carried {
        let Some(content) = file_content(files, carried_file) else {
            continue;
        };
        scanned += 1;
        let mut found: Vec<(String, &'static str)> = Vec::new();
        for spec in import_specifiers(&content) {
            let Some((file, conf)) = resolver.resolve(&spec, carried_file) else {
                continue;
            };
            if !changed.contains(&file) {
                continue;
            }
            match found.iter_mut().find(|(f, _)| *f == file) {
                None => found.push((file, conf)),
                Some(prev) if prev.1 == "stem" && conf == "path" => prev.1 = conf,
                Some(_) => {}
            }
        }
        for (cf, targets) in &changed_targets {
            if targets.contains(carried_file) && !found.iter().any(|(f, _)| f == cf) {
                found.push(((*cf).to_owned(), "path"));
            }
        }
        // `relatedFiles["__proto__"] = [...]` sets the prototype, not a key.
        if !found.is_empty() && carried_file != "__proto__" {
            related.insert(
                carried_file.clone(),
                Value::Array(
                    found
                        .into_iter()
                        .map(|(file, c)| json!({"file": file, "matchConfidence": c}))
                        .collect(),
                ),
            );
        }
    }
    let related: Map<String, Value> = ordered_entries(&related)
        .into_iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Ok(json!({
        "relatedFiles": related,
        "scanned": scanned,
        "aliasResolutionAttempted": !aliases.is_empty(),
    }))
}

/// The script's `main`: stdin to the one JSON line it prints (without the
/// newline `console.log` adds). Empty or unparseable input is `{}`.
///
/// # Errors
/// As [`compute_adjacency`]; the script then exits 1 with nothing on stdout.
pub fn run(stdin: &str) -> Result<String, String> {
    let input = if stdin.is_empty() { "{}" } else { stdin };
    let manifest: Value = serde_json::from_str(input).unwrap_or_else(|_| json!({}));
    compute_adjacency(&manifest).map(|v| v.to_string())
}
