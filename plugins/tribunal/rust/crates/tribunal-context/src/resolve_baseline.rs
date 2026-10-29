//! Port of `skills/review-pr/scripts/resolve-baseline.mjs`: the commit an
//! incremental re-review is scoped against. It tries the last posted tribunal
//! review of this PR at a different commit (from the metrics ledger), then the
//! latest human APPROVED or CHANGES_REQUESTED review.
//!
//! The script reads whatever JSON it is given and applies JavaScript's
//! coercions to it, so this port works on `serde_json::Value` and reproduces
//! those coercions, including the `TypeError`s a few inputs raise. A thrown
//! error is an `Err` holding V8's message; [`run`] turns it into exit code 1.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};
use tribunal_core::evidence::js_ext::{property_order, utf16_cmp};
use tribunal_core::js::{number_to_string, string_to_number, truthy};
use tribunal_core::lens::js_ext::utf16_prefix_lossy;
use tribunal_core::profile::js_ext::{json_stringify, strict_equals};

use crate::incremental_scope::read_file_utf8;

/// Logins that never count as a human-review baseline, besides any ending in
/// `[bot]`.
pub const BOT_LOGINS: [&str; 1] = ["coderabbitai"];

/// The two review states that are evidence someone reviewed the PR.
pub const QUALIFYING_REVIEW_STATES: [&str; 2] = ["APPROVED", "CHANGES_REQUESTED"];

/// The reason the script gives when neither source yields a baseline.
pub const NONE_REASON: &str =
    "no posted prior review with findings at a different commit, and no qualifying human review";

const TO_PRIMITIVE_ERROR: &str = "Cannot convert object to primitive value";

/// `ToString(v)`; `None` is `undefined`. An object with an own `toString` key
/// throws: JSON cannot make that key callable, so `OrdinaryToPrimitive` finds
/// neither method usable.
fn to_str(v: Option<&Value>) -> Result<String, String> {
    Ok(match v {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => number_to_string(n.as_f64().unwrap_or(f64::NAN)),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|e| match e {
                Value::Null => Ok(String::new()),
                e => to_str(Some(e)),
            })
            .collect::<Result<Vec<_>, _>>()?
            .join(","),
        Some(Value::Object(m)) if m.contains_key("toString") => {
            return Err(TO_PRIMITIVE_ERROR.into())
        }
        Some(Value::Object(_)) => "[object Object]".into(),
    })
}

enum Prim {
    Str(String),
    Num(f64),
}

/// `ToPrimitive` then, for anything but a string, `ToNumber`.
fn to_prim(v: &Value) -> Result<Prim, String> {
    Ok(match v {
        Value::Null => Prim::Num(0.0),
        Value::Bool(b) => Prim::Num(f64::from(u8::from(*b))),
        Value::Number(n) => Prim::Num(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => Prim::Str(s.clone()),
        other => Prim::Str(to_str(Some(other))?),
    })
}

fn prim_number(p: &Prim) -> f64 {
    match p {
        Prim::Str(s) => string_to_number(s),
        Prim::Num(x) => *x,
    }
}

/// `a > b`: string order by UTF-16 units when both sides are strings after
/// `ToPrimitive`, number order otherwise, NaN never greater.
fn js_gt(a: &Value, b: &Value) -> Result<bool, String> {
    let (pa, pb) = (to_prim(a)?, to_prim(b)?);
    if let (Prim::Str(x), Prim::Str(y)) = (&pa, &pb) {
        return Ok(utf16_cmp(x, y).is_gt());
    }
    Ok(prim_number(&pa) > prim_number(&pb))
}

/// `v < 3` for a truthy `v`.
fn js_lt_three(v: &Value) -> Result<bool, String> {
    Ok(prim_number(&to_prim(v)?) < 3.0)
}

/// `a || b` on values read from JSON.
fn or<'a>(a: Option<&'a Value>, b: &'a Value) -> &'a Value {
    match a {
        Some(v) if truthy(Some(v)) => v,
        _ => b,
    }
}

/// A property read on a value `JSON.parse` produced. Only objects have the
/// properties this script reads; `null` throws, as V8 does.
fn prop<'a>(v: &'a Value, key: &str) -> Result<Option<&'a Value>, String> {
    match v {
        Value::Object(m) => Ok(m.get(key)),
        Value::Null => Err(format!("Cannot read properties of null (reading '{key}')")),
        _ => Ok(None),
    }
}

