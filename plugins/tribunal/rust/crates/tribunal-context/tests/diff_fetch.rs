//! fetch-pr-context.mjs and the HEAD-WORKTREE region against their Rust
//! ports, on the same recorded `gh` and on twin git repositories.
//!
//! The script writes its patch under `$TMPDIR`, so each side gets its own
//! temporary directory, and the script runs through a small driver that sets
//! `TMPDIR` (and the shim's call log) before importing it as the main module.
//! Both outputs embed that directory; it is replaced by `<TMP>` on each side
//! before comparing. Worktree paths also carry mktemp's random suffix, which
//! is masked the same way, and the twin repositories' commits have fixed
//! dates, so their shas are the same on every run.
//!
//! The scripts' side of every case is read from
//! `tests/goldens/fetch-pr-context.json`, so a run starts only the port. It
//! holds three oracles, each with what it reads recorded under `recordedWith`:
//! `fetch` (the script run whole, its exports and `Date.parse`, keyed by the
//! case's name), `worktree` (the script's `prepareReview` running the
//! HEAD-WORKTREE region's bash in a twin repository, kept as the state it
//! left) and `region` (the region's functions through
//! `rust/oracle/region-oracle.mjs`). Inputs and outputs past 16 KiB, such as
//! the 60,000-byte diffs at the window edges, are kept as their SHA-256.
//! Editing a script, the region, a driver or the recorded `gh` without a
//! regeneration fails the oracle that reads it; once any file an oracle reads
//! is deleted, the oracle is retired, that check is skipped, and its recorded
//! outputs are final. Regenerate the whole binary at once, on a Mac, after
//! changing them, so that every case is recorded:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_fetch`.
mod common;
use common::*;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use serde_json::{json, Value};
use tribunal_context::exec::Tools;
use tribunal_context::fetch_pr_context as fpc;
use tribunal_context::harness_version::sha256_hex;
use tribunal_context::head_worktree as hw;

const SCRIPT: &str = "skills/review-pr/scripts/fetch-pr-context.mjs";
const EXTRACT: &str = "skills/review-pr/eval/extract-pure-region.mjs";
const REGION_ORACLE: &str = "rust/oracle/region-oracle.mjs";
const WORKFLOW: &str = "workflow-src/tribunal.js";
const REGION: &str = "HEAD-WORKTREE";
/// Inputs and outputs longer than this go into the golden as their SHA-256.
const BIG: usize = 16 * 1024;
const REPO: &str = "acme/widgets";
const VIEW_FIELDS: &str =
    "title,body,author,baseRefName,baseRefOid,headRefName,headRefOid,files,additions,deletions,reviews";
const BASE: &str = "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0";
const HEAD: &str = "cafecafecafecafecafecafecafecafecafecafe";
const PIN: &str = "feedfeedfeedfeedfeedfeedfeedfeedfeedfeed";

/// Runs the script as the main module with `TMPDIR` set (`-` unsets it) and
/// the shim's call log pointed at `log`.
const FETCH_DRIVER: &str = r#"
import { pathToFileURL } from "node:url";
const [tmp, log, script, ...rest] = process.argv.slice(2);
if (tmp === "-") delete process.env.TMPDIR; else process.env.TMPDIR = tmp;
if (log) process.env.GH_CALL_LOG = log;
process.argv = [process.argv[0], script, ...rest];
await import(pathToFileURL(script).href);
"#;

/// Calls the script's exports, and `Date.parse`, on a JSON list read from
/// stdin. NaN prints as null.
const EXPORTS_DRIVER: &str = r#"
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
const mod = await import(pathToFileURL(process.argv[2]).href);
const calls = JSON.parse(readFileSync(0, "utf8"));
const out = calls.map(([fn, ...args]) => (fn === "Date.parse" ? Date.parse(args[0]) : mod[fn](...args)));
process.stdout.write(JSON.stringify(out));
"#;

/// The script's own `--prep` worktree step: the region's bash written with
/// `$PREP_SHA` left in, run by `prepareReview` (which prunes stale checkouts
/// first). `prune <now>` runs `pruneStaleWorktrees` alone with that clock.
const WORKTREE_DRIVER: &str = r#"
import { writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
const [mode, plugin, tmp, log, ...rest] = process.argv.slice(2);
if (tmp === "-") delete process.env.TMPDIR; else process.env.TMPDIR = tmp;
if (log) process.env.GH_CALL_LOG = log;
const mod = await import(pathToFileURL(plugin + "/skills/review-pr/scripts/fetch-pr-context.mjs").href);
const { loadRegion } = await import(pathToFileURL(plugin + "/skills/review-pr/eval/extract-pure-region.mjs").href);
if (mode === "prep") {
  const [repo, pr, context, scriptPath] = rest;
  const { api } = loadRegion(plugin + "/workflow-src/tribunal.js", "HEAD-WORKTREE", ["headWorktreeBash"]);
  writeFileSync(scriptPath, api.headWorktreeBash("$PREP_SHA", repo, Number(pr)));
  const prep = await mod.prepareReview(JSON.parse(context), { worktreeScript: scriptPath, pr, tmpdir: tmp });
  process.stdout.write(JSON.stringify({ worktree: prep.worktree }));
} else {
  const removed = await mod.pruneStaleWorktrees({ now: Number(rest[0]) });
  process.stdout.write(JSON.stringify({ removed }));
}
"#;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| (*x).to_owned()).collect()
}

fn rec(args: &[String], stdout: &str, stderr: &str, code: i32) -> Value {
    json!({"args": args, "stdout": stdout, "stderr": stderr, "code": code})
}

fn view_args(pr: &str) -> Vec<String> {
    s(&["pr", "view", pr, "--repo", REPO, "--json", VIEW_FIELDS])
}
fn inline_args(pr: &str) -> Vec<String> {
    s(&[
        "api",
        "--paginate",
        "--slurp",
        &format!("repos/{REPO}/pulls/{pr}/comments"),
    ])
}
fn thread_args(pr: &str) -> Vec<String> {
    s(&[
        "api",
        "--paginate",
        "--slurp",
        &format!("repos/{REPO}/issues/{pr}/comments"),
    ])
}
fn viewer_args() -> Vec<String> {
    s(&["api", "user", "--jq", ".login"])
}
fn diff_args(pr: &str) -> Vec<String> {
    s(&["pr", "diff", pr, "--repo", REPO])
}
fn compare_diff_args(base: &str, head: &str) -> Vec<String> {
    s(&[
        "api",
        &format!("repos/{REPO}/compare/{base}...{head}"),
        "-H",
        "Accept: application/vnd.github.v3.diff",
    ])
}
fn compare_args(base: &str, head: &str) -> Vec<String> {
    s(&["api", &format!("repos/{REPO}/compare/{base}...{head}")])
}

/// The four calls every run makes, answered successfully.
fn common_calls(
    pr: &str,
    view: &Value,
    inline: &Value,
    thread: &Value,
    viewer: &str,
) -> Vec<Value> {
    vec![
        rec(&view_args(pr), &view.to_string(), "", 0),
        rec(&inline_args(pr), &inline.to_string(), "", 0),
        rec(&thread_args(pr), &thread.to_string(), "", 0),
        rec(&viewer_args(), &format!("{viewer}\n"), "", 0),
    ]
}

fn files(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(
                |i| json!({"path": format!("src/f{i}.ts"), "additions": i * 3, "deletions": i % 2}),
            )
            .collect(),
    )
}

fn reviews() -> Value {
    json!([
        {"author": {"login": "bob"}, "state": "CHANGES_REQUESTED", "body": "Please handle the null case.", "submittedAt": "2026-10-01T10:00:00Z"},
        {"author": {"login": "coderabbitai"}, "state": "COMMENTED", "body": "Actionable comments posted: 2", "submittedAt": "2026-10-01T09:00:00Z"},
        {"author": {"login": "carol"}, "state": "APPROVED", "body": "", "submittedAt": "2026-10-03T10:00:00Z"},
        {"author": {"login": "dave"}, "state": "COMMENTED", "body": "  \n", "submittedAt": "2026-10-04T10:00:00Z"},
        {"author": {"login": "erin"}, "body": "no state here", "submittedAt": "2026-10-05T10:00:00.5Z"},
        {"author": {"login": "bob"}, "state": "CHANGES_REQUESTED", "body": "", "submittedAt": "2026-10-06T10:00:00Z"},
        {"author": null, "state": "COMMENTED", "body": "ghost review"},
        {"author": {"login": "Dependabot"}, "state": "COMMENTED", "body": "bump", "submittedAt": "2026-10-02T10:00:00+02:00"},
    ])
}

fn view(nfiles: usize, base_ref: &str) -> Value {
    json!({
        "title": "Fix the widget cache",
        "body": "Fixes WID-42 for the cache.\n\nStacked on #12, see OPS-7.",
        "author": {"login": "alice"},
        "baseRefName": base_ref,
        "baseRefOid": BASE,
        "headRefName": "feature/cache",
        "headRefOid": HEAD,
        "files": files(nfiles),
        "additions": 30,
        "deletions": 4,
        "reviews": reviews(),
    })
}

fn inline_pages() -> Value {
    json!([
        [
            {"user": {"login": "coderabbitai[bot]"}, "path": "src/f1.ts", "line": null, "original_line": 12, "body": "nit: rename", "created_at": "2026-10-01T11:00:00Z"},
            {"user": {"login": "bob"}, "path": "src/f2.ts", "line": 3, "body": "why?", "created_at": "2026-10-02T11:00:00Z"}
        ],
        [
            {"user": {"login": "Renovate"}, "body": "", "created_at": "2026-10-07T11:00:00Z"},
            {"user": {"login": "alice"}, "path": "src/f2.ts", "body": "Because the cache is per request.", "created_at": "2026-10-08T11:00:00Z"},
            {"user": {"login": "some-bot[bot]"}, "path": "src/f3.ts", "body": "late bot", "created_at": "2026-10-09T11:00:00Z"}
        ]
    ])
}

fn thread_pages() -> Value {
    json!([[
        {"user": {"login": "github-actions"}, "body": "Coverage 80%", "created_at": "2026-10-01T08:00:00Z"},
        {"user": {"login": "frank"}, "body": "Is this stacked on #12?", "created_at": "2026-10-02T08:00:00Z"},
        {"user": {"login": "grace"}, "body": "no timestamp"},
        {"user": "heidi", "body": "user as a plain string", "created_at": "2026-10-03T08:00:00Z"}
    ]])
}

fn diff_text(n: usize) -> String {
    (0..n)
        .map(|i| format!("diff --git a/src/f{i}.ts b/src/f{i}.ts\n+line {i}\n"))
        .collect()
}

struct Side {
    out: Result<String, String>,
    calls: Vec<String>,
    patch: Option<Vec<u8>>,
}

fn read_calls(log: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    v.sort();
    v
}

fn write_driver(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    p
}

/// `TMPDIR` for one side: a path, or `None` to leave it unset (the Rust side
/// then gets the script's `/tmp` fallback).
#[derive(Clone)]
enum Tmp {
    Dir(String),
    Unset,
}

impl Tmp {
    fn arg(&self) -> String {
        match self {
            Tmp::Dir(d) => d.clone(),
            Tmp::Unset => "-".into(),
        }
    }
    fn resolved(&self) -> String {
        match self {
            Tmp::Dir(d) if !d.is_empty() => d.clone(),
            _ => "/tmp".into(),
        }
    }
}

fn patch_of(tmp: &Tmp, pr: &str) -> Option<Vec<u8>> {
    std::fs::read(fpc::node_path_join(
        &tmp.resolved(),
        &format!("peer-review-diff-{pr}.patch"),
    ))
    .ok()
}

async fn run_both(
    dir: &Path,
    calls: Vec<Value>,
    argv: &[String],
    js_tmp: Tmp,
    rs_tmp: Tmp,
    pr: &str,
) -> (Side, Side) {
    let rec_path = recordings(dir, calls);
    let js_side = run_js(dir, &rec_path, argv, &js_tmp, pr);
    let rs_side = run_rs(dir, &rec_path, argv, &rs_tmp, pr).await;
    (js_side, rs_side)
}

fn run_js(dir: &Path, rec_path: &Path, argv: &[String], js_tmp: &Tmp, pr: &str) -> Side {
    let driver = write_driver(dir, "fetch-driver.mjs", FETCH_DRIVER);
    let script = plugin_root().join(SCRIPT);
    let js_log = dir.join("js-calls.log");
    let _ = std::fs::remove_file(&js_log);

    let mut args = vec![
        js_tmp.arg(),
        js_log.to_string_lossy().into_owned(),
        script.to_string_lossy().into_owned(),
    ];
    args.extend(argv.iter().cloned());
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let js = run_script(driver.to_str().unwrap(), &arg_refs, dir, rec_path, None);
    let js_side = Side {
        out: if js.code == Some(0) {
            Ok(js.stdout)
        } else {
            Err(js.stderr)
        },
        calls: read_calls(&js_log),
        patch: patch_of(js_tmp, pr),
    };
    if let Tmp::Unset = js_tmp {
        let _ = std::fs::remove_file(format!("/tmp/peer-review-diff-{pr}.patch"));
    }
    js_side
}

async fn run_rs(dir: &Path, rec_path: &Path, argv: &[String], rs_tmp: &Tmp, pr: &str) -> Side {
    let rs_log = dir.join("rs-calls.log");
    let _ = std::fs::remove_file(&rs_log);
    let mut t: Tools = tools(rec_path);
    t.env
        .push(("GH_CALL_LOG".into(), rs_log.clone().into_os_string()));
    let out = fpc::run_cli(&t, argv, &rs_tmp.resolved()).await;
    let rs_side = Side {
        out,
        calls: read_calls(&rs_log),
        patch: patch_of(rs_tmp, pr),
    };
    if let Tmp::Unset = rs_tmp {
        let _ = std::fs::remove_file(format!("/tmp/peer-review-diff-{pr}.patch"));
    }
    rs_side
}

/// `d` with `.` and `..` segments resolved and no trailing slash, written
/// here rather than borrowed from the port it masks.
fn lexical(d: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in d.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            x => parts.push(x),
        }
    }
    format!("/{}", parts.join("/"))
}

