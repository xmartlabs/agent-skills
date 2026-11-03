//! The Python grounded typecheck against a checker the test writes: a fake
//! `mypy` or `pyright` executable put first on the PATH the tools run with. No
//! real checker, no network and no node run here.

#![cfg(unix)]

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use tribunal_context::exec::Tools;
use tribunal_context::grounded_typecheck::{remove_tree, run};
use tribunal_context::python_typecheck::{check, Checker};
use tribunal_pipeline::verify::{apply_typecheck_result, grounded_gate};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("tribunal-pytc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A fake checker named `name` in `<root>/bin` that records its arguments in
/// `<root>/bin/args`, prints `out` and exits with `code`.
fn fake_checker(root: &Path, name: &str, out: &str, code: i32) -> Tools {
    let bin = root.join("bin");
    write(&bin.join("out.txt"), out);
    let script = format!(
        "#!/bin/sh\nd=$(dirname \"$0\")\nprintf '%s\\n' \"$@\" > \"$d/args\"\ncat \"$d/out.txt\"\nexit {code}\n"
    );
    write(&bin.join(name), &script);
    std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
    tools_with_path(&bin)
}

fn tools_with_path(first: &Path) -> Tools {
    let old = std::env::var_os("PATH").unwrap_or_default();
    let mut path = OsString::from(first);
    path.push(":");
    path.push(old);
    Tools {
        env: vec![("PATH".into(), path)],
        ..Tools::default()
    }
}

fn project(root: &Path, files: &[&str]) -> PathBuf {
    let wt = root.join("wt");
    for f in files {
        write(&wt.join(f), "x = 1\n");
    }
    wt
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| (*x).to_owned()).collect()
}

const MIN: Duration = Duration::from_secs(20);

async fn run_mypy(root: &Path, tools: &Tools, pr: &[&str], timeout: Duration) -> Vec<String> {
    check(
        tools,
        Checker::Mypy,
        root,
        &root.join("wt"),
        &s(pr),
        timeout,
    )
    .await
}

#[tokio::test]
async fn keeps_errors_in_pr_files_and_drops_the_rest() {
    let root = scratch("diag");
    project(&root, &["src/a.py", "src/b.py", "README.md"]);
    let out = "src/a.py:3:5: error: Incompatible types  [assignment]\n\
src/a.py:4:1: note: a note\n\
src/a.py:5:1: warning: a warning  [misc]\n\
src/b.py:1:1: error: in a file the PR did not touch  [misc]\n\
Found 2 errors in 2 files\n";
    let tools = fake_checker(&root, "mypy", out, 1);
    let lines = run_mypy(&root, &tools, &["src/a.py", "README.md", "gone.py"], MIN).await;
    assert_eq!(
        lines,
        vec![
            "PY_CHECKER mypy",
            "src/a.py:3:5: error: Incompatible types  [assignment]",
            "---COVERED--- 1",
            "src/a.py",
        ]
    );
    let args = std::fs::read_to_string(root.join("bin/args")).unwrap();
    assert!(args.contains("--cache-dir=/dev/null"), "{args}");
    assert!(args.lines().last() == Some("src/a.py"), "{args}");
}

#[tokio::test]
async fn a_clean_run_says_so_and_still_reports_coverage() {
    let root = scratch("clean");
    project(&root, &["a.py", "b.py"]);
    let tools = fake_checker(&root, "mypy", "", 0);
    let lines = run_mypy(&root, &tools, &["a.py", "b.py"], MIN).await;
    assert_eq!(
        lines,
        vec![
            "PY_CHECKER mypy",
            "NO_DIAGNOSTICS",
            "---COVERED--- 2",
            "a.py",
            "b.py"
        ]
    );
}

#[tokio::test]
async fn a_missing_checker_degrades() {
    let root = scratch("missing");
    project(&root, &["a.py"]);
    let tools = tools_with_path(&root.join("empty"));
    let tools = Tools {
        env: vec![("PATH".into(), root.join("empty").into())],
        ..tools
    };
    assert_eq!(
        run_mypy(&root, &tools, &["a.py"], MIN).await,
        vec!["DEGRADED_PY_NOT_INSTALLED"]
    );
}

