//! The grounded typecheck for a Python repo: runs the checker the repo
//! configures (mypy, or pyright when that is all it configures) over the PR's
//! Python files at the pinned head, and prints what the TypeScript path
//! prints in the Python vocabulary the Verify phase reads: a `PY_CHECKER`
//! header, mypy-shaped error lines for the PR's files, `NO_DIAGNOSTICS`, and
//! `---COVERED---` with the files that were checked.
//!
//! It installs nothing and never reaches the network. A missing checker, a
//! crash or a timeout degrade to a `DEGRADED_PY_*` marker.

use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use serde_json::Value;
use tribunal_core::evidence::tsc_oracle::TscDiagnostic;

use crate::exec::{run as exec, Tools};

pub const NO_CONFIG: &str = "PY_NO_CONFIG";
pub const NOT_INSTALLED: &str = "DEGRADED_PY_NOT_INSTALLED";
pub const TIMEOUT: &str = "DEGRADED_PY_TIMEOUT";
pub const CHECKER_FAILED: &str = "DEGRADED_PY_CHECKER_FAILED";

pub const RUN_TIMEOUT: Duration = Duration::from_secs(300);
pub const MAX_DIAGNOSTICS: usize = 500;
const MAX_FILES: usize = 500;

/// Codes that say the checker's environment lacks a dependency, not that the
/// code is wrong. Keeping them would confirm findings against noise.
const ENVIRONMENT_CODES: [&str; 3] = ["import", "import-not-found", "import-untyped"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checker {
    Mypy,
    Pyright,
}

impl Checker {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Checker::Mypy => "mypy",
            Checker::Pyright => "pyright",
        }
    }
}

fn is_python(f: &str) -> bool {
    f.ends_with(".py") || f.ends_with(".pyi")
}

/// The PR's Python files.
#[must_use]
pub fn py_files(pr_files: &[String]) -> Vec<String> {
    pr_files.iter().filter(|f| is_python(f)).cloned().collect()
}

/// Which language the run checks. TypeScript keeps every PR that touches a
/// `.ts` or `.tsx` file in a repo with a `tsconfig.json`, and every PR with no
/// Python file, so its behavior does not change. Python takes a PR with
/// Python files and either no TypeScript file or no `tsconfig.json`. A PR that
/// mixes both in a repo with both gets the TypeScript check only: one run
/// prints one program's result.
#[must_use]
pub fn wants_python(has_tsconfig: bool, pr_files: &[String]) -> bool {
    let ts = pr_files
        .iter()
        .any(|f| f.ends_with(".ts") || f.ends_with(".tsx"));
    pr_files.iter().any(|f| is_python(f)) && !(ts && has_tsconfig)
}

fn has_section(path: &Path, wanted: impl Fn(&str) -> bool) -> bool {
    std::fs::read_to_string(path).is_ok_and(|t| {
        t.lines()
            .map(str::trim)
            .filter_map(|l| l.strip_prefix('[')?.split(']').next())
            .any(|name| wanted(name.trim()))
    })
}

fn in_table(name: &str, table: &str) -> bool {
    name == table || name.strip_prefix(table).is_some_and(|r| r.starts_with('.'))
}

/// The checker the repo configures, read from `top`. mypy wins when both are
/// configured.
#[must_use]
pub fn detect(top: &Path) -> Option<Checker> {
    let mypy = top.join("mypy.ini").is_file()
        || top.join(".mypy.ini").is_file()
        || has_section(&top.join("pyproject.toml"), |n| in_table(n, "tool.mypy"))
        || has_section(&top.join("setup.cfg"), |n| {
            n == "mypy" || n.starts_with("mypy-")
        });
    if mypy {
        return Some(Checker::Mypy);
    }
    let pyright = top.join("pyrightconfig.json").is_file()
        || has_section(&top.join("pyproject.toml"), |n| in_table(n, "tool.pyright"));
    pyright.then_some(Checker::Pyright)
}

fn diagnostic_line() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(.+?):([0-9]+):(?:[0-9]+:)? error: (.*?)(?:\s+\[([A-Za-z0-9_-]+)\])?$")
            .unwrap()
    })
}

fn normalize_path(p: &str) -> String {
    let p = p.replace('\\', "/");
    p.strip_prefix("./").unwrap_or(&p).to_owned()
}

/// Parses `path:line:col: error: message  [code]` lines. Notes, warnings and
/// anything else are skipped. A line without a code gets the checker's name.
#[must_use]
pub fn parse_diagnostics(text: &str) -> Vec<TscDiagnostic> {
    text.lines()
        .filter_map(|raw| {
            let m = diagnostic_line().captures(raw.trim_end())?;
            Some(TscDiagnostic {
                file: normalize_path(&m[1]),
                line: m[2].parse::<f64>().ok()?,
                code: m.get(4).map_or("mypy", |c| c.as_str()).to_owned(),
                message: m[3].trim().to_owned(),
            })
        })
        .collect()
}

