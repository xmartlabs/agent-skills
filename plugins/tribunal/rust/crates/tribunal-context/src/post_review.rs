//! Port of `skills/review-pr/scripts/post-review.mjs`: strip the draft
//! comments, put the verdict banner on top and the footer below, post through
//! `gh`, re-fetch the posted body and fail hard if a "Draft comments" section
//! got through, then print the PR URL. `--find-review-id <login>` prints the
//! newest review id by that author instead.
//!
//! Every `gh` call goes through [`Tools`]. JavaScript semantics kept: a
//! failing `gh` (non-zero, unstartable, or past `execFileSync`'s 1 MiB
//! `maxBuffer`) is an uncaught error, which removes the body file on its way
//! out, while the leak check's `process.exit(1)` skips the `finally` and
//! leaves it. `--find-review-id` sorts with V8's comparator-driven
//! insertion sort, so a review whose `submitted_at` does not parse lands
//! where V8 puts it.

use std::cmp::Ordering;
use std::path::PathBuf;

use serde_json::Value;
use tribunal_core::evidence::js_ext::{object_prototype_property, utf16_cmp};
use tribunal_core::js;
use tribunal_core::report::draft_comments_strip::strip_draft_comments_from_body;
use tribunal_core::report::js_ext::{strict_eq, to_js_string, to_number};

use crate::exec::Tools;
use crate::ledger::{parse_args, Arg, Args};
use crate::operator::{json_parse, node_fs_message, CliOutput};

/// `execFileSync`'s default `maxBuffer`.
const MAX_BUFFER: usize = 1024 * 1024;

/// Where the script would run: `process.cwd()` and `os.tmpdir()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostEnv {
    pub cwd: PathBuf,
    pub tmp_dir: PathBuf,
}

impl PostEnv {
    #[must_use]
    pub fn from_process() -> Self {
        let tmp = std::env::temp_dir();
        let tmp = tmp
            .to_str()
            .and_then(|s| s.strip_suffix('/'))
            .filter(|s| !s.is_empty())
            .map_or(tmp.clone(), PathBuf::from);
        Self {
            cwd: std::env::current_dir().unwrap_or_default(),
            tmp_dir: tmp,
        }
    }

    fn fs_path(&self, p: &str) -> PathBuf {
        if p.starts_with('/') {
            PathBuf::from(p)
        } else {
            self.cwd.join(p)
        }
    }
}

/// The review state a post submits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Approve,
    RequestChanges,
    Comment,
}

impl Verdict {
    /// `VERDICT_BANNERS[verdict]`.
    #[must_use]
    pub fn banner(self) -> &'static str {
        match self {
            Verdict::Approve => "### ✅ Approved",
            Verdict::RequestChanges => "### 🔴 Changes requested",
            Verdict::Comment => "### 💬 Comment",
        }
    }
}

/// Ports `composeBody`: banner, the report without its draft comments and
/// trailing whitespace, then the trimmed footer.
#[must_use]
pub fn compose_body(raw_report: &str, footer_text: &str, verdict: Verdict) -> String {
    let stripped = strip_draft_comments_from_body(raw_report);
    let stripped = stripped.trim_end_matches(js::is_js_whitespace);
    format!(
        "{}\n\n{stripped}\n\n{}\n",
        verdict.banner(),
        js::trim(footer_text)
    )
}