fn norm(s: &str, tmp: &Tmp) -> String {
    match tmp {
        Tmp::Dir(d) if !d.is_empty() => s.replace(&lexical(d), "<TMP>"),
        _ => s.replace("/tmp", "<TMP>"),
    }
}

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
        .collect()
}

fn case_dirs(name: &str) -> (PathBuf, Tmp, Tmp) {
    let dir = scratch(&format!("fetch-{name}"));
    let js_tmp = Tmp::Dir(dir.join("js-tmp").to_string_lossy().into_owned());
    let rs_tmp = Tmp::Dir(dir.join("rs-tmp").to_string_lossy().into_owned());
    std::fs::create_dir_all(js_tmp.resolved()).unwrap();
    std::fs::create_dir_all(rs_tmp.resolved()).unwrap();
    (dir, js_tmp, rs_tmp)
}

/// Runs one case on both sides with their own temp dirs and asserts the same
/// stdout (byte for byte once the temp dir is masked), the same crash, the
/// same `gh` calls and the same patch on disk. Returns the parsed line.
async fn same(name: &str, calls: Vec<Value>, argv: &[String], pr: &str) -> Option<Value> {
    let (dir, js_tmp, rs_tmp) = case_dirs(name);
    let input = json!({"calls": calls, "argv": argv, "pr": pr});
    same_core(name, &dir, calls, argv, pr, js_tmp, rs_tmp, input).await
}

/// `same` in temp dirs the case picks under `dir`, which its input names as
/// `<DIR>`.
async fn same_in(
    dir: &Path,
    calls: Vec<Value>,
    argv: &[String],
    pr: &str,
    js_tmp: Tmp,
    rs_tmp: Tmp,
    name: &str,
) -> Option<Value> {
    let input = json!({
        "calls": calls,
        "argv": argv,
        "pr": pr,
        "jsTmp": js_tmp.arg(),
        "rsTmp": rs_tmp.arg(),
    });
    let input = portable(&input, dir);
    same_core(name, dir, calls, argv, pr, js_tmp, rs_tmp, input).await
}

#[allow(clippy::too_many_arguments)]
async fn same_core(
    name: &str,
    dir: &Path,
    calls: Vec<Value>,
    argv: &[String],
    pr: &str,
    js_tmp: Tmp,
    rs_tmp: Tmp,
    input: Value,
) -> Option<Value> {
    let input = kept_input(input);
    if let Some(golden) = recorded("fetch") {
        let case = &golden[name];
        assert!(!case.is_null(), "{name}: not in the golden; {REGENERATE}");
        assert_eq!(
            case["input"], input,
            "{name}: the case changed; {REGENERATE}"
        );
        let rec_path = recordings(dir, calls);
        let rs = run_rs(dir, &rec_path, argv, &rs_tmp, pr).await;
        return compare(name, &Recorded::from_golden(case), &rs, &rs_tmp);
    }
    let (out, js) = same_in_recorded(dir, calls, argv, pr, js_tmp, rs_tmp, name).await;
    record("fetch", name, js.to_golden(input), dir);
    out
}

async fn same_in_recorded(
    dir: &Path,
    calls: Vec<Value>,
    argv: &[String],
    pr: &str,
    js_tmp: Tmp,
    rs_tmp: Tmp,
    name: &str,
) -> (Option<Value>, Recorded) {
    let (mut js, rs) = run_both(dir, calls, argv, js_tmp.clone(), rs_tmp.clone(), pr).await;
    // A script that crashes exits while the gh calls it already started are
    // still running, so their log lines can land after it is gone.
    if js.out.is_err() {
        for _ in 0..30 {
            if js.calls == rs.calls {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            js.calls = read_calls(&dir.join("js-calls.log"));
        }
    }
    let js = Recorded {
        out: js
            .out
            .as_ref()
            .ok()
            .map(|a| Kept::Bytes(norm(a, &js_tmp).into_bytes())),
        calls: js.calls,
        patch: js.patch.map(Kept::Bytes),
    };
    (compare(name, &js, &rs, &rs_tmp), js)
}

/// A recorded output: its bytes, or their SHA-256 where they would be most
/// of the golden.
enum Kept {
    Bytes(Vec<u8>),
    Sha(String),
}

impl Kept {
    fn matches(&self, b: &[u8]) -> bool {
        match self {
            Kept::Bytes(a) => a == b,
            Kept::Sha(h) => *h == sha256_hex(b),
        }
    }

    /// `case[key]` (text, or hex when `hex`), or `case[<key>Sha256]`; `None`
    /// where the case has neither.
    fn read(case: &Value, key: &str, hex: bool) -> Option<Kept> {
        if let Some(h) = case[format!("{key}Sha256")].as_str() {
            return Some(Kept::Sha(h.to_owned()));
        }
        case[key]
            .as_str()
            .map(|s| Kept::Bytes(if hex { unhex(s) } else { s.as_bytes().to_vec() }))
    }

    fn write(case: &mut Value, key: &str, kept: Option<&Kept>, hex_bytes: bool) {
        match kept {
            None => case[key] = Value::Null,
            Some(Kept::Bytes(b)) if b.len() > BIG => {
                case[format!("{key}Sha256")] = json!(sha256_hex(b));
            }
            Some(Kept::Bytes(b)) if hex_bytes => case[key] = json!(hex(b)),
            Some(Kept::Bytes(b)) => case[key] = json!(String::from_utf8(b.clone()).unwrap()),
            Some(Kept::Sha(h)) => case[format!("{key}Sha256")] = json!(h),
        }
    }
}

impl std::fmt::Display for Kept {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Kept::Bytes(b) => f.write_str(&String::from_utf8_lossy(b)),
            Kept::Sha(h) => write!(f, "<{BIG}+ bytes with SHA-256 {h}>"),
        }
    }
}

/// The script's side of a case as the comparison reads it: stdout with its
/// temp dir masked (`None` when it failed), the `gh` calls, the patch.
struct Recorded {
    out: Option<Kept>,
    calls: Vec<String>,
    patch: Option<Kept>,
}

impl Recorded {
    fn from_golden(case: &Value) -> Self {
        Self {
            out: Kept::read(case, "stdout", false),
            calls: serde_json::from_value(case["calls"].clone()).unwrap(),
            patch: Kept::read(case, "patch", true),
        }
    }

    fn to_golden(&self, input: Value) -> Value {
        let mut case = json!({"input": input, "calls": self.calls});
        Kept::write(&mut case, "stdout", self.out.as_ref(), false);
        Kept::write(&mut case, "patch", self.patch.as_ref(), true);
        case
    }
}

fn compare(name: &str, js: &Recorded, rs: &Side, rs_tmp: &Tmp) -> Option<Value> {
    assert_eq!(js.calls, rs.calls, "{name}: different gh calls");
    let same_patch = match (&js.patch, &rs.patch) {
        (None, None) => true,
        (Some(a), Some(b)) => a.matches(b),
        _ => false,
    };
    assert!(
        same_patch,
        "{name}: different patch on disk\n js: {:?}\n rs: {:?}",
        js.patch.as_ref().map(ToString::to_string),
        rs.patch.as_deref().map(String::from_utf8_lossy)
    );
    match (&js.out, &rs.out) {
        (Some(a), Ok(b)) => {
            let b = norm(b, rs_tmp);
            assert!(
                a.matches(b.as_bytes()),
                "{name}: stdout differs\n js: {a}\n rs: {b}"
            );
            Some(last_json(&b))
        }
        (None, Err(_)) => None,
        (a, b) => panic!(
            "{name}: one side failed\n js: {:?}\n rs: {b:?}",
            a.as_ref().map(ToString::to_string)
        ),
    }
}

const REGENERATE: &str =
    "regenerate: TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_fetch";

fn golden_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/fetch-pr-context.json")
}

/// `input` as the golden keeps it: itself, or its SHA-256 past `BIG`.
fn kept_input(input: Value) -> Value {
    let text = input.to_string();
    if text.len() > BIG {
        json!({"sha256": sha256_hex(text.as_bytes())})
    } else {
        input
    }
}

/// `v` with the scratch directory `dir` written as `<DIR>`.
fn portable(v: &Value, dir: &Path) -> Value {
    let text = v.to_string().replace(&*dir.to_string_lossy(), "<DIR>");
    serde_json::from_str(&text).unwrap()
}

/// The text of `entry` (a path under the plugin `root`) and of every module
/// it reaches through a relative import, by path in the plugin. `None` when
/// any of them is gone.
fn modules(root: &Path, entry: &str, into: &mut BTreeMap<String, String>) -> Option<()> {
    let import = regex::Regex::new(r#"(?:from|import)\s*\(?\s*"(\.{1,2}/[^"]+)""#).unwrap();
    let root = root.canonicalize().ok()?;
    let mut todo = vec![root.join(entry)];
    while let Some(file) = todo.pop() {
        let file = file.canonicalize().ok()?;
        let key = file
            .strip_prefix(&root)
            .ok()?
            .to_string_lossy()
            .into_owned();
        if into.contains_key(&key) {
            continue;
        }
        let text = std::fs::read_to_string(&file).ok()?;
        let dir = file.parent().unwrap();
        todo.extend(import.captures_iter(&text).map(|m| dir.join(&m[1])));
        into.insert(key, text);
    }
    Some(())
}

/// The HEAD-WORKTREE region of the workflow under `root`, each slice from its
/// BEGIN marker through its END marker, rather than the whole file, which
/// changes with every edit to the workflow. `None` when the workflow is gone.
fn region_text(root: &Path) -> Option<String> {
    let src = std::fs::read_to_string(root.join(WORKFLOW)).ok()?;
    let begin = format!("// ===== BEGIN {REGION} =====");
    let end = format!("// ===== END {REGION} =====");
    let mut slices = Vec::new();
    let mut from = 0;
    while let Some(b) = src[from..].find(&begin).map(|i| from + i) {
        let e = b + src[b..].find(&end).expect("an END for every BEGIN") + end.len();
        slices.push(&src[b..e]);
        from = e;
    }
    Some(slices.join("\n"))
}

/// What each oracle reads besides its input, under the plugin `root`: the
/// scripts it runs and every module they import, the workflow region it
/// slices, the recorded `gh` at `shim`, and this file's drivers and fake
/// `gh`s. An oracle is `None` once any file it reads is gone: it is retired,
/// and its golden is final.
fn oracle_texts(
    root: &Path,
    shim: &Path,
) -> BTreeMap<&'static str, Option<BTreeMap<String, String>>> {
    let read = |entries: &[&str],
                region: bool,
                gh: bool,
                own: &[(&str, &str)]|
     -> Option<BTreeMap<String, String>> {
        let mut texts = BTreeMap::new();
        for entry in entries {
            modules(root, entry, &mut texts)?;
        }
        if region {
            texts.insert(format!("{WORKFLOW}#{REGION}"), region_text(root)?);
        }
        if gh {
            texts.insert("gh shim".to_owned(), std::fs::read_to_string(shim).ok()?);
        }
        for (k, v) in own {
            texts.insert((*k).to_owned(), (*v).to_owned());
        }
        Some(texts)
    };
    BTreeMap::from([
        (
            "fetch",
            read(
                &[SCRIPT],
                false,
                true,
                &[
                    ("FETCH_DRIVER", FETCH_DRIVER),
                    ("EXPORTS_DRIVER", EXPORTS_DRIVER),
                    ("BYTES_DRIVER", BYTES_DRIVER),
                    ("BYTES_GH", BYTES_GH),
                ],
            ),
        ),
        (
            "worktree",
            read(
                &[SCRIPT, EXTRACT],
                true,
                true,
                &[
                    ("WORKTREE_DRIVER", WORKTREE_DRIVER),
                    ("CLONING_GH", CLONING_GH),
                ],
            ),
        ),
        ("region", read(&[REGION_ORACLE], true, false, &[])),
    ])
}

/// The oracles still on disk that differ from what the golden was recorded
/// with. A retired oracle is never stale.
fn stale(
    recorded_with: &Value,
    now: &BTreeMap<&'static str, Option<BTreeMap<String, String>>>,
) -> Vec<&'static str> {
    now.iter()
        .filter(|(name, texts)| {
            texts
                .as_ref()
                .is_some_and(|t| recorded_with[**name] != json!(t))
        })
        .map(|(name, _)| *name)
        .collect()
}

fn regenerating() -> bool {
    std::env::var("TRIBUNAL_REGEN_GOLDENS").is_ok_and(|v| v == "1")
}

/// The golden's entries for `oracle`, checked against what that oracle reads
/// on disk; `None` when regenerating.
fn recorded(oracle: &str) -> Option<&'static Value> {
    static GOLDEN: OnceLock<Option<(Value, Vec<&'static str>)>> = OnceLock::new();
    let (golden, stale_ones) = GOLDEN
        .get_or_init(|| {
            if regenerating() {
                return None;
            }
            let path = golden_path();
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}; {REGENERATE}", path.display()));
            let golden: Value = serde_json::from_str(&text).unwrap();
            let now = oracle_texts(&plugin_root(), &shim_dir().join("gh"));
            let stale_ones = stale(&golden["recordedWith"], &now);
            Some((golden, stale_ones))
        })
        .as_ref()?;
    assert!(
        !stale_ones.contains(&oracle),
        "{oracle}: a script, the region, a driver or the gh shim it reads changed since {} was recorded; {REGENERATE}",
        golden_path().display()
    );
    Some(&golden[oracle])
}

