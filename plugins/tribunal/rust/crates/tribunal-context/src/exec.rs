//! Running `gh`, `git` and the like. Every external call of the context phase
//! goes through [`run`], so a test can point `PATH` at a recorded `gh` and
//! both this crate and the JavaScript scripts it replaces see the same
//! answers.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// `None` when a signal ended the process.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    #[must_use]
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

/// The code Node's `spawnSync` puts in its error when a program cannot be
/// started (`spawnSync gh ETXTBSY`). Errors it does not name stay `UNKNOWN`.
#[must_use]
pub fn spawn_error_code(e: &std::io::Error) -> &'static str {
    use std::io::ErrorKind;
    match e.kind() {
        ErrorKind::NotFound => "ENOENT",
        ErrorKind::PermissionDenied => "EACCES",
        ErrorKind::ExecutableFileBusy => "ETXTBSY",
        ErrorKind::WouldBlock => "EAGAIN",
        ErrorKind::OutOfMemory => "ENOMEM",
        ErrorKind::ArgumentListTooLong => "E2BIG",
        _ => "UNKNOWN",
    }
}

/// Runs `program` with `args`, stdin closed, and waits for it. `cwd` and
/// `env` are optional; `env` adds to the inherited environment.
///
/// # Errors
/// When the program cannot be started at all. A non-zero exit is an
/// [`Output`], not an error: callers decide what a failed `gh` means.
pub async fn run(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: &[(OsString, OsString)],
) -> std::io::Result<Output> {
    let mut cmd = Command::new(crate::proc::resolve(program));
    cmd.args(args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let out = cmd.output().await?;
    Ok(Output {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// [`run`] for a command whose stdout is the only thing wanted.
///
/// # Errors
/// A failure to start, or a non-zero exit, with the command and its stderr.
pub async fn stdout_of(program: &str, args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    let out = run(program, args, cwd, &[])
        .await
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    if out.ok() {
        Ok(out.stdout)
    } else {
        Err(format!(
            "{program} {} exited {:?}: {}",
            args.join(" "),
            out.code,
            out.stderr.trim()
        ))
    }
}

/// Which `gh` and `git` to run, and anything to add to their environment.
/// The default is whatever `PATH` resolves. A test passes a recorded `gh`
/// here instead of touching the process environment, which every test thread
/// shares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tools {
    pub gh: String,
    pub git: String,
    pub env: Vec<(OsString, OsString)>,
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            gh: "gh".to_owned(),
            git: "git".to_owned(),
            env: Vec::new(),
        }
    }
}

impl Tools {
    /// # Errors
    /// As [`run`].
    pub async fn gh(&self, args: &[&str], cwd: Option<&Path>) -> std::io::Result<Output> {
        run(&self.gh, args, cwd, &self.env).await
    }

    /// # Errors
    /// As [`run`].
    pub async fn git(&self, args: &[&str], cwd: Option<&Path>) -> std::io::Result<Output> {
        run(&self.git, args, cwd, &self.env).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    #[test]
    fn a_failed_spawn_names_the_errno_node_would() {
        assert_eq!(
            spawn_error_code(&Error::from(ErrorKind::ExecutableFileBusy)),
            "ETXTBSY"
        );
        assert_eq!(
            spawn_error_code(&Error::from(ErrorKind::WouldBlock)),
            "EAGAIN"
        );
        assert_eq!(spawn_error_code(&Error::other("x")), "UNKNOWN");
    }

    // Raw OS codes are errnos only on Unix; on Windows 26 is a Win32 error.
    #[cfg(unix)]
    #[test]
    fn a_raw_errno_from_the_kernel_maps_too() {
        // ETXTBSY is 26 on Linux and macOS alike.
        assert_eq!(spawn_error_code(&Error::from_raw_os_error(26)), "ETXTBSY");
        assert_eq!(spawn_error_code(&Error::from_raw_os_error(2)), "ENOENT");
        assert_eq!(spawn_error_code(&Error::from_raw_os_error(13)), "EACCES");
    }
}