/// One `execFileSync("gh", args, { encoding: "utf8" })`. gh's stderr goes to
/// the script's own stderr, as the inherited stream does.
async fn gh(
    tools: &Tools,
    args: &[&str],
    env: &PostEnv,
    stderr: &mut String,
) -> Result<String, String> {
    let out = match tools.gh(args, Some(&env.cwd)).await {
        Ok(out) => out,
        Err(e) => {
            let code = crate::exec::spawn_error_code(&e);
            return Err(format!("spawnSync gh {code}"));
        }
    };
    stderr.push_str(&out.stderr);
    if out.stdout.len() + out.stderr.len() > MAX_BUFFER {
        return Err("spawnSync gh ENOBUFS".into());
    }
    if !out.ok() {
        return Err(format!("Command failed: gh {}", args.join(" ")));
    }
    Ok(out.stdout)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn time_clip(t: f64) -> f64 {
    if !t.is_finite() || t.abs() > 8.64e15 {
        f64::NAN
    } else {
        t.trunc() + 0.0
    }
}

/// `Date.parse` for `YYYY-MM-DDTHH:MM:SS[.fff…]Z`, the shape GitHub sends.
/// Every other string is NaN here (see the README's declared differences).
fn parse_iso(s: &str) -> f64 {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let part = b.get(r)?;
        part.iter()
            .all(u8::is_ascii_digit)
            .then(|| std::str::from_utf8(part).ok()?.parse().ok())
            .flatten()
    };
    let sep = |i: usize, c: u8| b.get(i) == Some(&c);
    if b.len() < 20
        || !(sep(4, b'-') && sep(7, b'-') && sep(10, b'T') && sep(13, b':') && sep(16, b':'))
    {
        return f64::NAN;
    }
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(sec)) = (
        num(0..4),
        num(5..7),
        num(8..10),
        num(11..13),
        num(14..16),
        num(17..19),
    ) else {
        return f64::NAN;
    };
    let mut ms = 0i64;
    let mut i = 19;
    if sep(19, b'.') {
        let start = 20;
        i = start;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return f64::NAN;
        }
        let frac = &s[start..i.min(start + 3)];
        ms = format!("{frac:0<3}").parse().unwrap_or(0);
    }
    if i + 1 != b.len() || b[i] != b'Z' {
        return f64::NAN;
    }
    let valid = (1..=12).contains(&mo)
        && (1..=31).contains(&d)
        && (h < 24 && mi < 60 && sec < 60 || h == 24 && mi == 0 && sec == 0 && ms == 0);
    if !valid {
        return f64::NAN;
    }
    let days = days_from_civil(y, mo, 1) + d - 1;
    #[allow(clippy::cast_precision_loss)]
    time_clip((days * 86_400_000 + h * 3_600_000 + mi * 60_000 + sec * 1000 + ms) as f64)
}

/// `new Date(v).valueOf()` for a JSON value (`None` is `undefined`).
fn date_value(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Value::String(s)) => parse_iso(s),
        Some(v @ (Value::Array(_) | Value::Object(_))) => parse_iso(&to_js_string(Some(v))),
        Some(v) => time_clip(to_number(Some(v))),
    }
}

/// V8's `Array.prototype.sort` below 64 elements: the leading run (reversed
/// when strictly descending), then binary insertion of the rest. A NaN from
/// `cmp` counts as "not less" everywhere. Above that
/// a stable merge sort stands in, which agrees whenever `cmp` is consistent.
fn v8_sort<T: Clone>(v: &mut [T], cmp: impl Fn(&T, &T) -> f64) {
    let n = v.len();
    if n < 2 {
        return;
    }
    if n >= 64 {
        v.sort_by(|a, b| {
            let o = cmp(a, b);
            if o < 0.0 {
                Ordering::Less
            } else if o > 0.0 {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        });
        return;
    }
    let mut run = 2;
    let descending = cmp(&v[1], &v[0]) < 0.0;
    let mut prev = 1;
    for idx in 2..n {
        let order = cmp(&v[idx], &v[prev]);
        // Torque's `order >= 0` is `!(order < 0)`: a NaN ends a descending run.
        if (order < 0.0) != descending {
            break;
        }
        prev = idx;
        run += 1;
    }
    if descending {
        v[..run].reverse();
    }
    for start in run..n {
        let pivot = v[start].clone();
        let (mut left, mut right) = (0, start);
        while left < right {
            let mid = left + ((right - left) >> 1);
            if cmp(&pivot, &v[mid]) < 0.0 {
                right = mid;
            } else {
                left = mid + 1;
            }
        }
        v[left..=start].rotate_right(1);
    }
}

fn null_read(prop: &str) -> String {
    format!("Cannot read properties of null (reading '{prop}')")
}

fn as_array<'a>(v: &'a Value, method: &str) -> Result<&'a Vec<Value>, String> {
    match v {
        Value::Array(a) => Ok(a),
        _ => Err(format!("reviews.{method} is not a function")),
    }
}