#[tokio::test]
async fn a_checker_that_outlives_its_bound_degrades() {
    let root = scratch("slow");
    project(&root, &["a.py"]);
    let bin = root.join("bin");
    write(&bin.join("mypy"), "#!/bin/sh\nexec sleep 30\n");
    std::fs::set_permissions(bin.join("mypy"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let tools = tools_with_path(&bin);
    let started = Instant::now();
    let lines = run_mypy(&root, &tools, &["a.py"], Duration::from_millis(300)).await;
    assert_eq!(lines, vec!["DEGRADED_PY_TIMEOUT"]);
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn a_crashing_checker_degrades_instead_of_reading_clean() {
    let root = scratch("crash");
    project(&root, &["a.py"]);
    let tools = fake_checker(&root, "mypy", "mypy: error: bad config\n", 2);
    assert_eq!(
        run_mypy(&root, &tools, &["a.py"], MIN).await,
        vec!["DEGRADED_PY_CHECKER_FAILED"]
    );
}

#[tokio::test]
async fn the_diagnostic_cap_holds() {
    let root = scratch("cap");
    project(&root, &["a.py"]);
    let out: String = (1..=650)
        .map(|i| format!("a.py:{i}:1: error: m  [misc]\n"))
        .collect();
    let tools = fake_checker(&root, "mypy", &out, 1);
    let lines = run_mypy(&root, &tools, &["a.py"], MIN).await;
    let errors = lines.iter().filter(|l| l.contains(": error: ")).count();
    assert_eq!(errors, 500);
    assert_eq!(lines.last().map(String::as_str), Some("a.py"));
}

#[tokio::test]
async fn dependencies_the_environment_lacks_are_not_diagnostics() {
    let root = scratch("env");
    project(&root, &["a.py"]);
    let out = "a.py:1:1: error: Cannot find implementation or library stub for module named \"dep\"  [import-not-found]\n";
    let tools = fake_checker(&root, "mypy", out, 1);
    let lines = run_mypy(&root, &tools, &["a.py"], MIN).await;
    assert_eq!(lines[1], "NO_DIAGNOSTICS");
}

#[tokio::test]
async fn pyright_json_is_read_as_the_same_lines() {
    let root = scratch("pyright");
    let wt = project(&root, &["src/a.py"]);
    let json = format!(
        r#"{{"generalDiagnostics": [{{"file": "{}/src/a.py", "severity": "error", "message": "bad", "range": {{"start": {{"line": 1, "character": 0}}}}, "rule": "reportGeneralTypeIssues"}}]}}"#,
        wt.display()
    );
    let tools = fake_checker(&root, "pyright", &json, 1);
    let lines = check(&tools, Checker::Pyright, &root, &wt, &s(&["src/a.py"]), MIN).await;
    assert_eq!(lines[0], "PY_CHECKER pyright");
    assert_eq!(
        lines[1],
        "src/a.py:2:1: error: bad  [reportGeneralTypeIssues]"
    );
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn repo(root: &Path, files: &[(&str, &str)]) -> PathBuf {
    let dir = root.join("repo");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    git(
        &dir,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/Acme/Widgets.git",
        ],
    );
    for (p, t) in files {
        write(&dir.join(p), t);
    }
    git(&dir, &["add", "-A"]);
    git(
        &dir,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "head",
        ],
    );
    dir
}

fn head(dir: &Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

async fn typecheck(root: &Path, dir: &Path, tools: &Tools, pr: &[&str]) -> String {
    let tmp = root.join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    // The engine hands the typecheck a clone of the head commit. Since the
    // fallback that made a worktree inside `dir` is gone, the test gives it one.
    let pin = root.join("pinned");
    let _ = std::fs::remove_dir_all(&pin);
    let cloned = Command::new("git")
        .args(["clone", "-q"])
        .arg(dir)
        .arg(&pin)
        .status()
        .unwrap();
    assert!(cloned.success());
    let ours = run(
        tools,
        "acme/widgets",
        "5",
        &head(dir),
        &s(pr),
        pin.to_str(),
        dir,
        &tmp.to_string_lossy(),
    )
    .await;
    if let Some(wt) = &ours.created_worktree {
        remove_tree(tools, wt, false, dir).await;
        assert!(!Path::new(wt).exists());
    }
    ours.text.replace(&head(dir), "<SHA>")
}

fn normalized(text: &str) -> String {
    text.lines()
        .filter(|l| !l.starts_with("CREATED_WORKTREE "))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn the_run_picks_the_language_and_the_checker_from_the_repo() {
    let root = scratch("run");
    let out = "src/a.py:2:1: error: bad  [assignment]\nsrc/b.py:2:1: error: other  [assignment]\n";
    let tools = fake_checker(&root, "mypy", out, 1);
    let dir = repo(
        &root,
        &[
            ("pyproject.toml", "[tool.mypy]\nstrict = true\n"),
            ("tsconfig.json", "{}"),
            ("src/a.py", "x: int = 1\n"),
            ("src/b.py", "y = 2\n"),
        ],
    );
    let text = typecheck(&root, &dir, &tools, &["src/a.py"]).await;
    assert_eq!(
        normalized(&text),
        "PY_CHECKER mypy\nsrc/a.py:2:1: error: bad  [assignment]\n---COVERED--- 1\nsrc/a.py"
    );

    let mut logs = Vec::new();
    let g = apply_typecheck_result(&text, &s(&["src/a.py"]), &mut logs).unwrap();
    assert!(g.available, "{logs:?}");
    assert_eq!(g.covered, vec!["src/a.py"]);
    assert_eq!(g.diags_by_file.len(), 1);
    assert_eq!(g.diags_by_file.get("src/a.py").unwrap()[0].line, 2.0);
}

/// Declared difference (`grounded_typecheck::run`): without a pinned checkout
/// the workflow's bash made a worktree in `cwd` and fetched the commit into
/// it. The port stops, so the checkout it takes packages from is only read.
#[tokio::test]
async fn without_a_pinned_checkout_nothing_is_made_in_the_repo() {
    let root = scratch("nopin");
    let tools = fake_checker(&root, "mypy", "", 0);
    let dir = repo(
        &root,
        &[("pyproject.toml", "[tool.mypy]\n"), ("src/a.py", "x = 1\n")],
    );
    let tmp = root.join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let ours = run(
        &tools,
        "acme/widgets",
        "5",
        &head(&dir),
        &s(&["src/a.py"]),
        None,
        &dir,
        &tmp.to_string_lossy(),
    )
    .await;
    assert_eq!(ours.text, "DEGRADED_NO_PINNED_TREE");
    assert!(ours.created_worktree.is_none());
    let list = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&list.stdout)
            .matches("worktree ")
            .count(),
        1
    );
    assert!(!root.join("bin/args").exists());
}

#[tokio::test]
async fn no_python_config_runs_nothing() {
    let root = scratch("noconfig");
    let tools = fake_checker(&root, "mypy", "", 0);
    let dir = repo(&root, &[("src/a.py", "x = 1\n")]);
    let text = typecheck(&root, &dir, &tools, &["src/a.py"]).await;
    assert_eq!(text, "PY_NO_CONFIG");
    assert!(!root.join("bin/args").exists());
    let mut logs = Vec::new();
    let g = apply_typecheck_result(&text, &s(&["src/a.py"]), &mut logs).unwrap();
    assert!(!g.available);
    assert!(logs[0].contains("no Python typecheck config"), "{logs:?}");
}

#[tokio::test]
async fn a_pr_with_no_python_file_still_takes_the_typescript_path() {
    let root = scratch("ts");
    let tools = fake_checker(&root, "mypy", "", 0);
    let dir = repo(
        &root,
        &[
            ("pyproject.toml", "[tool.mypy]\n"),
            ("src/a.ts", "export const n = 1;\n"),
        ],
    );
    let text = typecheck(&root, &dir, &tools, &["src/a.ts"]).await;
    assert_eq!(text, "NO_TSCONFIG");
    assert!(!root.join("bin/args").exists());
}

#[tokio::test]
async fn a_degraded_checker_makes_the_gate_unavailable() {
    for marker in [
        "DEGRADED_PY_NOT_INSTALLED",
        "DEGRADED_PY_TIMEOUT",
        "DEGRADED_PY_CHECKER_FAILED",
    ] {
        let mut logs = Vec::new();
        let g = apply_typecheck_result(marker, &s(&["a.py"]), &mut logs).unwrap();
        assert!(!g.available);
        assert!(logs[0].contains("fall through to panel"), "{logs:?}");
    }
}

#[tokio::test]
async fn a_clean_python_run_confirms_coverage_for_the_gate() {
    let text = "PY_CHECKER mypy\nNO_DIAGNOSTICS\n---COVERED--- 1\nsrc/a.py";
    let mut logs = Vec::new();
    let g = apply_typecheck_result(text, &s(&["src/a.py", "src/b.py"]), &mut logs).unwrap();
    assert!(g.available);
    assert!(g.diags_by_file.is_empty());
    assert_eq!(g.covered, vec!["src/a.py"]);
    assert!(logs[0].contains("covers 1/2"), "{logs:?}");
}

fn claim(file: &str, line: u32) -> serde_json::Value {
    serde_json::json!({"file": file, "line": line, "mechanicalClaim": true, "severity": "major"})
}

#[test]
fn silence_from_the_python_checker_never_refutes_a_finding() {
    let text = "PY_CHECKER mypy\nNO_DIAGNOSTICS\n---COVERED--- 1\nsrc/a.py";
    let pr = s(&["src/a.py"]);
    let g = apply_typecheck_result(text, &pr, &mut Vec::new()).unwrap();
    let gate = grounded_gate(&[claim("src/a.py", 10)], &g, &pr, &mut Vec::new()).unwrap();
    assert!(gate.refuted.is_empty());
    assert_eq!(gate.panel_input.len(), 1);
}

#[test]
fn a_python_diagnostic_near_the_finding_still_confirms_it() {
    let text = "PY_CHECKER mypy\nsrc/a.py:12:5: error: Incompatible types  [assignment]\n---COVERED--- 1\nsrc/a.py";
    let pr = s(&["src/a.py"]);
    let g = apply_typecheck_result(text, &pr, &mut Vec::new()).unwrap();
    let gate = grounded_gate(&[claim("src/a.py", 10)], &g, &pr, &mut Vec::new()).unwrap();
    assert_eq!(gate.confirmed.len(), 1);
    assert!(gate.panel_input.is_empty());
}

#[test]
fn typescript_silence_still_refutes_a_mechanical_claim() {
    let text = "NO_DIAGNOSTICS\n---COVERED--- 1\nsrc/a.ts";
    let pr = s(&["src/a.ts"]);
    let g = apply_typecheck_result(text, &pr, &mut Vec::new()).unwrap();
    let gate = grounded_gate(&[claim("src/a.ts", 10)], &g, &pr, &mut Vec::new()).unwrap();
    assert_eq!(gate.refuted.len(), 1);
}

#[tokio::test]
async fn the_repos_own_virtualenv_is_preferred() {
    let root = scratch("venv");
    let wt = project(&root, &["a.py"]);
    let top = root.join("top");
    let bin = top.join(".venv/bin");
    write(
        &bin.join("mypy"),
        "#!/bin/sh\necho 'a.py:1:1: error: from venv  [misc]'\nexit 1\n",
    );
    std::fs::set_permissions(bin.join("mypy"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let tools = Tools {
        env: vec![("PATH".into(), root.join("empty").into())],
        ..Tools::default()
    };
    let lines = check(&tools, Checker::Mypy, &top, &wt, &s(&["a.py"]), MIN).await;
    assert_eq!(lines[1], "a.py:1:1: error: from venv  [misc]");
}
