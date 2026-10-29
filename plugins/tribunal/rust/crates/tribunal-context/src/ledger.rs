//! Ports of `skills/review-pr/scripts/locate-last-row.mjs` and
//! `skills/review-pr/scripts/ledger-lock.mjs`, with the measurement paths
//! `skills/review-pr/eval/paths.mjs` resolves (`DATA_DIR`, `METRICS_PATH`,
//! the two backstops, `PROJECTS_DIR`, `projectDir()`).
//!
//! The paths come from a [`LedgerEnv`] instead of the process environment, so
//! a test points the ledger at a temp directory without touching
//! `~/.claude/tribunal-data`. The argument splitting every posting script
//! shares (`parseArgs`) lives here too.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use tribunal_core::js;
use tribunal_core::report::js_ext::{strict_eq, to_js_string};

use crate::operator::{node_fs_message, path_normalize, CliOutput};

/// One value of the scripts' `parseArgs`: a flag with no value is `true`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    Flag,
    Value(String),
}

impl Arg {
    /// The JavaScript value: `true` or the string.
    #[must_use]
    pub fn value(&self) -> Value {
        match self {
            Arg::Flag => Value::Bool(true),
            Arg::Value(s) => Value::String(s.clone()),
        }
    }

    /// `String(arg)`.
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Arg::Flag => "true".into(),
            Arg::Value(s) => s.clone(),
        }
    }

    #[must_use]
    pub fn truthy(&self) -> bool {
        match self {
            Arg::Flag => true,
            Arg::Value(s) => !s.is_empty(),
        }
    }

    /// `Number(arg)`.
    #[must_use]
    pub fn number(&self) -> f64 {
        match self {
            Arg::Flag => 1.0,
            Arg::Value(s) => js::string_to_number(s),
        }
    }
}

pub type Args = HashMap<String, Arg>;

/// Ports `parseArgs`: `--key value`, or `--key` alone when the next argument
/// is missing or starts with `--`. Anything else is skipped; a repeated key
/// keeps its last value.
#[must_use]
pub fn parse_args(argv: &[String]) -> Args {
    let mut out = Args::new();
    let mut i = 0;
    while i < argv.len() {
        if let Some(key) = argv[i].strip_prefix("--") {
            match argv.get(i + 1) {
                Some(next) if !next.starts_with("--") => {
                    out.insert(key.to_owned(), Arg::Value(next.clone()));
                    i += 1;
                }
                _ => {
                    out.insert(key.to_owned(), Arg::Flag);
                }
            }
        }
        i += 1;
    }
    out
}

/// `path.posix.resolve(cwd, ...parts)`.
#[must_use]
pub fn path_resolve(cwd: &str, parts: &[&str]) -> String {
    let mut acc = String::new();
    for p in parts.iter().rev().filter(|p| !p.is_empty()) {
        acc = if acc.is_empty() {
            (*p).to_owned()
        } else {
            format!("{p}/{acc}")
        };
        if p.starts_with('/') {
            break;
        }
    }
    if !acc.starts_with('/') {
        acc = if acc.is_empty() {
            cwd.to_owned()
        } else {
            format!("{cwd}/{acc}")
        };
    }
    let n = path_normalize(&acc);
    match n.strip_suffix('/') {
        Some(s) if !s.is_empty() => s.to_owned(),
        _ => n,
    }
}

/// What `paths.mjs` reads from the process: `os.homedir()`, `process.cwd()`,
/// `TRIBUNAL_DATA_DIR` and `TRIBUNAL_PROJECT_DIR` (an empty variable counts
/// as unset, as `||` does).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerEnv {
    pub home: String,
    pub cwd: String,
    pub data_dir: Option<String>,
    pub project_dir: Option<String>,
    /// How long to wait for the ledger lock: [`lock_wait`] in production.
    pub lock_wait: Duration,
}

impl LedgerEnv {
    #[must_use]
    pub fn from_process() -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Self {
            home: crate::proc::home_dir().unwrap_or_default(),
            cwd: std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            data_dir: var("TRIBUNAL_DATA_DIR"),
            project_dir: var("TRIBUNAL_PROJECT_DIR"),
            lock_wait: lock_wait(),
        }
    }

    fn resolve(&self, parts: &[&str]) -> String {
        path_resolve(&self.cwd, parts)
    }

    #[must_use]
    pub fn data_dir(&self) -> String {
        match self.data_dir.as_deref().filter(|d| !d.is_empty()) {
            Some(d) => self.resolve(&[d]),
            None => self.resolve(&[&self.home, ".claude", "tribunal-data"]),
        }
    }

    #[must_use]
    pub fn metrics_path(&self) -> String {
        self.resolve(&[&self.data_dir(), "metrics.jsonl"])
    }

    #[must_use]
    pub fn agent_costs_path(&self) -> String {
        self.resolve(&[&self.data_dir(), "agent-costs.jsonl"])
    }

    #[must_use]
    pub fn agent_counts_path(&self) -> String {
        self.resolve(&[&self.data_dir(), "agent-counts.jsonl"])
    }

    #[must_use]
    pub fn projects_dir(&self) -> String {
        self.resolve(&[&self.home, ".claude", "projects"])
    }

    /// Ports `projectDir()`: `TRIBUNAL_PROJECT_DIR` as given, else the
    /// cwd's slug under `PROJECTS_DIR`.
    #[must_use]
    pub fn default_project_dir(&self) -> String {
        if let Some(d) = self.project_dir.as_deref().filter(|d| !d.is_empty()) {
            return d.to_owned();
        }
        let slug: String = self
            .resolve(&[])
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        self.resolve(&[&self.projects_dir(), &slug])
    }

    /// Where a path the script names is on disk: relative paths are relative
    /// to `cwd`, not to this process's directory.
    #[must_use]
    pub fn fs_path(&self, p: &str) -> PathBuf {
        if p.starts_with('/') {
            PathBuf::from(p)
        } else {
            PathBuf::from(&self.cwd).join(p)
        }
    }
}