/// Ports `isBotAuthor`.
///
/// # Errors
/// When `String(login)` throws.
pub fn is_bot_author(login: Option<&Value>) -> Result<bool, String> {
    let l = to_str(Some(or(login, &Value::String(String::new()))))?;
    if l.is_empty() {
        return Ok(false);
    }
    // `/\[bot\]$/i` without `u`: only ASCII letters fold onto b, o and t.
    let tail = l.len() >= 5 && l.is_char_boundary(l.len() - 5);
    let suffix = tail && l[l.len() - 5..].eq_ignore_ascii_case("[bot]");
    Ok(suffix || BOT_LOGINS.contains(&l.as_str()))
}

/// Whether `!(x || []).length` is false, that is the `length` read is truthy.
fn has_truthy_length(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Object(m)) => truthy(m.get("length")),
        _ => false,
    }
}

/// Ports `projectFinding`. A key whose value is `undefined` is left out, as
/// `JSON.stringify` leaves it out.
fn project_finding(f: &Value) -> Result<Value, String> {
    let file = prop(f, "file")?;
    let empty = Value::String(String::new());
    let mut out = Map::new();
    out.insert("file".into(), or(file, &empty).clone());
    out.insert("title".into(), or(prop(f, "title")?, &empty).clone());
    if let Some(s) = prop(f, "severity")? {
        out.insert("severity".into(), s.clone());
    }
    for key in ["blocking", "survived", "posted"] {
        out.insert(key.into(), Value::Bool(truthy(prop(f, key)?)));
    }
    if let Some(o) = prop(f, "operatorOverride")? {
        out.insert("operatorOverride".into(), o.clone());
    }
    Ok(Value::Object(out))
}

fn decision(
    sha: Value,
    findings: Vec<Value>,
    source: &str,
    reason: String,
    ts: Value,
    reviewer: Option<Value>,
    review_state: Value,
) -> Value {
    let mut out = Map::new();
    out.insert("found".into(), Value::Bool(true));
    out.insert("sha".into(), sha);
    out.insert("findings".into(), Value::Array(findings));
    out.insert("source".into(), source.into());
    out.insert("reason".into(), reason.into());
    out.insert("ts".into(), ts);
    if let Some(r) = reviewer {
        out.insert("reviewer".into(), r);
    }
    out.insert("reviewState".into(), review_state);
    Value::Object(out)
}

/// Ports `resolveFromTribunalRows`. `rows` are already this PR's.
fn from_tribunal_rows(
    rows: &[Value],
    head_sha: &Value,
    since: &Value,
) -> Result<Option<Value>, String> {
    let empty = Value::String(String::new());
    let mut best: Option<&Value> = None;
    for row in rows {
        let version = prop(row, "schemaVersion")?;
        if !truthy(version) || js_lt_three(version.expect("truthy is present"))? {
            continue;
        }
        let sha = or(prop(row, "headSha")?, &empty);
        if !truthy(Some(sha)) || strict_equals(Some(sha), Some(head_sha)) {
            continue;
        }
        if truthy(Some(since)) {
            let Value::String(sha) = sha else {
                return Err("sha.startsWith is not a function".into());
            };
            if !sha.starts_with(&to_str(Some(since))?) {
                continue;
            }
        } else if !truthy(prop(row, "posted")?) {
            continue;
        }
        if !has_truthy_length(prop(row, "findings")?) {
            continue;
        }
        // The last matching row in file order wins.
        best = Some(row);
    }
    let Some(best) = best else { return Ok(None) };
    let Some(Value::Array(findings)) = prop(best, "findings")? else {
        return Err("(best.findings || []).map is not a function".into());
    };
    let findings = findings
        .iter()
        .map(project_finding)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(decision(
        prop(best, "headSha")?.cloned().unwrap_or(Value::Null),
        findings,
        "tribunal-row",
        "posted review with findings at a different commit".into(),
        or(prop(best, "ts")?, &Value::Null).clone(),
        Some(Value::Null),
        Value::Null,
    )))
}