fn pyright_lines(stdout: &str, wt: &Path) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<Value>(stdout) else {
        return Vec::new();
    };
    let root = wt.to_string_lossy().replace('\\', "/");
    let root = root.trim_end_matches('/');
    v.get("generalDiagnostics")
        .and_then(Value::as_array)
        .map(|ds| {
            ds.iter()
                .filter(|d| d.get("severity").and_then(Value::as_str) == Some("error"))
                .filter_map(|d| {
                    let file = d.get("file")?.as_str()?.replace('\\', "/");
                    let file = file
                        .strip_prefix(root)
                        .map_or(file.as_str(), |r| r.trim_start_matches('/'));
                    let at = d.get("range")?.get("start")?;
                    let line = at.get("line")?.as_u64()? + 1;
                    let col = at.get("character")?.as_u64()? + 1;
                    let msg = d.get("message")?.as_str()?.replace('\n', " ");
                    let rule = d.get("rule").and_then(Value::as_str).unwrap_or("pyright");
                    Some(format!("{file}:{line}:{col}: error: {msg}  [{rule}]"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The error lines of `raw` that sit in a checked file and say something about
/// the code, as printed, at most [`MAX_DIAGNOSTICS`].
fn kept_lines(raw: &str, checked: &[String]) -> Vec<String> {
    raw.lines()
        .filter(|l| {
            let Some(m) = diagnostic_line().captures(l.trim_end()) else {
                return false;
            };
            let code = m.get(4).map_or("", |c| c.as_str());
            checked.contains(&normalize_path(&m[1])) && !ENVIRONMENT_CODES.contains(&code)
        })
        .take(MAX_DIAGNOSTICS)
        .map(|l| l.trim_end().to_owned())
        .collect()
}

fn mypy_program(top: &Path) -> String {
    let venv = if cfg!(windows) {
        "Scripts/mypy.exe"
    } else {
        "bin/mypy"
    };
    [".venv", "venv"]
        .iter()
        .map(|d| top.join(d).join(venv))
        .find(|p| p.is_file())
        .map_or_else(|| "mypy".to_owned(), |p| p.to_string_lossy().into_owned())
}

/// Runs the checker over the PR's Python files that exist in `wt` and returns
/// the lines the typecheck prints. `top` is the ambient checkout, where a
/// virtualenv the repo keeps would live.
pub async fn check(
    tools: &Tools,
    checker: Checker,
    top: &Path,
    wt: &Path,
    pr_files: &[String],
    timeout: Duration,
) -> Vec<String> {
    if !wt.is_dir() {
        return vec!["DEGRADED_NO_CD".to_owned()];
    }
    let files: Vec<String> = py_files(pr_files)
        .into_iter()
        .filter(|f| wt.join(f).is_file())
        .take(MAX_FILES)
        .collect();
    let mut lines = vec![format!("PY_CHECKER {}", checker.name())];
    let mut diagnostics = Vec::new();
    if !files.is_empty() {
        let (program, mut args): (String, Vec<&str>) = match checker {
            Checker::Mypy => (
                mypy_program(top),
                vec![
                    "--no-color-output",
                    "--no-error-summary",
                    "--no-pretty",
                    "--show-column-numbers",
                    "--show-error-codes",
                    if cfg!(windows) {
                        "--cache-dir=nul"
                    } else {
                        "--cache-dir=/dev/null"
                    },
                ],
            ),
            Checker::Pyright => ("pyright".to_owned(), vec!["--outputjson"]),
        };
        args.extend(files.iter().map(String::as_str));
        let ran = tokio::time::timeout(timeout, exec(&program, &args, Some(wt), &tools.env)).await;
        let out = match ran {
            Err(_) => return vec![TIMEOUT.to_owned()],
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                return vec![NOT_INSTALLED.to_owned()]
            }
            Ok(Err(_)) => return vec![CHECKER_FAILED.to_owned()],
            Ok(Ok(o)) => o,
        };
        if !matches!(out.code, Some(0 | 1)) {
            return vec![CHECKER_FAILED.to_owned()];
        }
        let raw = match checker {
            Checker::Mypy => out.stdout,
            Checker::Pyright => pyright_lines(&out.stdout, wt).join("\n"),
        };
        diagnostics = kept_lines(&raw, &files);
    }
    if diagnostics.is_empty() {
        lines.push("NO_DIAGNOSTICS".to_owned());
    } else {
        lines.extend(diagnostics);
    }
    lines.push(format!("---COVERED--- {}", files.len()));
    lines.extend(files);
    lines
}

/// Whether `text` came from this module rather than the TypeScript check.
#[must_use]
pub fn is_python_text(text: &str) -> bool {
    text.lines().any(|l| {
        let l = l.trim();
        l == NO_CONFIG || l.starts_with("PY_CHECKER ") || l.starts_with("DEGRADED_PY_")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    fn dir(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tribunal-py-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for (p, t) in files {
            std::fs::write(d.join(p), t).unwrap();
        }
        d
    }

    #[test]
    fn detects_the_configured_checker() {
        let det = |name: &str, f: &[(&str, &str)]| detect(&dir(name, f));
        assert_eq!(
            det("none", &[("pyproject.toml", "[project]\nname='x'\n")]),
            None
        );
        assert_eq!(
            det("tm", &[("pyproject.toml", "[tool.mypy]\nstrict = true\n")]),
            Some(Checker::Mypy)
        );
        assert_eq!(
            det("tmo", &[("pyproject.toml", "[tool.mypy.overrides]\n")]),
            Some(Checker::Mypy)
        );
        assert_eq!(det("ini", &[("mypy.ini", "")]), Some(Checker::Mypy));
        assert_eq!(
            det("cfg", &[("setup.cfg", "[flake8]\n[mypy-foo.*]\n")]),
            Some(Checker::Mypy)
        );
        assert_eq!(det("cfg2", &[("setup.cfg", "[flake8]\n")]), None);
        assert_eq!(
            det("pr", &[("pyrightconfig.json", "{}")]),
            Some(Checker::Pyright)
        );
        assert_eq!(
            det("tp", &[("pyproject.toml", "  [tool.pyright]\n")]),
            Some(Checker::Pyright)
        );
        assert_eq!(det("tmx", &[("pyproject.toml", "[tool.mypyx]\n")]), None);
        assert_eq!(
            det("both", &[("pyrightconfig.json", "{}"), ("mypy.ini", "")]),
            Some(Checker::Mypy)
        );
    }

    #[test]
    fn language_choice_is_explicit() {
        let py = s(&["a.py"]);
        let ts = s(&["a.ts"]);
        let both = s(&["a.py", "a.ts"]);
        assert!(wants_python(false, &py));
        assert!(wants_python(true, &py));
        assert!(!wants_python(true, &ts));
        assert!(!wants_python(true, &both));
        assert!(wants_python(false, &both));
        assert!(!wants_python(true, &[]));
        assert!(!wants_python(false, &s(&["README.md"])));
    }

    #[test]
    fn parses_errors_only() {
        let text = "src/a.py:3:5: error: Incompatible types in assignment  [assignment]\n\
src/a.py:3:5: note: See docs\n\
src/b.py:9: error: Missing return statement\n\
src/c.py:1:1: warning: unused  [misc]\n\
.\\src\\d.py:4:2: error: bad  [arg-type]\n\
Found 3 errors in 2 files\n";
        let d = parse_diagnostics(text);
        let got: Vec<(&str, f64, &str)> = d
            .iter()
            .map(|x| (x.file.as_str(), x.line, x.code.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("src/a.py", 3.0, "assignment"),
                ("src/b.py", 9.0, "mypy"),
                ("src/d.py", 4.0, "arg-type"),
            ]
        );
        assert_eq!(d[0].message, "Incompatible types in assignment");
    }

    #[test]
    fn kept_lines_drop_other_files_and_environment_noise() {
        let raw = "src/a.py:1:1: error: x  [assignment]\n\
src/other.py:1:1: error: y  [assignment]\n\
src/a.py:2:1: error: Cannot find implementation or library stub  [import-not-found]\n\
src/a.py:3:1: warning: w  [misc]\n";
        assert_eq!(
            kept_lines(raw, &s(&["src/a.py"])),
            vec!["src/a.py:1:1: error: x  [assignment]"]
        );
    }

    #[test]
    fn kept_lines_stop_at_the_cap() {
        let raw: String = (1..=MAX_DIAGNOSTICS + 20)
            .map(|i| format!("a.py:{i}:1: error: m  [misc]\n"))
            .collect();
        assert_eq!(kept_lines(&raw, &s(&["a.py"])).len(), MAX_DIAGNOSTICS);
    }

    #[test]
    fn pyright_json_becomes_mypy_lines() {
        let json = r#"{"generalDiagnostics": [
            {"file": "/w/src/a.py", "severity": "error", "message": "bad\nmore",
             "range": {"start": {"line": 2, "character": 4}}, "rule": "reportAssignmentType"},
            {"file": "/w/src/a.py", "severity": "warning", "message": "w",
             "range": {"start": {"line": 0, "character": 0}}}]}"#;
        assert_eq!(
            pyright_lines(json, Path::new("/w")),
            vec!["src/a.py:3:5: error: bad more  [reportAssignmentType]"]
        );
        assert!(pyright_lines("not json", Path::new("/w")).is_empty());
    }

    #[test]
    fn python_text_is_recognized_and_typescript_text_is_not() {
        assert!(is_python_text("PY_NO_CONFIG"));
        assert!(is_python_text("PY_CHECKER mypy\nNO_DIAGNOSTICS"));
        assert!(is_python_text("DEGRADED_PY_TIMEOUT"));
        assert!(!is_python_text("NO_TSCONFIG"));
        assert!(!is_python_text("src/a.ts(1,1): error TS1: PY_CHECKER x"));
    }
}