/// Ports `findReviewId`: the id of the newest review by `author`, `None` when
/// there is none (or its id is `null`), printed as `String(id)`.
///
/// # Errors
/// What the script dies on: a failed `gh`, unparseable JSON, a non-array
/// answer, a `null` review.
pub async fn find_review_id(
    tools: &Tools,
    env: &PostEnv,
    pr: &str,
    repo: &str,
    author: &Value,
    stderr: &mut String,
) -> Result<Option<String>, String> {
    let raw = gh(
        tools,
        &["api", &format!("repos/{repo}/pulls/{pr}/reviews")],
        env,
        stderr,
    )
    .await?;
    let reviews = json_parse(&raw)?;
    let mut matches = Vec::new();
    for r in as_array(&reviews, "filter")? {
        if r.is_null() {
            return Err(null_read("user"));
        }
        let user = js::get(r, "user");
        if js::truthy(user) && strict_eq(user.and_then(|u| js::get(u, "login")), Some(author)) {
            matches.push(r);
        }
    }
    if matches.is_empty() {
        return Ok(None);
    }
    v8_sort(&mut matches, |a, b| {
        date_value(js::get(b, "submitted_at")) - date_value(js::get(a, "submitted_at"))
    });
    Ok(match js::get(matches[0], "id") {
        Some(Value::Null) => None,
        id => Some(to_js_string(id)),
    })
}

fn usage(code: i32, message: &str) -> CliOutput {
    CliOutput {
        stdout: String::new(),
        stderr: message.to_owned(),
        code,
    }
}

