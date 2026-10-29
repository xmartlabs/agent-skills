//! Port of `skills/review-pr/scripts/load-conventions.mjs`: read the repo's
//! own convention files (`profile.conventionFiles`) at the ref the profile was
//! read at, keep a file scoped with `paths:` only when the PR changes a
//! matching path, and write them as one file for every reviewer to read.
//!
//! The script compiles each glob to a JavaScript regex. No regex crate is a
//! dependency here, and the regexes a glob produces are a small family
//! (literals, `[^/]`, `.*`, an optional `(?:.*/)?`, and brace alternations
//! whose raw `?` can quantify), so [`Glob`] compiles that family itself and
//! matches over UTF-16 code units, as V8 does.

use std::collections::HashSet;

use serde_json::{Map, Value};
use tribunal_core::js;
use tribunal_core::profile::js_ext;

use crate::exec::Tools;
use crate::load_profile::{
    encode_uri_component, exec_file_sync, first_line_or_gh_failed, gh_repo_file, is_not_found,
    read_profile_file, sha256_hex, RepoFile,
};
use crate::operator::{flag, json_parse, read_file_utf8, write_file, CliOutput};

// ---------------------------------------------------------------------------
// changedPaths

/// `/^(?:---|\+\+\+) (?:[ab]\/(.+)|"[ab]\/(.+)")$/` on one line: the path,
/// which `.` keeps free of line terminators.
fn patch_path(line: &str) -> Option<&str> {
    let rest = line
        .strip_prefix("--- ")
        .or_else(|| line.strip_prefix("+++ "))?;
    let clean = |s: &str| !s.is_empty() && !s.chars().any(js::is_line_terminator);
    let plain = rest
        .strip_prefix("a/")
        .or_else(|| rest.strip_prefix("b/"))
        .filter(|p| clean(p));
    if plain.is_some() {
        return plain;
    }
    let quoted = rest.strip_prefix('"')?.strip_suffix('"')?;
    quoted
        .strip_prefix("a/")
        .or_else(|| quoted.strip_prefix("b/"))
        .filter(|p| clean(p))
}

