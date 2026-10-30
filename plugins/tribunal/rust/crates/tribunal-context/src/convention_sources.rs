//! The documentation bundles a profile names in `conventionSources`, read at a
//! pinned commit and selected for one PR by their OKF `code:` frontmatter, a
//! manifest's `covers`, or an `include` list. `load_conventions` appends what
//! this renders to the conventions file.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};
use tribunal_core::js;

use crate::exec::Tools;
use crate::load_conventions::Glob;
use crate::load_profile::{
    encode_uri_component, exec_file_sync, first_line_or_gh_failed, is_not_found,
};

pub const DEFAULT_BUDGET: f64 = 30000.0;
pub const MENU_BYTES: usize = 15000;
pub const BATCH_DOCS: usize = 50;
pub const BATCH_BYTES: f64 = 300_000.0;

fn is_sha(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_ws(c: char) -> bool {
    js::is_js_whitespace(c)
}

fn has_lt(s: &str) -> bool {
    s.chars().any(js::is_line_terminator)
}

/// What `ghApi` returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gh {
    Found(String),
    NotFound,
    Error(String),
}

/// `ghApi(args)`.
pub async fn gh_api(tools: &Tools, args: &[&str]) -> Gh {
    match exec_file_sync(tools.gh(args, None).await, args) {
        Ok(text) => Gh::Found(text),
        Err(f) => {
            let detail = format!("{}{}", f.stderr, f.message);
            if is_not_found(&detail) {
                Gh::NotFound
            } else {
                Gh::Error(first_line_or_gh_failed(&detail))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// OKF frontmatter

/// `/^"((?:[^"\\]|\\.)*)"/`: the quoted body, raw.
fn double_quoted(v: &str) -> Option<&str> {
    let rest = v.strip_prefix('"')?;
    let mut it = rest.char_indices();
    while let Some((i, c)) = it.next() {
        match c {
            '"' => return Some(&rest[..i]),
            '\\' => match it.next() {
                Some((_, n)) if !js::is_line_terminator(n) => {}
                _ => return None,
            },
            _ => {}
        }
    }
    None
}

/// `/^'((?:[^']|'')*)'/`: the quoted body, raw. When no lone quote closes
/// it, the regex backtracks into the last `''` and closes there.
fn single_quoted(v: &str) -> Option<&str> {
    let rest = v.strip_prefix('\'')?;
    let b = rest.as_bytes();
    let mut i = 0;
    let mut last_pair = None;
    while i < b.len() {
        if b[i] != b'\'' {
            i += 1;
        } else if b.get(i + 1) == Some(&b'\'') {
            last_pair = Some(i);
            i += 2;
        } else {
            return Some(&rest[..i]);
        }
    }
    last_pair.map(|q| &rest[..q])
}

/// `.replace(/\\(.)/g, "$1")`.
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' {
            if let Some(&n) = it.peek() {
                if !js::is_line_terminator(n) {
                    out.push(n);
                    it.next();
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

/// `v.replace(/(?:^|\s+)#.*$/, "")`.
fn strip_comment(v: &str) -> &str {
    let chars: Vec<(usize, char)> = v.char_indices().collect();
    let tail_ok = |from: usize| !has_lt(&v[from..]);
    for (k, &(i, c)) in chars.iter().enumerate() {
        if k == 0 && c == '#' && tail_ok(i + 1) {
            return "";
        }
        if is_ws(c) {
            let mut j = k;
            while j < chars.len() && is_ws(chars[j].1) {
                j += 1;
            }
            if j < chars.len() && chars[j].1 == '#' && tail_ok(chars[j].0 + 1) {
                return &v[..i];
            }
        }
    }
    v
}

/// `yamlScalar(raw)`.
#[must_use]
pub fn yaml_scalar(raw: &str) -> String {
    let v = raw.trim();
    if let Some(body) = double_quoted(v) {
        return unescape(body);
    }
    if let Some(body) = single_quoted(v) {
        return body.replace("''", "'");
    }
    strip_comment(v).trim().to_owned()
}

/// `/^\s*(?:#.*)?$/` on what follows a flow list's `]`.
fn flow_tail_ok(s: &str) -> bool {
    let t = s.trim_start_matches(is_ws);
    t.is_empty() || (t.starts_with('#') && !has_lt(&t[1..]))
}

/// `flowList(raw)`.
fn flow_list(raw: &str) -> Option<Vec<String>> {
    let t = raw.trim();
    let body = t.strip_prefix('[')?;
    let end = body
        .char_indices()
        .rev()
        .filter(|&(_, c)| c == ']')
        .find(|&(k, _)| !has_lt(&body[..k]) && flow_tail_ok(&body[k + 1..]))?
        .0;
    Some(
        body[..end]
            .split(',')
            .map(yaml_scalar)
            .filter(|s| !s.is_empty())
            .collect(),
    )
}

/// A doc's `code:` field: a list, or a mapping keyed by consumer in the
/// order the doc writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Code {
    List(Vec<String>),
    Map(Vec<(String, Vec<String>)>),
}

/// What `okfFrontmatter` returns.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Okf {
    pub type_: Option<String>,
    pub description: Option<String>,
    pub code: Option<Code>,
}

/// `/^([A-Za-z_][A-Za-z0-9_-]*)\s*:(.*)$/` and its key-only sibling
/// `/^([A-Za-z0-9_.-]+)\s*:(.*)$/`: the key and what follows the colon.
fn key_line(l: &str, first: fn(u8) -> bool, rest: fn(u8) -> bool) -> Option<(&str, &str)> {
    let b = l.as_bytes();
    if b.is_empty() || !first(b[0]) {
        return None;
    }
    let n = 1 + b[1..].iter().take_while(|&&c| rest(c)).count();
    let after = l[n..].trim_start_matches(is_ws);
    let value = after.strip_prefix(':')?;
    (!has_lt(value)).then_some((&l[..n], value))
}

fn top_key(l: &str) -> Option<(&str, &str)> {
    key_line(
        l,
        |c| c.is_ascii_alphabetic() || c == b'_',
        |c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-',
    )
}

fn map_key(l: &str) -> Option<(&str, &str)> {
    let k = |c: u8| c.is_ascii_alphanumeric() || b"_.-".contains(&c);
    key_line(l, k, k)
}

/// `/^-\s+(.*)$/` on a trimmed line.
fn dash_item(t: &str) -> Option<&str> {
    let rest = t.strip_prefix('-')?;
    let v = rest.trim_start_matches(is_ws);
    (v.len() < rest.len() && !has_lt(v)).then_some(v)
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::new();
    let mut in_ws = false;
    for c in s.chars() {
        if is_ws(c) {
            if !in_ws {
                out.push(' ');
            }
            in_ws = true;
        } else {
            out.push(c);
            in_ws = false;
        }
    }
    out
}

/// `okfFrontmatter(text)`.
#[must_use]
pub fn okf_frontmatter(text: &str) -> Okf {
    let mut out = Okf::default();
    let Some(fm) = crate::load_conventions::frontmatter(text) else {
        return out;
    };
    let lines: Vec<&str> = fm.split('\n').collect();
    let indented = |l: &str| l.chars().next().is_some_and(is_ws);
    let mut i = 0;
    while i < lines.len() {
        let Some((name, inline)) = top_key(lines[i]) else {
            i += 1;
            continue;
        };
        let mut j = i + 1;
        while j < lines.len() && (indented(lines[j]) || lines[j].trim().is_empty()) {
            j += 1;
        }
        let body = &lines[i + 1..j];
        if name == "type" || name == "description" {
            let first = yaml_scalar(inline);
            let fb = first.as_bytes();
            let block = matches!(fb.first(), Some(b'>' | b'|'))
                && (fb.len() == 1 || (fb.len() == 2 && matches!(fb[1], b'+' | b'-')));
            let rest: Vec<&str> = body
                .iter()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect();
            let mut parts: Vec<&str> = Vec::new();
            if !block && !first.is_empty() {
                parts.push(&first);
            }
            parts.extend(rest);
            let joined = collapse_ws(&parts.join(" "));
            let v = joined.trim().to_owned();
            let v = (!v.is_empty()).then_some(v);
            if name == "type" {
                out.type_ = v;
            } else {
                out.description = v;
            }
        } else if name == "code" {
            out.code = Some(code_field(inline, body));
        }
        i = j;
    }
    out
}

fn code_field(inline: &str, body: &[&str]) -> Code {
    let scalar = yaml_scalar(inline);
    if !scalar.is_empty() {
        return Code::List(flow_list(inline).unwrap_or_else(|| vec![scalar]));
    }
    let mut list = Vec::new();
    let mut map: Vec<(String, Vec<String>)> = Vec::new();
    let mut mapped = false;
    let mut current: Option<usize> = None;
    for l in body {
        let t = l.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(item) = dash_item(t) {
            let v = yaml_scalar(item);
            if v.is_empty() {
                continue;
            }
            if let Some(c) = current {
                map[c].1.push(v);
            } else if !mapped {
                list.push(v);
            }
            continue;
        }
        let Some((key, value)) = map_key(t) else {
            continue;
        };
        mapped = true;
        let at = match map.iter().position(|(k, _)| k == key) {
            Some(p) => p,
            None => {
                map.push((key.to_owned(), Vec::new()));
                map.len() - 1
            }
        };
        current = Some(at);
        let v = yaml_scalar(value);
        if !v.is_empty() {
            let items = flow_list(value).unwrap_or_else(|| vec![v]);
            map[at].1.extend(items);
            current = None;
        }
    }
    if mapped {
        Code::Map(map)
    } else {
        Code::List(list)
    }
}

/// `codeFor(code, consumer)`.
#[must_use]
pub fn code_for(code: Option<&Code>, consumer: Option<&str>) -> Option<Vec<String>> {
    match code? {
        Code::List(l) => Some(l.clone()),
        Code::Map(m) => Some(match consumer.filter(|c| !c.is_empty()) {
            Some(c) => m
                .iter()
                .find(|(k, _)| k == c)
                .map(|(_, v)| v.clone())
                .unwrap_or_default(),
            None => m.iter().flat_map(|(_, v)| v.clone()).collect(),
        }),
    }
}

// ---------------------------------------------------------------------------
// the manifest

/// One doc a manifest lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestDoc {
    pub path: String,
    pub description: String,
    pub type_: Option<String>,
    pub covers: Option<Code>,
}

/// `parseManifest(text, path)`: the docs, or the message that refuses it.
/// Unknown keys are ignored.
pub fn parse_manifest(text: &str, path: &str) -> Result<Vec<ManifestDoc>, String> {
    let v = serde_json::from_str::<Value>(text)
        .map_err(|e| format!("the manifest {path} is not JSON ({e})"))?;
    let version = js::get(&v, "schemaVersion");
    if !matches!(version, Some(Value::Number(n)) if n.as_f64() == Some(1.0)) {
        return Err(format!(
            "the manifest {path} has schemaVersion {}; this loader reads 1",
            version.map_or_else(|| "undefined".to_owned(), Value::to_string)
        ));
    }
    let Some(Value::Array(items)) = js::get(&v, "docs") else {
        return Err(format!("the manifest {path} must hold a docs array"));
    };
    let strings = |v: &Value| -> Option<Vec<String>> {
        match v {
            Value::Array(a) => a.iter().map(|e| e.as_str().map(str::to_owned)).collect(),
            _ => None,
        }
    };
    let mut docs = Vec::new();
    for (i, d) in items.iter().enumerate() {
        let (Some(Value::String(p)), Some(Value::String(description))) =
            (js::get(d, "path"), js::get(d, "description"))
        else {
            return Err(need(path, i));
        };
        // `.replace(/\s+/g, " ").trim()`, before the limit is counted.
        let description = collapse_ws(description).trim().to_owned();
        if p.is_empty() || description.chars().count() > 300 {
            return Err(need(path, i));
        }
        let type_ = match js::get(d, "type") {
            None => None,
            Some(Value::String(t)) => Some(t.clone()),
            Some(_) => {
                return Err(format!(
                    "the manifest {path}: docs[{i}].type must be a string"
                ))
            }
        };
        let covers = match js::get(d, "covers") {
            None => None,
            Some(c) => Some(
                match c {
                    Value::Array(_) => strings(c).map(Code::List),
                    Value::Object(m) => m.iter()
                        .map(|(k, v)| strings(v).map(|l| (k.clone(), l)))
                        .collect::<Option<Vec<_>>>()
                        .map(Code::Map),
                    _ => None,
                }
                .ok_or_else(|| {
                    format!("the manifest {path}: docs[{i}].covers must be a list of entries or an object of lists")
                })?,
            ),
        };
        docs.push(ManifestDoc {
            path: p.clone(),
            description,
            type_,
            covers,
        });
    }
    Ok(docs)
}

fn need(path: &str, i: usize) -> String {
    format!(
        "the manifest {path}: docs[{i}] needs a path and a description of at most 300 characters"
    )
}

// ---------------------------------------------------------------------------
// selection

/// One candidate doc.
#[derive(Debug, Clone)]
pub struct Doc {
    pub path: String,
    pub text: String,
    pub bytes: usize,
    pub okf: Okf,
}

/// One entry of `rankMatches`.
#[derive(Debug, Clone)]
pub struct Match {
    pub doc: usize,
    pub kind: u8,
    pub path: String,
    pub entry: String,
    pub claims: usize,
    pub matched: usize,
}

struct Hit {
    path: String,
    kind: u8,
    entry: String,
}

/// `rankMatches(docs, changed, consumer, {patterns})`; `doc` indexes `docs`.
/// Kind 0 is exact, 1 pattern, 2 directory. With `patterns` a leading `./`
/// is stripped from an entry, and a pattern `Glob::compile` refuses matches
/// nothing.
#[must_use]
pub fn rank_matches(
    docs: &[Doc],
    changed: &[String],
    consumer: Option<&str>,
    patterns: bool,
) -> Vec<Match> {
    let mut hits: Vec<(usize, Vec<Hit>)> = Vec::new();
    let mut claims: HashMap<String, usize> = HashMap::new();
    let mut globs: HashMap<String, Option<Glob>> = HashMap::new();
    for (d, doc) in docs.iter().enumerate() {
        let Some(entries) = code_for(doc.okf.code.as_ref(), consumer).filter(|e| !e.is_empty())
        else {
            continue;
        };
        let kinds: Vec<(&str, u8)> = entries
            .iter()
            .map(|raw| {
                let e = if patterns {
                    raw.strip_prefix("./").unwrap_or(raw)
                } else {
                    raw
                };
                let kind = if e.ends_with('/') {
                    2
                } else if patterns && e.contains(['*', '?', '{']) {
                    1
                } else {
                    0
                };
                (e, kind)
            })
            .collect();
        let mut per = Vec::new();
        for p in changed {
            let mut glob_test = |e: &str| {
                globs
                    .entry(e.to_owned())
                    .or_insert_with(|| Glob::compile(e).ok())
                    .as_ref()
                    .is_some_and(|g| g.test(p))
            };
            let best = kinds
                .iter()
                .find(|(e, k)| *k == 0 && e == p)
                .or_else(|| kinds.iter().find(|(e, k)| *k == 1 && glob_test(e)))
                .or_else(|| kinds.iter().find(|(e, k)| *k == 2 && p.starts_with(e)));
            if let Some(&(e, kind)) = best {
                per.push(Hit {
                    path: p.clone(),
                    kind,
                    entry: e.to_owned(),
                });
            }
        }
        if per.is_empty() {
            continue;
        }
        for h in &per {
            *claims.entry(h.path.clone()).or_default() += 1;
        }
        hits.push((d, per));
    }
    let mut ranked: Vec<Match> = hits
        .into_iter()
        .map(|(d, per)| {
            let matched = per.len();
            let best = per
                .into_iter()
                .map(|h| {
                    let c = claims[&h.path];
                    (h, c)
                })
                .min_by(|(a, ac), (b, bc)| {
                    a.kind
                        .cmp(&b.kind)
                        .then(ac.cmp(bc))
                        .then_with(|| a.path.cmp(&b.path))
                })
                .expect("non-empty");
            Match {
                doc: d,
                kind: best.0.kind,
                path: best.0.path,
                entry: best.0.entry,
                claims: best.1,
                matched,
            }
        })
        .collect();
    let type_rank = |m: &Match| u8::from(docs[m.doc].okf.type_.as_deref() != Some("convention"));
    ranked.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(a.claims.cmp(&b.claims))
            .then(type_rank(a).cmp(&type_rank(b)))
            .then(b.matched.cmp(&a.matched))
            .then_with(|| docs[a.doc].path.cmp(&docs[b.doc].path))
    });
    ranked
}

/// `matchReason(m)`.
#[must_use]
pub fn match_reason(m: &Match) -> String {
    let by = format!(
        "claimed by {} doc{}",
        m.claims,
        if m.claims == 1 { "" } else { "s" }
    );
    match m.kind {
        0 => format!("exact {}, {by}", m.path),
        1 => format!("pattern {} matches {}, {by}", m.entry, m.path),
        _ => format!("directory {} holds {}, {by}", m.entry, m.path),
    }
}

/// `menuLine(doc)`.
#[must_use]
pub fn menu_line(doc: &Doc) -> String {
    format!(
        "- `{}` — {}",
        doc.path,
        doc.okf.description.as_deref().unwrap_or("(no description)")
    )
}

fn menu_bytes(docs: &[&Doc]) -> usize {
    docs.iter().map(|d| menu_line(d).len() + 1).sum()
}

/// What `selectDocs` returns; `loaded` and `offered` index the docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    pub loaded: Vec<(usize, String)>,
    pub offered: Vec<usize>,
    pub omitted: usize,
    pub bytes_loaded: usize,
}