const SCAN_ROWS: usize = 50;

/// The located row, the file's non-blank lines, and where the row sits in
/// them.
#[derive(Debug, Clone, PartialEq)]
pub struct Located {
    pub row: Value,
    pub lines: Vec<String>,
    pub line_index: usize,
}

/// `{ pr, headSha, repo }` as the callers pass them. `pr` and `head_sha` are
/// whatever the argument was (`true` for a bare flag).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RowQuery {
    pub pr: Option<Value>,
    pub head_sha: Option<Value>,
    pub repo: Option<String>,
}

fn null_read(prop: &str) -> String {
    format!("Cannot read properties of null (reading '{prop}')")
}

/// Ports `mismatch`. `Err` is the TypeError a `null` row throws.
fn mismatch(row: &Value, q: &RowQuery) -> Result<Option<String>, String> {
    if let Some(pr) = &q.pr {
        if row.is_null() {
            return Err(null_read("pr"));
        }
        let have = js::get(row, "pr");
        if to_js_string(have) != to_js_string(Some(pr)) {
            return Ok(Some(format!(
                "refusing: last row is pr {}, expected {}",
                to_js_string(have),
                to_js_string(Some(pr))
            )));
        }
    }
    if let Some(repo) = q.repo.as_deref().filter(|r| !r.is_empty()) {
        if row.is_null() {
            return Err(null_read("repo"));
        }
        let have = js::get(row, "repo");
        if js::truthy(have) && !strict_eq(have, Some(&Value::String(repo.to_owned()))) {
            return Ok(Some(format!(
                "refusing: last row is repo {}, expected {repo}",
                to_js_string(have)
            )));
        }
    }
    if let Some(sha) = q.head_sha.as_ref().filter(|s| js::truthy(Some(s))) {
        if row.is_null() {
            return Err(null_read("headSha"));
        }
        let have = js::get(row, "headSha");
        if !strict_eq(have, Some(sha)) {
            return Ok(Some(format!(
                "refusing: last row headSha {} does not match --head-sha {}",
                to_js_string(have),
                to_js_string(Some(sha))
            )));
        }
    }
    Ok(None)
}

/// Ports `locateLastRow`: the newest of the last 50 rows that belongs to the
/// run, else the refusal the newest row gives. `metrics_path` is printed in
/// messages and read as given.
///
/// # Errors
/// The `RowMismatchError` message, or the TypeError a `null` row throws.
pub fn locate_last_row(metrics_path: &str, q: &RowQuery) -> Result<Located, String> {
    let text = crate::operator::read_file_utf8(metrics_path)
        .map_err(|e| format!("refusing: could not read {metrics_path}: {e}"))?;
    let lines: Vec<String> = text
        .split('\n')
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect();
    if lines.is_empty() {
        return Err(format!("refusing: {metrics_path} is empty"));
    }
    let last = lines.len() - 1;
    let mut newest: Option<String> = None;
    for i in (lines.len().saturating_sub(SCAN_ROWS)..lines.len()).rev() {
        let row = match crate::operator::json_parse(&lines[i]) {
            Ok(row) => row,
            Err(e) if i == last => {
                return Err(format!(
                    "refusing: last line of {metrics_path} does not parse: {e}"
                ))
            }
            Err(_) => continue,
        };
        match mismatch(&row, q)? {
            None => {
                return Ok(Located {
                    row,
                    lines,
                    line_index: i,
                })
            }
            Some(why) => {
                newest.get_or_insert(why);
            }
        }
    }
    Err(newest.unwrap_or_default())
}