fn read_utf8(env: &PostEnv, arg: &Arg) -> Result<String, String> {
    let Arg::Value(p) = arg else {
        return Err("The \"path\" argument must be of type string or an instance of Buffer or URL. Received type boolean (true)".into());
    };
    let bytes = std::fs::read(env.fs_path(p)).map_err(|e| {
        let syscall = if e.kind() == std::io::ErrorKind::IsADirectory {
            "read"
        } else {
            "open"
        };
        node_fs_message(&e, syscall, (syscall == "open").then_some(p.as_str()))
    })?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

enum Mode {
    Overwrite(String),
    Post(Verdict, &'static str),
}

/// The script's command line: `node post-review.mjs --pr <n> --repo <o/r>
/// (--find-review-id <login> | --report-file <f> --footer-file <f>
/// (--approve|--request-changes|--comment|--overwrite <id>))`.
pub async fn run(argv: &[String], tools: &Tools, env: &PostEnv) -> CliOutput {
    let args = parse_args(argv);
    let mut stderr = String::new();
    let result = main(&args, tools, env, &mut stderr).await;
    match result {
        Ok(mut out) => {
            stderr.push_str(&out.stderr);
            out.stderr = stderr;
            out
        }
        Err(e) => {
            stderr.push_str(&e);
            stderr.push('\n');
            CliOutput::crash(stderr)
        }
    }
}

async fn main(
    args: &Args,
    tools: &Tools,
    env: &PostEnv,
    stderr: &mut String,
) -> Result<CliOutput, String> {
    let truthy = |k: &str| args.get(k).is_some_and(Arg::truthy);
    if !truthy("pr") || !truthy("repo") {
        return Ok(usage(2, "usage: --pr <n> --repo <owner/repo> [--find-review-id <login> | --report-file <path> (--approve|--request-changes|--comment|--overwrite <id>)]\n"));
    }
    let pr = args["pr"].text();
    let repo = args["repo"].text();

    if let Some(login) = args.get("find-review-id").filter(|a| a.truthy()) {
        return Ok(
            match find_review_id(tools, env, &pr, &repo, &login.value(), stderr).await? {
                Some(id) => CliOutput {
                    stdout: format!("{id}\n"),
                    stderr: String::new(),
                    code: 0,
                },
                None => usage(
                    1,
                    &format!("no review found by {} on PR #{pr}\n", login.text()),
                ),
            },
        );
    }

    if !truthy("report-file") {
        return Ok(usage(2, "--report-file is required to post\n"));
    }
    let mode = if truthy("overwrite") {
        Mode::Overwrite(args["overwrite"].text())
    } else if truthy("approve") {
        Mode::Post(Verdict::Approve, "--approve")
    } else if truthy("request-changes") {
        Mode::Post(Verdict::RequestChanges, "--request-changes")
    } else if truthy("comment") {
        Mode::Post(Verdict::Comment, "--comment")
    } else {
        return Ok(usage(
            2,
            "one of --approve, --request-changes, --comment, --overwrite <id> is required\n",
        ));
    };

    if !truthy("footer-file") {
        return Ok(usage(2, "--footer-file is required — build it with scripts/build-post-footer.mjs; every posted review ends with a metadata footer, never hand-typed\n"));
    }
    let footer = match read_utf8(env, &args["footer-file"]) {
        Ok(t) => js::trim(&t).to_owned(),
        Err(e) => return Ok(usage(2, &format!("--footer-file could not be read: {e}\n"))),
    };
    if footer.is_empty() {
        return Ok(usage(
            2,
            "--footer-file is empty — refusing to post without a footer\n",
        ));
    }

    let verdict = match &mode {
        Mode::Post(v, _) => *v,
        Mode::Overwrite(id) => {
            let endpoint = format!("repos/{repo}/pulls/{pr}/reviews/{id}");
            let raw = gh(tools, &["api", &endpoint], env, stderr).await?;
            let existing = json_parse(&raw)?;
            if existing.is_null() {
                return Err(null_read("state"));
            }
            let key = to_js_string(js::get(&existing, "state"));
            match key.as_str() {
                "APPROVED" => Verdict::Approve,
                "CHANGES_REQUESTED" => Verdict::RequestChanges,
                "COMMENTED" => Verdict::Comment,
                k if object_prototype_property(k).is_some() => {
                    read_utf8(env, &args["report-file"])?;
                    return Err(format!("buildVerdictBanner: unknown verdict \"{k}\""));
                }
                _ => {
                    let msg = format!(
                        "cannot determine verdict banner: review {id} has state \"{key}\"\n"
                    );
                    return Ok(usage(1, &msg));
                }
            }
        }
    };

    let raw_report = read_utf8(env, &args["report-file"])?;
    let body = compose_body(&raw_report, &footer, verdict);
    let tmp = env.tmp_dir.to_string_lossy();
    let body_file = format!("{tmp}/post-review-body-{}.md", std::process::id());
    std::fs::write(&body_file, &body).map_err(|e| node_fs_message(&e, "open", Some(&body_file)))?;

    let posted = post_and_check(tools, env, &mode, &pr, &repo, &body_file, stderr).await;
    match posted {
        Ok(Some(url)) => {
            let _ = std::fs::remove_file(&body_file);
            Ok(CliOutput {
                stdout: format!("{url}\n"),
                stderr: String::new(),
                code: 0,
            })
        }
        Ok(None) => Ok(usage(1, "FATAL: posted review body still contains a 'Draft comments' section — strip step failed or was bypassed\n")),
        Err(e) => {
            let _ = std::fs::remove_file(&body_file);
            Err(e)
        }
    }
}

/// Posts, re-fetches, checks for the leak, reads the URL. `Ok(None)` is the
/// leak.
async fn post_and_check(
    tools: &Tools,
    env: &PostEnv,
    mode: &Mode,
    pr: &str,
    repo: &str,
    body_file: &str,
    stderr: &mut String,
) -> Result<Option<String>, String> {
    match mode {
        Mode::Overwrite(id) => {
            let field = format!("body=@{body_file}");
            gh(
                tools,
                &[
                    "api",
                    &format!("repos/{repo}/pulls/{pr}/reviews/{id}"),
                    "-X",
                    "PUT",
                    "-F",
                    &field,
                ],
                env,
                stderr,
            )
            .await?;
        }
        Mode::Post(_, flag) => {
            gh(
                tools,
                &[
                    "pr",
                    "review",
                    pr,
                    "--repo",
                    repo,
                    "--body-file",
                    body_file,
                    flag,
                ],
                env,
                stderr,
            )
            .await?;
        }
    }

    let raw = gh(
        tools,
        &["api", &format!("repos/{repo}/pulls/{pr}/reviews")],
        env,
        stderr,
    )
    .await?;
    let reviews = json_parse(&raw)?;
    let target: Option<&Value> = match mode {
        Mode::Overwrite(id) => {
            let mut found = None;
            for r in as_array(&reviews, "find")? {
                if r.is_null() {
                    return Err(null_read("id"));
                }
                if to_js_string(js::get(r, "id")) == *id {
                    found = Some(r);
                    break;
                }
            }
            found
        }
        Mode::Post(..) => {
            let mut max: Option<&Value> = None;
            for r in as_array(&reviews, "reduce")? {
                max = match max {
                    Some(m) if js::truthy(Some(m)) => {
                        if r.is_null() {
                            return Err(null_read("id"));
                        }
                        if js_greater(js::get(r, "id"), js::get(m, "id")) {
                            Some(r)
                        } else {
                            Some(m)
                        }
                    }
                    _ => Some(r),
                };
            }
            max
        }
    };
    let posted_body = match target.filter(|t| js::truthy(Some(t))) {
        Some(t) => match js::get(t, "body").filter(|b| js::truthy(Some(b))) {
            Some(b) => to_js_string(Some(b)),
            None => String::new(),
        },
        None => String::new(),
    };
    if posted_body.contains("Draft comments") {
        return Ok(None);
    }

    let url = gh(
        tools,
        &[
            "pr", "view", pr, "--repo", repo, "--json", "url", "--jq", ".url",
        ],
        env,
        stderr,
    )
    .await?;
    Ok(Some(js::trim(&url).to_owned()))
}

/// `a > b` on two JSON values: both through `ToPrimitive`, then compared as
/// strings (by UTF-16 units) when both are strings, else as numbers.
fn js_greater(a: Option<&Value>, b: Option<&Value>) -> bool {
    let prim = |v: Option<&Value>| -> Option<Value> {
        match v {
            Some(v @ (Value::Array(_) | Value::Object(_))) => {
                Some(Value::String(to_js_string(Some(v))))
            }
            v => v.cloned(),
        }
    };
    let (pa, pb) = (prim(a), prim(b));
    if let (Some(Value::String(x)), Some(Value::String(y))) = (&pa, &pb) {
        return utf16_cmp(x, y) == Ordering::Greater;
    }
    to_number(pa.as_ref()) > to_number(pb.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_banner_goes_first_and_the_draft_comments_go() {
        let body = compose_body(
            "## Report\n\n### Draft comments\n- x\n  \n",
            "  <sub>f</sub>\n",
            Verdict::Approve,
        );
        assert_eq!(body, "### ✅ Approved\n\n## Report\n\n<sub>f</sub>\n");
    }

    #[test]
    fn iso_dates_parse_as_v8_does() {
        assert_eq!(parse_iso("2026-10-22T10:00:00.5Z"), 1_792_663_200_500.0);
        assert_eq!(parse_iso("2026-02-30T00:00:00Z"), 1_772_409_600_000.0);
        assert_eq!(parse_iso("2026-10-22T24:00:00Z"), 1_792_713_600_000.0);
        assert!(parse_iso("2026-13-01T00:00:00Z").is_nan());
        assert!(parse_iso("2026-10-22T10:00:60Z").is_nan());
        assert_eq!(date_value(Some(&json!(null))), 0.0);
        assert!(date_value(None).is_nan());
    }

    #[test]
    fn a_nan_comparator_sorts_as_v8_does() {
        let mut v = vec![3.0, f64::NAN, 1.0, 2.0];
        v8_sort(&mut v, |a, b| b - a);
        assert_eq!(format!("{v:?}"), "[3.0, NaN, 2.0, 1.0]");
    }
}
