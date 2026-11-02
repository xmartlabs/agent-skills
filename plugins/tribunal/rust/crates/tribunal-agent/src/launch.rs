//! What a `claude` command line has to start on Windows.
//!
//! Rust looks a bare name up in `PATH` as `<name>.exe` only, and refuses most
//! of the arguments tribunal passes (a multi-line system prompt, a JSON
//! schema) to a `.cmd`, which `cmd.exe` would also cut at 8191 characters.
//! Claude Code installed with npm or pnpm is such a `.cmd`, a launcher that
//! starts one file next to it: the native `claude.exe` the package's
//! postinstall copies into `bin/` (2.1.x), or `cli.js` under node (older
//! releases). [`resolve`] reads the launcher and starts that file directly,
//! with no `cmd.exe` in between. The module is plain path logic, so its tests
//! run on every platform; only `ClaudeAgent::resolve_launcher` calls it, on
//! Windows.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The package's `bin/claude.exe` is a 500-byte script until its postinstall
/// copies the native binary over it.
const STUB_BYTES: u64 = 4096;

const FIX: &str = "install Claude Code with the native installer, or set \
    TRIBUNAL_CLAUDE_BIN to a claude.exe";

/// The program to start and the arguments that go before tribunal's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub lead: Vec<PathBuf>,
}

impl Launch {
    fn of(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            lead: Vec::new(),
        }
    }
}

/// `bin` as Windows has to start it, given the `PATH` value. A `claude.exe`
/// anywhere on `PATH` wins, as it does for Rust's own lookup; without one,
/// the first `claude.cmd` (or `.bat`) is read for the file it starts. A name
/// found nowhere comes back as given, so the spawn error reports it.
pub fn resolve(bin: &Path, path: Option<&OsStr>) -> Result<Launch, String> {
    match ext(bin).as_deref() {
        Some("cmd" | "bat") => return from_shim(bin),
        Some(_) => return Ok(Launch::of(bin)),
        None => {}
    }
    if bin.components().count() > 1 {
        return Ok(Launch::of(bin));
    }
    let dirs: Vec<PathBuf> = path
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    let named = |e: &str| {
        let mut name = bin.as_os_str().to_owned();
        name.push(e);
        name
    };
    if let Some(exe) = dirs
        .iter()
        .map(|d| d.join(named(".exe")))
        .find(|p| p.is_file())
    {
        return Ok(Launch::of(exe));
    }
    let shim = dirs
        .iter()
        .flat_map(|d| [d.join(named(".cmd")), d.join(named(".bat"))])
        .find(|p| p.is_file());
    match shim {
        Some(shim) => from_shim(&shim),
        None => Ok(Launch::of(bin)),
    }
}

fn ext(p: &Path) -> Option<String> {
    p.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
}

