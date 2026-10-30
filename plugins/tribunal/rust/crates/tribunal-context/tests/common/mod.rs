//! Shared harness for the context differential tests: a recorded `gh` that
//! the JavaScript script finds on `PATH` and the Rust port receives in
//! `Tools`, and a runner for the script under test.
//!
//! These tests need `node` on PATH. Without it they fail; they do not skip.
#![allow(dead_code)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use tribunal_context::exec::Tools;

/// `plugins/tribunal`, the root the scripts resolve their imports from.
pub fn plugin_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

pub fn shim_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/shim")
}

/// A scratch directory unique to this test process and `name`.
pub fn scratch(name: &str) -> PathBuf {
    let prefix = format!("tribunal-context-{name}-");
    sweep_dead(&std::env::temp_dir(), &prefix);
    let dir = std::env::temp_dir().join(format!("{prefix}{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Removes `<prefix><pid>` directories whose process has exited. A test has
/// no teardown hook (the generated cases live in a static), so this is what
/// keeps each run from leaving its scratch trees behind for good.
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

/// One recorded `gh` answer.
pub fn call(args: &[&str], stdout: &str, code: i32) -> Value {
    json!({"args": args, "stdout": stdout, "stderr": "", "code": code})
}

/// Writes the recordings and returns their path.
pub fn recordings(dir: &Path, calls: Vec<Value>) -> PathBuf {
    let path = dir.join("gh-recordings.json");
    std::fs::write(&path, json!({ "calls": calls }).to_string()).unwrap();
    path
}

/// The `Tools` a Rust port runs with: the recorded `gh`, the real `git`. The
/// `gh` is the compiled shim, which answers as `tests/shim/gh` does without
/// starting node for each call; `shim.rs` checks the two agree.
pub fn tools(recordings: &Path) -> Tools {
    Tools {
        gh: env!("CARGO_BIN_EXE_tribunal-test-gh").to_owned(),
        git: "git".to_owned(),
        env: vec![(
            OsString::from("GH_RECORDINGS"),
            recordings.as_os_str().to_owned(),
        )],
    }
}

pub struct ScriptRun {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `node <plugin_root>/<script> <args>` with the recorded `gh` first on
/// PATH, in `cwd`, with `stdin` piped in.
pub fn run_script(
    script: &str,
    args: &[&str],
    cwd: &Path,
    recordings: &Path,
    stdin: Option<&str>,
) -> ScriptRun {
    run_script_env(script, args, cwd, recordings, stdin, &[])
}

/// [`run_script`] with `env` added to the script's environment.
pub fn run_script_env(
    script: &str,
    args: &[&str],
    cwd: &Path,
    recordings: &Path,
    stdin: Option<&str>,
    env: &[(&str, &str)],
) -> ScriptRun {
    use std::io::Write;
    let path = std::env::join_paths(std::iter::once(shim_dir()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let mut child = Command::new("node")
        .arg(plugin_root().join(script))
        .args(args)
        .current_dir(cwd)
        .env("PATH", path)
        .env("GH_RECORDINGS", recordings)
        .envs(env.iter().copied())
        .env_remove("CLAUDE_PLUGIN_ROOT")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("node must be on PATH");
    let mut input = child.stdin.take().unwrap();
    if let Some(s) = stdin {
        input.write_all(s.as_bytes()).unwrap();
    }
    drop(input);
    let out = child.wait_with_output().unwrap();
    ScriptRun {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// The last line of a script's stdout parsed as JSON: the scripts print one
/// result line.
pub fn last_json(stdout: &str) -> Value {
    let line = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    serde_json::from_str(line).unwrap_or_else(|e| panic!("not JSON ({e}): {line}"))
}