/// Ports `resolveFromHumanReviews`.
fn from_human_reviews(reviews: &[Value], head_sha: &Value) -> Result<Option<Value>, String> {
    let empty = Value::String(String::new());
    // (review, commit, at)
    let mut best: Option<(&Value, &Value, &Value)> = None;
    for r in reviews {
        let state = prop(r, "state")?;
        let qualifies = matches!(state, Some(Value::String(s)) if QUALIFYING_REVIEW_STATES.contains(&s.as_str()));
        if !qualifies || is_bot_author(prop(r, "author")?)? {
            continue;
        }
        let commit = or(prop(r, "commit")?, &empty);
        if !truthy(Some(commit)) || strict_equals(Some(commit), Some(head_sha)) {
            continue;
        }
        let at = or(prop(r, "at")?, &empty);
        let replace = match best {
            None => true,
            Some((_, _, best_at)) => js_gt(at, or(Some(best_at), &empty))?,
        };
        if replace {
            best = Some((r, commit, at));
        }
    }
    let Some((r, commit, _)) = best else {
        return Ok(None);
    };
    let state = prop(r, "state")?.cloned().unwrap_or(Value::Null);
    let author = prop(r, "author")?;
    let reason = format!("{} review by {}", to_str(Some(&state))?, to_str(author)?);
    Ok(Some(decision(
        commit.clone(),
        Vec::new(),
        "human-review",
        reason,
        Value::Null,
        author.cloned(),
        state,
    )))
}

/// Ports `resolveBaseline`. `head_sha` and `since` are the values after the
/// script's `|| ""`.
///
/// # Errors
/// V8's `TypeError` message for the inputs the script throws on.
pub fn resolve_baseline(
    rows: &[Value],
    reviews: &[Value],
    head_sha: &Value,
    since: &Value,
) -> Result<Value, String> {
    if let Some(d) = from_tribunal_rows(rows, head_sha, since)? {
        return Ok(d);
    }
    if let Some(d) = from_human_reviews(reviews, head_sha)? {
        return Ok(d);
    }
    let mut out = Map::new();
    out.insert("found".into(), Value::Bool(false));
    out.insert("sha".into(), Value::Null);
    out.insert("findings".into(), Value::Array(Vec::new()));
    out.insert("source".into(), Value::Null);
    out.insert("reason".into(), NONE_REASON.into());
    for key in ["ts", "reviewer", "reviewState"] {
        out.insert(key.into(), Value::Null);
    }
    Ok(Value::Object(out))
}

