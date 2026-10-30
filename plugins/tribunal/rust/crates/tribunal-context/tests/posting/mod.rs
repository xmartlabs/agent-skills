//! Shared harness for the posting and ledger differential tests: each case
//! compares the port with what the real script printed on the same inputs,
//! read from a golden under `tests/goldens/`; only a regeneration
//! (`TRIBUNAL_REGEN_GOLDENS=1`) runs the script under node.
//!
//! The scripts run with a cleared environment: `HOME`, `TRIBUNAL_DATA_DIR`
//! and `TMPDIR` point at the case's temp directories, and `PATH` holds only
//! the case's fake `gh`, a directory with `node` in it, `/usr/bin` and
//! `/bin`, so neither side can reach a real `gh` or the real ledger.
//!
//! A golden keeps the text or SHA-256 of the modules it was recorded from. A
//! module no longer on disk was retired with the Workflow, so its golden is
//! final and its staleness check is skipped; the comparison still runs.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};
use tribunal_context::harness_version::sha256_hex;

use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::TestRunner;
use tribunal_context::operator::CliOutput;

pub const CASES: usize = 600;

/// The ledger lock wait the tests that hold the lock give both sides, through
/// `TRIBUNAL_LEDGER_LOCK_WAIT_MS` and `LedgerEnv::lock_wait`.
pub const LOCK_WAIT_MS: u64 = 300;

pub fn sample<S: Strategy>(n: usize, strategy: S) -> Vec<S::Value> {
    let mut runner = TestRunner::deterministic();
    (0..n)
        .map(|_| strategy.new_tree(&mut runner).unwrap().current())
        .collect()
}

pub fn plugin_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

pub fn script(name: &str) -> PathBuf {
    plugin_root().join("skills/review-pr/scripts").join(name)
}

/// A fresh directory unique to this process and `name`, with its real path
/// (the scripts see `process.cwd()` through symlinks resolved).
pub fn scratch(name: &str) -> PathBuf {
    let prefix = format!("tribunal-posting-{name}-");
    sweep_dead(&std::env::temp_dir(), &prefix);
    let dir = std::env::temp_dir().join(format!("{prefix}{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

/// Removes `<prefix><pid>` directories whose process has exited, so runs do
/// not leave their scratch trees behind.
pub fn sweep_dead(base: &Path, prefix: &str) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix(prefix))
            .and_then(|p| p.parse::<i32>().ok())
        else {
            continue;
        };
        let gone = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None)
            == Err(nix::errno::Errno::ESRCH);
        if gone {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// A directory holding only a `node` symlink, so `PATH` need not include
/// wherever else `node` lives (Homebrew's bin also holds the real `gh`).
pub fn node_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let out = Command::new("sh")
            .args(["-c", "command -v node"])
            .output()
            .expect("sh");
        let node = String::from_utf8(out.stdout).unwrap().trim().to_owned();
        assert!(!node.is_empty(), "node must be on PATH");
        let dir = scratch("node-bin");
        std::os::unix::fs::symlink(&node, dir.join("node")).unwrap();
        dir
    })
    .clone()
}