/// `selectDocs({docs, included, select, changed, consumer, budget, patterns})`.
#[must_use]
pub fn select_docs(
    docs: &[Doc],
    included: &[String],
    select: &str,
    changed: &[String],
    consumer: Option<&str>,
    budget: f64,
    patterns: bool,
) -> Picked {
    let is_included: HashSet<&str> = included.iter().map(String::as_str).collect();
    let mut queue: Vec<(usize, String)> = Vec::new();
    for (d, doc) in docs.iter().enumerate() {
        if is_included.contains(doc.path.as_str()) {
            queue.push((d, "include".into()));
        }
    }
    let mut matched: HashSet<usize> = HashSet::new();
    if select == "all" {
        for (d, doc) in docs.iter().enumerate() {
            if !is_included.contains(doc.path.as_str()) {
                queue.push((d, "all".into()));
            }
        }
    } else {
        for m in rank_matches(docs, changed, consumer, patterns) {
            matched.insert(m.doc);
            if !is_included.contains(docs[m.doc].path.as_str()) {
                queue.push((m.doc, match_reason(&m)));
            }
        }
    }
    let mut loaded = Vec::new();
    let mut used = 0usize;
    let mut i = 0;
    while i < queue.len() {
        #[allow(clippy::cast_precision_loss)]
        if (used + docs[queue[i].0].bytes) as f64 > budget {
            break;
        }
        used += docs[queue[i].0].bytes;
        loaded.push(queue[i].clone());
        i += 1;
    }
    let rest: Vec<usize> = queue[i..].iter().map(|q| q.0).collect();
    let taken: HashSet<usize> = loaded.iter().map(|l| l.0).collect();
    let conventions: Vec<usize> = if select == "all" {
        Vec::new()
    } else {
        (0..docs.len())
            .filter(|&d| {
                docs[d].okf.type_.as_deref() == Some("convention")
                    && !taken.contains(&d)
                    && !matched.contains(&d)
                    && !is_included.contains(docs[d].path.as_str())
            })
            .collect()
    };
    let refs = |ix: &[usize]| ix.iter().map(|&d| &docs[d]).collect::<Vec<_>>();
    let mut offered: Vec<usize> = rest.iter().chain(conventions.iter()).copied().collect();
    if menu_bytes(&refs(&offered)) > MENU_BYTES {
        offered = rest;
    }
    let mut omitted = 0;
    while !offered.is_empty() && menu_bytes(&refs(&offered)) > MENU_BYTES {
        offered.pop();
        omitted += 1;
    }
    Picked {
        loaded,
        offered,
        omitted,
        bytes_loaded: used,
    }
}