/// The file an npm or pnpm launcher starts: the last quoted path relative to
/// the launcher's own directory (`%dp0%\…` from npm, `%~dp0\…` from pnpm).
/// A launcher of a script names `node.exe` that way too, but earlier.
fn from_shim(shim: &Path) -> Result<Launch, String> {
    let text =
        std::fs::read(shim).map_err(|e| format!("cannot read {}: {e}; {FIX}", shim.display()))?;
    let text = String::from_utf8_lossy(&text);
    let rel = text
        .split('"')
        .skip(1)
        .step_by(2)
        .filter_map(|q| {
            let lower = q.to_ascii_lowercase();
            ["%dp0%\\", "%~dp0\\"]
                .iter()
                .find(|p| lower.starts_with(*p))
                .map(|p| &q[p.len()..])
        })
        .last()
        .ok_or_else(|| {
            format!(
                "{} is a launcher tribunal cannot read (no %dp0% path in it); {FIX}",
                shim.display()
            )
        })?;
    let dir = shim.parent().unwrap_or(Path::new(""));
    let target = rel
        .split(['\\', '/'])
        .filter(|c| !c.is_empty() && *c != ".")
        .fold(dir.to_path_buf(), |p, c| p.join(c));
    let meta = std::fs::metadata(&target).map_err(|_| {
        format!(
            "{} starts {}, which does not exist; {FIX}",
            shim.display(),
            target.display()
        )
    })?;
    match ext(&target).as_deref() {
        Some("exe") if meta.len() < STUB_BYTES => Err(format!(
            "{} is still the npm package's placeholder: its postinstall did not \
             run (--ignore-scripts) or the native package was not downloaded \
             (--omit=optional); reinstall without those, or {FIX}",
            target.display()
        )),
        Some("exe") => Ok(Launch::of(target)),
        Some("js" | "cjs" | "mjs") => {
            let local = dir.join("node.exe");
            Ok(Launch {
                program: if local.is_file() {
                    local
                } else {
                    PathBuf::from("node")
                },
                lead: vec![target],
            })
        }
        _ => Err(format!(
            "{} starts {}, which tribunal does not know how to run; {FIX}",
            shim.display(),
            target.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Generated by cmd-shim 7.0.0 (npm 11) and @zkochan/cmd-shim 7.0.0
    // (pnpm), for a target of the native binary and of a node script.
    const NPM_EXE: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\"%dp0%\\node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe\"   %*\r\n";
    const NPM_JS: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@anthropic-ai\\claude-code\\cli.js\" %*\r\n";
    const PNPM_EXE: &str = "@SETLOCAL\r\n@\"%~dp0\\..\\store\\claude-code\\claude.exe\"   %*\r\n";
    const PNPM_JS: &str = "@SETLOCAL\r\n@IF EXIST \"%~dp0\\node.exe\" (\r\n  \"%~dp0\\node.exe\"  \"%~dp0\\..\\store\\claude-code\\cli.js\" %*\r\n) ELSE (\r\n  @SET PATHEXT=%PATHEXT:;.JS;=;%\r\n  node  \"%~dp0\\..\\store\\claude-code\\cli.js\" %*\r\n)\r\n";

    const NATIVE: usize = 8192;

    fn scratch() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "tribunal-launch-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn file(p: &Path, bytes: usize) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![b'x'; bytes]).unwrap();
    }

    fn put(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn path_of(dirs: &[&Path]) -> std::ffi::OsString {
        std::env::join_paths(dirs).unwrap()
    }

    fn npm_exe(prefix: &Path) -> PathBuf {
        prefix.join("node_modules/@anthropic-ai/claude-code/bin/claude.exe")
    }

    #[test]
    fn an_npm_launcher_starts_the_native_binary_it_points_at() {
        let prefix = scratch();
        put(&prefix.join("claude.cmd"), NPM_EXE);
        file(&npm_exe(&prefix), NATIVE);
        let got = resolve(Path::new("claude"), Some(&path_of(&[&prefix]))).unwrap();
        assert_eq!(got, Launch::of(npm_exe(&prefix)));
    }

    #[test]
    fn a_claude_exe_on_path_wins_over_an_earlier_launcher() {
        let (npm, native) = (scratch(), scratch());
        put(&npm.join("claude.cmd"), NPM_EXE);
        file(&npm_exe(&npm), NATIVE);
        file(&native.join("claude.exe"), NATIVE);
        let got = resolve(Path::new("claude"), Some(&path_of(&[&npm, &native]))).unwrap();
        assert_eq!(got, Launch::of(native.join("claude.exe")));
    }

    #[test]
    fn an_npm_launcher_of_a_node_script_runs_it_under_node() {
        let prefix = scratch();
        put(&prefix.join("claude.cmd"), NPM_JS);
        let cli = prefix.join("node_modules/@anthropic-ai/claude-code/cli.js");
        file(&cli, 10);
        let got = resolve(Path::new("claude"), Some(&path_of(&[&prefix]))).unwrap();
        assert_eq!(got.program, PathBuf::from("node"));
        assert_eq!(got.lead, vec![cli]);
        file(&prefix.join("node.exe"), NATIVE);
        let got = resolve(Path::new("claude"), Some(&path_of(&[&prefix]))).unwrap();
        assert_eq!(got.program, prefix.join("node.exe"));
    }

    #[test]
    fn pnpm_launchers_resolve_relative_to_their_directory() {
        let root = scratch();
        let bin = root.join("bin");
        let store = root.join("bin/../store/claude-code");
        put(&bin.join("claude.cmd"), PNPM_EXE);
        file(&store.join("claude.exe"), NATIVE);
        let got = resolve(Path::new("claude"), Some(&path_of(&[&bin]))).unwrap();
        assert_eq!(got, Launch::of(store.join("claude.exe")));
        put(&bin.join("claude.cmd"), PNPM_JS);
        file(&store.join("cli.js"), 10);
        let got = resolve(Path::new("claude"), Some(&path_of(&[&bin]))).unwrap();
        assert_eq!(got.lead, vec![store.join("cli.js")]);
    }

    #[test]
    fn the_placeholder_left_by_a_skipped_postinstall_is_refused() {
        let prefix = scratch();
        put(&prefix.join("claude.cmd"), NPM_EXE);
        file(&npm_exe(&prefix), 500);
        let e = resolve(Path::new("claude"), Some(&path_of(&[&prefix]))).unwrap_err();
        assert!(e.contains("placeholder"), "{e}");
        assert!(e.contains("native installer"), "{e}");
    }

    /// The `windows` CI job runs this after `npm install -g
    /// @anthropic-ai/claude-code`: Rust cannot start the bare name, and the
    /// program resolved from the launcher answers `--version`.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs Claude Code installed with npm and no claude.exe on PATH"]
    fn the_npm_install_on_this_machine_starts() {
        assert!(std::process::Command::new("claude")
            .arg("--version")
            .output()
            .is_err());
        let path = std::env::var_os("PATH");
        let got = resolve(Path::new("claude"), path.as_deref()).unwrap();
        assert!(
            got.program
                .ends_with("node_modules/@anthropic-ai/claude-code/bin/claude.exe"),
            "{got:?}"
        );
        let out = std::process::Command::new(&got.program)
            .args(&got.lead)
            .arg("--version")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.contains("Claude Code"),
            "{out:?}"
        );
    }

    #[test]
    fn a_launcher_whose_target_is_missing_says_so() {
        let prefix = scratch();
        put(&prefix.join("claude.cmd"), NPM_EXE);
        let e = resolve(Path::new("claude"), Some(&path_of(&[&prefix]))).unwrap_err();
        assert!(e.contains("does not exist"), "{e}");
    }

    #[test]
    fn a_launcher_with_no_relative_path_is_refused() {
        let prefix = scratch();
        put(
            &prefix.join("claude.cmd"),
            "@echo off\r\nC:\\tools\\claude.exe %*\r\n",
        );
        let e = resolve(Path::new("claude"), Some(&path_of(&[&prefix]))).unwrap_err();
        assert!(e.contains("cannot read"), "{e}");
    }

    #[test]
    fn an_explicit_cmd_is_read_and_other_paths_pass_through() {
        let prefix = scratch();
        let shim = prefix.join("claude.cmd");
        put(&shim, NPM_EXE);
        file(&npm_exe(&prefix), NATIVE);
        assert_eq!(resolve(&shim, None).unwrap(), Launch::of(npm_exe(&prefix)));
        for given in ["C:\\tools\\claude.exe", "/opt/claude", "tools/claude"] {
            assert_eq!(resolve(Path::new(given), None).unwrap(), Launch::of(given));
        }
    }

    #[test]
    fn a_name_found_nowhere_comes_back_as_given() {
        let empty = scratch();
        let got = resolve(Path::new("claude"), Some(&path_of(&[&empty]))).unwrap();
        assert_eq!(got, Launch::of("claude"));
        assert_eq!(
            resolve(Path::new("claude"), None).unwrap(),
            Launch::of("claude")
        );
    }
}