/// `JSON.parse` into a `Value`, keys in the order `JSON.parse` gives them.
///
/// serde_json refuses nesting deeper than 128 levels, and `JSON.parse` has no
/// such limit, so the structure is walked here with an explicit stack. Strings
/// and numbers are still read by serde_json, one token at a time.
pub fn parse_json(text: &str) -> Result<Value, String> {
    enum Frame {
        Array(Vec<Value>),
        Object(Map<String, Value>, String),
    }
    let b = text.as_bytes();
    let mut i = 0;
    let skip_ws = |i: &mut usize| {
        while matches!(b.get(*i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            *i += 1;
        }
    };
    let unexpected = |i: usize| match text[i..].chars().next() {
        Some(c) => format!("unexpected {c:?} at byte {i}"),
        None => "unexpected end of input".to_owned(),
    };
    let string = |i: &mut usize| -> Result<String, String> {
        if b.get(*i) != Some(&b'"') {
            return Err(unexpected(*i));
        }
        let mut j = *i + 1;
        while j < b.len() && b[j] != b'"' {
            j += if b[j] == b'\\' { 2 } else { 1 };
        }
        if j >= b.len() {
            return Err("unterminated string".to_owned());
        }
        let s = serde_json::from_str::<String>(&text[*i..=j]).map_err(|e| e.to_string())?;
        *i = j + 1;
        Ok(s)
    };
    let mut stack: Vec<Frame> = Vec::new();
    loop {
        skip_ws(&mut i);
        let mut value = match b.get(i) {
            Some(b'[') => {
                i += 1;
                skip_ws(&mut i);
                if b.get(i) == Some(&b']') {
                    i += 1;
                    Value::Array(Vec::new())
                } else {
                    stack.push(Frame::Array(Vec::new()));
                    continue;
                }
            }
            Some(b'{') => {
                i += 1;
                skip_ws(&mut i);
                if b.get(i) == Some(&b'}') {
                    i += 1;
                    Value::Object(Map::new())
                } else {
                    let key = string(&mut i)?;
                    skip_ws(&mut i);
                    if b.get(i) != Some(&b':') {
                        return Err(unexpected(i));
                    }
                    i += 1;
                    stack.push(Frame::Object(Map::new(), key));
                    continue;
                }
            }
            Some(b'"') => Value::String(string(&mut i)?),
            Some(b'-' | b'0'..=b'9') => {
                let start = i;
                while matches!(
                    b.get(i),
                    Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
                ) {
                    i += 1;
                }
                match serde_json::from_str::<Value>(&text[start..i]) {
                    Ok(n @ Value::Number(_)) => n,
                    Ok(_) => return Err(unexpected(start)),
                    Err(e) => return Err(e.to_string()),
                }
            }
            _ => {
                let lit = [
                    ("true", Value::Bool(true)),
                    ("false", Value::Bool(false)),
                    ("null", Value::Null),
                ]
                .into_iter()
                .find(|(word, _)| text[i..].starts_with(word));
                let Some((word, v)) = lit else {
                    return Err(unexpected(i));
                };
                i += word.len();
                v
            }
        };
        // Hand the finished value to its container, closing every container
        // it completes, until one expects another element.
        loop {
            skip_ws(&mut i);
            match stack.last_mut() {
                None => {
                    return if i == b.len() {
                        Ok(value)
                    } else {
                        Err(unexpected(i))
                    }
                }
                Some(Frame::Array(items)) => {
                    items.push(value);
                    match b.get(i) {
                        Some(b',') => {
                            i += 1;
                            break;
                        }
                        Some(b']') => {
                            i += 1;
                            let Some(Frame::Array(items)) = stack.pop() else {
                                unreachable!()
                            };
                            value = Value::Array(items);
                        }
                        _ => return Err(unexpected(i)),
                    }
                }
                Some(Frame::Object(map, key)) => {
                    // A repeated key keeps its first position and takes the
                    // last value, as it does in JavaScript.
                    map.insert(std::mem::take(key), value);
                    match b.get(i) {
                        Some(b',') => {
                            i += 1;
                            skip_ws(&mut i);
                            *key = string(&mut i)?;
                            skip_ws(&mut i);
                            if b.get(i) != Some(&b':') {
                                return Err(unexpected(i));
                            }
                            i += 1;
                            break;
                        }
                        Some(b'}') => {
                            i += 1;
                            let Some(Frame::Object(map, _)) = stack.pop() else {
                                unreachable!()
                            };
                            value = Value::Object(property_order(map));
                        }
                        _ => return Err(unexpected(i)),
                    }
                }
            }
        }
    }
}

/// `path.resolve`: absolute against the working directory, with `.` and `..`
/// applied to the text rather than looked up on disk, so `missing/../data`
/// is `data`.
fn resolve_path(p: &Path) -> PathBuf {
    let base = if p.is_absolute() {
        PathBuf::new()
    } else {
        std::env::current_dir().unwrap_or_default()
    };
    let mut out = PathBuf::from("/");
    for c in base.components().chain(p.components()) {
        match c {
            Component::Normal(s) => out.push(s),
            Component::ParentDir => {
                out.pop();
            }
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
        }
    }
    out
}

/// Where `eval/paths.mjs` puts the ledger: `$TRIBUNAL_DATA_DIR/metrics.jsonl`
/// when that is set and non-empty, else `~/.claude/tribunal-data/metrics.jsonl`,
/// resolved as `path.resolve` resolves it.
pub fn metrics_path_from(data_dir: Option<&OsStr>, home: &Path) -> PathBuf {
    resolve_path(&match data_dir.filter(|d| !d.is_empty()) {
        Some(dir) => Path::new(dir).join("metrics.jsonl"),
        None => home
            .join(".claude")
            .join("tribunal-data")
            .join("metrics.jsonl"),
    })
}

/// [`metrics_path_from`] with the process environment.
pub fn metrics_path() -> PathBuf {
    let home = crate::proc::home_dir()
        .map(PathBuf::from)
        .unwrap_or_default();
    metrics_path_from(std::env::var_os("TRIBUNAL_DATA_DIR").as_deref(), &home)
}

/// Ports `eval/rows.mjs` `allRows`: one value per non-blank line, unparseable
/// and falsy ones dropped, a ledger that is missing, unreadable or too large
/// for a V8 string read as empty. Keys
/// come back in the order `JSON.parse` gives them.
pub fn all_rows(path: &Path) -> Vec<Value> {
    let Some(text) = read_file_utf8(path) else {
        return Vec::new();
    };
    text.split('\n')
        .map(tribunal_core::js::trim)
        .filter(|t| !t.is_empty())
        .filter_map(|t| parse_json(t).ok())
        .filter(|v| truthy(Some(v)))
        .collect()
}

/// Ports `readMetricsRows`: rows of PR `pr`, and of `repo` when one is given
/// (a row without a repo still matches).
pub fn read_metrics_rows(pr: f64, repo: Option<&str>, rows: Vec<Value>) -> Vec<Value> {
    rows.into_iter()
        .filter(|row| {
            let Value::Object(m) = row else { return false };
            let pr_matches =
                matches!(m.get("pr"), Some(Value::Number(n)) if n.as_f64() == Some(pr));
            let repo_matches = match repo {
                None => true,
                Some(repo) => {
                    !truthy(m.get("repo"))
                        || matches!(m.get("repo"), Some(Value::String(r)) if r == repo)
                }
            };
            pr_matches && repo_matches
        })
        .collect()
}

/// What the script prints and how it exits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// The script's main path: `stdin` is its input, `ledger` the metrics file.
///
/// A `TypeError` ends the run with code 1 and V8's message as `TypeError: …`
/// on stderr; Node also prints the source line and a stack there, which this
/// does not. An invalid stdin names this parser's error, not V8's.
///
/// `JSON.parse` takes any nesting depth, and dropping or printing a deeply
/// nested `Value` recurses, so the work runs on a thread with a large stack.
pub fn run(stdin: &str, ledger: &Path) -> Run {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(1 << 30)
            .spawn_scoped(s, || run_on_this_thread(stdin, ledger))
            .expect("spawn the resolve-baseline thread")
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e))
    })
}