/// `PATH` for a script: the fake `gh` directory first when there is one.
pub fn safe_path(fake_dir: Option<&Path>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(d) = fake_dir {
        parts.push(d.to_string_lossy().into_owned());
    }
    parts.push(node_bin().to_string_lossy().into_owned());
    parts.push("/usr/bin".into());
    parts.push("/bin".into());
    parts.join(":")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `node <script> <args>` in `cwd` with only `env` (plus `PATH`).
pub fn node(script: &Path, args: &[String], cwd: &Path, env: &[(&str, String)], path: &str) -> Run {
    let out = Command::new(node_bin().join("node"))
        .arg(script)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", path)
        .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
        .stdin(Stdio::null())
        .output()
        .expect("node");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// Node printed an uncaught exception: a stack, which the port does not
/// reproduce.
pub fn crashed(js: &Run) -> bool {
    js.stderr.contains("\n    at ")
}

/// The difference between the script's run and the port's, or `None`.
/// stderr is compared except where the script crashed.
pub fn diff_cli(js: &Run, rs: &CliOutput) -> Option<String> {
    let mut d = Vec::new();
    if js.code != rs.code {
        d.push(format!("code js {} rs {}", js.code, rs.code));
    }
    if js.stdout != rs.stdout {
        d.push(format!(
            "stdout\n  js {:?}\n  rs {:?}",
            js.stdout, rs.stdout
        ));
    }
    if !crashed(js) && js.stderr != rs.stderr {
        d.push(format!(
            "stderr\n  js {:?}\n  rs {:?}",
            js.stderr, rs.stderr
        ));
    }
    (!d.is_empty()).then(|| d.join("\n"))
}

/// Runs `f` over `items` on a few threads, keeping order.
pub fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(usize, &T) -> R + Sync) -> Vec<R> {
    let workers = 8.min(items.len().max(1));
    let mut out: Vec<Option<R>> = (0..items.len()).map(|_| None).collect();
    let chunks: Vec<(usize, &mut [Option<R>])> = {
        let size = items.len().div_ceil(workers).max(1);
        out.chunks_mut(size)
            .enumerate()
            .map(|(i, c)| (i * size, c))
            .collect()
    };
    std::thread::scope(|s| {
        for (start, chunk) in chunks {
            let f = &f;
            s.spawn(move || {
                for (j, slot) in chunk.iter_mut().enumerate() {
                    *slot = Some(f(start + j, &items[start + j]));
                }
            });
        }
    });
    out.into_iter().map(Option::unwrap).collect()
}

/// Fails with the first differing case and the count.
pub fn assert_same(what: &str, diffs: &[Option<String>]) {
    let bad: Vec<(usize, &String)> = diffs
        .iter()
        .enumerate()
        .filter_map(|(i, d)| d.as_ref().map(|d| (i, d)))
        .collect();
    assert!(
        bad.is_empty(),
        "{what}: {} of {} cases differ. First (case {}):\n{}",
        bad.len(),
        diffs.len(),
        bad[0].0,
        bad[0].1
    );
}

/// Reads a file, `None` when it is not there.
pub fn read_opt(p: &Path) -> Option<String> {
    std::fs::read(p)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

// ---------------------------------------------------------------------------
// Goldens: a script's side recorded once, so a run starts only the port.

/// `TRIBUNAL_REGEN_GOLDENS=1`: run the script and write the golden again.
pub fn regenerating() -> bool {
    std::env::var("TRIBUNAL_REGEN_GOLDENS").is_ok_and(|v| v == "1")
}

pub fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/goldens/{name}"))
}

/// The text of `entry` (a path in the plugin) and of every module it reaches
/// through a relative import, by path in the plugin. Bare specifiers
/// (`node:fs`) are Node's own.
pub fn module_texts(entry: &str) -> std::collections::BTreeMap<String, String> {
    let import = regex::Regex::new(r#"(?:from|import)\s*\(?\s*"(\.{1,2}/[^"]+)""#).unwrap();
    let root = plugin_root();
    let mut seen = std::collections::BTreeMap::new();
    let mut todo = vec![root.join(entry).canonicalize().unwrap()];
    while let Some(file) = todo.pop() {
        let key = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if seen.contains_key(&key) {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for m in import.captures_iter(&text) {
            let dep = file.parent().unwrap().join(&m[1]);
            todo.push(
                dep.canonicalize()
                    .unwrap_or_else(|e| panic!("{}: {e}", dep.display())),
            );
        }
        seen.insert(key, text);
    }
    seen
}

/// The text of `rel`, a path in the plugin; `None` once it is gone.
pub fn on_disk(rel: &str) -> Option<String> {
    on_disk_in(&plugin_root(), rel)
}

fn on_disk_in(root: &Path, rel: &str) -> Option<String> {
    match std::fs::read_to_string(root.join(rel)) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => panic!("{rel}: {e}"),
    }
}

/// `p` with `.` and `..` folded away, without touching the disk.
fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

/// [`module_texts`] for a tree the Workflow's retirement may have thinned:
/// `None` when `entry` is gone, and a module it imports that is gone maps to
/// `None` instead of failing.
pub fn module_texts_opt(entry: &str) -> Option<BTreeMap<String, Option<String>>> {
    module_texts_in(&plugin_root(), entry)
}

fn module_texts_in(root: &Path, entry: &str) -> Option<BTreeMap<String, Option<String>>> {
    let import = regex::Regex::new(r#"(?:from|import)\s*\(?\s*"(\.{1,2}/[^"]+)""#).unwrap();
    on_disk_in(root, entry)?;
    let mut seen = BTreeMap::new();
    let mut todo = vec![root.join(entry).canonicalize().unwrap()];
    while let Some(file) = todo.pop() {
        let key = file
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if seen.contains_key(&key) {
            continue;
        }
        let Some(text) = on_disk_in(root, &key) else {
            seen.insert(key, None);
            continue;
        };
        for m in import.captures_iter(&text) {
            let dep = file.parent().unwrap().join(&m[1]);
            todo.push(dep.canonicalize().unwrap_or_else(|_| lexical(&dep)));
        }
        seen.insert(key, Some(text));
    }
    Some(seen)
}

/// The modules in `recorded` (`{path: kept}`, `kept` being what `keep` makes
/// of a module's text) that `entry` no longer reaches as recorded: changed
/// text, a new import, or a recorded module still on disk that it stopped
/// importing. A module that is gone is retired and skipped; with `entry`
/// itself gone the golden is final and nothing is.
pub fn stale_modules_by(
    recorded: &Value,
    entry: &str,
    keep: impl Fn(&str) -> Value,
) -> Vec<String> {
    stale_modules_in(&plugin_root(), recorded, entry, keep)
}

/// [`stale_modules_by`] over the plugin tree at `root`.
pub fn stale_modules_in(
    root: &Path,
    recorded: &Value,
    entry: &str,
    keep: impl Fn(&str) -> Value,
) -> Vec<String> {
    let Some(now) = module_texts_in(root, entry) else {
        return Vec::new();
    };
    let mut names: BTreeSet<&String> = now.keys().collect();
    names.extend(recorded.as_object().into_iter().flat_map(|m| m.keys()));
    names
        .into_iter()
        .filter(|k| match now.get(*k) {
            Some(Some(text)) => recorded.get(k.as_str()) != Some(&keep(text)),
            Some(None) => false,
            None => on_disk_in(root, k).is_some(),
        })
        .cloned()
        .collect()
}

/// [`stale_modules_by`] for a golden that keeps the modules' text.
pub fn stale_modules(recorded: &Value, entry: &str) -> Vec<String> {
    stale_modules_by(recorded, entry, |t| json!(t))
}

/// The SHA-256 of every module `entry` reaches, for a golden that keeps
/// hashes; regenerating needs them all on disk.
pub fn module_shas(entry: &str) -> Value {
    json!(module_texts(entry)
        .into_iter()
        .map(|(k, t)| (k, sha256_hex(t.as_bytes())))
        .collect::<BTreeMap<_, _>>())
}

/// [`stale_modules_by`] for a golden made with [`module_shas`].
pub fn stale_module_shas(recorded: &Value, entry: &str) -> Vec<String> {
    stale_modules_by(recorded, entry, |t| json!(sha256_hex(t.as_bytes())))
}

/// The golden `name`, checked with `stale` against what it was recorded with
/// (its `recordedWith`), which names what changed since; `None` when
/// regenerating.
pub fn recorded(
    name: &str,
    regenerate: &str,
    stale: impl FnOnce(&Value) -> Vec<String>,
) -> Option<Value> {
    if regenerating() {
        return None;
    }
    let path = golden_path(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; {regenerate}", path.display()));
    let golden: Value = serde_json::from_str(&text).unwrap();
    let stale = stale(&golden["recordedWith"]);
    assert!(
        stale.is_empty(),
        "{stale:?} changed since {} was recorded; {regenerate}",
        path.display()
    );
    Some(golden)
}

/// The script's side of one test's fixed cases, kept under `test` in the
/// golden `name`, which the fixed tests of one binary share. Each entry keeps
/// what `with` returned when it was recorded, checked with `stale`, and each
/// case's input, checked exactly. Regenerating calls `run`, compares nothing
/// itself, and writes the entry back beside the others.
pub fn fixed(
    name: &str,
    test: &str,
    regenerate: &str,
    inputs: &[Value],
    with: impl FnOnce() -> Value,
    stale: impl FnOnce(&Value) -> Vec<String>,
    run: impl FnOnce() -> Vec<Value>,
) -> Vec<Value> {
    static WRITING: Mutex<()> = Mutex::new(());
    let path = golden_path(name);
    let read = || -> Option<Value> {
        let text = std::fs::read_to_string(&path).ok()?;
        Some(serde_json::from_str(&text).unwrap())
    };
    if regenerating() {
        let js = run();
        assert_eq!(js.len(), inputs.len(), "{test}: one result per input");
        let cases: Vec<Value> = inputs
            .iter()
            .zip(&js)
            .map(|(input, js)| json!({"input": input, "js": js}))
            .collect();
        let entry = json!({"recordedWith": with(), "cases": cases});
        let _w = WRITING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut golden = read().unwrap_or_else(|| json!({}));
        golden[test] = entry;
        let text = serde_json::to_string_pretty(&golden).unwrap();
        std::fs::write(&path, text + "\n").unwrap();
        return js;
    }
    let golden = read().unwrap_or_else(|| panic!("{}: missing; {regenerate}", path.display()));
    let entry = &golden[test];
    assert!(
        entry.is_object(),
        "{test} is not in {}; {regenerate}",
        path.display()
    );
    let stale = stale(&entry["recordedWith"]);
    assert!(
        stale.is_empty(),
        "{stale:?} changed since {test} in {} was recorded; {regenerate}",
        path.display()
    );
    let cases = entry["cases"].as_array().unwrap();
    assert_eq!(
        cases.len(),
        inputs.len(),
        "{test}: the cases changed; {regenerate}"
    );
    cases
        .iter()
        .zip(inputs)
        .map(|(c, input)| {
            assert_eq!(
                &c["input"], input,
                "{test}: the cases changed; {regenerate}"
            );
            c["js"].clone()
        })
        .collect()
}

pub fn write_golden(name: &str, recorded_with: serde_json::Value, cases: Vec<serde_json::Value>) {
    write_golden_with(name, recorded_with, cases, serde_json::json!({}));
}

/// [`write_golden`] with more top-level fields, for what a test needs back
/// that is not a script's text: `extra` must be an object.
pub fn write_golden_with(
    name: &str,
    recorded_with: serde_json::Value,
    cases: Vec<serde_json::Value>,
    extra: serde_json::Value,
) {
    let mut golden = serde_json::json!({"recordedWith": recorded_with, "cases": cases});
    for (k, v) in extra.as_object().expect("extra is an object") {
        golden[k] = v.clone();
    }
    let text = serde_json::to_string_pretty(&golden).unwrap();
    std::fs::write(golden_path(name), text + "\n").unwrap();
}

/// `v` with each `(from, to)` replaced, as text: the scratch directory and the
/// plugin go into a golden as placeholders and come back out as this run's.
pub fn swapped(v: &serde_json::Value, pairs: &[(&str, &str)]) -> serde_json::Value {
    let text = pairs
        .iter()
        .fold(v.to_string(), |t, (from, to)| t.replace(from, to));
    serde_json::from_str(&text).unwrap()
}

/// `v` with `scratch` and the plugin root as `<SCRATCH>` and `<PLUGIN>`, for
/// a golden.
pub fn to_placeholders(v: &Value, scratch: &Path) -> Value {
    let s = scratch.to_string_lossy();
    let p = plugin_root().to_string_lossy().into_owned();
    swapped(v, &[(&s, "<SCRATCH>"), (&p, "<PLUGIN>")])
}

/// [`to_placeholders`] undone with this run's paths.
pub fn from_placeholders(v: &Value, scratch: &Path) -> Value {
    let s = scratch.to_string_lossy();
    let p = plugin_root().to_string_lossy().into_owned();
    swapped(v, &[("<SCRATCH>", &s), ("<PLUGIN>", &p)])
}

/// The SHA-256 of `text`, as a golden keeps a bulky input.
pub fn sha(text: &str) -> Value {
    json!(sha256_hex(text.as_bytes()))
}

pub fn run_json(run: &Run) -> serde_json::Value {
    serde_json::json!({"code": run.code, "stdout": run.stdout, "stderr": run.stderr})
}

pub fn run_from(v: &serde_json::Value) -> Run {
    Run {
        code: i32::try_from(v["code"].as_i64().unwrap()).unwrap(),
        stdout: v["stdout"].as_str().unwrap().to_owned(),
        stderr: v["stderr"].as_str().unwrap().to_owned(),
    }
}