/// Adds `entry` to the golden under `oracle` and `name`, and writes it out.
/// Tests add their entries from several threads, so the file is rewritten
/// after each one and is whole once the binary finishes. `dir` is the case's
/// scratch directory, which the golden cannot keep.
fn record(oracle: &str, name: &str, entry: Value, dir: &Path) {
    static WITH: OnceLock<Value> = OnceLock::new();
    static ENTRIES: Mutex<BTreeMap<String, BTreeMap<String, Value>>> = Mutex::new(BTreeMap::new());
    let text = entry.to_string();
    for d in [Some(dir.to_path_buf()), dir.canonicalize().ok()]
        .into_iter()
        .flatten()
    {
        assert!(
            !text.contains(&*d.to_string_lossy()),
            "{name}: the script's side names the case's directory, which the golden cannot keep"
        );
    }
    let with = WITH.get_or_init(|| {
        let texts: BTreeMap<_, _> = oracle_texts(&plugin_root(), &shim_dir().join("gh"))
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    v.unwrap_or_else(|| panic!("{k}: a file it reads is missing")),
                )
            })
            .collect();
        json!(texts)
    });
    let mut entries = ENTRIES.lock().unwrap();
    assert!(
        entries
            .entry(oracle.to_owned())
            .or_default()
            .insert(name.to_owned(), entry)
            .is_none(),
        "two {oracle} entries are named {name}"
    );
    let mut golden = json!({"recordedWith": with});
    for (k, v) in entries.iter() {
        golden[k] = json!(v);
    }
    let text = serde_json::to_string_pretty(&golden).unwrap();
    std::fs::write(golden_path(), text + "\n").unwrap();
}

fn argv(pr: &str, extra: &[&str]) -> Vec<String> {
    let mut v = s(&[
        "--pr",
        pr,
        "--repo",
        REPO,
        "--deploy-branches",
        "main, staging,,production",
    ]);
    v.extend(s(extra));
    v
}

#[tokio::test]
async fn fetch_pr_context_matches_js_unpinned() {
    let pr = "101";
    let mut calls = common_calls(
        pr,
        &view(3, "development"),
        &inline_pages(),
        &thread_pages(),
        "  alice ",
    );
    calls.push(rec(&diff_args(pr), &diff_text(3), "", 0));
    let out = same(
        "unpinned",
        calls.clone(),
        &argv(pr, &["--bot-comments"]),
        pr,
    )
    .await
    .unwrap();
    // The case is not vacuous: every list it compares has entries.
    assert_eq!(out["context"]["jiraTicket"], "WID-42");
    // Three bot inline comments and two bot reviews with a body.
    assert_eq!(out["context"]["botComments"].as_array().unwrap().len(), 5);
    assert_eq!(out["raw"]["changesRequested"].as_array().unwrap().len(), 2);
    assert!(out["errors"].as_array().unwrap().is_empty());
    same("unpinned-nobots", calls, &argv(pr, &[]), pr)
        .await
        .unwrap();
}

#[tokio::test]
async fn fetch_pr_context_matches_js_at_the_diff_window_edges() {
    let small = |n| diff_text(n);
    let sized = |bytes: usize, unit: &str| unit.repeat(bytes / unit.len());
    let cases: Vec<(usize, String, &[&str])> = vec![
        (12, small(12), &[]),
        (13, small(13), &[]),
        (13, small(13), &["--no-diff-text"]),
        (13, sized(60000, "x"), &[]),
        (13, sized(60001, "x"), &[]),
        // 30,000 characters but 60,000 bytes, then one character more.
        (13, sized(60000, "é"), &[]),
        (13, sized(60002, "é"), &[]),
        (0, String::new(), &[]),
    ];
    for (i, (n, diff, extra)) in cases.into_iter().enumerate() {
        let pr = format!("{}", 110 + i);
        let mut calls = common_calls(&pr, &view(n, "main-ish"), &json!([]), &json!([]), "alice");
        calls.push(rec(&diff_args(&pr), &diff, "", 0));
        // Diffs of 60,000 bytes: the golden keeps their SHA-256.
        let out = same(&format!("window-{i}"), calls, &argv(&pr, extra), &pr)
            .await
            .unwrap();
        let in_band = !out["context"]["diff"].as_str().unwrap().is_empty();
        assert_eq!(
            in_band,
            [false, true, false, true, false, true, false, false][i],
            "case {i}"
        );
    }
}

#[tokio::test]
async fn fetch_pr_context_matches_js_pinned() {
    let pr = "121";
    let mut v = view(2, "development");
    v["headRefOid"] = json!(HEAD);
    let compare = json!({"files": [
        {"filename": "src/a.ts", "additions": 2, "deletions": 1},
        {"filename": "src/b.ts", "additions": 1.5, "deletions": 0},
        {"filename": "", "additions": 4, "deletions": 4},
        {"additions": 1, "deletions": 1},
        {"filename": "src/c.ts", "additions": 1e21, "deletions": 1},
    ]});
    let mut calls = common_calls(pr, &v, &inline_pages(), &thread_pages(), "alice");
    calls.push(rec(&compare_diff_args(BASE, PIN), &diff_text(4), "", 0));
    calls.push(rec(&compare_args(BASE, PIN), &compare.to_string(), "", 0));
    for (i, cutoff) in [
        "2026-10-04T00:00:00Z",
        "2026-10-05T10:00:00.5Z",
        "2026-10-05T10:00:00.501Z",
        "2026-10-02T09:00:00+01:00",
    ]
    .iter()
    .enumerate()
    {
        let out = same(
            &format!("pinned-{i}"),
            calls.clone(),
            &argv(
                pr,
                &[
                    "--head-sha",
                    &format!(" {PIN} "),
                    "--thread-before",
                    cutoff,
                    "--bot-comments",
                ],
            ),
            pr,
        )
        .await
        .unwrap();
        assert_eq!(out["context"]["headRefOid"], PIN);
    }
    // Churn needs every count to be a finite number.
    let bad = json!({"files": [{"filename": "src/a.ts", "additions": "2", "deletions": 1}]});
    let mut calls = common_calls(pr, &v, &json!([]), &json!([]), "alice");
    calls.push(rec(&compare_diff_args(BASE, PIN), "d", "", 0));
    calls.push(rec(&compare_args(BASE, PIN), &bad.to_string(), "", 0));
    let out = same(
        "pinned-string-counts",
        calls,
        &argv(pr, &["--head-sha", PIN]),
        pr,
    )
    .await
    .unwrap();
    assert_eq!(out["context"]["fileChurn"], json!([]));
    assert_eq!(out["context"]["additions"], json!(2));
}

