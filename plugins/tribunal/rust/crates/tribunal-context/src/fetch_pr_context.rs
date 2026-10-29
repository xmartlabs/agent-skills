//! Port of `skills/review-pr/scripts/fetch-pr-context.mjs` in its in-context
//! form: the PR's `gh` calls, run in parallel, assembled into one
//! `{context, raw, errors}` object.
//!
//! The `--relay` form and the `--prep` it carries are not ported: they exist to
//! hand this object to a helper agent. The pinned checkout `--prep` builds is
//! [`crate::head_worktree`].
//!
//! Values read from `gh` keep their JSON types where the script passes them
//! through (`title`, `state`, a comment's `line`), and a property the script
//! reads as `undefined` is left out of the object, as `JSON.stringify` does.

use std::path::Path;

use serde_json::{Map, Value};
use tribunal_core::evidence::js_ext::{
    number_value, property_order_deep, string_to_number, to_js_string, to_number,
};
use tribunal_core::evidence::prior_reviews::is_bot_login;
use tribunal_core::js;
use tribunal_core::profile::js_ext::json_stringify;

use crate::exec::{Output, Tools};

/// `DIFF_FIELD_MIN_FILES`: below this many files `context.diff` stays empty.
pub const DIFF_FIELD_MIN_FILES: usize = 13;
/// `DIFF_FIELD_MAX_BYTES`: above this many bytes `context.diff` stays empty.
pub const DIFF_FIELD_MAX_BYTES: u64 = 60000;

const VIEW_FIELDS: &str =
    "title,body,author,baseRefName,baseRefOid,headRefName,headRefOid,files,additions,deletions,reviews";

/// What `fetchPrContext` takes. `tmpdir` is `process.env.TMPDIR || "/tmp"`,
/// which [`default_tmpdir`] reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchOptions {
    pub pr_number: String,
    pub repo: String,
    pub head_sha: String,
    pub thread_before: String,
    pub with_bot_comments: bool,
    pub with_diff_text: bool,
    pub deploy_branches: Vec<String>,
    pub tmpdir: String,
}

/// `process.env.TMPDIR || "/tmp"`.
pub fn default_tmpdir() -> String {
    std::env::var("TMPDIR")
        .ok()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(crate::proc::fallback_tmpdir)
}

/// One `gh` answer as `runGh` resolves it: `Ok(stdout)` or `Err(detail)`.
type GhResult = Result<String, String>;

/// `runGh`: a failure carries the trimmed stderr, or Node's error message when
/// stderr is blank, cut to 300 UTF-16 units.
pub async fn run_gh(tools: &Tools, args: &[&str]) -> GhResult {
    match tools.gh(args, None).await {
        Ok(out) => gh_result(args, out),
        Err(e) => Err(slice_utf16(&format!("spawn gh {}", errno_name(&e)), 300)),
    }
}

fn gh_result(args: &[&str], out: Output) -> GhResult {
    if out.ok() {
        return Ok(out.stdout);
    }
    let stderr = js::trim(&out.stderr);
    // execFile's message names the command as it was given, `gh`, whatever
    // binary Tools resolved it to.
    let detail = if stderr.is_empty() {
        let mut cmd = "gh".to_owned();
        if !args.is_empty() {
            cmd.push(' ');
            cmd.push_str(&args.join(" "));
        }
        format!("Command failed: {cmd}\n{}", out.stderr)
    } else {
        stderr.to_owned()
    };
    Err(slice_utf16(&detail, 300))
}

/// `s.slice(0, n)` over UTF-16 units. A surrogate pair the cut would split is
/// dropped whole: a Rust `String` cannot hold the lone half JavaScript keeps.
fn slice_utf16(s: &str, n: usize) -> String {
    let mut units = 0;
    let mut out = String::new();
    for c in s.chars() {
        units += c.len_utf16();
        if units > n {
            break;
        }
        out.push(c);
    }
    out
}

fn errno_name(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(2) => "ENOENT".into(),
        Some(13) => "EACCES".into(),
        Some(20) => "ENOTDIR".into(),
        Some(21) => "EISDIR".into(),
        Some(n) => format!("errno {n}"),
        None => e.to_string(),
    }
}

#[cfg(target_os = "linux")]
const ENAMETOOLONG: i32 = 36;
#[cfg(not(target_os = "linux"))]
const ENAMETOOLONG: i32 = 63;
#[cfg(target_os = "linux")]
const ELOOP: i32 = 40;
#[cfg(not(target_os = "linux"))]
const ELOOP: i32 = 62;

