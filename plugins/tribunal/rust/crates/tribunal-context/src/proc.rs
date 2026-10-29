//! What starting and stopping a child takes on each platform: finding the
//! program, giving it its own group, killing everything it started.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use tokio::process::Command;

/// Where `${TMPDIR:-/tmp}` lands when `TMPDIR` is unset: `/tmp` as the
/// scripts spell it, and on Windows, which sets `TEMP` instead and has no
/// `/tmp`, the system's temporary directory.
#[must_use]
pub fn fallback_tmpdir() -> String {
    #[cfg(unix)]
    return "/tmp".to_owned();
    #[cfg(windows)]
    return std::env::temp_dir()
        .to_string_lossy()
        .trim_end_matches('\\')
        .to_owned();
}

/// `$HOME`, or on Windows, where only shells like Git Bash set it,
/// `%USERPROFILE%`.
#[must_use]
pub fn home_dir() -> Option<String> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    #[cfg(windows)]
    return var("HOME").or_else(|| var("USERPROFILE"));
    #[cfg(not(windows))]
    return var("HOME");
}

/// `program` as the OS should start it. On Windows a bare name is looked up
/// in `PATH` with each `PATHEXT` extension, because Rust tries `.exe` only
/// and `npm`, `npx` and most npm-installed CLIs are `.cmd` files. Anything
/// with a directory in it, or not found, is returned as given.
#[must_use]
pub fn resolve(program: &str) -> OsString {
    #[cfg(windows)]
    if !program.contains(['/', '\\']) && Path::new(program).extension().is_none() {
        if let Some(found) = find_in_path(program) {
            return found.into_os_string();
        }
    }
    program.into()
}

/// The first `PATH` entry holding `program`, as `command -v` reports it.
#[must_use]
pub fn find_in_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|d| {
        candidates(program)
            .into_iter()
            .map(|name| d.join(name))
            .find(|p| is_executable(p))
    })
}

#[cfg(unix)]
fn candidates(program: &str) -> Vec<String> {
    vec![program.to_owned()]
}

#[cfg(windows)]
fn candidates(program: &str) -> Vec<String> {
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    exts.split(';')
        .filter(|e| !e.is_empty())
        .map(|e| format!("{program}{}", e.to_ascii_lowercase()))
        .collect()
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(windows)]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// A command for `program` that leads its own process group on Unix, so
/// [`kill_tree`] can take its descendants with it.
#[must_use]
pub fn group_command(program: &str) -> Command {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut cmd = Command::new(resolve(program));
    #[cfg(unix)]
    cmd.process_group(0);
    cmd
}

/// SIGTERM to the group a [`group_command`] child leads; on Windows,
/// `taskkill /T /F`, which walks the tree from the pid.
pub async fn kill_tree(pid: u32) {
    #[cfg(unix)]
    {
        let pg = nix::unistd::Pid::from_raw(pid as i32);
        let _ = nix::sys::signal::killpg(pg, nix::sys::signal::Signal::SIGTERM);
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }
}

/// `ln -s target link` for a directory. Windows needs a privilege for a
/// symlink that a junction does not, so a junction is the fallback there.
///
/// # Errors
/// When neither link can be made.
pub fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            return Ok(());
        }
        let ok = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?
            .success();
        if ok {
            Ok(())
        } else {
            Err(std::io::Error::other("mklink /J failed"))
        }
    }
}