/// The script's command line: `node locate-last-row.mjs [--pr <n>]
/// [--head-sha <sha>]`, printing the located row as JSON.
#[must_use]
pub fn run(argv: &[String], env: &LedgerEnv) -> CliOutput {
    let args = parse_args(argv);
    let q = RowQuery {
        pr: args.get("pr").map(Arg::value),
        head_sha: args.get("head-sha").map(Arg::value),
        repo: None,
    };
    match locate_last_row(&env.metrics_path(), &q) {
        Ok(found) => CliOutput {
            stdout: format!("{}\n", js::stringify(&found.row, "")),
            stderr: String::new(),
            code: 0,
        },
        Err(e) => CliOutput {
            stdout: String::new(),
            stderr: format!("{e}\n"),
            code: 1,
        },
    }
}

pub const WAIT: Duration = Duration::from_millis(15_000);
pub const STALE: Duration = Duration::from_millis(60_000);
const POLL: Duration = Duration::from_millis(50);

/// The lock wait `TRIBUNAL_LEDGER_LOCK_WAIT_MS` asks for, read as the script
/// reads it: a positive integer of milliseconds, else [`WAIT`]. The tests that
/// hold the lock on purpose shorten it.
#[must_use]
pub fn lock_wait_from(var: Option<&str>) -> Duration {
    var.filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) && !v.starts_with('0'))
        .and_then(|v| v.parse().ok())
        .map_or(WAIT, Duration::from_millis)
}

/// [`lock_wait_from`] on this process's environment.
#[must_use]
pub fn lock_wait() -> Duration {
    lock_wait_from(
        std::env::var("TRIBUNAL_LEDGER_LOCK_WAIT_MS")
            .ok()
            .as_deref(),
    )
}

/// Ports `withLedgerLock`: a `<path>.lock` directory held around `f`, waiting
/// up to `wait`.
///
/// # Errors
/// A `mkdir` failure other than EEXIST, in Node's words, or the timeout.
pub fn with_ledger_lock<T>(path: &str, wait: Duration, f: impl FnOnce() -> T) -> Result<T, String> {
    with_ledger_lock_timed(path, wait, STALE, f)
}

/// [`with_ledger_lock`] with its wait and staleness limits as parameters. A
/// lock whose stat or removal fails is retried at once, as the script's
/// `continue` does.
///
/// # Errors
/// As [`with_ledger_lock`].
pub fn with_ledger_lock_timed<T>(
    path: &str,
    wait: Duration,
    stale: Duration,
    f: impl FnOnce() -> T,
) -> Result<T, String> {
    let lock = format!("{path}.lock");
    let deadline = SystemTime::now() + wait;
    loop {
        match std::fs::create_dir(&lock) {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let age = std::fs::metadata(&lock)
                    .and_then(|m| m.modified())
                    .map(|t| SystemTime::now().duration_since(t).unwrap_or_default());
                match age {
                    Ok(age) if age > stale => {
                        let _ = std::fs::remove_dir(&lock);
                        continue;
                    }
                    Ok(_) => {}
                    Err(_) => continue,
                }
                if SystemTime::now() > deadline {
                    return Err(format!(
                        "could not lock {path} within {} ms",
                        wait.as_millis()
                    ));
                }
                std::thread::sleep(POLL);
            }
            Err(e) => return Err(node_fs_message(&e, "mkdir", Some(&lock))),
        }
    }
    let out = f();
    let _ = std::fs::remove_dir(&lock);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_split_as_the_scripts_split_them() {
        let argv: Vec<String> = ["x", "--pr", "--posted", "true", "--pr", "7", "--v", ""]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let a = parse_args(&argv);
        assert_eq!(a["pr"], Arg::Value("7".into()));
        assert_eq!(a["posted"], Arg::Value("true".into()));
        assert_eq!(a["v"], Arg::Value(String::new()));
    }

    #[test]
    fn resolve_matches_node() {
        assert_eq!(path_resolve("/a/b", &["c/", "../d"]), "/a/b/d");
        assert_eq!(path_resolve("/a", &["/x/", "y"]), "/x/y");
        assert_eq!(path_resolve("/", &[]), "/");
    }

    #[test]
    fn the_lock_wait_takes_only_a_positive_integer() {
        let ms = Duration::from_millis;
        assert_eq!(WAIT, ms(15_000));
        assert_eq!(lock_wait_from(Some("300")), ms(300));
        for v in [
            None,
            Some(""),
            Some("0"),
            Some("007"),
            Some("-5"),
            Some("1.5"),
            Some("1e3"),
            Some(" 300"),
            Some("abc"),
            Some("99999999999999999999999"),
        ] {
            assert_eq!(lock_wait_from(v), WAIT, "{v:?}");
        }
    }

    #[test]
    fn the_slug_replaces_slashes_dots_and_underscores() {
        let env = LedgerEnv {
            home: "/h".into(),
            cwd: "/w/a_b.c".into(),
            data_dir: None,
            project_dir: None,
            lock_wait: WAIT,
        };
        assert_eq!(env.default_project_dir(), "/h/.claude/projects/-w-a-b-c");
        assert_eq!(env.metrics_path(), "/h/.claude/tribunal-data/metrics.jsonl");
    }
}