#[tokio::test]
async fn fetch_pr_context_matches_js_when_the_pinned_compare_cannot_run() {
    let pr = "131";
    let mut v = view(2, "development");
    v.as_object_mut().unwrap().remove("baseRefOid");
    // No compare call is recorded: a side that made one would fail on it.
    let calls = common_calls(pr, &v, &json!([]), &json!([]), "alice");
    let out = same("pinned-no-base", calls, &argv(pr, &["--head-sha", PIN]), pr)
        .await
        .unwrap();
    assert_eq!(out["errors"][0]["step"], "compare-diff");

    let mut calls = common_calls(pr, &view(2, "x"), &json!([]), &json!([]), "alice");
    calls.push(rec(&compare_diff_args(BASE, PIN), "", "", 1));
    calls.push(rec(
        &compare_args(BASE, PIN),
        "",
        "HTTP 404: Not Found (https://api.github.com/x)\n",
        1,
    ));
    let out = same(
        "pinned-compare-fails",
        calls,
        &argv(pr, &["--head-sha", PIN]),
        pr,
    )
    .await
    .unwrap();
    assert_eq!(out["errors"].as_array().unwrap().len(), 2);

    let mut calls = common_calls(pr, &view(2, "x"), &json!([]), &json!([]), "alice");
    calls.push(rec(&compare_diff_args(BASE, PIN), "diff", "", 0));
    calls.push(rec(&compare_args(BASE, PIN), "{not json", "", 0));
    same(
        "pinned-compare-garbled",
        calls,
        &argv(pr, &["--head-sha", PIN]),
        pr,
    )
    .await
    .unwrap();

    // A base given as a number is interpolated as JavaScript prints it.
    let mut v = view(1, "x");
    v["baseRefOid"] = json!(12345);
    let mut calls = common_calls(pr, &v, &json!([]), &json!([]), "alice");
    calls.push(rec(&compare_diff_args("12345", PIN), "diff", "", 0));
    calls.push(rec(&compare_args("12345", PIN), "{\"files\": []}", "", 0));
    same(
        "pinned-numeric-base",
        calls,
        &argv(pr, &["--head-sha", PIN]),
        pr,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn fetch_pr_context_matches_js_when_every_call_fails() {
    let pr = "141";
    let long = "e".repeat(400);
    let calls = vec![
        rec(
            &view_args(pr),
            "",
            "GraphQL: Could not resolve to a PullRequest\n",
            1,
        ),
        rec(&inline_args(pr), "", "", 1),
        rec(&thread_args(pr), "", " \n\t", 2),
        rec(&viewer_args(), "", &long, 1),
        rec(&diff_args(pr), "partial", "", 1),
    ];
    let out = same("all-fail", calls, &argv(pr, &[]), pr).await.unwrap();
    assert_eq!(out["errors"].as_array().unwrap().len(), 5);
    assert_eq!(out["context"]["diffBytes"], Value::Null);
}

#[tokio::test]
async fn fetch_pr_context_matches_js_on_malformed_output() {
    let pr = "151";
    let variants: Vec<(&str, &str, &str, &str)> = vec![
        ("not json", "{}", "[[1], 2, [[3]]]", "\n"),
        ("null", "[]", "\"text\"", ""),
        ("[1, 2]", "[[], []]", "[]", "bob"),
        ("\"a string view\"", "", "{\"pages\": 1}", " x y "),
        (
            "{\"files\": \"src/a.ts\", \"reviews\": {\"0\": 1}}",
            "[{}]",
            "[[{\"user\": {}}]]",
            "v",
        ),
    ];
    for (i, (v, inline, thread, viewer)) in variants.into_iter().enumerate() {
        let calls = vec![
            rec(&view_args(pr), v, "", 0),
            rec(&inline_args(pr), inline, "", 0),
            rec(&thread_args(pr), thread, "", 0),
            rec(&viewer_args(), viewer, "", 0),
            rec(&diff_args(pr), "", "", 0),
        ];
        same(&format!("malformed-{i}"), calls, &argv(pr, &[]), pr).await;
    }
}

#[tokio::test]
async fn fetch_pr_context_matches_js_on_deploy_branches() {
    let pr = "161";
    for (i, (base, list)) in [
        ("production", "staging,production"),
        ("production", ""),
        ("production", " production "),
        ("Production", "production"),
        ("", ","),
    ]
    .into_iter()
    .enumerate()
    {
        let mut calls = common_calls(pr, &view(1, base), &json!([]), &json!([]), "alice");
        calls.push(rec(&diff_args(pr), "d", "", 0));
        let a = s(&["--repo", REPO, "--deploy-branches", list, "--pr", pr]);
        let out = same(&format!("deploy-{i}"), calls, &a, pr).await.unwrap();
        assert_eq!(
            out["context"]["isDeploymentPR"],
            json!(i == 0 || i == 2),
            "case {i}"
        );
    }
}

#[tokio::test]
async fn fetch_pr_context_matches_js_on_odd_value_types() {
    let pr = "171";
    let v = json!({
        "title": 5,
        "body": ["see", "AB-1"],
        "author": {"login": 7},
        "baseRefName": "dev",
        "baseRefOid": BASE,
        "headRefName": {"z": 1, "2": 2, "a": 3, "1": 4},
        "headRefOid": "",
        "files": [{"path": 3, "additions": 1, "deletions": 2}, "x", {"path": ""}, 4, {"path": "ok", "additions": -0.0, "deletions": 0.1}],
        "additions": "12",
        "deletions": [3],
        "reviews": [
            {"author": "plain-string", "state": {"b": 1, "a": 2}, "body": 42, "submittedAt": 17},
            {"author": {"login": "x[bot]"}, "state": "CHANGES_REQUESTED", "body": true},
            {"author": [], "state": null, "body": false, "submittedAt": ""},
            {"author": {"login": ""}, "state": "CHANGES_REQUESTED"},
            7,
            "str",
        ],
    });
    let inline = json!([[{"user": {"login": "renovate"}, "line": 0, "original_line": 9, "body": 1}, {"user": null, "path": 5, "line": false}]]);
    let mut calls = common_calls(pr, &v, &inline, &json!([[true, 5, "s"]]), "alice");
    calls.push(rec(&diff_args(pr), "d", "", 0));
    same("odd-types", calls, &argv(pr, &["--bot-comments"]), pr)
        .await
        .unwrap();
}

#[tokio::test]
async fn fetch_pr_context_crashes_where_js_does() {
    let pr = "181";
    let null_file = json!({"files": [null], "reviews": []});
    let null_review = json!({"files": [], "reviews": [null]});
    let string_files = json!({"files": "src/a.ts"});
    for (i, (v, inline)) in [
        (null_file, json!([])),
        (null_review, json!([])),
        (json!({}), json!([[null]])),
        (string_files, json!([])),
    ]
    .into_iter()
    .enumerate()
    {
        let mut calls = common_calls(pr, &v, &inline, &json!([]), "alice");
        calls.push(rec(&diff_args(pr), "d", "", 0));
        let out = same(&format!("crash-{i}"), calls, &argv(pr, &[]), pr).await;
        assert!(out.is_none(), "case {i} should crash on both sides");
    }
    let mut calls = common_calls(pr, &view(1, "x"), &json!([]), &json!([]), "alice");
    calls.push(rec(&compare_diff_args(BASE, PIN), "d", "", 0));
    calls.push(rec(&compare_args(BASE, PIN), "{\"files\": [null]}", "", 0));
    assert!(
        same("crash-compare", calls, &argv(pr, &["--head-sha", PIN]), pr)
            .await
            .is_none()
    );
}

#[tokio::test]
async fn fetch_pr_context_usage_lines_match_js() {
    let cases: Vec<Vec<&str>> = vec![
        vec![],
        vec!["--pr", "abc", "--repo", REPO, "--deploy-branches", "main"],
        vec!["--pr", "12", "--repo", REPO],
        vec!["--pr", "12", "--deploy-branches", "main"],
        vec!["--pr", "12", "--repo", "--deploy-branches", "main"],
        vec![
            "--pr",
            "12",
            "--repo",
            REPO,
            "--deploy-branches",
            "--bot-comments",
        ],
        vec!["--pr", "12", "--repo", REPO, "--deploy-branches"],
        vec!["--pr", "1.5", "--repo", REPO, "--deploy-branches", "main"],
        vec!["--pr", "", "--repo", REPO, "--deploy-branches", "main"],
        vec![
            "--pr",
            "12",
            "--repo",
            REPO,
            "--deploy-branches",
            "main",
            "--thread-before",
            "yesterday",
        ],
        vec![
            "--pr",
            "12",
            "--repo",
            REPO,
            "--deploy-branches",
            "main",
            "--thread-before",
            "2026-10-22T10:00:00Z ",
        ],
        vec!["--pr", "x", "--thread-before", "2026-13-01T00:00:00Z"],
        vec!["--thread-before", "  "],
    ];
    for (i, a) in cases.iter().enumerate() {
        let pr = "191";
        // Only the case with a valid date and arguments goes on to gh.
        let mut calls = common_calls(pr, &view(0, "x"), &json!([]), &json!([]), "alice");
        calls.push(rec(&diff_args("12"), "d", "", 0));
        calls.extend(common_calls(
            "12",
            &view(0, "x"),
            &json!([]),
            &json!([]),
            "alice",
        ));
        same(&format!("usage-{i}"), calls, &s(a), "12").await;
    }
}

#[tokio::test]
async fn fetch_pr_context_matches_js_on_temp_dirs() {
    let pr = "201";
    let mut calls = common_calls(pr, &view(2, "x"), &json!([]), &json!([]), "alice");
    calls.push(rec(&diff_args(pr), &diff_text(2), "", 0));
    let dir = scratch("fetch-tmpdirs");

    // A TMPDIR that does not exist: the write fails and is reported.
    let js = Tmp::Dir(dir.join("missing-js").to_string_lossy().into_owned());
    let rs = Tmp::Dir(dir.join("missing-rs").to_string_lossy().into_owned());
    let out = same_in(
        &dir,
        calls.clone(),
        &argv(pr, &[]),
        pr,
        js,
        rs,
        "tmp-missing",
    )
    .await
    .unwrap();
    assert_eq!(out["errors"][0]["step"], "diff-write");

    // A trailing slash, and a path with `..` in it, are normalized by path.join.
    for (i, suffix) in ["/", "/./sub/../"].iter().enumerate() {
        let js_dir = dir.join(format!("js-{i}"));
        let rs_dir = dir.join(format!("rs-{i}"));
        std::fs::create_dir_all(js_dir.join("sub")).unwrap();
        std::fs::create_dir_all(rs_dir.join("sub")).unwrap();
        let js = Tmp::Dir(format!("{}{suffix}", js_dir.display()));
        let rs = Tmp::Dir(format!("{}{suffix}", rs_dir.display()));
        same_in(
            &dir,
            calls.clone(),
            &argv(pr, &[]),
            pr,
            js,
            rs,
            &format!("tmp-slash-{i}"),
        )
        .await
        .unwrap();
    }

    // Unset and empty TMPDIR both fall back to /tmp. The PR number is this
    // test's own, so the shared file is not another test's.
    let pr = "9700201";
    let mut calls = common_calls(pr, &view(2, "x"), &json!([]), &json!([]), "alice");
    calls.push(rec(&diff_args(pr), "unset tmpdir diff\n", "", 0));
    same_in(
        &dir,
        calls.clone(),
        &argv(pr, &[]),
        pr,
        Tmp::Unset,
        Tmp::Unset,
        "tmp-unset",
    )
    .await
    .unwrap();
    same_in(
        &dir,
        calls,
        &argv(pr, &[]),
        pr,
        Tmp::Dir(String::new()),
        Tmp::Unset,
        "tmp-empty",
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn fetch_pr_context_matches_js_on_generated_prs() {
    let review_pool = reviews().as_array().unwrap().clone();
    let strategy = (
        0usize..16,
        proptest::sample::subsequence(review_pool, 0..8),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        prop_oneof![
            Just(""),
            Just("2026-10-03T10:00:00Z"),
            Just("2026-10-01T10:00:00Z"),
            Just("2026-10-01T09:30:00-00:30")
        ],
        0usize..3,
    );
    let mut runner = TestRunner::deterministic();
    for i in 0..30 {
        let (n, revs, bots, no_diff, pinned, cutoff, failing) =
            strategy.new_tree(&mut runner).unwrap().current();
        let pr = format!("{}", 300 + i);
        let mut v = view(n, if i % 5 == 0 { "staging" } else { "dev" });
        v["reviews"] = Value::Array(revs);
        let mut calls = common_calls(&pr, &v, &inline_pages(), &thread_pages(), "alice");
        let ok_or_fail = |args: Vec<String>, out: &str, k: usize| {
            if failing == k {
                rec(&args, "", "boom", 1)
            } else {
                rec(&args, out, "", 0)
            }
        };
        let mut extra: Vec<String> = Vec::new();
        if pinned {
            let cf = json!({"files": files(n).as_array().unwrap().iter().map(|f| json!({"filename": f["path"], "additions": f["additions"], "deletions": f["deletions"]})).collect::<Vec<_>>()});
            calls.push(ok_or_fail(compare_diff_args(BASE, PIN), &diff_text(n), 1));
            calls.push(ok_or_fail(compare_args(BASE, PIN), &cf.to_string(), 2));
            extra.extend(s(&["--head-sha", PIN]));
        } else {
            calls.push(ok_or_fail(diff_args(&pr), &diff_text(n), 1));
        }
        if bots {
            extra.push("--bot-comments".into());
        }
        if no_diff {
            extra.push("--no-diff-text".into());
        }
        if !cutoff.is_empty() {
            extra.extend(s(&["--thread-before", cutoff]));
        }
        let e: Vec<&str> = extra.iter().map(String::as_str).collect();
        same(&format!("gen-{i}"), calls, &argv(&pr, &e), &pr)
            .await
            .unwrap();
    }
}

/// The script's exports, and `Date.parse`, on `calls`: run under
/// regeneration, read back from the golden otherwise, where the calls are
/// kept as their SHA-256.
fn exports_oracle(name: &str, calls: &Value) -> Vec<Value> {
    let key = format!("exports-{name}");
    let input = sha256_hex(calls.to_string().as_bytes());
    if let Some(golden) = recorded("fetch") {
        let kept = &golden[&key];
        assert!(!kept.is_null(), "{key}: not in the golden; {REGENERATE}");
        assert_eq!(
            kept["callsSha256"], input,
            "{key}: the calls changed; {REGENERATE}"
        );
        return serde_json::from_value(kept["results"].clone()).unwrap();
    }
    let dir = scratch(&format!("fetch-exports-{name}"));
    let results = run_exports(&dir, calls);
    record(
        "fetch",
        &key,
        json!({"callsSha256": input, "results": results}),
        &dir,
    );
    results
}

fn run_exports(dir: &Path, calls: &Value) -> Vec<Value> {
    let driver = write_driver(dir, "exports-driver.mjs", EXPORTS_DRIVER);
    let mut child = Command::new("node")
        .arg(&driver)
        .arg(plugin_root().join(SCRIPT))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("node must be on PATH");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(calls.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice::<Vec<Value>>(&out.stdout).unwrap()
}

fn date_strategy() -> impl Strategy<Value = String> {
    let year = prop_oneof![
        Just("2026".to_owned()),
        Just("1970".to_owned()),
        Just("0000".to_owned()),
        Just("+002026".to_owned()),
        Just("-000001".to_owned()),
        Just("+275760".to_owned()),
        Just("-271821".to_owned()),
        (0u32..10000).prop_map(|y| format!("{y:04}")),
    ];
    let two = |hi: u32| (0..hi).prop_map(|n| format!("{n:02}"));
    let zone = prop_oneof![
        Just("Z".to_owned()),
        Just("z".to_owned()),
        (
            prop_oneof![Just('+'), Just('-')],
            two(26),
            two(61),
            any::<bool>()
        )
            .prop_map(|(s, h, m, colon)| format!("{s}{h}{}{m}", if colon { ":" } else { "" })),
    ];
    let frac = prop_oneof![
        Just(String::new()),
        Just(".".to_owned()),
        "[0-9]{1,12}".prop_map(|d| format!(".{d}"))
    ];
    let time = proptest::option::of((
        prop_oneof![Just('T'), Just('t')],
        two(26),
        prop_oneof![two(61), Just("0".to_owned())],
        proptest::option::of((two(61), frac)),
        zone,
    ));
    (
        year,
        proptest::option::of((two(14), proptest::option::of(two(33)))),
        time,
    )
        .prop_map(|(y, md, t)| {
            let mut out = y;
            if let Some((m, d)) = md {
                out.push_str(&format!("-{m}"));
                if let Some(d) = d {
                    out.push_str(&format!("-{d}"));
                }
            }
            if let Some((sep, h, mi, sec, z)) = t {
                out.push_str(&format!("{sep}{h}:{mi}"));
                if let Some((sec, f)) = sec {
                    out.push_str(&format!(":{sec}{f}"));
                }
                out.push_str(&z);
            }
            out
        })
}

#[test]
fn date_parse_matches_v8_on_iso_strings() {
    let mut runner = TestRunner::deterministic();
    let strat = date_strategy();
    let mut inputs: Vec<String> = (0..4000)
        .map(|_| strat.new_tree(&mut runner).unwrap().current())
        .collect();
    inputs.extend(
        [
            "2026-10-22T24:00:00.000Z",
            "2026-10-22T24:00:00.001Z",
            "+275760-09-13T00:00:00Z",
            "+275760-09-13T00:00:00.001Z",
            "-271821-04-20T00:00:00Z",
            "-271821-04-19T23:59:59.999Z",
            "2026-02-29",
            "2024-02-29T12:00Z",
            "",
            "2026-10-22T10:00:00Z ",
            "-000000-01-01T00:00:00Z",
            "2026-10-22T10:00:00.0691423760Z",
            "2026-10-22T10:00:00.00123456789Z",
            "2026-10-22T10:00:00.000000000000000000001Z",
            "2026-10-22T10:00:00.0123Z",
            "2026-10-22T10:00:00.9999999999Z",
        ]
        .map(str::to_owned),
    );
    let calls = Value::Array(inputs.iter().map(|d| json!(["Date.parse", d])).collect());
    let js = exports_oracle("dates", &calls);
    let mut finite = 0;
    let mut wrong = Vec::new();
    for (d, j) in inputs.iter().zip(js) {
        let r = fpc::date_parse(d);
        let same = match j.as_f64() {
            Some(x) => {
                finite += 1;
                r == x
            }
            None => r.is_nan(),
        };
        if !same {
            wrong.push(format!("{d:?}: V8 {j}, Rust {r}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} differ:\n{}",
        wrong.len(),
        wrong[..wrong.len().min(25)].join("\n")
    );
    assert!(finite > 500, "too few valid dates generated: {finite}");
}

#[test]
fn ticket_keys_and_cutoffs_match_js() {
    let mut runner = TestRunner::deterministic();
    let token = prop_oneof![
        Just("A"),
        Just("B"),
        Just("Z"),
        Just("ABC"),
        Just("a"),
        Just("x"),
        Just("-"),
        Just("1"),
        Just("42"),
        Just("_"),
        Just(" "),
        Just("é"),
        Just("\n"),
        Just("."),
        Just("ÄB"),
        Just("0"),
    ];
    let strat = proptest::collection::vec(token, 0..12).prop_map(|v| v.concat());
    let mut calls = Vec::new();
    let mut expect: Vec<Box<dyn Fn() -> Value>> = Vec::new();
    for _ in 0..1500 {
        let title = strat.new_tree(&mut runner).unwrap().current();
        let body = strat.new_tree(&mut runner).unwrap().current();
        calls.push(json!(["jiraTicketOf", title, body]));
        let (t, b) = (json!(title), json!(body));
        expect.push(Box::new(move || {
            json!(fpc::jira_ticket_of(Some(&t), Some(&b)).unwrap())
        }));
    }
    for title in [
        json!(null),
        json!(5),
        json!(["AB-1"]),
        json!({"a": 1}),
        json!(false),
    ] {
        calls.push(json!(["jiraTicketOf", title.clone(), "body CD-2"]));
        expect.push(Box::new(move || {
            json!(fpc::jira_ticket_of(Some(&title), Some(&json!("body CD-2"))).unwrap())
        }));
    }
    let ats = [
        json!("2026-10-01T10:00:00Z"),
        json!(""),
        json!(null),
        json!("garbage"),
        json!(["2026-10-01T10:00:00Z"]),
        json!("2026-10-03T10:00:00Z"),
    ];
    for at in ats {
        for cutoff in ["", "2026-10-02T00:00:00Z", "2026-10-01T10:00:00Z"] {
            calls.push(json!(["postedBefore", at.clone(), cutoff]));
            let at = at.clone();
            expect.push(Box::new(move || {
                json!(fpc::posted_before(Some(&at), cutoff).unwrap())
            }));
        }
    }
    let js = exports_oracle("tickets", &Value::Array(calls.clone()));
    for ((c, j), e) in calls.iter().zip(js).zip(expect) {
        assert_eq!(j, e(), "{c}");
    }
}

/// Each declared difference, with its witness.
#[tokio::test]
async fn declared_differences_have_witnesses() {
    // V8's legacy parser reads a space-separated or local-time date; the port
    // reads only the ISO format with a zone.
    let js = exports_oracle(
        "witness",
        &json!([
            ["Date.parse", "2026-10-22 10:00:00Z"],
            ["Date.parse", "2026-10-22T10:00:00"],
            ["Date.parse", "Oct 22 2026"],
            ["Date.parse", "404-03-07"]
        ]),
    );
    assert!(js.iter().all(Value::is_number), "{js:?}");
    assert!(fpc::date_parse("2026-10-22 10:00:00Z").is_nan());
    assert!(fpc::date_parse("2026-10-22T10:00:00").is_nan());
    assert!(fpc::date_parse("Oct 22 2026").is_nan());
    assert!(fpc::date_parse("404-03-07").is_nan());

    // A 300-unit cut through a surrogate pair: JavaScript keeps the lone high
    // half, which a Rust String cannot hold, so the port drops the pair.
    let pr = "211";
    let stderr = format!("{}\u{1F600}tail", "a".repeat(299));
    let calls = vec![
        rec(&view_args(pr), "{}", "", 0),
        rec(&inline_args(pr), "[]", "", 0),
        rec(&thread_args(pr), "[]", "", 0),
        rec(&viewer_args(), "", &stderr, 1),
        rec(&diff_args(pr), "d", "", 0),
    ];
    let dir = scratch("fetch-surrogate");
    let t = Tmp::Dir(dir.to_string_lossy().into_owned());
    let a = argv(pr, &[]);
    let js = js_stdout("surrogate", &dir, &calls, &a, &t, pr);
    let rs = run_rs(&dir, &recordings(&dir, calls), &a, &t, pr).await;
    let rs = rs.out.unwrap();
    assert!(
        js.contains(&format!("{}\\ud83d\"", "a".repeat(299))),
        "{js}"
    );
    assert!(
        rs.contains(&format!("\"detail\":\"{}\"", "a".repeat(299))),
        "{rs}"
    );
}

// ---------------------------------------------------------------------------
// HEAD-WORKTREE
// ---------------------------------------------------------------------------

/// The region's functions on `calls`, each `{"ok": value}` or `{"err":
/// message}`: run through `region-oracle.mjs` under regeneration, read back
/// from the golden otherwise, where the calls are kept as their SHA-256.
fn region_results(name: &str, calls: &[Value]) -> Vec<Value> {
    let input = sha256_hex(Value::Array(calls.to_vec()).to_string().as_bytes());
    if let Some(golden) = recorded("region") {
        let kept = &golden[name];
        assert!(!kept.is_null(), "{name}: not in the golden; {REGENERATE}");
        assert_eq!(
            kept["callsSha256"], input,
            "{name}: the calls changed; {REGENERATE}"
        );
        return serde_json::from_value(kept["results"].clone()).unwrap();
    }
    let mut child = Command::new("node")
        .arg(plugin_root().join(REGION_ORACLE))
        .arg(REGION)
        .arg("headWorktreeBash,parseHeadWorktree,pinnedCheckoutBlock")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("node must be on PATH");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(Value::Array(calls.to_vec()).to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let results: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap();
    let dir = scratch(&format!("fetch-region-{name}"));
    record(
        "region",
        name,
        json!({"callsSha256": input, "results": results}),
        &dir,
    );
    results
}

fn region_oracle(calls: &[Value]) -> Vec<Value> {
    region_results("matches", calls)
        .into_iter()
        .map(|r| {
            r.get("ok")
                .cloned()
                .unwrap_or_else(|| panic!("region threw: {r}"))
        })
        .collect()
}

#[test]
fn head_worktree_region_matches_js() {
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for sha in ["$PREP_SHA", HEAD, "", "a b"] {
        for repo in [REPO, "o/r.js", "x"] {
            for pr in [1u64, 1911, 42_000_000] {
                calls.push(json!({"fn": "headWorktreeBash", "args": [sha, repo, pr]}));
                rust.push(json!(hw::head_worktree_bash(sha, repo, &pr.to_string())));
            }
        }
    }
    let lines = [
        "CREATED_WORKTREE /tmp/peer-review-wt-1-abc",
        "CREATED_WORKTREE ",
        "  CLONED:/x/y  ",
        "CLONED:",
        "CLONED:\u{a0}",
        "WORKTREE_FAILED",
        "Preparing worktree (detached HEAD 7495b91f)",
        "",
        "\t",
        "CREATED_WORKTREE\u{a0}/p",
        "CREATED_WORKTREE  /two spaces ",
        "\r",
        "\u{feff}CLONED:/bom",
    ];
    let mut runner = TestRunner::deterministic();
    let strat = proptest::collection::vec(proptest::sample::select(lines.to_vec()), 0..5)
        .prop_map(|v| v.join("\n"));
    let mut texts: Vec<Value> = (0..300)
        .map(|_| json!(strat.new_tree(&mut runner).unwrap().current()))
        .collect();
    texts.extend([
        json!(null),
        json!(0),
        json!(7),
        json!(true),
        json!({"a": 1}),
        json!(["CLONED:/arr", "x"]),
        json!(""),
    ]);
    for t in &texts {
        calls.push(json!({"fn": "parseHeadWorktree", "args": [t]}));
        rust.push(hw::parse_head_worktree(Some(t)).unwrap().to_value());
    }
    calls.push(json!({"fn": "parseHeadWorktree", "args": []}));
    rust.push(hw::parse_head_worktree(None).unwrap().to_value());

    let pins = [
        json!(null),
        json!({"path": "/tmp/wt", "kind": "worktree", "ok": true}),
        json!({"path": "", "ok": true}),
        json!({"path": "/x", "ok": false}),
        json!({"path": 5, "ok": 1}),
        json!({"path": ["a", "b"], "ok": "yes"}),
        json!({"path": null, "kind": "none", "ok": false}),
        json!("str"),
        json!([]),
        json!(0),
    ];
    let shas = [
        json!(HEAD),
        json!(""),
        json!(null),
        json!(0),
        json!(5),
        json!(["a"]),
        json!({"x": 1}),
    ];
    for p in &pins {
        for sha in &shas {
            calls.push(json!({"fn": "pinnedCheckoutBlock", "args": [p, sha]}));
            rust.push(json!(hw::pinned_checkout_block(Some(p), Some(sha)).unwrap()));
        }
        calls.push(json!({"fn": "pinnedCheckoutBlock", "args": [p]}));
        rust.push(json!(hw::pinned_checkout_block(Some(p), None).unwrap()));
    }
    let js = region_oracle(&calls);
    for ((c, j), r) in calls.iter().zip(js).zip(rust) {
        assert_eq!(j, r, "{c}");
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        // Fixed dates make the same commits on every run, so the golden can
        // name their shas.
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

struct Origin {
    bare: PathBuf,
    seed: PathBuf,
    c1: String,
    c2: String,
}

fn commit(repo: &Path, file: &str, text: &str) -> String {
    std::fs::write(repo.join(file), text).unwrap();
    git(repo, &["add", file]);
    git(repo, &["commit", "-q", "-m", file]);
    git(repo, &["rev-parse", "HEAD"])
}

/// A bare `acme/widgets` remote with `main` (c1) and `feature` (c2).
fn origin(dir: &Path) -> Origin {
    let bare = dir.join("remote/acme/widgets.git");
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare", "-b", "main"]);
    let seed = dir.join("seed");
    git(
        dir,
        &[
            "clone",
            "-q",
            bare.to_str().unwrap(),
            seed.to_str().unwrap(),
        ],
    );
    git(&seed, &["checkout", "-q", "-b", "main"]);
    let c1 = commit(&seed, "a.txt", "one\n");
    git(&seed, &["push", "-q", "origin", "HEAD:main"]);
    git(&seed, &["checkout", "-q", "-b", "feature"]);
    let c2 = commit(&seed, "b.txt", "two\n");
    git(&seed, &["push", "-q", "origin", "feature"]);
    Origin { bare, seed, c1, c2 }
}

fn clone_twin(o: &Origin, dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    git(
        dir,
        &["clone", "-q", o.bare.to_str().unwrap(), p.to_str().unwrap()],
    );
    p
}

/// `peer-review-wt-<pr>-<suffix>` with mktemp's random suffix masked.
fn mask_suffix(text: &str, pr: &str) -> String {
    let prefix = format!("peer-review-wt-{pr}-");
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find(&prefix) {
        out.push_str(&rest[..i + prefix.len()]);
        rest = &rest[i + prefix.len()..];
        let n = rest.bytes().take_while(u8::is_ascii_alphanumeric).count();
        out.push_str("XXXXXX");
        rest = &rest[n..];
    }
    out.push_str(rest);
    out
}

/// One side's view of the result: what was printed and the state it left.
#[derive(Debug, PartialEq)]
struct WtState {
    worktree: Value,
    head: Option<String>,
    list: String,
    leftovers: Vec<String>,
    removed: Value,
    gh_calls: Vec<String>,
}

impl WtState {
    fn to_json(&self) -> Value {
        json!({
            "worktree": self.worktree,
            "head": self.head,
            "list": self.list,
            "leftovers": self.leftovers,
            "removed": self.removed,
            "ghCalls": self.gh_calls,
        })
    }

    fn from_json(v: &Value) -> Self {
        let strings = |k: &str| serde_json::from_value(v[k].clone()).unwrap();
        Self {
            worktree: v["worktree"].clone(),
            head: v["head"].as_str().map(str::to_owned),
            list: v["list"].as_str().unwrap().to_owned(),
            leftovers: strings("leftovers"),
            removed: v["removed"].clone(),
            gh_calls: strings("ghCalls"),
        }
    }
}

fn state(
    repo: &Path,
    tmp: &Path,
    worktree: Value,
    removed: Value,
    log: &Path,
    pr: &str,
) -> WtState {
    let masks = |s: &str| {
        mask_suffix(
            &s.replace(&tmp.to_string_lossy().to_string(), "<TMP>")
                .replace(&repo.to_string_lossy().to_string(), "<REPO>"),
            pr,
        )
    };
    let pin = hw::parse_head_worktree(Some(&worktree)).unwrap();
    let head = pin
        .path
        .as_ref()
        .map(|p| git(Path::new(p), &["rev-parse", "HEAD"]));
    let list = masks(&git(repo, &["worktree", "list", "--porcelain"]));
    let mut leftovers: Vec<String> = std::fs::read_dir(tmp)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    leftovers = leftovers.iter().map(|l| mask_suffix(l, pr)).collect();
    leftovers.sort();
    WtState {
        worktree: Value::String(masks(worktree.as_str().unwrap_or("<null>"))),
        head,
        list,
        leftovers,
        removed: Value::String(masks(&removed.to_string())),
        gh_calls: read_calls(log).iter().map(|c| masks(c)).collect(),
    }
}

/// A scratch directory by its real path, which is what git prints, so the
/// masks match it on every machine.
fn wt_scratch(name: &str) -> PathBuf {
    scratch(name).canonicalize().unwrap()
}

/// The script's side of worktree case `name`: under regeneration `run`
/// produces it and the golden keeps it; otherwise it is read back, and
/// `input`, with `dir` as `<DIR>`, must be what it was recorded on.
fn worktree_side(name: &str, dir: &Path, input: Value, run: impl FnOnce() -> Value) -> Value {
    let input = portable(&input, dir);
    if let Some(golden) = recorded("worktree") {
        let kept = &golden[name];
        assert!(!kept.is_null(), "{name}: not in the golden; {REGENERATE}");
        assert_eq!(
            kept["input"], input,
            "{name}: the case changed; {REGENERATE}"
        );
        return kept["js"].clone();
    }
    let js = run();
    record("worktree", name, json!({"input": input, "js": js}), dir);
    js
}

/// Stale and fresh review checkouts, the same in both twins: one stale and
/// removable, one stale but locked, one fresh, one stale with a name the
/// pattern refuses, and one whose directory is gone. Without `age` none of
/// them is made to look old.
fn plant_worktrees(repo: &Path, tmp: &Path, sha: &str, age: bool) {
    let add = |name: &str| {
        let p = tmp.join(name);
        git(
            repo,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                p.to_str().unwrap(),
                sha,
            ],
        );
        p
    };
    let old = |p: &Path| {
        if !age {
            return;
        }
        let st = Command::new("touch")
            .args(["-t", "202001010000"])
            .arg(p)
            .status()
            .unwrap();
        assert!(st.success());
    };
    old(&add("peer-review-wt-77-abc123"));
    let locked = add("peer-review-wt-77-lock01");
    old(&locked);
    git(repo, &["worktree", "lock", locked.to_str().unwrap()]);
    add("peer-review-wt-77-fresh1");
    old(&add("peer-review-wt-x-old999"));
    let gone = add("peer-review-wt-78-gone00");
    std::fs::remove_dir_all(gone).unwrap();
}

/// Whether `touch -t` made `p` look old to `prune_stale_worktrees`, which
/// reads the birth time. macOS moves the birth time back with the modification
/// time; Linux cannot move it, so a planted checkout stays fresh there.
fn aged(p: &Path) -> bool {
    let meta = std::fs::metadata(p).unwrap();
    let born = meta.created().or_else(|_| meta.modified()).unwrap();
    born.elapsed().is_ok_and(|age| age.as_secs() > 86_400)
}

struct Twin {
    repo: PathBuf,
    tmp: PathBuf,
    log: PathBuf,
}

fn twin_pair(dir: &Path, o: &Origin) -> (Twin, Twin) {
    let mk = |name: &str| {
        let repo = clone_twin(o, dir, &format!("twin-{name}"));
        let tmp = dir.join(format!("tmp-{name}"));
        std::fs::create_dir_all(&tmp).unwrap();
        Twin {
            repo,
            tmp,
            log: dir.join(format!("gh-{name}.log")),
        }
    };
    (mk("js"), mk("rs"))
}

/// Runs the script's `prepareReview` worktree step in `js.repo` and the port
/// in `rs.repo`, and returns each side's resulting state. The script's side
/// is case `name` of the golden's `worktree` oracle.
#[allow(clippy::too_many_arguments)]
async fn prep_both(
    name: &str,
    dir: &Path,
    js: &Twin,
    rs: &Twin,
    context: &Value,
    repo: &str,
    pr: &str,
    tmp_override: Option<(&str, &str)>,
) -> (WtState, WtState) {
    let rec_path = recordings(dir, vec![]);
    let _ = std::fs::remove_file(&js.log);
    let _ = std::fs::remove_file(&rs.log);
    let (js_tmp, rs_tmp) =
        tmp_override.unwrap_or((js.tmp.to_str().unwrap(), rs.tmp.to_str().unwrap()));
    let input = json!({"context": context, "repo": repo, "pr": pr, "tmp": [js_tmp, rs_tmp]});
    let js_state = worktree_side(name, dir, input, || {
        let driver = write_driver(dir, "worktree-driver.mjs", WORKTREE_DRIVER);
        let script_path = dir.join("head-worktree.sh");
        let plugin = plugin_root();
        let args = [
            "prep",
            plugin.to_str().unwrap(),
            js_tmp,
            js.log.to_str().unwrap(),
            repo,
            pr,
            &context.to_string(),
            script_path.to_str().unwrap(),
        ];
        let out = run_script(driver.to_str().unwrap(), &args, &js.repo, &rec_path, None);
        assert_eq!(out.code, Some(0), "{}", out.stderr);
        let js_worktree = last_json(&out.stdout)["worktree"].clone();
        state(&js.repo, &js.tmp, js_worktree, Value::Null, &js.log, pr).to_json()
    });

    let mut t: Tools = tools(&rec_path);
    t.env
        .push(("GH_CALL_LOG".into(), rs.log.clone().into_os_string()));
    t.env.push(("TMPDIR".into(), rs_tmp.into()));
    let rs_worktree = hw::prepare_head_worktree(
        &t,
        context,
        repo,
        pr,
        Some(&rs.repo),
        &hw::bash_tmpdir(Some(rs_tmp)),
    )
    .await
    .unwrap()
    .map_or(Value::Null, Value::String);
    (
        WtState::from_json(&js_state),
        state(&rs.repo, &rs.tmp, rs_worktree, Value::Null, &rs.log, pr),
    )
}

fn ctx(sha: &str) -> Value {
    json!({"isDeploymentPR": false, "headRefOid": sha, "diffPath": "", "files": []})
}

/// Whether the stale checkout is pruned depends on whether this machine can
/// age it (see `aged`), so the golden keeps the script's side both ways:
/// regenerating also plants checkouts left fresh, the state a machine that
/// cannot age them reaches, and each run reads the side its own planting
/// matches.
#[tokio::test]
async fn head_worktree_builds_the_same_checkout_as_the_bash() {
    let ages: &[bool] = if regenerating() {
        &[true, false]
    } else {
        &[true]
    };
    let mut done = Vec::new();
    for &age in ages {
        let dir = wt_scratch(&format!("wt-local-{age}"));
        let o = origin(&dir);
        let (js, rs) = twin_pair(&dir, &o);
        plant_worktrees(&js.repo, &js.tmp, &o.c1, age);
        plant_worktrees(&rs.repo, &rs.tmp, &o.c1, age);
        let stale = aged(&rs.tmp.join("peer-review-wt-77-abc123"));
        let name = if stale { "local-aged" } else { "local-fresh" };
        if done.contains(&name) {
            continue;
        }
        done.push(name);
        let (a, b) = prep_both(name, &dir, &js, &rs, &ctx(&o.c2), REPO, "301", None).await;
        assert_eq!(a, b);
        assert_eq!(a.head.as_deref(), Some(o.c2.as_str()));
        assert!(a
            .worktree
            .as_str()
            .unwrap()
            .starts_with("CREATED_WORKTREE <TMP>/peer-review-wt-301-XXXXXX"));
        // The stale unlocked checkout is gone where it could be aged; the rest stay.
        assert_eq!(a.list.contains("abc123"), !stale, "{}", a.list);
        assert!(
            a.list.contains("lock01") && a.list.contains("fresh1"),
            "{}",
            a.list
        );
    }
}

#[tokio::test]
async fn head_worktree_fetches_a_commit_only_the_remote_has() {
    let dir = wt_scratch("wt-fetch");
    let o = origin(&dir);
    let (js, rs) = twin_pair(&dir, &o);
    git(&o.seed, &["checkout", "-q", "-b", "late"]);
    let c3 = commit(&o.seed, "c.txt", "three\n");
    git(&o.seed, &["push", "-q", "origin", "late"]);
    let (a, b) = prep_both(
        "fetch",
        &dir,
        &js,
        &rs,
        &ctx(&c3),
        "ACME/WIDGETS",
        "302",
        None,
    )
    .await;
    assert_eq!(a, b);
    assert_eq!(a.head.as_deref(), Some(c3.as_str()));
}

#[tokio::test]
async fn head_worktree_falls_back_and_fails_as_the_bash_does() {
    let dir = wt_scratch("wt-fail");
    let o = origin(&dir);
    let (js, rs) = twin_pair(&dir, &o);
    // The remote is not the PR's repo: no worktree, and the clone fallback
    // asks the recorded gh, which has no answer.
    let (a, b) = prep_both(
        "fail-other-repo",
        &dir,
        &js,
        &rs,
        &ctx(&o.c1),
        "other/thing",
        "303",
        None,
    )
    .await;
    assert_eq!(a, b);
    assert_eq!(a.worktree, json!("WORKTREE_FAILED"));
    assert_eq!(a.gh_calls.len(), 1, "{:?}", a.gh_calls);
    assert!(a.leftovers.is_empty(), "{:?}", a.leftovers);

    // A commit nobody has.
    let (a, b) = prep_both(
        "fail-no-commit",
        &dir,
        &js,
        &rs,
        &ctx(&"0".repeat(40)),
        REPO,
        "304",
        None,
    )
    .await;
    assert_eq!(a, b);
    assert_eq!(a.worktree, json!("WORKTREE_FAILED"));

    // A `.` in the repo name is a regex wildcard to grep.
    let (a, b) = prep_both(
        "fail-dot",
        &dir,
        &js,
        &rs,
        &ctx(&o.c1),
        "acme.widgets",
        "305",
        None,
    )
    .await;
    assert_eq!(a, b);
    assert_eq!(a.head.as_deref(), Some(o.c1.as_str()));

    // Declared difference: `.` is the only metacharacter the port honours,
    // since a GitHub owner or name cannot hold the others. To grep `s*` is
    // "any number of s", so the bash builds a worktree the port does not.
    let (a, b) = prep_both(
        "fail-star",
        &dir,
        &js,
        &rs,
        &ctx(&o.c1),
        "acme/widgets*",
        "307",
        None,
    )
    .await;
    assert_eq!(a.head.as_deref(), Some(o.c1.as_str()));
    assert_eq!(b.worktree, json!("WORKTREE_FAILED"));
}

#[tokio::test]
async fn head_worktree_matches_the_bash_when_mktemp_fails() {
    let dir = wt_scratch("wt-mktemp");
    let o = origin(&dir);
    let (js, rs) = twin_pair(&dir, &o);
    let js_missing = dir.join("no-such-js");
    let rs_missing = dir.join("no-such-rs");
    let (a, b) = prep_both(
        "mktemp",
        &dir,
        &js,
        &rs,
        &ctx(&o.c1),
        REPO,
        "306",
        Some((js_missing.to_str().unwrap(), rs_missing.to_str().unwrap())),
    )
    .await;
    assert_eq!(a, b);
    // `$WT` is empty and `git -C ""` reads the ambient repo, so the bash
    // prints a marker with no path, which parses as no checkout.
    assert_eq!(a.worktree, json!("CREATED_WORKTREE"));
}

#[tokio::test]
async fn head_worktree_skips_deployment_prs_and_missing_heads() {
    let dir = wt_scratch("wt-skip");
    let o = origin(&dir);
    let (js, rs) = twin_pair(&dir, &o);
    plant_worktrees(&js.repo, &js.tmp, &o.c1, true);
    plant_worktrees(&rs.repo, &rs.tmp, &o.c1, true);
    for (i, c) in [
        json!({"isDeploymentPR": true, "headRefOid": o.c1}),
        json!({"isDeploymentPR": false, "headRefOid": ""}),
        json!({}),
    ]
    .iter()
    .enumerate()
    {
        let (a, b) = prep_both(
            &format!("skip-{i}"),
            &dir,
            &js,
            &rs,
            c,
            REPO,
            &format!("{}", 310 + i),
            None,
        )
        .await;
        assert_eq!(a, b);
        assert_eq!(a.worktree, json!("<null>"));
        // Nothing was pruned either: prepareReview returns first.
        assert!(a.list.contains("abc123"));
    }
}

/// Three hours on, every planted checkout the pattern takes is stale whether
/// or not this machine could age it, so one recorded side serves both.
#[tokio::test]
async fn prune_stale_worktrees_matches_js_with_a_clock() {
    let dir = wt_scratch("wt-prune");
    let o = origin(&dir);
    let (js, rs) = twin_pair(&dir, &o);
    plant_worktrees(&js.repo, &js.tmp, &o.c1, true);
    plant_worktrees(&rs.repo, &rs.tmp, &o.c1, true);
    let rec_path = recordings(&dir, vec![]);
    let ahead = 3.0 * 3_600_000.0;
    let now = hw::now_ms() + ahead;
    let input = json!({"aheadMs": ahead, "maxAgeMs": hw::STALE_WORKTREE_MS});
    let a = worktree_side("prune", &dir, input, || {
        let driver = write_driver(&dir, "worktree-driver.mjs", WORKTREE_DRIVER);
        let plugin = plugin_root();
        let out = run_script(
            driver.to_str().unwrap(),
            &[
                "prune",
                plugin.to_str().unwrap(),
                js.tmp.to_str().unwrap(),
                "",
                &format!("{now}"),
            ],
            &js.repo,
            &rec_path,
            None,
        );
        assert_eq!(out.code, Some(0), "{}", out.stderr);
        let js_removed = last_json(&out.stdout)["removed"].clone();
        // The planted names are the same on both sides, so nothing is masked.
        state(&js.repo, &js.tmp, Value::Null, js_removed, &js.log, "none").to_json()
    });
    let a = WtState::from_json(&a);
    let rs_removed = json!(
        hw::prune_stale_worktrees(
            &tools(&rec_path),
            Some(&rs.repo),
            now,
            hw::STALE_WORKTREE_MS
        )
        .await
    );
    let b = state(&rs.repo, &rs.tmp, Value::Null, rs_removed, &rs.log, "none");
    assert_eq!(a, b);
    // Three hours on, the fresh checkout is stale too; the locked one stays.
    assert!(
        a.removed.as_str().unwrap().contains("fresh1"),
        "{:?}",
        a.removed
    );
    assert!(a.list.contains("lock01"));
}

// ---------------------------------------------------------------------------
// Adversarial cases: inputs the cases above did not reach.
// ---------------------------------------------------------------------------

/// The four common calls with the view given as raw JSON text, for values
/// `serde_json::Value` cannot build.
fn calls_with_view_text(pr: &str, view_text: &str, inline_text: &str) -> Vec<Value> {
    vec![
        rec(&view_args(pr), view_text, "", 0),
        rec(&inline_args(pr), inline_text, "", 0),
        rec(&thread_args(pr), "[]", "", 0),
        rec(&viewer_args(), "alice\n", "", 0),
    ]
}

/// Runs one case on both sides and returns their stdout, for a declared
/// difference where the two are expected to part. The JavaScript line is left
/// unparsed: `serde_json` could not read every line it prints.
async fn both_lines(name: &str, calls: Vec<Value>, argv: &[String], pr: &str) -> (String, Value) {
    let dir = scratch(&format!("fetch-{name}"));
    let t = Tmp::Dir(dir.to_string_lossy().into_owned());
    let js = js_stdout(name, &dir, &calls, argv, &t, pr);
    let rs = run_rs(&dir, &recordings(&dir, calls), argv, &t, pr).await;
    (js, last_json(&rs.out.unwrap()))
}

/// The script's stdout on one case that must succeed, with its temp dir as
/// `<TMP>`: run under regeneration, read back from the golden otherwise.
fn js_stdout(
    name: &str,
    dir: &Path,
    calls: &[Value],
    argv: &[String],
    tmp: &Tmp,
    pr: &str,
) -> String {
    let input = kept_input(json!({"calls": calls, "argv": argv, "pr": pr}));
    if let Some(golden) = recorded("fetch") {
        let kept = &golden[name];
        assert!(!kept.is_null(), "{name}: not in the golden; {REGENERATE}");
        assert_eq!(
            kept["input"], input,
            "{name}: the case changed; {REGENERATE}"
        );
        return kept["stdout"].as_str().unwrap().to_owned();
    }
    let rec_path = recordings(dir, calls.to_vec());
    let js = run_js(dir, &rec_path, argv, tmp, pr);
    let out = norm(&js.out.unwrap(), tmp);
    record("fetch", name, json!({"input": input, "stdout": out}), dir);
    out
}

/// Declared difference: `serde_json` refuses what `JSON.parse` reads as an
/// infinity (a literal past f64's range) and nesting past 128 levels, so the
/// port reports that answer as unparseable. GitHub's payloads hold neither:
/// their counts are integers and their nesting is fixed by the schema. A
/// literal that underflows parses on both sides.
#[tokio::test]
async fn json_the_port_cannot_parse_is_a_declared_difference() {
    let pr = "401";
    let deep = format!("{}1{}", "[".repeat(300), "]".repeat(300));
    let cases = [
        (
            r#"{"title":"t","files":[{"path":"a","additions":1e400,"deletions":1}],"additions":1e400}"#.to_owned(),
            "[]".to_owned(),
        ),
        (
            format!(r#"{{"title":"t","files":[],"headRefName":{deep}}}"#),
            format!(r#"[[{{"user":{{"login":"bob"}},"body":"hi","x":{deep}}}]]"#),
        ),
    ];
    for (i, (view_text, inline)) in cases.iter().enumerate() {
        let mut calls = calls_with_view_text(pr, view_text, inline);
        calls.push(rec(&diff_args(pr), "d", "", 0));
        let (js, rs) = both_lines(&format!("unparseable-{i}"), calls, &argv(pr, &[]), pr).await;
        assert!(
            js.starts_with(r#"{"context":{"title":"t","#),
            "case {i}: {js}"
        );
        assert!(js.trim_end().ends_with(r#""errors":[]}"#), "case {i}: {js}");
        assert_eq!(
            rs["errors"][0],
            json!({"step": "pr-view", "detail": "unparseable output"})
        );
    }
    assert_eq!(js_number_of("1e400"), Value::Null);
    let mut calls =
        calls_with_view_text(pr, r#"{"title":"t","files":[],"additions":1e-400}"#, "[]");
    calls.push(rec(&diff_args(pr), "d", "", 0));
    same("underflow", calls, &argv(pr, &[]), pr).await.unwrap();
}

/// What `JSON.stringify(JSON.parse(text))` gives, through node under
/// regeneration and from the golden otherwise.
fn js_number_of(text: &str) -> Value {
    let name = format!("number-of-{text}");
    if let Some(golden) = recorded("fetch") {
        let kept = &golden[&name];
        assert!(!kept.is_null(), "{name}: not in the golden; {REGENERATE}");
        return kept["result"].clone();
    }
    let out = Command::new("node")
        .args([
            "-e",
            "process.stdout.write(JSON.stringify([JSON.parse(process.argv[1])]))",
            text,
        ])
        .output()
        .unwrap();
    let result = serde_json::from_slice::<Value>(&out.stdout).unwrap()[0].clone();
    let dir = scratch("fetch-number-of");
    record("fetch", &name, json!({"result": result}), &dir);
    result
}

/// `String(x)` and `Number(x)` throw on an object whose own `toString` is not
/// callable, which every JSON `toString` key is. Each case puts one such value
/// where the script converts it.
#[tokio::test]
async fn fetch_pr_context_crashes_on_unconvertible_objects_where_js_does() {
    let pr = "403";
    let bad = json!({"toString": 1});
    let base_view = || {
        json!({"title": "no key", "body": "b", "author": {"login": "alice"}, "baseRefName": "dev",
               "baseRefOid": BASE, "files": [], "reviews": []})
    };
    let review = |author: Value, body: Value, at: Value| json!({"author": author, "state": "COMMENTED", "body": body, "submittedAt": at});
    // (name, view, inline pages, thread pages, extra argv, crashes in JS)
    type Case<'a> = (&'a str, Value, Value, Value, Vec<&'a str>, bool);
    let mut cases: Vec<Case> = Vec::new();
    let mut v = base_view();
    v["reviews"] = json!([review(
        json!({"login": "bob"}),
        bad.clone(),
        json!("2026-10-01T00:00:00Z")
    )]);
    cases.push(("human-review-body", v, json!([]), json!([]), vec![], true));
    let mut v = base_view();
    v["reviews"] = json!([review(json!({"login": "x[bot]"}), bad.clone(), json!(""))]);
    cases.push((
        "bot-review-body",
        v.clone(),
        json!([]),
        json!([]),
        vec![],
        false,
    ));
    cases.push((
        "bot-review-body-flag",
        v,
        json!([]),
        json!([]),
        vec!["--bot-comments"],
        true,
    ));
    let mut v = base_view();
    v["body"] = bad.clone();
    cases.push(("view-body", v, json!([]), json!([]), vec![], true));
    let mut v = base_view();
    v["title"] = json!(["a", bad.clone()]);
    cases.push(("view-title-array", v, json!([]), json!([]), vec![], true));
    let mut v = base_view();
    v["additions"] = bad.clone();
    cases.push(("view-additions", v, json!([]), json!([]), vec![], true));
    cases.push((
        "thread-login",
        base_view(),
        json!([]),
        json!([[{"user": {"login": bad.clone()}, "body": "b"}]]),
        vec![],
        true,
    ));
    cases.push((
        "inline-login",
        base_view(),
        json!([[{"user": {"login": [bad.clone()]}, "body": "b"}]]),
        json!([]),
        vec![],
        true,
    ));
    cases.push((
        "user-itself",
        base_view(),
        json!([[{"user": bad.clone(), "body": "b"}]]),
        json!([]),
        vec![],
        false,
    ));
    let mut v = base_view();
    v["reviews"] = json!([review(json!({"login": "bob"}), json!("b"), bad.clone())]);
    cases.push((
        "review-at-no-cutoff",
        v.clone(),
        json!([]),
        json!([]),
        vec![],
        false,
    ));
    cases.push((
        "review-at-cutoff",
        v,
        json!([]),
        json!([]),
        vec!["--thread-before", "2026-10-02T00:00:00Z"],
        true,
    ));
    let mut v = base_view();
    v["body"] = json!({"valueOf": 1});
    cases.push(("value-of-only", v, json!([]), json!([]), vec![], false));
    for (name, v, inline, thread, extra, crashes) in cases {
        let mut calls = common_calls(pr, &v, &inline, &thread, "alice");
        calls.push(rec(&diff_args(pr), "d", "", 0));
        let out = same(&format!("conv-{name}"), calls, &argv(pr, &extra), pr).await;
        assert_eq!(out.is_none(), crashes, "{name}");
    }

    // Pinned: a base that cannot be printed stops the script before the
    // compare calls and before the patch is written; a count that cannot be
    // converted stops it after.
    let mut v = base_view();
    v["baseRefOid"] = bad.clone();
    let calls = common_calls(pr, &v, &json!([]), &json!([]), "alice");
    assert!(
        same("conv-base", calls, &argv(pr, &["--head-sha", PIN]), pr)
            .await
            .is_none()
    );
    let mut calls = common_calls(pr, &base_view(), &json!([]), &json!([]), "alice");
    calls.push(rec(&compare_diff_args(BASE, PIN), "d", "", 0));
    calls.push(rec(
        &compare_args(BASE, PIN),
        &json!({"files": [{"filename": "a", "additions": bad, "deletions": 1}]}).to_string(),
        "",
        0,
    ));
    assert!(
        same("conv-count", calls, &argv(pr, &["--head-sha", PIN]), pr)
            .await
            .is_none()
    );
}

/// `Number(x) || 0` on every count shape `Number` reads differently from a
/// plain decimal, unpinned (the view's totals) and pinned (each file's).
#[tokio::test]
async fn fetch_pr_context_matches_js_on_count_strings() {
    let shapes = [
        json!(" 12 "),
        json!("\u{a0}7\u{feff}"),
        json!("1e3"),
        json!(".5"),
        json!("5."),
        json!("+.5"),
        json!("-0x10"),
        json!("0x1F"),
        json!("0b101"),
        json!("0o17"),
        json!("0x"),
        json!("1_000"),
        json!("Infinity"),
        json!("-Infinity"),
        json!("infinity"),
        json!("1n"),
        json!("00012"),
        json!("1.2.3"),
        json!("1e"),
        json!("\u{85}3"),
        json!([]),
        json!([5]),
        json!([1, 2]),
        json!([null]),
        json!([[" 4 "]]),
        json!(true),
        json!({}),
        json!(-0.0),
    ];
    let pr = "405";
    for (i, x) in shapes.iter().enumerate() {
        let mut v = view(1, "dev");
        v["additions"] = x.clone();
        v["deletions"] = json!([x.clone()]);
        let mut calls = common_calls(pr, &v, &json!([]), &json!([]), "alice");
        calls.push(rec(&diff_args(pr), "d", "", 0));
        same(&format!("count-{i}"), calls, &argv(pr, &[]), pr)
            .await
            .unwrap();
        let cf = json!({"files": [
            {"filename": "a", "additions": x, "deletions": 1},
            {"filename": "b", "additions": 2, "deletions": x},
        ]});
        let mut calls = common_calls(pr, &view(1, "dev"), &json!([]), &json!([]), "alice");
        calls.push(rec(&compare_diff_args(BASE, PIN), "d", "", 0));
        calls.push(rec(&compare_args(BASE, PIN), &cf.to_string(), "", 0));
        same(
            &format!("count-pinned-{i}"),
            calls,
            &argv(pr, &["--head-sha", PIN]),
            pr,
        )
        .await
        .unwrap();
    }
}

/// `trim()` decides which bodies count as text, and what the viewer is.
/// U+0085 and U+180E and U+200B are not JavaScript whitespace; U+FEFF and
/// U+3000 are.
#[tokio::test]
async fn fetch_pr_context_matches_js_on_blank_lookalikes() {
    let pr = "406";
    for (i, blank) in [
        "\u{85}",
        "\u{180e}",
        "\u{200b}",
        "\u{feff}",
        "\u{3000}\u{2028}",
        "\r\n\u{b}",
    ]
    .iter()
    .enumerate()
    {
        let mut v = view(1, "dev");
        v["body"] = json!(blank);
        v["reviews"] = json!([
            {"author": {"login": "bob"}, "state": "COMMENTED", "body": blank},
            {"author": {"login": "x[bot]"}, "state": "COMMENTED", "body": blank},
        ]);
        let mut calls = common_calls(
            pr,
            &v,
            &json!([]),
            &json!([]),
            &format!("{blank}alice{blank}"),
        );
        calls.push(rec(&diff_args(pr), "d", "", 0));
        same(
            &format!("blank-{i}"),
            calls,
            &argv(pr, &["--bot-comments"]),
            pr,
        )
        .await
        .unwrap();
    }
}

/// Errors Node names by code that the first cases never raised: a patch
/// name past the file system's limit, and a temp dir that is a symlink loop.
#[tokio::test]
async fn fetch_pr_context_matches_js_on_more_write_errors() {
    let pr = format!("1{}", "0".repeat(260));
    let mut calls = common_calls(&pr, &view(1, "x"), &json!([]), &json!([]), "alice");
    calls.push(rec(&diff_args(&pr), "d", "", 0));
    let out = same("name-too-long", calls, &argv(&pr, &[]), &pr)
        .await
        .unwrap();
    assert_eq!(out["errors"][0]["step"], "diff-write");

    let pr = "404";
    let dir = scratch("fetch-loop");
    let mut calls = common_calls(pr, &view(1, "x"), &json!([]), &json!([]), "alice");
    calls.push(rec(&diff_args(pr), "d", "", 0));
    let js_loop = dir.join("js-loop");
    let rs_loop = dir.join("rs-loop");
    std::os::unix::fs::symlink(&js_loop, &js_loop).unwrap();
    std::os::unix::fs::symlink(&rs_loop, &rs_loop).unwrap();
    let out = same_in(
        &dir,
        calls,
        &argv(pr, &[]),
        pr,
        Tmp::Dir(js_loop.to_string_lossy().into_owned()),
        Tmp::Dir(rs_loop.to_string_lossy().into_owned()),
        "loop",
    )
    .await
    .unwrap();
    assert_eq!(out["errors"][0]["step"], "diff-write");
}

/// A `gh` that writes raw bytes, in two writes when the recording splits
/// them, so invalid UTF-8 and a sequence cut between reads reach both sides.
const BYTES_GH: &str = r#"#!/usr/bin/env node
const { readFileSync, appendFileSync } = require("node:fs");
const args = process.argv.slice(2);
if (process.env.GH_CALL_LOG) appendFileSync(process.env.GH_CALL_LOG, JSON.stringify(args) + "\n");
const hit = JSON.parse(readFileSync(process.env.GH_BYTES, "utf8")).find((c) => JSON.stringify(c.args) === JSON.stringify(args));
if (!hit) process.exit(99);
const out = Buffer.from(hit.hex, "hex");
const cut = hit.cut ?? out.length;
process.stdout.write(out.subarray(0, cut));
setTimeout(() => { process.stdout.write(out.subarray(cut)); process.exitCode = hit.code; }, 150);
"#;

const BYTES_DRIVER: &str = r#"
import { pathToFileURL } from "node:url";
const [tmp, log, ghdir, script, ...rest] = process.argv.slice(2);
process.env.TMPDIR = tmp;
process.env.GH_CALL_LOG = log;
process.env.PATH = ghdir + ":" + process.env.PATH;
process.argv = [process.argv[0], script, ...rest];
await import(pathToFileURL(script).href);
"#;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[tokio::test]
async fn fetch_pr_context_matches_js_on_invalid_utf8() {
    let dir = scratch("fetch-bytes");
    let ghdir = dir.join("bin");
    std::fs::create_dir_all(&ghdir).unwrap();
    let gh = ghdir.join("gh");
    std::fs::write(&gh, BYTES_GH).unwrap();
    std::fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let tails: [(&[u8], Option<usize>); 8] = [
        (b"caf\xe9 latin-1\n", None),
        (b"A\xe0\x80A\xed\xa0\x80A\xf4\x90\x80\x80\xff\xfe\n", None),
        (b"ends mid sequence \xe2\x82", None),
        (b"ends \xe0\x80", None),
        (b"split \xe2\x82\xac euro", Some(7)),
        (b"split \xf0\x9f\x98\x80 emoji", Some(8)),
        (b"split \xe0\x80 bad", Some(7)),
        (b"\xef\xbb\xbfbom", None),
    ];
    for (i, (tail, cut)) in tails.iter().enumerate() {
        let pr = format!("{}", 420 + i);
        let mut diff = diff_text(13).into_bytes();
        let cut = cut.map(|c| diff.len() + c);
        diff.extend_from_slice(tail);
        let viewer = format!("al\u{e9}x {}", String::from_utf8_lossy(tail));
        let mut stderr_like = b"x".to_vec();
        stderr_like.extend_from_slice(tail);
        let entries = json!([
            {"args": view_args(&pr), "hex": hex(view(13, "dev").to_string().as_bytes()), "code": 0},
            {"args": inline_args(&pr), "hex": hex(b"[]"), "code": 0},
            {"args": thread_args(&pr), "hex": hex(&stderr_like), "code": 0},
            {"args": viewer_args(), "hex": hex(viewer.as_bytes()), "code": 0},
            {"args": diff_args(&pr), "hex": hex(&diff), "cut": cut, "code": 0},
        ]);
        let rec_path = dir.join(format!("bytes-{i}.json"));
        std::fs::write(&rec_path, entries.to_string()).unwrap();
        let js_tmp = dir.join(format!("js-{i}"));
        let rs_tmp = dir.join(format!("rs-{i}"));
        std::fs::create_dir_all(&js_tmp).unwrap();
        std::fs::create_dir_all(&rs_tmp).unwrap();
        let js_log = dir.join(format!("js-{i}.log"));
        let rs_log = dir.join(format!("rs-{i}.log"));
        let a = argv(&pr, &[]);
        let name = format!("bytes-{i}");
        let input = sha256_hex(
            json!({"entries": entries, "argv": a})
                .to_string()
                .as_bytes(),
        );
        let patch_of =
            |d: &Path| std::fs::read(d.join(format!("peer-review-diff-{pr}.patch"))).unwrap();
        let js = match recorded("fetch") {
            Some(golden) => {
                let kept = &golden[&name];
                assert!(!kept.is_null(), "{name}: not in the golden; {REGENERATE}");
                assert_eq!(
                    kept["inputSha256"], input,
                    "{name}: the case changed; {REGENERATE}"
                );
                kept.clone()
            }
            None => {
                let driver = write_driver(&dir, "bytes-driver.mjs", BYTES_DRIVER);
                let mut args = vec![
                    js_tmp.to_string_lossy().into_owned(),
                    js_log.to_string_lossy().into_owned(),
                    ghdir.to_string_lossy().into_owned(),
                    plugin_root().join(SCRIPT).to_string_lossy().into_owned(),
                ];
                args.extend(a.iter().cloned());
                let out = Command::new("node")
                    .arg(&driver)
                    .args(&args)
                    .env("GH_BYTES", &rec_path)
                    .output()
                    .unwrap();
                assert!(
                    out.status.success(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
                let js_out = String::from_utf8(out.stdout).unwrap();
                let kept = json!({
                    "inputSha256": input,
                    "stdout": js_out.replace(js_tmp.to_str().unwrap(), "<TMP>"),
                    "calls": read_calls(&js_log),
                    "patch": hex(&patch_of(&js_tmp)),
                });
                record("fetch", &name, kept.clone(), &dir);
                kept
            }
        };
        let t = Tools {
            gh: gh.to_string_lossy().into_owned(),
            git: "git".into(),
            env: vec![
                ("GH_BYTES".into(), rec_path.clone().into_os_string()),
                ("GH_CALL_LOG".into(), rs_log.clone().into_os_string()),
            ],
        };
        let rs_out = fpc::run_cli(&t, &a, rs_tmp.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(
            js["stdout"].as_str().unwrap(),
            rs_out.replace(rs_tmp.to_str().unwrap(), "<TMP>"),
            "case {i}"
        );
        let js_calls: Vec<String> = serde_json::from_value(js["calls"].clone()).unwrap();
        assert_eq!(js_calls, read_calls(&rs_log), "case {i}");
        assert_eq!(
            unhex(js["patch"].as_str().unwrap()),
            patch_of(&rs_tmp),
            "case {i}"
        );
    }
}

/// A relative `TMPDIR` is resolved by the bash against the directory it runs
/// in, which is the repository the checkout is built from.
#[tokio::test]
async fn head_worktree_matches_the_bash_with_a_relative_tmpdir() {
    let dir = wt_scratch("wt-relative");
    let o = origin(&dir);
    let (js, rs) = twin_pair(&dir, &o);
    std::fs::create_dir_all(js.repo.join("rel-tmp")).unwrap();
    std::fs::create_dir_all(rs.repo.join("rel-tmp")).unwrap();
    let rec_path = recordings(&dir, vec![]);
    let context = ctx(&o.c2);
    let head = |repo: &Path, wt: &str| {
        git(
            &repo.join(&wt["CREATED_WORKTREE ".len()..]),
            &["rev-parse", "HEAD"],
        )
    };
    let input = json!({"context": context, "repo": REPO, "pr": "330", "tmp": "rel-tmp"});
    let kept = worktree_side("relative", &dir, input, || {
        let driver = write_driver(&dir, "worktree-driver.mjs", WORKTREE_DRIVER);
        let script_path = dir.join("head-worktree.sh");
        let plugin = plugin_root();
        let args = [
            "prep",
            plugin.to_str().unwrap(),
            "rel-tmp",
            js.log.to_str().unwrap(),
            REPO,
            "330",
            &context.to_string(),
            script_path.to_str().unwrap(),
        ];
        let out = run_script(driver.to_str().unwrap(), &args, &js.repo, &rec_path, None);
        assert_eq!(out.code, Some(0), "{}", out.stderr);
        let js_wt = last_json(&out.stdout)["worktree"]
            .as_str()
            .unwrap()
            .to_owned();
        json!({"worktree": mask_suffix(&js_wt, "330"), "head": head(&js.repo, &js_wt)})
    });
    let js_wt = kept["worktree"].as_str().unwrap();
    let mut t: Tools = tools(&rec_path);
    t.env.push(("TMPDIR".into(), "rel-tmp".into()));
    let rs_wt = hw::prepare_head_worktree(&t, &context, REPO, "330", Some(&rs.repo), "rel-tmp")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mask_suffix(js_wt, "330"), mask_suffix(&rs_wt, "330"));
    assert!(
        js_wt.starts_with("CREATED_WORKTREE rel-tmp/peer-review-wt-330-"),
        "{js_wt}"
    );
    assert_eq!(kept["head"], json!(o.c2));
    assert_eq!(head(&rs.repo, &rs_wt), o.c2);
}

/// A `gh` whose `repo clone` really clones, from the bare remote in
/// `GH_BARE`, so the clone fallback's success branch runs on both sides.
const CLONING_GH: &str = r#"#!/usr/bin/env node
const { appendFileSync } = require("node:fs");
const { spawnSync } = require("node:child_process");
const args = process.argv.slice(2);
if (process.env.GH_CALL_LOG) appendFileSync(process.env.GH_CALL_LOG, JSON.stringify(args) + "\n");
if (args[0] !== "repo" || args[1] !== "clone" || args[4] !== "--") process.exit(99);
const r = spawnSync("git", ["clone", ...args.slice(5), "file://" + process.env.GH_BARE, args[3]], { stdio: "inherit" });
process.exit(r.status ?? 1);
"#;

/// The clone fallback when it succeeds: the remote is not the PR's repo, so
/// no worktree is tried, and `gh repo clone` plus a fetch of the sha build
/// the checkout.
#[tokio::test]
async fn head_worktree_clones_as_the_bash_does() {
    let dir = wt_scratch("wt-clone");
    let o = origin(&dir);
    let (js, rs) = twin_pair(&dir, &o);
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    std::fs::write(&gh, CLONING_GH).unwrap();
    std::fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    // c2 is on `feature`, which a --depth 1 clone of `main` does not hold.
    let context = ctx(&o.c2);
    let input = json!({"context": context, "repo": "other/thing", "pr": "350"});
    let a = worktree_side("clone", &dir, input, || {
        let driver = write_driver(&dir, "worktree-driver.mjs", WORKTREE_DRIVER);
        let script_path = dir.join("head-worktree.sh");
        let plugin = plugin_root();
        let path = std::env::join_paths(
            std::iter::once(bin.clone())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let out = Command::new("node")
            .arg(&driver)
            .args([
                "prep",
                plugin.to_str().unwrap(),
                js.tmp.to_str().unwrap(),
                js.log.to_str().unwrap(),
                "other/thing",
                "350",
                &context.to_string(),
                script_path.to_str().unwrap(),
            ])
            .current_dir(&js.repo)
            .env("PATH", path)
            .env("GH_BARE", &o.bare)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let js_wt = last_json(&String::from_utf8_lossy(&out.stdout))["worktree"].clone();
        state(&js.repo, &js.tmp, js_wt, Value::Null, &js.log, "350").to_json()
    });
    let a = WtState::from_json(&a);
    let t = Tools {
        gh: gh.to_string_lossy().into_owned(),
        git: "git".into(),
        env: vec![
            ("GH_BARE".into(), o.bare.clone().into_os_string()),
            ("GH_CALL_LOG".into(), rs.log.clone().into_os_string()),
        ],
    };
    let rs_wt = hw::prepare_head_worktree(
        &t,
        &context,
        "other/thing",
        "350",
        Some(&rs.repo),
        rs.tmp.to_str().unwrap(),
    )
    .await
    .unwrap()
    .map_or(Value::Null, Value::String);
    let b = state(&rs.repo, &rs.tmp, rs_wt, Value::Null, &rs.log, "350");
    assert_eq!(a, b);
    assert!(
        a.worktree
            .as_str()
            .unwrap()
            .starts_with("CLONED:<TMP>/peer-review-wt-350-"),
        "{a:?}"
    );
    assert_eq!(a.head.as_deref(), Some(o.c2.as_str()));
    assert_eq!(a.gh_calls.len(), 1);
}

/// The region's functions throw where a template literal or `String()` meets
/// an object with its own `toString`.
#[test]
fn head_worktree_region_throws_where_js_does() {
    let bad = json!({"toString": 1});
    let calls = vec![
        json!({"fn": "parseHeadWorktree", "args": [bad]}),
        json!({"fn": "parseHeadWorktree", "args": [["CLONED:/x", bad]]}),
        json!({"fn": "parseHeadWorktree", "args": [{"valueOf": 1}]}),
        json!({"fn": "pinnedCheckoutBlock", "args": [null, bad]}),
        json!({"fn": "pinnedCheckoutBlock", "args": [{"ok": true, "path": bad}, HEAD]}),
        json!({"fn": "pinnedCheckoutBlock", "args": [{"ok": false, "path": bad}, HEAD]}),
        json!({"fn": "pinnedCheckoutBlock", "args": [{"ok": true, "path": [bad]}, HEAD]}),
    ];
    let js = region_results("throws", &calls);
    for (c, j) in calls.iter().zip(js) {
        let args = c["args"].as_array().unwrap();
        let rs: Result<Value, String> = if c["fn"] == "parseHeadWorktree" {
            hw::parse_head_worktree(args.first()).map(|p| p.to_value())
        } else {
            hw::pinned_checkout_block(args.first(), args.get(1)).map(Value::String)
        };
        match (j.get("ok"), rs) {
            (Some(a), Ok(b)) => assert_eq!(*a, b, "{c}"),
            (None, Err(_)) => {}
            (a, b) => panic!("{c}: js {a:?}, rust {b:?}"),
        }
    }
}

/// `prepareReview` prunes, then rejects when Node cannot put the sha in the
/// bash's environment; the port returns an error at the same point.
#[tokio::test]
async fn head_worktree_rejects_a_sha_node_cannot_pass() {
    let dir = wt_scratch("wt-badsha");
    let o = origin(&dir);
    let (js, rs) = twin_pair(&dir, &o);
    let rec_path = recordings(&dir, vec![]);
    let left = |t: &Path| std::fs::read_dir(t).unwrap().count();
    for (i, sha) in [
        json!({"toString": 1}),
        json!("ab\u{0}cd"),
        json!([{"toString": 1}]),
    ]
    .into_iter()
    .enumerate()
    {
        let context =
            json!({"isDeploymentPR": false, "headRefOid": sha, "diffPath": "", "files": []});
        let pr = format!("{}", 340 + i);
        let input = json!({"context": context, "repo": REPO, "pr": pr});
        let kept = worktree_side(&format!("badsha-{i}"), &dir, input, || {
            let driver = write_driver(&dir, "worktree-driver.mjs", WORKTREE_DRIVER);
            let script_path = dir.join("head-worktree.sh");
            let plugin = plugin_root();
            let args = [
                "prep",
                plugin.to_str().unwrap(),
                js.tmp.to_str().unwrap(),
                js.log.to_str().unwrap(),
                REPO,
                &pr,
                &context.to_string(),
                script_path.to_str().unwrap(),
            ];
            let out = run_script(driver.to_str().unwrap(), &args, &js.repo, &rec_path, None);
            json!({"code": out.code, "stdout": out.stdout, "left": left(&js.tmp)})
        });
        assert_ne!(kept["code"], json!(0), "case {i}: {}", kept["stdout"]);
        let rs_out = hw::prepare_head_worktree(
            &tools(&rec_path),
            &context,
            REPO,
            &pr,
            Some(&rs.repo),
            rs.tmp.to_str().unwrap(),
        )
        .await;
        assert!(rs_out.is_err(), "case {i}: {rs_out:?}");
        assert_eq!(kept["left"], json!(0));
        assert_eq!(left(&rs.tmp), 0);
    }
}

/// The staleness check on a copy of the plugin rebuilt from the golden: an
/// oracle holds while the copy matches, fails once a file it reads is edited,
/// and is skipped once a file it reads is deleted, since a retired oracle's
/// golden is final.
#[test]
fn a_retired_oracle_is_not_held_to_the_golden() {
    // Regeneration rewrites the golden while this would read it.
    if regenerating() {
        return;
    }
    let golden: Value =
        serde_json::from_str(&std::fs::read_to_string(golden_path()).unwrap()).unwrap();
    let with = &golden["recordedWith"];
    let root = scratch("fetch-retired");
    let shim = root.join("shim/gh");
    for texts in with.as_object().unwrap().values() {
        for (key, text) in texts.as_object().unwrap() {
            let path = match key.split_once('#') {
                Some((file, _)) => root.join(file),
                None if key == "gh shim" => shim.clone(),
                None if key.contains('/') => root.join(key),
                // This file's own drivers and fake `gh`s.
                None => continue,
            };
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text.as_str().unwrap()).unwrap();
        }
    }
    let now = || oracle_texts(&root, &shim);
    assert_eq!(stale(with, &now()), Vec::<&str>::new());

    let script = root.join(SCRIPT);
    let text = std::fs::read_to_string(&script).unwrap();
    std::fs::write(&script, format!("{text}// edited\n")).unwrap();
    assert_eq!(stale(with, &now()), ["fetch", "worktree"]);
    std::fs::remove_file(&script).unwrap();
    let n = now();
    assert!(n["fetch"].is_none() && n["worktree"].is_none() && n["region"].is_some());
    assert_eq!(stale(with, &n), Vec::<&str>::new());
    std::fs::write(&script, text).unwrap();

    let workflow = root.join(WORKFLOW);
    let text = std::fs::read_to_string(&workflow).unwrap();
    let begin = format!("BEGIN {REGION} =====");
    std::fs::write(
        &workflow,
        text.replace(&begin, &format!("{begin}\n// edited")),
    )
    .unwrap();
    assert_eq!(stale(with, &now()), ["region", "worktree"]);
    std::fs::remove_file(&workflow).unwrap();
    let n = now();
    assert!(n["fetch"].is_some() && n["worktree"].is_none() && n["region"].is_none());
    assert_eq!(stale(with, &n), Vec::<&str>::new());
}
