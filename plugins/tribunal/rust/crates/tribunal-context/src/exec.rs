//! Running `gh`, `git` and the like. Every external call of the context phase
//! goes through [`run`], so a test can point `PATH` at a recorded `gh` and
//! both this crate and the JavaScript scripts it replaces see the same
//! answers.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
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
    /// The separate read token for another repo's docs and anchors, when the
    /// operator gave one. `None` runs every read with the ambient credentials.
    pub conventions: Option<ConventionsToken>,
    /// Where offered docs of another repo are written for the reviewers. Set
    /// only for the loader, and only read when `conventions` is set.
    pub conventions_docs_dir: Option<PathBuf>,
}

/// The token named by [`CONVENTIONS_TOKEN_VAR`]. `Tools` derives `Debug`, so
/// this type keeps the value out of anything that prints one.
#[derive(Clone, PartialEq, Eq)]
pub struct ConventionsToken(OsString);

impl ConventionsToken {
    #[must_use]
    pub fn new(token: impl Into<OsString>) -> Option<Self> {
        let token = token.into();
        (!token.is_empty()).then_some(Self(token))
    }
}

impl std::fmt::Debug for ConventionsToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConventionsToken(<redacted>)")
    }
}

/// The variable that carries the token into the binary.
pub const CONVENTIONS_TOKEN_VAR: &str = "TRIBUNAL_CONVENTIONS_TOKEN";

/// Takes the token out of this process's environment and returns it, so no
/// child inherits it: not a reviewer, not an external head and not a tool run
/// from the reviewed repo. Call it before any thread starts.
#[must_use]
pub fn take_conventions_token() -> Option<ConventionsToken> {
    let token = std::env::var_os(CONVENTIONS_TOKEN_VAR);
    std::env::remove_var(CONVENTIONS_TOKEN_VAR);
    token.and_then(ConventionsToken::new)
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            gh: "gh".to_owned(),
            git: "git".to_owned(),
            env: Vec::new(),
            conventions: None,
            conventions_docs_dir: None,
        }
    }
}

impl Tools {
    /// The tools for a read of another repo's docs or anchors: `gh` and `git`
    /// run with the conventions token as `GH_TOKEN`, the variable `gh` ranks
    /// above `GITHUB_TOKEN` and a stored login. Without a token, these tools.
    #[must_use]
    pub fn for_conventions(&self) -> Self {
        let mut tools = self.clone();
        if let Some(ConventionsToken(token)) = &self.conventions {
            tools.env.retain(|(k, _)| k != "GH_TOKEN");
            tools.env.push(("GH_TOKEN".into(), token.clone()));
        }
        tools
    }

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
    fn the_conventions_token_only_overrides_the_handle_for_conventions() {
        let tools = Tools {
            env: vec![("GH_TOKEN".into(), "main".into()), ("A".into(), "1".into())],
            conventions: ConventionsToken::new("docs"),
            ..Tools::default()
        };
        assert!(tools.env.contains(&("GH_TOKEN".into(), "main".into())));
        assert_eq!(
            tools.for_conventions().env,
            [("A".into(), "1".into()), ("GH_TOKEN".into(), "docs".into())]
        );
    }

    #[test]
    fn without_a_token_the_handle_for_conventions_is_the_same_tools() {
        let tools = Tools {
            env: vec![("GH_TOKEN".into(), "main".into())],
            ..Tools::default()
        };
        assert_eq!(tools.for_conventions(), tools);
        assert_eq!(ConventionsToken::new(""), None);
    }

    #[test]
    fn printing_the_tools_never_prints_the_token() {
        let tools = Tools {
            conventions: ConventionsToken::new("sentinel-token"),
            ..Tools::default()
        };
        assert!(!format!("{tools:?}").contains("sentinel-token"));
    }

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