// ---------------------------------------------------------------------------
// gh

/// `submodulePointer(api, repo, ref, path)`.
async fn submodule_pointer(
    tools: &Tools,
    repo: &str,
    ref_: &str,
    path: &str,
) -> Result<String, String> {
    let endpoint = format!(
        "repos/{repo}/contents/{path}?ref={}",
        encode_uri_component(ref_)
    );
    let text = match gh_api(tools, &["api", &endpoint]).await {
        Gh::Error(e) => return Err(e),
        Gh::NotFound => return Err(format!("{path} does not exist at {ref_}")),
        Gh::Found(t) => t,
    };
    let v = serde_json::from_str::<Value>(&text).map_err(|e| e.to_string())?;
    if !v.is_object() || !matches!(js::get(&v, "type"), Some(Value::String(t)) if t == "submodule")
    {
        return Err(format!("{path} is not a submodule at {ref_}"));
    }
    match js::get(&v, "sha") {
        Some(Value::String(s)) if is_sha(s) => Ok(s.clone()),
        _ => Err(format!("the submodule {path} at {ref_} names no commit")),
    }
}

async fn resolve_commit(tools: &Tools, repo: &str, ref_: &str) -> Result<String, String> {
    if is_sha(ref_) {
        return Ok(ref_.to_owned());
    }
    let endpoint = format!("repos/{repo}/commits/{}", encode_uri_component(ref_));
    match gh_api(tools, &["api", &endpoint, "--jq", ".sha"]).await {
        Gh::Error(e) => Err(e),
        Gh::NotFound => Err(format!("{repo} has no ref {ref_}")),
        Gh::Found(t) => {
            let sha = t.trim();
            if is_sha(sha) {
                Ok(sha.to_owned())
            } else {
                Err(format!("{repo} answered no commit for {ref_}"))
            }
        }
    }
}