/// `changedPaths`: every path the patch touches, both sides of a rename, in
/// first-seen order, with anything from a tab on dropped.
#[must_use]
pub fn changed_paths(patch: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in patch.split('\n') {
        if let Some(p) = patch_path(line) {
            let p = p.split('\t').next().unwrap_or_default().to_owned();
            if seen.insert(p.clone()) {
                out.push(p);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// globToRegExp

#[derive(Debug, Clone, PartialEq, Eq)]
enum Atom {
    Lit(u16),
    /// `[^/]`
    NotSlash,
    /// `.`: anything but a line terminator.
    Any,
    Group(Vec<Vec<Item>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quant {
    One,
    Opt,
    Star,
    /// `??` or `*?`: same matches, and nothing may follow.
    Lazy(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    atom: Atom,
    q: Quant,
}

const fn item(atom: Atom, q: Quant) -> Item {
    Item { atom, q }
}

/// A glob compiled the way `globToRegExp` compiles it, anchored at both ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glob {
    items: Vec<Item>,
}

const STAR: u16 = b'*' as u16;
const QMARK: u16 = b'?' as u16;

impl Glob {
    /// `globToRegExp(glob)`.
    ///
    /// # Errors
    /// Where `new RegExp` throws: a raw `?` with nothing to repeat inside a
    /// `{...}` alternative. The message is V8's.
    pub fn compile(glob: &str) -> Result<Glob, String> {
        let g: Vec<u16> = glob.encode_utf16().collect();
        let mut items = Vec::new();
        let mut i = 0;
        while i < g.len() {
            let c = g[i];
            if c == STAR && g.get(i + 1) == Some(&STAR) {
                if g.get(i + 2) == Some(&u16::from(b'/')) {
                    items.push(item(
                        Atom::Group(vec![vec![
                            item(Atom::Any, Quant::Star),
                            item(Atom::Lit(u16::from(b'/')), Quant::One),
                        ]]),
                        Quant::Opt,
                    ));
                    i += 2;
                } else {
                    items.push(item(Atom::Any, Quant::Star));
                    i += 1;
                }
            } else if c == STAR {
                items.push(item(Atom::NotSlash, Quant::Star));
            } else if c == QMARK {
                items.push(item(Atom::NotSlash, Quant::One));
            } else if c == u16::from(b'{') {
                match g[i..].iter().position(|&u| u == u16::from(b'}')) {
                    None => items.push(item(Atom::Lit(c), Quant::One)),
                    Some(off) => {
                        let end = i + off;
                        let alts = g[i + 1..end]
                            .split(|&u| u == u16::from(b','))
                            .map(|alt| Self::alternative(alt, glob))
                            .collect::<Result<Vec<_>, _>>()?;
                        items.push(item(Atom::Group(alts), Quant::One));
                        i = end;
                    }
                }
            } else {
                items.push(item(Atom::Lit(c), Quant::One));
            }
            i += 1;
        }
        Ok(Glob { items })
    }

    /// One alternative of `{a,b}`: metacharacters are escaped, `*` becomes
    /// `[^/]*`, and a `?` is left raw, so it quantifies what precedes it.
    fn alternative(alt: &[u16], glob: &str) -> Result<Vec<Item>, String> {
        let mut out: Vec<Item> = Vec::new();
        for &u in alt {
            if u == STAR {
                out.push(item(Atom::NotSlash, Quant::Star));
            } else if u == QMARK {
                let q = out.last_mut().map(|last| &mut last.q);
                match q {
                    Some(q @ Quant::One) => *q = Quant::Opt,
                    Some(q @ Quant::Opt) => *q = Quant::Lazy(true),
                    Some(q @ Quant::Star) => *q = Quant::Lazy(false),
                    _ => {
                        return Err(format!(
                            "Invalid regular expression: /^{}$/: Nothing to repeat",
                            glob_source(glob)
                        ))
                    }
                }
            } else {
                out.push(item(Atom::Lit(u), Quant::One));
            }
        }
        Ok(out)
    }

    /// `re.test(s)`.
    #[must_use]
    pub fn test(&self, s: &str) -> bool {
        let units: Vec<u16> = s.encode_utf16().collect();
        seq(&self.items, &units, 0, &|p| p == units.len())
    }
}

/// The regex source `globToRegExp` builds, for the error message only.
fn glob_source(glob: &str) -> String {
    const META: &str = ".+^$()|[]\\";
    let esc = |c: char| {
        if META.contains(c) {
            format!("\\{c}")
        } else {
            c.to_string()
        }
    };
    let chars: Vec<char> = glob.chars().collect();
    let mut re = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '*' && chars.get(i + 1) == Some(&'*') {
            if chars.get(i + 2) == Some(&'/') {
                re += "(?:.*/)?";
                i += 2;
            } else {
                re += ".*";
                i += 1;
            }
        } else if c == '*' {
            re += "[^/]*";
        } else if c == '?' {
            re += "[^/]";
        } else if c == '{' {
            match chars[i..].iter().position(|&x| x == '}') {
                None => re += "\\{",
                Some(off) => {
                    let body: String = chars[i + 1..i + off].iter().collect();
                    let alts: Vec<String> = body
                        .split(',')
                        .map(|a| a.chars().map(esc).collect::<String>().replace('*', "[^/]*"))
                        .collect();
                    re += &format!("(?:{})", alts.join("|"));
                    i += off;
                }
            }
        } else {
            re += &esc(c);
        }
        i += 1;
    }
    re
}

fn atom_at(a: &Atom, s: &[u16], pos: usize, k: &dyn Fn(usize) -> bool) -> bool {
    match a {
        Atom::Group(alts) => alts.iter().any(|alt| seq(alt, s, pos, k)),
        single => match s.get(pos) {
            Some(&u) if unit_matches(single, u) => k(pos + 1),
            _ => false,
        },
    }
}

fn unit_matches(a: &Atom, u: u16) -> bool {
    match a {
        Atom::Lit(c) => *c == u,
        Atom::NotSlash => u != u16::from(b'/'),
        Atom::Any => !matches!(u, 0x0A | 0x0D | 0x2028 | 0x2029),
        Atom::Group(_) => false,
    }
}

/// Backtracking match of `items` from `pos`, then `k`. Quantified atoms are
/// single units or the one optional group, so greedy order is enough.
fn seq(items: &[Item], s: &[u16], pos: usize, k: &dyn Fn(usize) -> bool) -> bool {
    let Some((first, rest)) = items.split_first() else {
        return k(pos);
    };
    let next = |p: usize| seq(rest, s, p, k);
    match first.q {
        Quant::One => atom_at(&first.atom, s, pos, &next),
        Quant::Opt | Quant::Lazy(true) => atom_at(&first.atom, s, pos, &next) || next(pos),
        Quant::Star | Quant::Lazy(false) => {
            let mut n = 0;
            while s
                .get(pos + n)
                .is_some_and(|&u| unit_matches(&first.atom, u))
            {
                n += 1;
            }
            (0..=n).rev().any(|j| next(pos + j))
        }
    }
}

// ---------------------------------------------------------------------------
// scopeOf

fn is_ws(c: char) -> bool {
    js::is_js_whitespace(c)
}

/// `s.trim().replace(/^(["'])(.*)\1$/, "$2")`.
fn unquote(s: &str) -> String {
    let t = js::trim(s);
    for q in ['"', '\''] {
        if t.len() >= 2 && t.starts_with(q) && t.ends_with(q) {
            let mid = &t[1..t.len() - 1];
            if !mid.chars().any(js::is_line_terminator) {
                return mid.to_owned();
            }
        }
    }
    t.to_owned()
}

/// `/^paths\s*:/`: the rest of the line after the match.
fn after_paths_key(l: &str) -> Option<&str> {
    l.strip_prefix("paths")?
        .trim_start_matches(is_ws)
        .strip_prefix(':')
}

/// `/^\s+-\s+(.+)$/`: the captured item.
fn list_item(l: &str) -> Option<&str> {
    let lead = l.len() - l.trim_start_matches(is_ws).len();
    if lead == 0 {
        return None;
    }
    let after_dash = l[lead..].strip_prefix('-')?;
    let rest = after_dash.trim_start_matches(is_ws);
    let ws = &after_dash[..after_dash.len() - rest.len()];
    if ws.is_empty() {
        return None;
    }
    if !rest.is_empty() {
        return (!rest.chars().any(js::is_line_terminator)).then_some(rest);
    }
    // Nothing after the spaces: `\s+` gives back its last character to `.+`.
    let last = ws.chars().next_back()?;
    if ws.chars().count() >= 2 && !js::is_line_terminator(last) {
        Some(&ws[ws.len() - last.len_utf8()..])
    } else {
        None
    }
}

/// `/^---\n([\s\S]*?)\n---(?:\n|$)/`: the frontmatter body.
fn frontmatter(text: &str) -> Option<&str> {
    if !text.starts_with("---\n") {
        return None;
    }
    let b = text.as_bytes();
    (4..b.len())
        .filter(|&p| b[p] == b'\n' && b[p..].starts_with(b"\n---"))
        .find(|&p| p + 4 == b.len() || b[p + 4] == b'\n')
        .map(|p| &text[4..p])
}

/// `scopeOf`: the `paths:` globs of a rule file's frontmatter, or `None` when
/// it has none.
///
/// # Errors
/// A `paths:` this cannot read, worded as the script words it.
pub fn scope_of(text: &str) -> Result<Option<Vec<String>>, String> {
    let Some(fm) = frontmatter(text) else {
        return Ok(None);
    };
    let lines: Vec<&str> = fm.split('\n').collect();
    let Some(i) = lines.iter().position(|l| after_paths_key(l).is_some()) else {
        return Ok(None);
    };
    let inline = js::trim(after_paths_key(lines[i]).unwrap_or_default());
    if !inline.is_empty() {
        let list = inline
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .filter(|m| !m.chars().any(js::is_line_terminator));
        let globs: Vec<String> = match list {
            Some(m) => m
                .split(',')
                .map(unquote)
                .filter(|g| !g.is_empty())
                .collect(),
            None => vec![unquote(inline)],
        };
        if globs.is_empty() {
            return Err("its paths: frontmatter is empty".into());
        }
        return Ok(Some(globs));
    }
    let mut globs = Vec::new();
    for l in &lines[i + 1..] {
        if let Some(it) = list_item(l) {
            globs.push(unquote(it));
        } else if l.chars().next().is_some_and(|c| !is_ws(c)) {
            break;
        } else if !js::trim(l).is_empty() {
            return Err(format!(
                "its paths: frontmatter has a line this cannot read: {l}"
            ));
        }
    }
    if globs.is_empty() {
        return Err("its paths: frontmatter lists no path".into());
    }
    Ok(Some(globs))
}

// ---------------------------------------------------------------------------
// gh

/// What `ghRepoDir` returns. `names` keeps each file entry's `name` as the
/// JSON value it is (`None` when absent).
#[derive(Debug, Clone, PartialEq)]
pub enum RepoDir {
    Found(Vec<Option<Value>>),
    NotFound,
    Error(String),
}

/// `ghRepoDir`: the file names in `<repo>:<dir>` at `ref_`.
pub async fn gh_repo_dir(tools: &Tools, repo: &str, ref_: &str, dir: &str) -> RepoDir {
    let endpoint = format!(
        "repos/{repo}/contents/{dir}?ref={}",
        encode_uri_component(ref_)
    );
    let args = ["api", endpoint.as_str()];
    // Every failure inside the script's `try`, a JSON or TypeError included,
    // goes through the same 404 test.
    let detail = match exec_file_sync(tools.gh(&args, None).await, &args) {
        Err(f) => format!("{}{}", f.stderr, f.message),
        Ok(out) => match json_parse(&out) {
            Err(e) => e,
            Ok(Value::Array(list)) => {
                let mut names = Vec::new();
                let mut null_entry = false;
                for e in &list {
                    if e.is_null() {
                        null_entry = true;
                        break;
                    }
                    if matches!(js::get(e, "type"), Some(Value::String(t)) if t == "file") {
                        names.push(js::get(e, "name").cloned());
                    }
                }
                if !null_entry {
                    return RepoDir::Found(names);
                }
                js_ext::null_property_error("type")
            }
            Ok(_) => return RepoDir::NotFound,
        },
    };
    if is_not_found(&detail) {
        RepoDir::NotFound
    } else {
        RepoDir::Error(first_line_or_gh_failed(&detail))
    }
}

// ---------------------------------------------------------------------------
// loadConventions

fn fail(error: String) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("ok".into(), Value::Bool(false));
    m.insert("error".into(), Value::String(error));
    m
}

struct Read {
    path: String,
    text: String,
    globs: Option<Vec<String>>,
}

/// `Array.prototype.sort()` with no comparator: `undefined` last, the rest by
/// their string form in UTF-16 order.
fn js_default_sort(names: &mut [Option<Value>]) {
    names.sort_by(|a, b| match (a, b) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, _) => std::cmp::Ordering::Greater,
        (_, None) => std::cmp::Ordering::Less,
        (Some(x), Some(y)) => js_ext::to_string(Some(x))
            .encode_utf16()
            .cmp(js_ext::to_string(Some(y)).encode_utf16()),
    });
}

fn template(v: Option<&Value>) -> String {
    js_ext::to_string(v)
}

/// `loadConventions`: `{ok, files, skipped, missing, text}` or `{ok: false,
/// error}`, keys in the script's order (`text` is `null` when the profile
/// lists no convention files).
///
/// # Errors
/// Where the script throws instead of returning: a scoped rule whose glob
/// does not compile. The script then dies with a stack trace.
pub async fn load_conventions(
    tools: &Tools,
    profile: &Value,
    repo: Option<&Value>,
    ref_: &str,
    changed: &[String],
) -> Result<Map<String, Value>, String> {
    let entries = match js::get(profile, "conventionFiles") {
        Some(Value::Array(a)) if !a.is_empty() => a.clone(),
        _ => {
            let mut m = Map::new();
            m.insert("ok".into(), Value::Bool(true));
            for k in ["files", "skipped", "missing"] {
                m.insert(k.into(), Value::Array(Vec::new()));
            }
            m.insert("text".into(), Value::Null);
            return Ok(m);
        }
    };
    if ref_.is_empty() {
        return Ok(fail(
            "--ref <the ref the profile was read at> is required when the profile lists conventionFiles".into(),
        ));
    }
    let repo = template(repo);
    let mut seen = HashSet::new();
    let mut read: Vec<Read> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for entry in &entries {
        let entry = template(Some(entry));
        let (dir, base) = match entry.rfind('/') {
            Some(i) => (&entry[..i], &entry[i + 1..]),
            None => ("", entry.as_str()),
        };
        let paths: Vec<String> = if base.contains('*') {
            let mut names = match gh_repo_dir(tools, &repo, ref_, dir).await {
                RepoDir::Error(e) => {
                    let shown = if dir.is_empty() { "." } else { dir };
                    return Ok(fail(format!(
                        "could not list {repo}:{shown} at {ref_} ({e}). Refusing to review without the conventions the profile names."
                    )));
                }
                RepoDir::NotFound => Vec::new(),
                RepoDir::Found(names) => names,
            };
            let re = Glob::compile(base)?;
            names.retain(|n| re.test(&template(n.as_ref())));
            js_default_sort(&mut names);
            names
                .iter()
                .map(|n| {
                    let n = template(n.as_ref());
                    if dir.is_empty() {
                        n
                    } else {
                        format!("{dir}/{n}")
                    }
                })
                .collect()
        } else {
            vec![entry.clone()]
        };
        for path in paths {
            if !seen.insert(path.clone()) {
                continue;
            }
            let text = match gh_repo_file(tools, &repo, ref_, &path).await {
                RepoFile::Error(e) => {
                    return Ok(fail(format!(
                        "could not read {repo}:{path} at {ref_} ({e}). Refusing to review without the conventions the profile names."
                    )))
                }
                RepoFile::NotFound => {
                    missing.push(path);
                    continue;
                }
                RepoFile::Found(t) => t,
            };
            let mut globs = match scope_of(&text) {
                Ok(g) => g,
                Err(e) => return Ok(fail(format!("{repo}:{path} at {ref_}: {e}"))),
            };
            if globs.is_none() && path.contains('/') && path.ends_with("/CLAUDE.md") {
                globs = Some(vec![format!(
                    "{}**",
                    &path[..path.len() - "CLAUDE.md".len()]
                )]);
            }
            read.push(Read { path, text, globs });
        }
    }
    let mut files: Vec<(Map<String, Value>, String)> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    for f in &read {
        let mut meta = Map::new();
        meta.insert("path".into(), Value::String(f.path.clone()));
        meta.insert("sha256".into(), Value::String(sha256_hex(&f.text)));
        meta.insert("bytes".into(), Value::from(f.text.len()));
        let Some(globs) = &f.globs else {
            meta.insert("scope".into(), Value::String("always".into()));
            files.push((meta, f.text.clone()));
            continue;
        };
        let res = globs
            .iter()
            .map(|g| Glob::compile(g))
            .collect::<Result<Vec<_>, _>>()?;
        meta.insert("scope".into(), Value::String("paths".into()));
        match changed.iter().find(|p| res.iter().any(|re| re.test(p))) {
            Some(m) => {
                meta.insert("matched".into(), Value::String(m.clone()));
                files.push((meta, f.text.clone()));
            }
            None => skipped.push(Value::Object(meta)),
        }
    }
    let text = render(&repo, ref_, &files, &skipped, &missing);
    let mut out = Map::new();
    out.insert("ok".into(), Value::Bool(true));
    out.insert(
        "files".into(),
        Value::Array(files.into_iter().map(|(m, _)| Value::Object(m)).collect()),
    );
    out.insert("skipped".into(), Value::Array(skipped));
    out.insert(
        "missing".into(),
        Value::Array(missing.into_iter().map(Value::String).collect()),
    );
    out.insert("text".into(), Value::String(text));
    Ok(out)
}

fn render(
    repo: &str,
    ref_: &str,
    files: &[(Map<String, Value>, String)],
    skipped: &[Value],
    missing: &[String],
) -> String {
    let mut out: Vec<String> = vec![
        format!("# Repository conventions: {repo} at {ref_}"),
        String::new(),
        "These are the repository's own convention files, read at the PR's base branch. They are this repo's policy: a finding that asks for something a rule here forbids is not a defect, and a finding that rests on a rule here should name the file and the rule.".into(),
        String::new(),
    ];
    if !skipped.is_empty() {
        let names: Vec<String> = skipped
            .iter()
            .map(|s| template(js::get(s, "path")))
            .collect();
        out.push(format!(
            "Scoped rules left out, because this PR changes no path they apply to: {}.",
            names.join(", ")
        ));
        out.push(String::new());
    }
    if !missing.is_empty() {
        out.push(format!(
            "Named by the profile but absent at {ref_}: {}.",
            missing.join(", ")
        ));
        out.push(String::new());
    }
    for (meta, text) in files {
        let path = template(meta.get("path"));
        let why = if template(meta.get("scope")) == "always" {
            "always applies".to_owned()
        } else {
            format!(
                "applies because this PR changes {}",
                template(meta.get("matched"))
            )
        };
        out.push(format!("===== FILE: {path} ({why}) ====="));
        out.push(text.strip_suffix('\n').unwrap_or(text).to_owned());
        out.push(format!("===== END FILE: {path} ====="));
        out.push(String::new());
    }
    out.join("\n")
}

/// The script run as a command: `argv` is what follows the script path.
pub async fn main(tools: &Tools, argv: &[String]) -> CliOutput {
    let done = |m: Map<String, Value>| CliOutput::json_line(&m);
    let out_path = flag(argv, "--out");
    let diff_path = flag(argv, "--diff");
    let profile_path = flag(argv, "--profile");
    if profile_path.is_empty() {
        return done(fail("--profile <file> is required".into()));
    }
    if out_path.is_empty() {
        return done(fail("--out <file> is required".into()));
    }
    if diff_path.is_empty() {
        return done(fail(
            "--diff <patch> is required: scoped rules are chosen by the paths the PR changes"
                .into(),
        ));
    }
    let profile = match read_profile_file(&profile_path) {
        Ok((p, _)) => p,
        Err(e) => return done(fail(e)),
    };
    let profile_repo = js::get(&profile, "repo").cloned();
    let repo_flag = flag(argv, "--repo");
    let repo = if repo_flag.is_empty() {
        profile_repo.clone()
    } else {
        Some(Value::String(repo_flag))
    };
    if !js_ext::strict_equals(repo.as_ref(), profile_repo.as_ref()) {
        return done(fail(format!(
            "the profile describes {}, not {}",
            template(profile_repo.as_ref()),
            template(repo.as_ref())
        )));
    }
    let patch = match read_file_utf8(&diff_path) {
        Ok(p) => p,
        Err(e) => return done(fail(format!("could not read --diff {diff_path}: {e}"))),
    };
    let ref_ = flag(argv, "--ref");
    let got = match load_conventions(
        tools,
        &profile,
        repo.as_ref(),
        &ref_,
        &changed_paths(&patch),
    )
    .await
    {
        Ok(g) => g,
        Err(e) => return CliOutput::crash(e),
    };
    if !js::truthy(got.get("ok")) {
        return done(got);
    }
    let mut rest = got;
    let text = rest.shift_remove("text").unwrap_or(Value::Null);
    let mut out = Map::new();
    out.insert("ok".into(), Value::Bool(true));
    let Value::String(text) = text else {
        out.insert("out".into(), Value::Null);
        out.insert("sha256".into(), Value::Null);
        out.insert("ref".into(), Value::Null);
        out.extend(rest);
        return done(out);
    };
    if let Err(e) = write_file(&out_path, &text) {
        return done(fail(format!("could not write --out {out_path}: {e}")));
    }
    out.insert("out".into(), Value::String(out_path));
    out.insert("sha256".into(), Value::String(sha256_hex(&text)));
    out.insert("ref".into(), Value::String(ref_));
    out.extend(rest);
    done(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        let g = Glob::compile("src/**/*.ts").unwrap();
        assert!(g.test("src/a.ts"));
        assert!(g.test("src/a/b/c.ts"));
        assert!(!g.test("lib/a.ts"));
        assert!(Glob::compile("{a,b}.md").unwrap().test("b.md"));
        assert!(Glob::compile("{?a}").is_err());
        assert!(Glob::compile("{ab?}").unwrap().test("a"));
    }

    #[test]
    fn scopes() {
        assert_eq!(scope_of("# x").unwrap(), None);
        assert_eq!(
            scope_of("---\npaths:\n  - \"src/**\"\n  - lib/*\n---\nbody").unwrap(),
            Some(vec!["src/**".into(), "lib/*".into()])
        );
        assert_eq!(
            scope_of("---\npaths: [a, 'b']\n---\n").unwrap(),
            Some(vec!["a".into(), "b".into()])
        );
        assert!(scope_of("---\npaths: []\n---\n").is_err());
    }

    #[test]
    fn patch_paths() {
        let p = "--- a/x.ts\n+++ b/y.ts\n--- /dev/null\n+++ \"b/sp ace.ts\"\n+++ b/t\tx\n";
        assert_eq!(changed_paths(p), ["x.ts", "y.ts", "sp ace.ts", "t"]);
    }
}