/// libuv's name and message for an errno a file write can raise.
fn uv_error(errno: i32) -> Option<(&'static str, &'static str)> {
    Some(match errno {
        1 => ("EPERM", "operation not permitted"),
        2 => ("ENOENT", "no such file or directory"),
        5 => ("EIO", "i/o error"),
        13 => ("EACCES", "permission denied"),
        16 => ("EBUSY", "resource busy or locked"),
        17 => ("EEXIST", "file already exists"),
        20 => ("ENOTDIR", "not a directory"),
        21 => ("EISDIR", "illegal operation on a directory"),
        22 => ("EINVAL", "invalid argument"),
        23 => ("ENFILE", "file table overflow"),
        24 => ("EMFILE", "too many open files"),
        26 => ("ETXTBSY", "text file is busy"),
        27 => ("EFBIG", "file too large"),
        28 => ("ENOSPC", "no space left on device"),
        30 => ("EROFS", "read-only file system"),
        ENAMETOOLONG => ("ENAMETOOLONG", "name too long"),
        ELOOP => ("ELOOP", "too many symbolic links encountered"),
        _ => return None,
    })
}

/// Node's message for a failed `fs` call: `ENOENT: no such file or
/// directory, open '<path>'`.
fn fs_error_message(e: &std::io::Error, syscall: &str, path: &str) -> String {
    match e.raw_os_error().and_then(uv_error) {
        Some((code, text)) => format!("{code}: {text}, {syscall} '{path}'"),
        None => format!("{e}, {syscall} '{path}'"),
    }
}

/// V8's `TypeError` for `String(x)` or `Number(x)` on an object whose own
/// `toString` cannot be called, which is every `toString` key JSON can give.
const NO_PRIMITIVE: &str = "Cannot convert object to primitive value";

/// `String(v)`, which throws where JavaScript's does.
pub(crate) fn js_string(v: &Value) -> Result<String, String> {
    match v {
        Value::Object(m) if m.contains_key("toString") => Err(NO_PRIMITIVE.into()),
        Value::Array(a) => {
            let mut parts = Vec::with_capacity(a.len());
            for e in a {
                parts.push(if e.is_null() {
                    String::new()
                } else {
                    js_string(e)?
                });
            }
            Ok(parts.join(","))
        }
        other => Ok(to_js_string(other)),
    }
}

/// `String(v || "")`.
fn js_string_or_empty(v: Option<&Value>) -> Result<String, String> {
    match v {
        Some(v) if js::truthy(Some(v)) => js_string(v),
        _ => Ok(String::new()),
    }
}

/// `Number(v)`, which throws where JavaScript's does.
fn js_number(v: Option<&Value>) -> Result<f64, String> {
    match v {
        Some(Value::Object(m)) if m.contains_key("toString") => Err(NO_PRIMITIVE.into()),
        Some(a @ Value::Array(_)) => Ok(string_to_number(&js_string(a)?)),
        other => Ok(to_number(other)),
    }
}

/// `path.join(dir, name)` for a `name` with no separator: the POSIX
/// normalization of `dir/name`.
pub fn node_path_join(dir: &str, name: &str) -> String {
    let joined = if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    };
    let absolute = joined.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| *p != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let body = parts.join("/");
    match (absolute, body.is_empty()) {
        (true, _) => format!("/{body}"),
        (false, true) => ".".into(),
        (false, false) => body,
    }
}

/// `login(u)`: an object's truthy `login`, else a truthy non-object as is,
/// else `""`.
fn login(u: Option<&Value>) -> Value {
    match u {
        Some(v @ (Value::Object(_) | Value::Array(_))) => match js::get(v, "login") {
            Some(l) if js::truthy(Some(l)) => l.clone(),
            _ => Value::String(String::new()),
        },
        Some(v) if js::truthy(Some(v)) => v.clone(),
        _ => Value::String(String::new()),
    }
}

/// `BOT_LOGIN.test(x)`, which reads `String(x)`.
fn is_bot(v: &Value) -> Result<bool, String> {
    Ok(is_bot_login(&js_string(v)?))
}

/// `x || fallback` on a property read.
fn or_value(v: Option<&Value>, fallback: Value) -> Value {
    match v {
        Some(x) if js::truthy(Some(x)) => x.clone(),
        _ => fallback,
    }
}

fn or_empty(v: Option<&Value>) -> Value {
    or_value(v, Value::String(String::new()))
}

/// Sets `key` unless the value is `undefined`, as `JSON.stringify` drops it.
fn put(m: &mut Map<String, Value>, key: &str, v: Option<&Value>) {
    if let Some(v) = v {
        m.insert(key.to_owned(), v.clone());
    }
}