struct Blob {
    path: String,
    sha: String,
    size: f64,
}

struct Listing {
    blobs: Vec<Blob>,
    truncated: bool,
}

/// `treeJq(root, md)`: the tree listing's `--jq` filter, the blobs under
/// `root` (`""` for every one), only the `.md` ones when `md`.
pub fn tree_jq(root: &str, md: bool) -> String {
    let mut tests = vec![r#".type == "blob""#.to_owned()];
    if md {
        tests.push(r#"(.path | endswith(".md"))"#.to_owned());
    }
    if !root.is_empty() {
        tests.push(format!("(.path | startswith({}))", js_string(root)));
    }
    format!(
        "{{truncated, tree: [.tree[] | select({}) | {{path, sha, size}}]}}",
        tests.join(" and ")
    )
}

/// `listBlobs(api, repo, sha, root, md)`.
async fn list_blobs(
    tools: &Tools,
    repo: &str,
    sha: &str,
    root: &str,
    md: bool,
) -> Result<Listing, String> {
    let endpoint = format!("repos/{repo}/git/trees/{sha}?recursive=1");
    let jq = tree_jq(root, md);
    let text = match gh_api(tools, &["api", &endpoint, "--jq", &jq]).await {
        Gh::Error(e) => return Err(e),
        Gh::NotFound => return Err(format!("{repo} has no tree at {sha}")),
        Gh::Found(t) => t,
    };
    let v = serde_json::from_str::<Value>(&text).map_err(|e| e.to_string())?;
    let Some(Value::Array(tree)) = js::get(&v, "tree") else {
        return Err(format!("{repo} answered no tree at {sha}"));
    };
    let blobs = tree
        .iter()
        .filter_map(|b| match (js::get(b, "path"), js::get(b, "sha")) {
            (Some(Value::String(p)), Some(Value::String(s)))
                if p.starts_with(root) && (!md || p.ends_with(".md")) =>
            {
                Some(Blob {
                    path: p.clone(),
                    sha: s.clone(),
                    size: match js::get(b, "size") {
                        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
                        _ => 0.0,
                    },
                })
            }
            _ => None,
        })
        .collect();
    Ok(Listing {
        blobs,
        truncated: matches!(js::get(&v, "truncated"), Some(Value::Bool(true))),
    })
}

fn js_string(s: &str) -> String {
    Value::String(s.to_owned()).to_string()
}

/// `readBlobs(api, repo, sha, blobs, cache)`.
async fn read_blobs(
    tools: &Tools,
    repo: &str,
    sha: &str,
    blobs: &[&Blob],
    cache: &mut HashMap<String, String>,
) -> Result<(), String> {
    let mut parts = repo.splitn(3, '/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    let mut seen = HashSet::new();
    let unique: Vec<&Blob> = blobs
        .iter()
        .filter(|b| !cache.contains_key(&b.sha))
        .filter(|b| seen.insert(b.sha.clone()))
        .copied()
        .collect();
    let mut i = 0;
    while i < unique.len() {
        let mut batch: Vec<&Blob> = Vec::new();
        let mut bytes = 0.0;
        while i < unique.len()
            && batch.len() < BATCH_DOCS
            && (batch.is_empty() || bytes + unique[i].size <= BATCH_BYTES)
        {
            bytes += unique[i].size;
            batch.push(unique[i]);
            i += 1;
        }
        let fields: Vec<String> = batch
            .iter()
            .enumerate()
            .map(|(k, b)| {
                format!(
                    "b{k}: object(expression: {}) {{ ... on Blob {{ text }} }}",
                    js_string(&format!("{sha}:{}", b.path))
                )
            })
            .collect();
        let query = format!(
            "query {{ repository(owner: {}, name: {}) {{ {} }} }}",
            js_string(owner),
            js_string(name),
            fields.join(" ")
        );
        let arg = format!("query={query}");
        let text = match gh_api(tools, &["api", "graphql", "-f", &arg]).await {
            Gh::Error(e) => return Err(e),
            Gh::NotFound => return Err(format!("{repo} is not readable through GraphQL")),
            Gh::Found(t) => t,
        };
        let v = serde_json::from_str::<Value>(&text).map_err(|e| e.to_string())?;
        let r = js::get(&v, "data")
            .filter(|d| js::truthy(Some(d)))
            .and_then(|d| js::get(d, "repository"))
            .filter(|r| js::is_object_like(r))
            .ok_or_else(|| format!("{repo} answered no repository through GraphQL"))?;
        for (k, b) in batch.iter().enumerate() {
            if let Some(Value::String(t)) =
                js::get(r, &format!("b{k}")).and_then(|o| js::get(o, "text"))
            {
                cache.insert(b.sha.clone(), t.clone());
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// one source

/// Where a `submodule:` ref reads its pointer.
#[derive(Debug, Clone)]
pub struct PointerRefs {
    pub head: Option<String>,
    pub head_why: Option<String>,
    pub base: String,
}

/// What `locate` found: the commit, how a `submodule:` ref found it, whether
/// the PR moved that pointer off the base's, and the candidate blobs, already
/// limited to `root` and, unless select is `manifest`, to `.md`.
struct Located {
    sha: String,
    pointer_from: Option<&'static str>,
    pointer_changed: bool,
    blobs: Vec<Blob>,
    truncated: bool,
    warnings: Vec<String>,
}

/// Why `locate` stopped: `sha` is `None` when no commit was resolved.
struct Failed {
    error: String,
    sha: Option<String>,
    pointer_from: Option<&'static str>,
    pointer_changed: bool,
    warnings: Vec<String>,
}

/// `locate({source, index, repo, profileRef, pointerRefs, api})`.
async fn locate(
    tools: &Tools,
    source: &Value,
    index: usize,
    repo: &str,
    profile_ref: &str,
    pointer_refs: &PointerRefs,
) -> Result<Located, Failed> {
    let in_repo = js::get(source, "repo").is_none();
    let docs_repo = if in_repo {
        repo
    } else {
        str_of(source, "repo")
    };
    let src_ref = str_of(source, "ref");
    let at = format!("conventionSources[{index}] {docs_repo}");
    let mut warnings = Vec::new();
    let mut pointer_from = None;
    let mut pointer_changed = false;
    let got = if in_repo {
        resolve_commit(tools, repo, profile_ref).await
    } else if let Some(path) = src_ref.strip_prefix("submodule:") {
        let head = match &pointer_refs.head {
            Some(h) => submodule_pointer(tools, repo, h, path).await,
            None => Err(pointer_refs.head_why.clone().unwrap_or_default()),
        };
        match head {
            Ok(s) => {
                pointer_from = Some("head");
                let base = submodule_pointer(tools, repo, &pointer_refs.base, path).await;
                if base.as_deref() != Ok(s.as_str()) {
                    pointer_changed = true;
                    warnings.push(format!(
                        "{at}: the docs come from this PR's submodule change ({s}), not yet on {}; treat them as proposed, not as the repo's conventions",
                        pointer_refs.base
                    ));
                }
                Ok(s)
            }
            Err(why) => {
                let base = submodule_pointer(tools, repo, &pointer_refs.base, path).await;
                if base.is_ok() {
                    pointer_from = Some("base");
                    warnings.push(format!(
                        "{at}: the submodule pointer at the PR head could not be read ({why}); using the one at {}",
                        pointer_refs.base
                    ));
                }
                base
            }
        }
    } else {
        resolve_commit(tools, docs_repo, src_ref).await
    };
    let sha = match got {
        Ok(s) => s,
        Err(error) => {
            return Err(Failed {
                error,
                sha: None,
                pointer_from,
                pointer_changed,
                warnings,
            })
        }
    };
    let root = js::get(source, "root")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let md = js::get(source, "select").and_then(Value::as_str) != Some("manifest");
    match list_blobs(tools, docs_repo, &sha, root, md).await {
        Ok(l) => Ok(Located {
            sha,
            pointer_from,
            pointer_changed,
            blobs: l.blobs,
            truncated: l.truncated,
            warnings,
        }),
        Err(error) => Err(Failed {
            error,
            sha: Some(sha),
            pointer_from,
            pointer_changed,
            warnings,
        }),
    }
}

/// What `manifestListing` returns: the listed blobs, indexes into `blobs`,
/// with their manifest docs, and the paths left out.
struct ManifestListing {
    blobs: Vec<(usize, ManifestDoc)>,
    stale: Vec<String>,
    outside: Vec<String>,
}

/// `manifestListing({api, repo, sha, source, blobs, cache})`.
async fn manifest_listing(
    tools: &Tools,
    repo: &str,
    sha: &str,
    source: &Value,
    blobs: &[Blob],
    cache: &mut HashMap<String, String>,
) -> Result<ManifestListing, String> {
    let path = str_of(source, "manifest");
    let root = str_of(source, "root");
    let Some(blob) = blobs.iter().find(|b| b.path == path) else {
        return Err(format!("the manifest {path} does not exist at {sha}"));
    };
    if blob.size > BATCH_BYTES {
        return Err(format!(
            "the manifest {path} is too large to read (over {BATCH_BYTES} bytes)"
        ));
    }
    read_blobs(tools, repo, sha, &[blob], cache).await?;
    let Some(text) = cache.get(&blob.sha) else {
        return Err(format!("the manifest {path} is not readable at {sha}"));
    };
    let docs = parse_manifest(text, path)?;
    let tree: HashMap<&str, usize> = blobs
        .iter()
        .enumerate()
        .map(|(i, b)| (b.path.as_str(), i))
        .collect();
    let mut seen = HashSet::new();
    let mut out = ManifestListing {
        blobs: Vec::new(),
        stale: Vec::new(),
        outside: Vec::new(),
    };
    for d in docs {
        if !seen.insert(d.path.clone()) {
            continue;
        }
        if d.path == path || !d.path.starts_with(root) {
            out.outside.push(d.path);
        } else if let Some(&i) = tree.get(d.path.as_str()) {
            out.blobs.push((i, d));
        } else {
            out.stale.push(d.path);
        }
    }
    Ok(out)
}

/// What `loadSource` returns.
pub struct Loaded {
    pub entry: Map<String, Value>,
    pub warnings: Vec<String>,
    pub text: String,
}

fn str_of<'a>(source: &'a Value, k: &str) -> &'a str {
    js::get(source, k)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn strings(source: &Value, k: &str) -> Vec<String> {
    match js::get(source, k) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

fn paths_value(v: impl IntoIterator<Item = String>) -> Value {
    Value::Array(v.into_iter().map(Value::String).collect())
}

/// `pointerChanged: true`, right after `pointerFrom`; absent otherwise.
fn mark_pointer_changed(entry: &mut Map<String, Value>) {
    let at = entry
        .keys()
        .position(|k| k == "pointerFrom")
        .map_or(entry.len(), |i| i + 1);
    entry.shift_insert(at, "pointerChanged".into(), Value::Bool(true));
}

/// `loadSource({source, index, repo, profileRef, pointerRefs, changed, api,
/// cache})`.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
pub async fn load_source(
    tools: &Tools,
    source: &Value,
    index: usize,
    repo: &str,
    profile_ref: &str,
    pointer_refs: &PointerRefs,
    changed: &[String],
    cache: &mut HashMap<String, String>,
) -> Loaded {
    let in_repo = js::get(source, "repo").is_none();
    let src_repo = if in_repo {
        repo
    } else {
        str_of(source, "repo")
    };
    let src_ref = if in_repo {
        profile_ref
    } else {
        str_of(source, "ref")
    };
    let at = format!("conventionSources[{index}] {src_repo}");
    let budget_v = js::get(source, "budgetBytes")
        .cloned()
        .unwrap_or_else(|| Value::from(30000));
    let budget = budget_v.as_f64().unwrap_or(DEFAULT_BUDGET);
    let select = js::get(source, "select")
        .and_then(Value::as_str)
        .unwrap_or("okf-code")
        .to_owned();
    let consumer = js::get(source, "consumer").and_then(Value::as_str);
    let mut entry = Map::new();
    entry.insert(
        "repo".into(),
        if in_repo {
            Value::Null
        } else {
            Value::String(src_repo.into())
        },
    );
    entry.insert("ref".into(), Value::String(src_ref.into()));
    entry.insert("resolvedSha".into(), Value::Null);
    entry.insert("pointerFrom".into(), Value::Null);
    entry.insert("select".into(), Value::String(select.clone()));
    if let Some(root) = js::get(source, "root") {
        entry.insert("root".into(), root.clone());
    }
    if let Some(manifest) = js::get(source, "manifest").filter(|_| select == "manifest") {
        entry.insert("manifest".into(), manifest.clone());
    }
    entry.insert("loaded".into(), Value::Array(Vec::new()));
    entry.insert("offered".into(), Value::Array(Vec::new()));
    entry.insert("bytesLoaded".into(), Value::from(0));
    entry.insert("budgetBytes".into(), budget_v);
    entry.insert("missing".into(), Value::Array(Vec::new()));
    let mut warnings = Vec::new();
    let failed = |mut entry: Map<String, Value>, mut warnings: Vec<String>, error: String| {
        warnings.push(format!("{at}: {error}. Reviewing without it."));
        let text = format!("Documentation source {src_repo} ({src_ref}) could not be read: {error}. Reviewers get none of its docs.\n");
        entry.insert("error".into(), Value::String(error));
        Loaded {
            entry,
            warnings,
            text,
        }
    };
    let mut listed = match locate(tools, source, index, repo, profile_ref, pointer_refs).await {
        Ok(l) => l,
        Err(f) => {
            warnings.extend(f.warnings);
            if let Some(sha) = f.sha {
                entry.insert("resolvedSha".into(), Value::String(sha));
            }
            if let Some(p) = f.pointer_from {
                entry.insert("pointerFrom".into(), Value::String(p.into()));
            }
            if f.pointer_changed {
                mark_pointer_changed(&mut entry);
            }
            return failed(entry, warnings, f.error);
        }
    };
    warnings.append(&mut listed.warnings);
    let sha = listed.sha.clone();
    entry.insert("resolvedSha".into(), Value::String(sha.clone()));
    if let Some(p) = listed.pointer_from {
        entry.insert("pointerFrom".into(), Value::String(p.into()));
    }
    if listed.pointer_changed {
        mark_pointer_changed(&mut entry);
    }
    if listed.truncated {
        warnings.push(format!(
            "{at}: GitHub truncated the tree at {sha}; docs past the cut are not seen"
        ));
    }
    let include_src = strings(source, "include");
    let compile = |gs: &[String]| {
        gs.iter()
            .map(|g| Glob::compile(g))
            .collect::<Result<Vec<_>, _>>()
    };
    let (include, exclude) = match (compile(&include_src), compile(&strings(source, "exclude"))) {
        (Ok(i), Ok(e)) => (i, e),
        (Err(e), _) | (_, Err(e)) => return failed(entry, warnings, e),
    };
    let mut meta: HashMap<String, ManifestDoc> = HashMap::new();
    let mut stale = Vec::new();
    let blobs: Vec<&Blob> = if select == "manifest" {
        let got = match manifest_listing(tools, src_repo, &sha, source, &listed.blobs, cache).await
        {
            Ok(g) => g,
            Err(e) => return failed(entry, warnings, e),
        };
        if !got.outside.is_empty() {
            warnings.push(format!(
                "{at}: not loaded, outside root or not a doc: {}",
                got.outside.join(", ")
            ));
        }
        stale = got.stale;
        got.blobs
            .into_iter()
            .map(|(i, d)| {
                meta.insert(d.path.clone(), d);
                &listed.blobs[i]
            })
            .collect()
    } else {
        listed.blobs.iter().collect()
    };
    let mut kept: Vec<&Blob> = blobs
        .iter()
        .filter(|b| !exclude.iter().any(|re| re.test(&b.path)))
        .copied()
        .collect();
    kept.sort_by(|a, b| a.path.cmp(&b.path));
    let included: Vec<String> = kept
        .iter()
        .filter(|b| include.iter().any(|re| re.test(&b.path)))
        .map(|b| b.path.clone())
        .collect();
    let candidates: Vec<&Blob> = if select == "all" && !include.is_empty() {
        kept.iter()
            .filter(|b| included.contains(&b.path))
            .copied()
            .collect()
    } else {
        kept
    };
    // GraphQL returns no text for a blob past the batch cap: it is not read.
    let (pool, large): (Vec<&Blob>, Vec<&Blob>) =
        candidates.into_iter().partition(|b| b.size <= BATCH_BYTES);
    if !large.is_empty() {
        let paths: Vec<&str> = large.iter().map(|b| b.path.as_str()).collect();
        warnings.push(format!(
            "{at}: too large to load (over {BATCH_BYTES} bytes): {}",
            paths.join(", ")
        ));
    }
    let all_paths: HashSet<&str> = blobs.iter().map(|b| b.path.as_str()).collect();
    let mut missing: Vec<String> = include_src
        .iter()
        .filter(|inc| !inc.contains(['*', '?', '{']) && !all_paths.contains(inc.as_str()))
        .cloned()
        .collect();
    for p in stale {
        if !missing.contains(&p) {
            missing.push(p);
        }
    }
    if let Err(e) = read_blobs(tools, src_repo, &sha, &pool, cache).await {
        // The paths already found absent stay in the failed entry, as the
        // script's do.
        entry.insert("missing".into(), paths_value(missing));
        return failed(entry, warnings, e);
    }
    let mut docs = Vec::new();
    for b in &pool {
        let Some(text) = cache.get(&b.sha) else {
            missing.push(b.path.clone());
            continue;
        };
        let okf = match meta.get(&b.path) {
            Some(m) => Okf {
                type_: m.type_.clone(),
                description: Some(m.description.clone()),
                code: m.covers.clone(),
            },
            None => okf_frontmatter(text),
        };
        docs.push(Doc {
            path: b.path.clone(),
            bytes: text.len(),
            okf,
            text: text.clone(),
        });
    }
    if !missing.is_empty() {
        warnings.push(format!("{at}: absent at {sha}: {}", missing.join(", ")));
    }
    let picked = select_docs(
        &docs,
        &included,
        &select,
        changed,
        consumer,
        budget,
        select == "manifest",
    );
    entry.insert(
        "loaded".into(),
        Value::Array(
            picked
                .loaded
                .iter()
                .map(|(d, reason)| {
                    let mut m = Map::new();
                    m.insert("path".into(), Value::String(docs[*d].path.clone()));
                    m.insert("bytes".into(), Value::from(docs[*d].bytes));
                    m.insert("reason".into(), Value::String(reason.clone()));
                    Value::Object(m)
                })
                .collect(),
        ),
    );
    entry.insert(
        "offered".into(),
        paths_value(picked.offered.iter().map(|&d| docs[d].path.clone())),
    );
    entry.insert("bytesLoaded".into(), Value::from(picked.bytes_loaded));
    entry.insert("missing".into(), paths_value(missing));
    if picked.loaded.is_empty() {
        warnings.push(format!(
            "{at}: no doc loaded ({} offered)",
            picked.offered.len()
        ));
    }
    let pointer = entry
        .get("pointerFrom")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let text = render_source(
        src_repo,
        src_ref,
        &sha,
        &select,
        str_of(source, "manifest"),
        pointer.as_deref(),
        listed.pointer_changed.then_some(profile_ref),
        &picked,
        &docs,
    );
    Loaded {
        entry,
        warnings,
        text,
    }
}

#[allow(clippy::too_many_arguments)]
fn render_source(
    repo: &str,
    ref_: &str,
    sha: &str,
    select: &str,
    manifest: &str,
    pointer: Option<&str>,
    proposed_against: Option<&str>,
    picked: &Picked,
    docs: &[Doc],
) -> String {
    let from = match pointer {
        Some("head") => ", the pointer at the PR head",
        Some("base") => ", the pointer at the profile's ref",
        _ => "",
    };
    let how = match select {
        "all" => "They are loaded in path order.".to_owned(),
        "manifest" => format!("A doc is loaded because the manifest {manifest} lists it for a path this PR changes, or because the profile always includes it."),
        _ => "A doc is loaded because its `code:` frontmatter names a path this PR changes, or because the profile always includes it.".to_owned(),
    };
    let mut out: Vec<String> = vec![
        format!("===== SOURCE: {repo}@{sha} ({ref_}{from}) ====="),
        String::new(),
    ];
    if let Some(base) = proposed_against {
        out.push(format!("These docs come from this PR's own submodule change and are not yet on {base}. Treat them as proposed changes to the conventions, to be reviewed like the rest of the diff, not as rules the PR must follow."));
        out.push(String::new());
    }
    out.extend([
        format!("These docs come from {repo}, a documentation bundle this repo's profile names, at commit {sha}. {how} They are this repo's documented conventions: a finding that rests on one should name its path."),
        String::new(),
    ]);
    for (d, _) in &picked.loaded {
        let doc = &docs[*d];
        out.push(format!("<!-- source: {repo}@{sha} {} -->", doc.path));
        out.push(doc.text.strip_suffix('\n').unwrap_or(&doc.text).to_owned());
        out.push(format!("<!-- end source: {} -->", doc.path));
        out.push(String::new());
    }
    if !picked.offered.is_empty() {
        out.push(format!("## More docs from {repo}, on demand"));
        out.push(String::new());
        out.push(format!("Pull one only when the diff leads there: gh api \"repos/{repo}/contents/<path>?ref={sha}\" -H \"Accept: application/vnd.github.raw\"."));
        out.push(String::new());
        out.extend(picked.offered.iter().map(|&d| menu_line(&docs[d])));
        if picked.omitted > 0 {
            out.push(format!(
                "- ({} more not listed, to keep this menu short)",
                picked.omitted
            ));
        }
        out.push(String::new());
    }
    out.push(format!("===== END SOURCE: {repo} ====="));
    out.push(String::new());
    out.join("\n")
}

/// What `loadSources` returns.
pub struct Sources {
    pub sources: Vec<Value>,
    pub warnings: Vec<String>,
    pub text: String,
}

/// `loadSources({sources, repo, ref, headSha, pr, changed})`.
pub async fn load_sources(
    tools: &Tools,
    sources: &[Value],
    repo: &str,
    ref_: &str,
    head_sha: &str,
    pr: &str,
    changed: &[String],
) -> Sources {
    let mut cache = HashMap::new();
    let mut results = Vec::new();
    let mut pointer_refs: Option<PointerRefs> = None;
    for (index, source) in sources.iter().enumerate() {
        if pointer_refs.is_none() && str_of(source, "ref").starts_with("submodule:") {
            let mut p = PointerRefs {
                head: (!head_sha.is_empty()).then(|| head_sha.to_owned()),
                head_why: None,
                base: ref_.to_owned(),
            };
            if p.head.is_none() && !pr.is_empty() {
                let got = gh_api(
                    tools,
                    &[
                        "pr",
                        "view",
                        pr,
                        "--repo",
                        repo,
                        "--json",
                        "headRefOid",
                        "--jq",
                        ".headRefOid",
                    ],
                )
                .await;
                let sha = match &got {
                    Gh::Found(t) => t.trim().to_owned(),
                    _ => String::new(),
                };
                if is_sha(&sha) {
                    p.head = Some(sha);
                } else {
                    p.head_why = Some(match got {
                        Gh::Error(e) => e,
                        _ => format!("gh named no head commit for PR {pr}"),
                    });
                }
            } else if p.head.is_none() {
                p.head_why = Some("no PR head was given".into());
            }
            pointer_refs = Some(p);
        }
        let refs = pointer_refs.clone().unwrap_or(PointerRefs {
            head: None,
            head_why: None,
            base: ref_.to_owned(),
        });
        results
            .push(load_source(tools, source, index, repo, ref_, &refs, changed, &mut cache).await);
    }
    Sources {
        sources: results
            .iter()
            .map(|r| Value::Object(r.entry.clone()))
            .collect(),
        warnings: results.iter().flat_map(|r| r.warnings.clone()).collect(),
        text: results
            .iter()
            .map(|r| r.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(path: &str, type_: &str, code: Option<Code>, bytes: usize) -> Doc {
        Doc {
            path: path.into(),
            text: String::new(),
            bytes,
            okf: Okf {
                type_: Some(type_.into()),
                description: Some(format!("about {path}")),
                code,
            },
        }
    }

    fn list(xs: &[&str]) -> Option<Code> {
        Some(Code::List(xs.iter().map(|s| (*s).to_owned()).collect()))
    }

    fn strs(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn frontmatter_reads_lists_mappings_and_folded_descriptions() {
        let o = okf_frontmatter("---\ntype: convention   # required\ndescription: >\n  Two\n  lines.\ncode:\n  - src/routes/\n  - 'server/core/x.ts'  # base\n---\n");
        assert_eq!(o.type_.as_deref(), Some("convention"));
        assert_eq!(o.description.as_deref(), Some("Two lines."));
        assert_eq!(o.code, list(&["src/routes/", "server/core/x.ts"]));
        let m = okf_frontmatter("---\ncode:\n  server:\n    - a.ts\n  client: [src/]\n---\n");
        assert_eq!(
            code_for(m.code.as_ref(), Some("server")),
            Some(strs(&["a.ts"]))
        );
        assert_eq!(code_for(m.code.as_ref(), Some("web")), Some(vec![]));
        assert_eq!(
            code_for(m.code.as_ref(), None),
            Some(strs(&["a.ts", "src/"]))
        );
        assert_eq!(okf_frontmatter("---\ntype: x\n---\n").code, None);
        assert_eq!(
            okf_frontmatter("---\ncode: [a, \"b/\"]\n---\n").code,
            list(&["a", "b/"])
        );
    }

    #[test]
    fn exact_ranks_before_directory_and_hubs_rank_last() {
        let docs = vec![
            doc("a-dir.md", "convention", list(&["server/routes/"]), 1),
            doc("b-hub.md", "concept", list(&["server/types.ts"]), 1),
            doc("c-hub.md", "concept", list(&["server/types.ts"]), 1),
            doc("d-hub.md", "convention", list(&["server/types.ts"]), 1),
            doc("e-own.md", "feature", list(&["server/routes/x.ts"]), 1),
            doc("f-none.md", "convention", None, 1),
        ];
        let changed = strs(&["server/routes/x.ts", "server/types.ts"]);
        let order: Vec<&str> = rank_matches(&docs, &changed, None, false)
            .iter()
            .map(|m| docs[m.doc].path.as_str())
            .collect();
        assert_eq!(
            order,
            ["e-own.md", "d-hub.md", "b-hub.md", "c-hub.md", "a-dir.md"]
        );
        let mapped = vec![doc(
            "m.md",
            "concept",
            Some(Code::Map(vec![
                ("server".into(), strs(&["server/a.ts"])),
                ("client".into(), strs(&["src/a.ts"])),
            ])),
            1,
        )];
        assert!(rank_matches(&mapped, &strs(&["src/a.ts"]), Some("server"), false).is_empty());
        assert_eq!(
            rank_matches(&mapped, &strs(&["src/a.ts"]), Some("client"), false).len(),
            1
        );
    }

    #[test]
    fn pattern_ranks_between_exact_and_directory() {
        let docs = vec![
            doc("a-dir.md", "convention", list(&["src/routes/"]), 1),
            doc("b-pattern.md", "convention", list(&["src/routes/*.ts"]), 1),
            doc("c-exact.md", "convention", list(&["src/routes/x.ts"]), 1),
            doc("d-deep.md", "concept", list(&["src/x/**"]), 1),
            doc("e-dot.md", "concept", list(&["./src/a.ts"]), 1),
        ];
        let ranked = rank_matches(&docs, &strs(&["src/routes/x.ts"]), None, true);
        let order: Vec<(&str, u8)> = ranked
            .iter()
            .map(|m| (docs[m.doc].path.as_str(), m.kind))
            .collect();
        assert_eq!(
            order,
            [("c-exact.md", 0), ("b-pattern.md", 1), ("a-dir.md", 2)]
        );
        assert_eq!(
            match_reason(&ranked[1]),
            "pattern src/routes/*.ts matches src/routes/x.ts, claimed by 3 docs"
        );
        let deep = rank_matches(&docs, &strs(&["src/x/a/b.ts"]), None, true);
        assert_eq!(deep.len(), 1);
        assert_eq!(docs[deep[0].doc].path, "d-deep.md");
        let dot = rank_matches(&docs, &strs(&["src/a.ts"]), None, true);
        assert_eq!(dot.len(), 1);
        assert_eq!(docs[dot[0].doc].path, "e-dot.md");
        assert!(rank_matches(&docs, &strs(&["src/a.ts"]), None, false).is_empty());
        let off: Vec<&str> = rank_matches(&docs, &strs(&["src/routes/x.ts"]), None, false)
            .iter()
            .map(|m| docs[m.doc].path.as_str())
            .collect();
        assert_eq!(off, ["c-exact.md", "a-dir.md"]);
        let literal = vec![doc("g.md", "concept", list(&["src/*.ts"]), 1)];
        assert_eq!(
            rank_matches(&literal, &strs(&["src/*.ts"]), None, false)[0].kind,
            0
        );
        let bad = vec![doc("h.md", "concept", list(&["{a,?}"]), 1)];
        assert!(rank_matches(&bad, &strs(&["a"]), None, true).is_empty());
    }

    #[test]
    fn include_loads_first_and_the_budget_stops_at_the_first_overflow() {
        let docs = vec![
            doc(
                "convention/a.md",
                "convention",
                list(&["server/a.ts"]),
                20000,
            ),
            doc(
                "convention/b.md",
                "convention",
                list(&["server/b.ts"]),
                8000,
            ),
            doc("guides/review.md", "convention", None, 5000),
            doc("convention/unmatched.md", "convention", list(&["lib/"]), 10),
            doc("concept/unmatched.md", "concept", list(&["lib/"]), 10),
        ];
        let p = select_docs(
            &docs,
            &strs(&["guides/review.md"]),
            "okf-code",
            &strs(&["server/a.ts", "server/b.ts"]),
            None,
            30000.0,
            false,
        );
        let loaded: Vec<(&str, &str)> = p
            .loaded
            .iter()
            .map(|(d, r)| (docs[*d].path.as_str(), r.as_str()))
            .collect();
        assert_eq!(
            loaded,
            [
                ("guides/review.md", "include"),
                ("convention/a.md", "exact server/a.ts, claimed by 1 doc")
            ]
        );
        assert_eq!(p.bytes_loaded, 25000);
        let offered: Vec<&str> = p.offered.iter().map(|&d| docs[d].path.as_str()).collect();
        assert_eq!(offered, ["convention/b.md", "convention/unmatched.md"]);
    }

    #[test]
    fn the_menu_is_cut_to_its_cap() {
        let docs: Vec<Doc> = (0..400)
            .map(|i| {
                doc(
                    &format!("convention/n{i:03}.md"),
                    "convention",
                    Some(Code::List(vec![format!("server/n{i}.ts")])),
                    50000,
                )
            })
            .collect();
        let changed: Vec<String> = (0..400).map(|i| format!("server/n{i}.ts")).collect();
        let p = select_docs(&docs, &[], "okf-code", &changed, None, 30000.0, false);
        let refs: Vec<&Doc> = p.offered.iter().map(|&d| &docs[d]).collect();
        assert!(p.loaded.is_empty() && p.omitted > 0 && menu_bytes(&refs) <= MENU_BYTES);
        assert_eq!(p.offered.len() + p.omitted, 400);
    }

    #[test]
    fn the_tree_filter_without_a_root_is_the_one_before_roots() {
        assert_eq!(
            tree_jq("", true),
            r#"{truncated, tree: [.tree[] | select(.type == "blob" and (.path | endswith(".md"))) | {path, sha, size}]}"#
        );
        assert_eq!(
            tree_jq("guides/", true),
            r#"{truncated, tree: [.tree[] | select(.type == "blob" and (.path | endswith(".md")) and (.path | startswith("guides/"))) | {path, sha, size}]}"#
        );
        assert_eq!(
            tree_jq("docs/\"q\"/", false),
            r#"{truncated, tree: [.tree[] | select(.type == "blob" and (.path | startswith("docs/\"q\"/"))) | {path, sha, size}]}"#
        );
    }

    #[test]
    fn a_manifest_is_refused_with_a_message_naming_the_problem() {
        let at = "the manifest m.json";
        let err = |t: &str| parse_manifest(t, "m.json").unwrap_err();
        let need = |i: usize| {
            format!("{at}: docs[{i}] needs a path and a description of at most 300 characters")
        };
        assert!(err("{").starts_with(&format!("{at} is not JSON (")));
        assert_eq!(
            err(r#"{"schemaVersion": 2, "docs": []}"#),
            format!("{at} has schemaVersion 2; this loader reads 1")
        );
        assert_eq!(
            err(r#"{"schemaVersion": "1", "docs": []}"#),
            format!("{at} has schemaVersion \"1\"; this loader reads 1")
        );
        assert_eq!(
            err(r#"{"docs": []}"#),
            format!("{at} has schemaVersion undefined; this loader reads 1")
        );
        assert_eq!(
            err(r#"{"schemaVersion": 1, "docs": {}}"#),
            format!("{at} must hold a docs array")
        );
        let one = |doc: &str| err(&format!(r#"{{"schemaVersion": 1, "docs": [{doc}]}}"#));
        assert_eq!(one(r#"{"path": "a.md"}"#), need(0));
        assert_eq!(one(r#"{"path": "", "description": "d"}"#), need(0));
        assert_eq!(one("null"), need(0));
        let x301 = format!(
            r#"{{"path": "a.md", "description": "{}"}}"#,
            "x".repeat(301)
        );
        assert_eq!(one(&x301), need(0));
        let emoji301 = format!(
            r#"{{"path": "a.md", "description": "{}"}}"#,
            "\u{1F600}".repeat(301)
        );
        assert_eq!(one(&emoji301), need(0));
        let emoji300 = format!(
            r#"{{"schemaVersion": 1, "docs": [{{"path": "a.md", "description": "{}"}}]}}"#,
            "\u{1F600}".repeat(300)
        );
        assert!(parse_manifest(&emoji300, "m.json").is_ok());
        assert_eq!(
            one(r#"{"path": "a.md", "description": "d", "type": 3}"#),
            format!("{at}: docs[0].type must be a string")
        );
        for covers in [r#""src/""#, "[1]", r#"{"server": "x"}"#, "null"] {
            assert_eq!(
                one(&format!(
                    r#"{{"path": "a.md", "description": "d", "covers": {covers}}}"#
                )),
                format!("{at}: docs[0].covers must be a list of entries or an object of lists")
            );
        }
    }

    #[test]
    fn a_manifest_ignores_unknown_keys_and_reads_covers_like_code() {
        let emoji150 = "\u{1F600}".repeat(150);
        let text = format!(
            r#"{{"schemaVersion": 1, "generator": "x", "docs": [
                {{"path": "a.md", "description": "{emoji150}", "owner": "dev"}},
                {{"path": "b.txt", "description": "d", "type": "convention", "covers": ["src/"]}},
                {{"path": "c.md", "description": "d", "covers": {{"server": ["a/"], "2": ["n"], "client": []}}}}
            ]}}"#
        );
        let docs = parse_manifest(&text, "m.json").unwrap();
        assert_eq!(docs.len(), 3);
        assert_eq!(docs[0].type_, None);
        assert_eq!(docs[0].covers, None);
        assert_eq!(docs[1].type_.as_deref(), Some("convention"));
        assert_eq!(docs[1].covers, list(&["src/"]));
        assert_eq!(
            docs[2].covers,
            Some(Code::Map(vec![
                ("server".into(), strs(&["a/"])),
                ("2".into(), strs(&["n"])),
                ("client".into(), vec![]),
            ]))
        );
        assert_eq!(
            code_for(docs[2].covers.as_ref(), Some("server")),
            Some(strs(&["a/"]))
        );
    }

    #[test]
    fn scalars_unquote_like_the_script() {
        assert_eq!(yaml_scalar(" \"a \\\"b\\\"\" "), "a \"b\"");
        assert_eq!(yaml_scalar("'it''s'"), "it's");
        assert_eq!(yaml_scalar("'open'' end"), "open");
        assert_eq!(yaml_scalar("feature   # one"), "feature");
        assert_eq!(yaml_scalar("a#b"), "a#b");
        assert_eq!(yaml_scalar("#x"), "");
    }
}