fn run_on_this_thread(stdin: &str, ledger: &Path) -> Run {
    let mut stderr = String::new();
    let fail = |stderr: String, stdout: String, msg: String| Run {
        code: 1,
        stdout,
        stderr: format!("{stderr}TypeError: {msg}\n"),
    };
    let input: Value = match parse_json(stdin) {
        Ok(v) => v,
        Err(e) => {
            return Run {
                code: 1,
                stdout: String::new(),
                stderr: format!("resolve-baseline: stdin is not valid JSON: {e}\n"),
            }
        }
    };
    let pr = match prop(&input, "pr") {
        Err(msg) => return fail(stderr, String::new(), msg),
        Ok(Some(Value::Number(n))) => n.as_f64().filter(|x| x.is_finite()),
        Ok(_) => None,
    };
    let Some(pr) = pr else {
        return Run {
            code: 1,
            stdout: String::new(),
            stderr: "resolve-baseline: input must carry a numeric \"pr\"\n".into(),
        };
    };
    let field = |key: &str| input.get(key);
    let repo = match field("repo") {
        Some(Value::String(r)) if !r.is_empty() => Some(r.as_str()),
        _ => None,
    };
    if repo.is_none() {
        stderr.push_str("resolve-baseline: no \"repo\" given, matching on the PR number alone\n");
    }
    let rows = read_metrics_rows(pr, repo, all_rows(ledger));
    let reviews = match field("reviews") {
        Some(Value::Array(a)) => a.as_slice(),
        _ => &[],
    };
    let empty = Value::String(String::new());
    let head_sha = or(field("headSha"), &empty);
    let since = or(field("since"), &empty);
    let decision = match resolve_baseline(&rows, reviews, head_sha, since) {
        Ok(d) => d,
        Err(msg) => return fail(stderr, String::new(), msg),
    };
    let stdout = format!("{}\n", json_stringify(Some(&decision)));
    let found = truthy(decision.get("found"));
    let reason = to_str(decision.get("reason")).unwrap_or_default();
    if found {
        // `String(decision.sha)` runs after stdout is written, so a throw
        // here still leaves the decision on stdout.
        let sha = match to_str(decision.get("sha")) {
            Ok(s) => s,
            Err(msg) => return fail(stderr, stdout, msg),
        };
        let source = to_str(decision.get("source")).unwrap_or_default();
        let count = decision
            .get("findings")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        stderr.push_str(&format!(
            "baseline: {source} at {} \u{2014} {reason} ({count} finding(s))\n",
            utf16_prefix_lossy(&sha, 8)
        ));
    } else {
        stderr.push_str(&format!("baseline: none \u{2014} {reason}\n"));
    }
    Run {
        code: 0,
        stdout,
        stderr,
    }
}