/// `jiraTicketOf(title, body)`: the first `/\b[A-Z]{2,}[A-Z0-9]*-\d+\b/` in the
/// title, else in the body. The body is converted only when the title has no
/// key.
///
/// # Errors
/// Where `String(title)` or `String(body)` throws.
pub fn jira_ticket_of(title: Option<&Value>, body: Option<&Value>) -> Result<String, String> {
    if let Some(key) = ticket_key(&js_string_or_empty(title)?) {
        return Ok(key);
    }
    Ok(ticket_key(&js_string_or_empty(body)?).unwrap_or_default())
}

fn is_word(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The pattern can only match one way from a given start: the `[A-Z0-9]` run
/// is taken whole (no char of it is `-`), and the digit run must end where a
/// word boundary is, which backtracking into the digits cannot create.
fn ticket_key(s: &str) -> Option<String> {
    let chars: Vec<char> = s.chars().collect();
    let at = |i: usize| chars.get(i).copied();
    for i in 0..chars.len() {
        if i > 0 && is_word(at(i - 1)) {
            continue;
        }
        if !(at(i).is_some_and(|c| c.is_ascii_uppercase())
            && at(i + 1).is_some_and(|c| c.is_ascii_uppercase()))
        {
            continue;
        }
        let mut j = i + 2;
        while at(j).is_some_and(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
            j += 1;
        }
        if at(j) != Some('-') {
            continue;
        }
        let mut k = j + 1;
        while at(k).is_some_and(|c| c.is_ascii_digit()) {
            k += 1;
        }
        if k == j + 1 || is_word(at(k)) {
            continue;
        }
        return Some(chars[i..k].iter().collect());
    }
    None
}

/// `postedBefore(at, cutoff)`. With a cutoff, an item whose timestamp does not
/// parse is dropped.
///
/// # Errors
/// Where `Date.parse` throws converting `at` to a string.
pub fn posted_before(at: Option<&Value>, cutoff: &str) -> Result<bool, String> {
    if cutoff.is_empty() {
        return Ok(true);
    }
    let t = date_parse(&js_string_or_empty(at)?);
    Ok(t.is_finite() && t < date_parse(cutoff))
}

/// `Date.parse` for the ECMAScript date-time string format as V8 reads it:
/// `YYYY`, `±YYYYYY`, optional `-MM` and `-DD`, and a `T`/`t` time with
/// `Z`/`z` or a `±HH:mm`/`±HHmm` offset. A day up to 31 is accepted in any
/// month and rolls over, `24:00` is midnight next day, and a fraction of any
/// length is cut to milliseconds. NaN for anything else, including the forms
/// V8 reads as local time or hands to its legacy parser (declared differences).
pub fn date_parse(s: &str) -> f64 {
    parse_iso(s.as_bytes()).unwrap_or(f64::NAN)
}

fn digits(b: &[u8], i: &mut usize, n: usize) -> Option<i64> {
    let slice = b.get(*i..*i + n)?;
    if !slice.iter().all(u8::is_ascii_digit) {
        return None;
    }
    *i += n;
    Some(slice.iter().fold(0, |a, d| a * 10 + i64::from(d - b'0')))
}

fn parse_iso(b: &[u8]) -> Option<f64> {
    let mut i = 0;
    let year = match b.first()? {
        sign @ (b'+' | b'-') => {
            i = 1;
            let y = digits(b, &mut i, 6)?;
            if *sign == b'-' {
                if y == 0 {
                    return None;
                }
                -y
            } else {
                y
            }
        }
        _ => digits(b, &mut i, 4)?,
    };
    let mut month = 1;
    let mut day = 1;
    if b.get(i) == Some(&b'-') {
        i += 1;
        month = digits(b, &mut i, 2)?;
        if b.get(i) == Some(&b'-') {
            i += 1;
            day = digits(b, &mut i, 2)?;
        }
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut ms_of_day = 0i64;
    if matches!(b.get(i), Some(b'T' | b't')) {
        i += 1;
        let hour = digits(b, &mut i, 2)?;
        if b.get(i) != Some(&b':') {
            return None;
        }
        i += 1;
        let minute = digits(b, &mut i, 2)?;
        let mut second = 0;
        let mut milli = 0;
        if b.get(i) == Some(&b':') {
            i += 1;
            second = digits(b, &mut i, 2)?;
            if b.get(i) == Some(&b'.') {
                i += 1;
                let start = i;
                while b.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
                if i == start {
                    return None;
                }
                milli = v8_milliseconds(&b[start..i]);
            }
        }
        if hour > 24 || minute > 59 || second > 59 {
            return None;
        }
        if hour == 24 && (minute != 0 || second != 0 || milli != 0) {
            return None;
        }
        let offset = match b.get(i) {
            Some(b'Z' | b'z') => {
                i += 1;
                0
            }
            Some(sign @ (b'+' | b'-')) => {
                let negative = *sign == b'-';
                i += 1;
                let oh = digits(b, &mut i, 2)?;
                if b.get(i) == Some(&b':') {
                    i += 1;
                }
                let om = digits(b, &mut i, 2)?;
                if oh > 23 || om > 59 {
                    return None;
                }
                let o = (oh * 60 + om) * 60_000;
                if negative {
                    -o
                } else {
                    o
                }
            }
            // Local time: V8 reads it with the host's time zone.
            _ => return None,
        };
        ms_of_day = ((hour * 60 + minute) * 60 + second) * 1000 + milli - offset;
    }
    if i != b.len() {
        return None;
    }
    let t = days_from_civil(year, month, day) as f64 * 86_400_000.0 + ms_of_day as f64;
    (t.abs() <= 8.64e15).then_some(t)
}

/// V8's `ReadMilliseconds`: the scanner keeps the first nine digits after any
/// leading zeros, while the length counts every digit and is capped at nine,
/// so `.0691423760` reads as 691 ms, not 69.
fn v8_milliseconds(frac: &[u8]) -> i64 {
    let significant = frac.iter().skip_while(|d| **d == b'0');
    let number = significant
        .take(9)
        .fold(0i64, |a, d| a * 10 + i64::from(d - b'0'));
    match frac.len().min(9) {
        1 => number * 100,
        2 => number * 10,
        3 => number,
        len => number / 10i64.pow(len as u32 - 3),
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date; a day past the end of
/// its month rolls into the next, as `MakeDay` does.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn null_read(prop: &str) -> String {
    format!("Cannot read properties of null (reading '{prop}')")
}

/// A property read that throws on `null` as JavaScript does. Other non-objects
/// read as `undefined`.
fn read<'a>(v: &'a Value, prop: &str) -> Result<Option<&'a Value>, String> {
    if v.is_null() {
        return Err(null_read(prop));
    }
    Ok(js::get(v, prop))
}

/// `slurpedPages`: `gh api --paginate --slurp` is an array of pages, each
/// flattened one level.
fn slurped_pages(v: Value) -> Vec<Value> {
    match v {
        Value::Array(pages) => pages
            .into_iter()
            .flat_map(|p| match p {
                Value::Array(items) => items,
                other => vec![other],
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn parse_json(s: &str) -> Option<Value> {
    serde_json::from_str::<Value>(s)
        .ok()
        .map(property_order_deep)
}

struct Errors(Vec<Value>);

impl Errors {
    fn fail(&mut self, step: &str, detail: &str) {
        let detail = if detail.is_empty() {
            "unparseable output"
        } else {
            detail
        };
        let mut m = Map::new();
        m.insert("step".into(), Value::String(step.into()));
        m.insert("detail".into(), Value::String(detail.into()));
        self.0.push(Value::Object(m));
    }

    /// `parse(step, res, JSON.parse)`.
    fn parse(&mut self, step: &str, res: GhResult) -> Option<Value> {
        match res {
            Err(d) => {
                self.fail(step, &d);
                None
            }
            Ok(s) => match parse_json(&s) {
                Some(v) => Some(v),
                None => {
                    self.fail(step, "unparseable output");
                    None
                }
            },
        }
    }
}

/// Ports `fetchPrContext`. `Err` where the script throws instead of reporting:
/// a `null` file, review or comment entry in the `gh` output.
pub async fn fetch_pr_context(tools: &Tools, o: &FetchOptions) -> Result<Value, String> {
    let pr = o.pr_number.as_str();
    let repo = o.repo.as_str();
    let pinned = !o.head_sha.is_empty();
    let view_args = ["pr", "view", pr, "--repo", repo, "--json", VIEW_FIELDS];
    let inline_path = format!("repos/{repo}/pulls/{pr}/comments");
    let thread_path = format!("repos/{repo}/issues/{pr}/comments");
    let inline_args = ["api", "--paginate", "--slurp", inline_path.as_str()];
    let thread_args = ["api", "--paginate", "--slurp", thread_path.as_str()];
    let viewer_args = ["api", "user", "--jq", ".login"];
    let diff_args = ["pr", "diff", pr, "--repo", repo];

    // Unpinned, the diff starts with everything else. Pinned, the two compare
    // calls need the base commit, which only the view returns.
    let view_then_compare = async {
        let view = run_gh(tools, &view_args).await;
        if !pinned {
            return (view, None);
        }
        let base = view
            .as_ref()
            .ok()
            .and_then(|s| parse_json(s))
            .and_then(|v| {
                js::get(&v, "baseRefOid")
                    .filter(|b| js::truthy(Some(b)))
                    .cloned()
            });
        let Some(base) = base else {
            return (view, Some(Compare::NoBase));
        };
        // The template literal throws before either compare call starts.
        let base = match js_string(&base) {
            Ok(b) => b,
            Err(e) => return (view, Some(Compare::Throws(e))),
        };
        let range = format!("repos/{repo}/compare/{base}...{}", o.head_sha);
        let diff_args = [
            "api",
            range.as_str(),
            "-H",
            "Accept: application/vnd.github.v3.diff",
        ];
        let files_args = ["api", range.as_str()];
        let (d, c) = tokio::join!(run_gh(tools, &diff_args), run_gh(tools, &files_args));
        (view, Some(Compare::Ran(d, c)))
    };
    let unpinned_diff = async {
        if pinned {
            None
        } else {
            Some(run_gh(tools, &diff_args).await)
        }
    };
    let ((view_res, compare), diff_unpinned, inline_res, thread_res, viewer_res) = tokio::join!(
        view_then_compare,
        unpinned_diff,
        run_gh(tools, &inline_args),
        run_gh(tools, &thread_args),
        run_gh(tools, &viewer_args),
    );

    let mut errors = Errors(Vec::new());
    let view = errors
        .parse("pr-view", view_res)
        .filter(|v| js::truthy(Some(v)))
        .unwrap_or_else(|| Value::Object(Map::new()));

    let mut compare_files: Option<Vec<Value>> = None;
    let diff_res: GhResult = match (pinned, compare) {
        (_, Some(Compare::Throws(e))) => return Err(e),
        (true, Some(Compare::Ran(d, c))) => {
            if let Some(Value::Array(files)) = errors
                .parse("compare-files", c)
                .as_ref()
                .and_then(|cmp| js::get(cmp, "files"))
            {
                compare_files = Some(files.clone());
            }
            d
        }
        (true, _) => Err("no baseRefOid from pr-view, cannot build the pinned compare".into()),
        (false, _) => diff_unpinned.expect("the unpinned diff ran"),
    };

    let diff_path = node_path_join(&o.tmpdir, &format!("peer-review-diff-{pr}.patch"));
    let mut written_path = String::new();
    let mut diff_bytes: Option<u64> = None;
    let mut diff_text = String::new();
    match diff_res {
        // The path is recorded once the write succeeds, before the size is
        // read back, as the script assigns it.
        Ok(text) => match std::fs::write(Path::new(&diff_path), text.as_bytes()) {
            Err(e) => errors.fail("diff-write", &fs_error_message(&e, "open", &diff_path)),
            Ok(()) => {
                written_path = diff_path.clone();
                match std::fs::metadata(&diff_path) {
                    Ok(m) => {
                        diff_bytes = Some(m.len());
                        diff_text = text;
                    }
                    Err(e) => errors.fail("diff-write", &fs_error_message(&e, "stat", &diff_path)),
                }
            }
        },
        Err(d) => errors.fail(if pinned { "compare-diff" } else { "pr-diff" }, &d),
    }

    // (path, additions, deletions) per file, each `undefined` when absent.
    let empty = Vec::new();
    let (source, path_key): (&Vec<Value>, &str) = if pinned {
        (compare_files.as_ref().unwrap_or(&empty), "filename")
    } else {
        match js::get(&view, "files") {
            Some(Value::Array(a)) => (a, "path"),
            Some(other) if js::truthy(Some(other)) => {
                return Err("(view.files || []).map is not a function".into())
            }
            _ => (&empty, "path"),
        }
    };
    let mut rows: Vec<FileRow> = Vec::new();
    for f in source {
        rows.push(FileRow {
            path: read(f, path_key)?.cloned(),
            additions: js::get(f, "additions").cloned(),
            deletions: js::get(f, "deletions").cloned(),
        });
    }
    let files: Vec<Value> = rows
        .iter()
        .filter_map(|r| r.path.clone())
        .filter(|p| js::truthy(Some(p)))
        .collect();
    let sum = |pick: fn(&FileRow) -> Option<&Value>| -> Result<f64, String> {
        let mut a = 0.0;
        for r in &rows {
            a += or_zero(js_number(pick(r))?);
        }
        Ok(a)
    };
    let (additions, deletions) = if pinned {
        (
            sum(|r| r.additions.as_ref())?,
            sum(|r| r.deletions.as_ref())?,
        )
    } else {
        (
            or_zero(js_number(js::get(&view, "additions"))?),
            or_zero(js_number(js::get(&view, "deletions"))?),
        )
    };
    let finite = |v: &Option<Value>| {
        v.as_ref()
            .and_then(Value::as_f64)
            .is_some_and(f64::is_finite)
    };
    let file_churn: Vec<Value> = if rows
        .iter()
        .all(|r| finite(&r.additions) && finite(&r.deletions))
    {
        rows.iter()
            .map(|r| {
                let mut m = Map::new();
                put(&mut m, "file", r.path.as_ref());
                let count =
                    |v: &Option<Value>| v.as_ref().and_then(Value::as_f64).unwrap_or_default();
                let churn = count(&r.additions) + count(&r.deletions);
                m.insert("churn".into(), number_value(churn));
                Value::Object(m)
            })
            .collect()
    } else {
        Vec::new()
    };

    let reviews_all: &[Value] = match js::get(&view, "reviews") {
        Some(Value::Array(a)) => a,
        _ => &[],
    };
    let mut reviews: Vec<&Value> = Vec::new();
    for r in reviews_all {
        if posted_before(read(r, "submittedAt")?, &o.thread_before)? {
            reviews.push(r);
        }
    }
    let comments = |res: Option<Value>| -> Result<Vec<Value>, String> {
        let mut kept = Vec::new();
        for c in res.map(slurped_pages).unwrap_or_default() {
            if posted_before(read(&c, "created_at")?, &o.thread_before)? {
                kept.push(c);
            }
        }
        Ok(kept)
    };
    let inline = comments(errors.parse_pages("inline-comments", inline_res))?;
    let thread = comments(errors.parse_pages("thread-comments", thread_res))?;
    let viewer = match viewer_res {
        Ok(s) => js::trim(&s).to_owned(),
        Err(d) => {
            errors.fail("viewer", &d);
            String::new()
        }
    };

    let user_of = |c: &Value| login(js::get(c, "user"));
    let author_of = |r: &Value| login(js::get(r, "author"));
    let at_of = |r: &Value, key: &str| or_empty(js::get(r, key));
    let has_text = |v: Option<&Value>| -> Result<bool, String> {
        Ok(!js::trim(&js_string_or_empty(v)?).is_empty())
    };

    let mut bot_comments: Vec<Value> = Vec::new();
    if o.with_bot_comments {
        for c in &inline {
            if !is_bot(&user_of(c))? {
                continue;
            }
            let mut m = Map::new();
            m.insert("user".into(), user_of(c));
            m.insert("path".into(), or_empty(js::get(c, "path")));
            let line = [js::get(c, "line"), js::get(c, "original_line")]
                .into_iter()
                .flatten()
                .find(|v| !v.is_null())
                .cloned()
                .unwrap_or(Value::Null);
            m.insert("line".into(), line);
            m.insert("body".into(), or_empty(js::get(c, "body")));
            bot_comments.push(Value::Object(m));
        }
        for r in &reviews {
            if !(is_bot(&author_of(r))? && has_text(js::get(r, "body"))?) {
                continue;
            }
            let mut m = Map::new();
            m.insert("user".into(), author_of(r));
            put(&mut m, "body", js::get(r, "body"));
            bot_comments.push(Value::Object(m));
        }
    }

    let prior_reviews: Vec<Value> = reviews
        .iter()
        .map(|r| {
            let mut m = Map::new();
            m.insert("author".into(), author_of(r));
            put(&mut m, "state", js::get(r, "state"));
            m.insert("at".into(), at_of(r, "submittedAt"));
            m.insert("summary".into(), Value::String(String::new()));
            Value::Object(m)
        })
        .collect();

    let file_count = files.len();
    let base_ref_name = js::get(&view, "baseRefName");
    let mut context = Map::new();
    context.insert("title".into(), or_empty(js::get(&view, "title")));
    context.insert("baseRefName".into(), or_empty(base_ref_name));
    context.insert(
        "headRefName".into(),
        or_empty(js::get(&view, "headRefName")),
    );
    context.insert(
        "headRefOid".into(),
        if pinned {
            Value::String(o.head_sha.clone())
        } else {
            or_empty(js::get(&view, "headRefOid"))
        },
    );
    context.insert("author".into(), author_of(&view));
    context.insert(
        "jiraTicket".into(),
        Value::String(jira_ticket_of(
            js::get(&view, "title"),
            js::get(&view, "body"),
        )?),
    );
    context.insert("fileCount".into(), Value::from(file_count));
    context.insert("additions".into(), number_value(additions));
    context.insert("deletions".into(), number_value(deletions));
    context.insert("files".into(), Value::Array(files));
    context.insert("fileChurn".into(), Value::Array(file_churn));
    context.insert("diffPath".into(), Value::String(written_path));
    context.insert(
        "diffBytes".into(),
        diff_bytes.map_or(Value::Null, Value::from),
    );
    let in_band = o.with_diff_text
        && file_count >= DIFF_FIELD_MIN_FILES
        && diff_bytes.is_some_and(|b| b <= DIFF_FIELD_MAX_BYTES);
    context.insert(
        "diff".into(),
        Value::String(if in_band { diff_text } else { String::new() }),
    );
    context.insert("botComments".into(), Value::Array(bot_comments));
    context.insert("viewer".into(), Value::String(viewer));
    context.insert("priorReviews".into(), Value::Array(prior_reviews));
    context.insert(
        "isDeploymentPR".into(),
        Value::Bool(
            matches!(base_ref_name, Some(Value::String(b)) if o.deploy_branches.contains(b)),
        ),
    );

    let view_author = author_of(&view);
    let description = if has_text(js::get(&view, "body"))? && !is_bot(&view_author)? {
        let mut m = Map::new();
        m.insert("author".into(), view_author.clone());
        put(&mut m, "body", js::get(&view, "body"));
        vec![Value::Object(m)]
    } else {
        Vec::new()
    };
    let changes_requested: Vec<Value> = reviews
        .iter()
        .enumerate()
        .filter(|(_, r)| matches!(js::get(r, "state"), Some(Value::String(s)) if s == "CHANGES_REQUESTED"))
        .map(|(index, r)| {
            let mut m = Map::new();
            m.insert("index".into(), Value::from(index));
            m.insert("author".into(), author_of(r));
            m.insert("at".into(), at_of(r, "submittedAt"));
            m.insert("body".into(), or_empty(js::get(r, "body")));
            Value::Object(m)
        })
        .collect();
    let mut human_reviews: Vec<Value> = Vec::new();
    for r in &reviews {
        if is_bot(&author_of(r))? || !has_text(js::get(r, "body"))? {
            continue;
        }
        let mut m = Map::new();
        m.insert("author".into(), author_of(r));
        put(&mut m, "state", js::get(r, "state"));
        m.insert("at".into(), at_of(r, "submittedAt"));
        put(&mut m, "body", js::get(r, "body"));
        human_reviews.push(Value::Object(m));
    }
    let mut thread_comments: Vec<Value> = Vec::new();
    for c in &thread {
        if is_bot(&user_of(c))? {
            continue;
        }
        let mut m = Map::new();
        m.insert("user".into(), user_of(c));
        m.insert("at".into(), at_of(c, "created_at"));
        m.insert("body".into(), or_empty(js::get(c, "body")));
        thread_comments.push(Value::Object(m));
    }
    let mut inline_comments: Vec<Value> = Vec::new();
    for c in &inline {
        if is_bot(&user_of(c))? {
            continue;
        }
        let mut m = Map::new();
        m.insert("user".into(), user_of(c));
        m.insert("path".into(), or_empty(js::get(c, "path")));
        m.insert("at".into(), at_of(c, "created_at"));
        m.insert("body".into(), or_empty(js::get(c, "body")));
        inline_comments.push(Value::Object(m));
    }

    let mut raw = Map::new();
    raw.insert("description".into(), Value::Array(description));
    raw.insert("changesRequested".into(), Value::Array(changes_requested));
    raw.insert("humanReviews".into(), Value::Array(human_reviews));
    raw.insert("threadComments".into(), Value::Array(thread_comments));
    raw.insert("inlineComments".into(), Value::Array(inline_comments));

    let mut out = Map::new();
    out.insert("context".into(), Value::Object(context));
    out.insert("raw".into(), Value::Object(raw));
    out.insert("errors".into(), Value::Array(errors.0));
    Ok(Value::Object(out))
}

impl Errors {
    /// `parse(step, res, slurpedPages)`: a parse failure is reported, a
    /// non-array is no comments.
    fn parse_pages(&mut self, step: &str, res: GhResult) -> Option<Value> {
        self.parse(step, res)
    }
}

/// How a pinned run's compare step went.
enum Compare {
    /// No truthy `baseRefOid`: no compare call is made.
    NoBase,
    /// Printing the base into the range threw, before either call.
    Throws(String),
    /// The diff and the files answers.
    Ran(GhResult, GhResult),
}

/// One entry of the script's `fileRows`; `None` is `undefined`.
struct FileRow {
    path: Option<Value>,
    additions: Option<Value>,
    deletions: Option<Value>,
}

/// `Number(x) || 0`.
fn or_zero(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 {
        0.0
    } else {
        x
    }
}

/// The script's `flag(name)`: the argument after the first `name`, unless it
/// is missing, empty or another flag.
fn flag<'a>(argv: &'a [String], name: &str) -> &'a str {
    match argv.iter().position(|a| a == name) {
        Some(i) => match argv.get(i + 1) {
            Some(v) if !v.is_empty() && !v.starts_with("--") => v,
            _ => "",
        },
        None => "",
    }
}

fn usage(detail: &str) -> String {
    let mut e = Map::new();
    e.insert("step".into(), Value::String("usage".into()));
    e.insert("detail".into(), Value::String(detail.into()));
    let mut m = Map::new();
    m.insert("context".into(), Value::Null);
    m.insert("raw".into(), Value::Null);
    m.insert("errors".into(), Value::Array(vec![Value::Object(e)]));
    json_stringify(Some(&Value::Object(m)))
}

/// The command line's options, or the one usage line the script prints
/// instead of running.
///
/// # Errors
/// The usage line, without its newline.
pub fn parse_cli(argv: &[String], tmpdir: &str) -> Result<FetchOptions, String> {
    let pr_number = flag(argv, "--pr");
    let repo = flag(argv, "--repo");
    let deploy_branches = argv
        .iter()
        .position(|a| a == "--deploy-branches")
        .and_then(|i| argv.get(i + 1))
        .filter(|v| !v.starts_with("--"))
        .map(|v| {
            v.split(',')
                .map(|b| js::trim(b).to_owned())
                .filter(|b| !b.is_empty())
                .collect::<Vec<_>>()
        });
    let thread_before = js::trim(flag(argv, "--thread-before")).to_owned();
    if !thread_before.is_empty() && !date_parse(&thread_before).is_finite() {
        return Err(usage(&format!(
            "--thread-before is not a date: {thread_before}"
        )));
    }
    let numeric = !pr_number.is_empty() && pr_number.bytes().all(|b| b.is_ascii_digit());
    let (true, false, Some(deploy_branches)) = (numeric, repo.is_empty(), deploy_branches) else {
        return Err(usage(
            "--pr <n> --repo <owner/name> --deploy-branches <a,b,c> required (the list comes from the project profile)",
        ));
    };
    Ok(FetchOptions {
        pr_number: pr_number.to_owned(),
        repo: repo.to_owned(),
        head_sha: js::trim(flag(argv, "--head-sha")).to_owned(),
        thread_before,
        with_bot_comments: argv.iter().any(|a| a == "--bot-comments"),
        with_diff_text: !argv.iter().any(|a| a == "--no-diff-text"),
        deploy_branches,
        tmpdir: tmpdir.to_owned(),
    })
}

/// What the script prints for `argv`, newline included: the usage line, or
/// the `{context, raw, errors}` line.
///
/// # Errors
/// Where the script crashes (see [`fetch_pr_context`]), and for `--relay`,
/// the helper-agent form this port leaves out.
pub async fn run_cli(tools: &Tools, argv: &[String], tmpdir: &str) -> Result<String, String> {
    let opts = match parse_cli(argv, tmpdir) {
        Ok(o) => o,
        Err(line) => return Ok(format!("{line}\n")),
    };
    if !flag(argv, "--relay").is_empty() {
        return Err("--relay is the helper-agent form and is not ported".into());
    }
    let out = fetch_pr_context(tools, &opts).await?;
    Ok(format!("{}\n", json_stringify(Some(&out))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_keys_need_two_letters_and_word_boundaries() {
        let s = |x: &str| Some(Value::String(x.into()));
        let key = |t: Option<Value>, b: Option<Value>| jira_ticket_of(t.as_ref(), b.as_ref());
        assert_eq!(key(s("fix ABC-12: x"), None), Ok("ABC-12".into()));
        assert_eq!(key(s("A-Z0-9"), s("see AB2-7.")), Ok("AB2-7".into()));
        assert_eq!(key(s("xABC-1 ABC-1x"), None), Ok(String::new()));
        // The body is converted only when the title has no key.
        let bad = Some(serde_json::json!({"toString": 1}));
        assert_eq!(key(s("ABC-1"), bad.clone()), Ok("ABC-1".into()));
        assert!(key(s("none"), bad).is_err());
    }

    #[test]
    fn iso_dates_parse_as_v8_does() {
        assert_eq!(date_parse("2026-10-22T10:00:00Z"), 1_792_663_200_000.0);
        assert_eq!(date_parse("2026-02-30T00:00:00Z"), 1_772_409_600_000.0);
        assert_eq!(date_parse("2026-10-22T24:00:00Z"), 1_792_713_600_000.0);
        assert!(date_parse("2026-10-22T10:00:00").is_nan());
        assert!(date_parse("+275760-09-13T00:00:00.001Z").is_nan());
    }

    #[test]
    fn path_join_normalizes() {
        assert_eq!(node_path_join("/tmp/", "a.patch"), "/tmp/a.patch");
        assert_eq!(node_path_join("/x//y/../z", "a"), "/x/z/a");
        assert_eq!(node_path_join("rel", "a"), "rel/a");
    }
}
