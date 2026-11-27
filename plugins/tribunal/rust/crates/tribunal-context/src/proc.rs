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

/// How long [`kill_inside`] waits after SIGTERM before SIGKILL.
const INSIDE_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// Kills every process whose working directory is `dir` or below it, other
/// than this one, and returns their pids. A tree the run is about to remove
/// has nothing left to run.
///
/// Claude Code starts each Bash command in a process group of its own, so
/// killing an agent's group misses a command the agent left running, and once
/// the agent exits that command is reparented to init. Its working directory
/// is still the checkout the agent started in. On 2026-11-27 a reviewer's
/// `find / -path ...` outlived its review that way, at several cores, until
/// it was killed by hand.
///
/// Linux reads `/proc/<pid>/cwd` and macOS asks libproc. Elsewhere, Windows
/// included, nothing is found.
pub async fn kill_inside(dir: &Path) -> Vec<u32> {
    let Ok(root) = std::fs::canonicalize(dir) else {
        return Vec::new();
    };
    let me = std::process::id();
    let pids: Vec<u32> = cwds()
        .into_iter()
        .filter(|(pid, cwd)| *pid != me && cwd.starts_with(&root))
        .map(|(pid, _)| pid)
        .collect();
    if pids.is_empty() {
        return pids;
    }
    signal_all(&pids, false);
    tokio::time::sleep(INSIDE_GRACE).await;
    signal_all(&pids, true);
    pids
}

#[cfg(unix)]
fn signal_all(pids: &[u32], hard: bool) {
    use nix::sys::signal::{kill, Signal};
    let sig = if hard {
        Signal::SIGKILL
    } else {
        Signal::SIGTERM
    };
    for &pid in pids {
        let _ = kill(nix::unistd::Pid::from_raw(pid as i32), sig);
    }
}

#[cfg(not(unix))]
fn signal_all(_: &[u32], _: bool) {}

#[cfg(target_os = "linux")]
fn cwds() -> Vec<(u32, PathBuf)> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.filter_map(Result::ok)
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            let cwd = std::fs::read_link(e.path().join("cwd")).ok()?;
            Some((pid, cwd))
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn cwds() -> Vec<(u32, PathBuf)> {
    use std::ffi::CStr;
    use std::os::raw::{c_int, c_void};
    let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if n <= 0 {
        return Vec::new();
    }
    // Room for processes started between the two calls.
    let mut pids: Vec<c_int> = vec![0; n as usize + 64];
    let bytes = (pids.len() * std::mem::size_of::<c_int>()) as c_int;
    let got = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast::<c_void>(), bytes) };
    if got <= 0 {
        return Vec::new();
    }
    pids.truncate(got as usize);
    pids.into_iter()
        .filter(|&pid| pid > 0)
        .filter_map(|pid| {
            let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
            let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as c_int;
            let wrote = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDVNODEPATHINFO,
                    0,
                    std::ptr::addr_of_mut!(info).cast::<c_void>(),
                    size,
                )
            };
            if wrote != size {
                return None;
            }
            // `vip_path` is a char[MAXPATHLEN] that libc spells as 32x32.
            let raw = &info.pvi_cdir.vip_path;
            let flat = unsafe {
                std::slice::from_raw_parts(raw.as_ptr().cast::<u8>(), std::mem::size_of_val(raw))
            };
            let path = CStr::from_bytes_until_nul(flat).ok()?.to_str().ok()?;
            (!path.is_empty()).then(|| (pid as u32, PathBuf::from(path)))
        })
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn cwds() -> Vec<(u32, PathBuf)> {
    Vec::new()
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

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use std::os::unix::process::{CommandExt, ExitStatusExt};

    fn scratch(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p =
            std::env::temp_dir().join(format!("tribunal-proc-{}-{n}-{tag}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    // Each in a group of its own, as Claude Code starts a Bash command.
    fn sleeper(cwd: &Path) -> std::process::Child {
        std::process::Command::new("sleep")
            .arg("30")
            .current_dir(cwd)
            .process_group(0)
            .spawn()
            .unwrap()
    }

    #[tokio::test]
    async fn kills_what_runs_inside_the_tree_and_nothing_outside() {
        let tree = scratch("tree");
        let nested = tree.join("src/deep");
        std::fs::create_dir_all(&nested).unwrap();
        let other = scratch("other");
        let mut at_root = sleeper(&tree);
        let mut below = sleeper(&nested);
        let mut outside = sleeper(&other);

        let killed = kill_inside(&tree).await;

        assert!(killed.contains(&at_root.id()), "{killed:?}");
        assert!(killed.contains(&below.id()), "{killed:?}");
        assert!(!killed.contains(&outside.id()), "{killed:?}");
        assert!(at_root.wait().unwrap().signal().is_some());
        assert!(below.wait().unwrap().signal().is_some());
        assert!(
            outside.try_wait().unwrap().is_none(),
            "outside sleeper died"
        );
        outside.kill().unwrap();
        let _ = outside.wait();
        let _ = std::fs::remove_dir_all(&tree);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[tokio::test]
    async fn a_missing_tree_kills_nothing() {
        let gone = scratch("gone");
        std::fs::remove_dir_all(&gone).unwrap();
        assert!(kill_inside(&gone).await.is_empty());
    }
}
