//! `claude -p` as a managed child: its own process group, a timeout, and a
//! kill path that takes the whole group, tool subprocesses included.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(unix)]
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::result::CliResult;
use crate::{Agent, Credentials, DispatchRequest, DispatchResult, Exit};

/// How long a timed-out group has between SIGTERM and SIGKILL.
#[cfg(unix)]
const GRACE: Duration = Duration::from_secs(5);

const STDERR_TAIL_BYTES: usize = 4096;

/// A system prompt longer than this goes to the child in a file instead of on
/// the command line. Windows caps a whole command line at 32,767 characters
/// and Linux caps one argument at 128 KiB, and a lens prompt that carries the
/// methodology and the conventions passes both.
const SYSTEM_PROMPT_ARG_BYTES: usize = 12_000;

/// A system prompt written for one child. The file is private to the user,
/// since the conventions of a private repository are in it, and it goes when
/// the dispatch ends, whichever way that is.
struct PromptFile(PathBuf);

impl PromptFile {
    fn create(text: &str) -> std::io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "tribunal-system-prompt-{}-{}.md",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = Self(path);
        opts.open(&file.0)?.write_all(text.as_bytes())?;
        Ok(file)
    }
}

impl Drop for PromptFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Variables a parent Claude Code session sets for itself. A child that
/// inherits them believes it is part of that session.
/// The CLI's prompt-cache lifetime. Unset, it is 1 hour on a subscription,
/// and 1-hour cache writes cost 2x input against 1.25x for 5 minutes. A
/// dispatch's turns run seconds apart, so the engine asks for 5 minutes
/// unless the operator set the variable.
const PROMPT_CACHE_TTL: &str = "CLAUDE_CODE_PROMPT_CACHE_TTL";
const DEFAULT_PROMPT_CACHE_TTL: &str = "5m";

const SESSION_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
];

/// What else a child loads besides the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Isolation {
    /// No settings file and no MCP server: the operator's global CLAUDE.md,
    /// plugins, skills and servers stay out, and so does everything the
    /// reviewed repository's `.claude/` carries. Measured on 2.1.308, a
    /// one-file task loaded 4,570 input tokens this way against 188,430
    /// inheriting everything.
    #[default]
    Isolated,
    /// The operator's own settings, plugins, skills and servers, as a
    /// Workflow subagent has them. Kept to compare the two.
    Inherit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeAgent {
    pub bin: PathBuf,
    pub credentials: Credentials,
    pub isolation: Isolation,
    /// A directory each dispatch's event stream is saved to, as
    /// `<label>.jsonl`. `None` keeps none.
    pub transcripts: Option<PathBuf>,
}

impl ClaudeAgent {
    #[must_use]
    pub fn new(bin: impl Into<PathBuf>) -> Self {
        Self {
            bin: bin.into(),
            credentials: Credentials::Session,
            isolation: Isolation::Isolated,
            transcripts: None,
        }
    }

    /// The agent the `tribunal` binary dispatches with: the CLI named by
    /// `TRIBUNAL_CLAUDE_BIN` (else `claude`) on the session login, or on the API
    /// key in `TRIBUNAL_ANTHROPIC_API_KEY` when that is set. A plain
    /// `ANTHROPIC_API_KEY` left in the operator's shell never selects the key:
    /// the session login stays the default, and that key is dropped from each
    /// child. `lookup` reads one environment variable.
    #[must_use]
    pub fn from_env(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let mut agent = Self::new(lookup("TRIBUNAL_CLAUDE_BIN").unwrap_or_else(|| "claude".into()));
        if let Some(key) = lookup("TRIBUNAL_ANTHROPIC_API_KEY").filter(|k| !k.is_empty()) {
            agent.credentials = Credentials::ApiKey(key);
        }
        agent
    }

    /// The first line `claude --version` prints, such as `2.1.316 (Claude
    /// Code)`, for the metrics row. `None` when the CLI cannot be started or
    /// prints nothing; a run does not depend on it.
    #[must_use]
    pub fn cli_version(&self) -> Option<String> {
        let out = std::process::Command::new(&self.bin)
            .arg("--version")
            .env_clear()
            .envs(self.env(std::env::vars_os()))
            .stdin(Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text.lines().next()?.trim();
        (out.status.success() && !line.is_empty()).then(|| line.to_owned())
    }

    /// On Windows, points `bin` at what an npm or pnpm `claude.cmd` starts,
    /// since that launcher cannot carry the CLI's arguments (see
    /// [`crate::launch`]). Elsewhere it changes nothing. An error says why
    /// no dispatch could start, before any work is done.
    pub fn resolve_launcher(&mut self) -> Result<(), String> {
        #[cfg(windows)]
        {
            let path = std::env::var_os("PATH");
            self.bin = crate::launch::resolve(&self.bin, path.as_deref())?;
        }
        Ok(())
    }

    /// The command line, without the binary. `--agent` is never used: on
    /// 2.1.308 it drops `--json-schema`, so the agent body goes in as the
    /// system prompt instead. The output is the event stream, whose last
    /// event is the result object `--output-format json` prints alone. The
    /// prompt goes on stdin, not here: Windows caps a whole command line at
    /// 32,767 characters and Linux caps one argument at 128 KiB.
    #[must_use]
    pub fn args(&self, req: &DispatchRequest) -> Vec<String> {
        self.args_with_prompt_file(req, None)
    }

    /// [`ClaudeAgent::args`], with the system prompt read from `prompt_file`
    /// when there is one.
    fn args_with_prompt_file(
        &self,
        req: &DispatchRequest,
        prompt_file: Option<&Path>,
    ) -> Vec<String> {
        let mut a = vec![
            "-p".to_owned(),
            "--output-format".to_owned(),
            "stream-json".to_owned(),
            "--verbose".to_owned(),
            "--no-session-persistence".to_owned(),
        ];
        match (prompt_file, &req.system_prompt) {
            (Some(path), _) => a.extend([
                "--system-prompt-file".to_owned(),
                path.to_string_lossy().into_owned(),
            ]),
            (None, Some(sp)) => a.extend(["--system-prompt".to_owned(), sp.clone()]),
            (None, None) => {}
        }
        if let Some(m) = req.model.as_deref().filter(|m| !m.is_empty()) {
            a.extend(["--model".to_owned(), m.to_owned()]);
        }
        if let Some(e) = req.effort.as_deref().filter(|e| !e.is_empty()) {
            a.extend(["--effort".to_owned(), e.to_owned()]);
        }
        let tools = req.tools.join(",");
        a.extend(["--tools".to_owned(), tools.clone()]);
        if !tools.is_empty() {
            a.extend(["--allowedTools".to_owned(), tools]);
        }
        if let Some(schema) = &req.schema {
            a.extend(["--json-schema".to_owned(), schema.to_string()]);
        }
        // A child's working directory is the checkout of the pull request
        // under review, so its project and local settings are the PR's own:
        // their hooks, `env` and MCP servers would run on the reviewer's
        // machine. No child loads them, inheriting or not.
        if self.isolation == Isolation::Isolated && !req.inherit_settings {
            a.extend([
                "--setting-sources".to_owned(),
                String::new(),
                "--strict-mcp-config".to_owned(),
            ]);
        } else {
            a.extend(["--setting-sources".to_owned(), "user".to_owned()]);
        }
        a
    }

    /// The child's environment: the parent's, minus the variables that tie
    /// it to a running session and, on the session login, minus any key the
    /// CLI would rank above that login.
    #[must_use]
    pub fn env(
        &self,
        parent: impl IntoIterator<Item = (OsString, OsString)>,
    ) -> Vec<(OsString, OsString)> {
        let drop_keys = matches!(self.credentials, Credentials::Session);
        let mut env: Vec<(OsString, OsString)> = parent
            .into_iter()
            .filter(|(k, _)| {
                let k = k.to_string_lossy();
                // The binary removes it at startup; a library caller may not.
                k != "TRIBUNAL_CONVENTIONS_TOKEN"
                    && !SESSION_VARS.contains(&k.as_ref())
                    && !(drop_keys && (k == "ANTHROPIC_API_KEY" || k == "ANTHROPIC_AUTH_TOKEN"))
            })
            .collect();
        if let Credentials::ApiKey(key) = &self.credentials {
            env.retain(|(k, _)| k != "ANTHROPIC_API_KEY");
            env.push(("ANTHROPIC_API_KEY".into(), key.into()));
        }
        if !env.iter().any(|(k, _)| k == PROMPT_CACHE_TTL) {
            env.push((PROMPT_CACHE_TTL.into(), DEFAULT_PROMPT_CACHE_TTL.into()));
        }
        env
    }
}

impl Agent for ClaudeAgent {
    async fn run(&self, req: &DispatchRequest) -> DispatchResult {
        if req.prompt.is_empty() {
            return not_started(format!("refusing to dispatch {}: empty prompt", req.label));
        }
        if !req.cwd.is_dir() {
            return not_started(format!(
                "refusing to dispatch {}: {} is not a directory",
                req.label,
                req.cwd.display()
            ));
        }
        let prompt_file = match req
            .system_prompt
            .as_deref()
            .filter(|sp| sp.len() > SYSTEM_PROMPT_ARG_BYTES)
        {
            Some(text) => match PromptFile::create(text) {
                Ok(file) => Some(file),
                Err(e) => {
                    return not_started(format!(
                        "cannot write the system prompt of {} to a file: {e}",
                        req.label
                    ))
                }
            },
            None => None,
        };
        let mut cmd = Command::new(&self.bin);
        cmd.args(self.args_with_prompt_file(req, prompt_file.as_ref().map(|f| f.0.as_path())))
            .env_clear()
            .envs(self.env(std::env::vars_os()))
            .current_dir(&req.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return not_started(format!(
                    "cannot start {}: {e}{}",
                    self.bin.display(),
                    SPAWN_HINT
                ))
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            let prompt = req.prompt.clone();
            tokio::spawn(async move {
                let _ = stdin.write_all(prompt.as_bytes()).await;
            });
        }
        let pid = child.id();
        match tokio::time::timeout(req.timeout, child.wait_with_output()).await {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let exit = match out.status.code() {
                    Some(c) => Exit::Code(c),
                    None => Exit::Killed(signal_of(&out.status)),
                };
                DispatchResult {
                    exit,
                    output: CliResult::parse(&stdout).ok(),
                    stderr_tail: tail(&String::from_utf8_lossy(&out.stderr)),
                    tool_calls: tool_calls(&stdout),
                    transcript: self.save_transcript(&req.label, &stdout),
                }
            }
            Ok(Err(e)) => DispatchResult {
                exit: Exit::Killed(None),
                output: None,
                stderr_tail: format!("waiting on the child failed: {e}"),
                tool_calls: Vec::new(),
                transcript: None,
            },
            Err(_) => {
                if let Some(pid) = pid {
                    kill_tree(pid).await;
                }
                DispatchResult {
                    exit: Exit::TimedOut,
                    output: None,
                    stderr_tail: format!("{} timed out after {:?}", req.label, req.timeout),
                    tool_calls: Vec::new(),
                    transcript: None,
                }
            }
        }
    }
}

/// `resolve_launcher` already swapped an npm `claude.cmd` for its target, so
/// a spawn that still fails found neither on PATH.
#[cfg(windows)]
const SPAWN_HINT: &str = " (tribunal needs claude.exe, or the claude.cmd an npm install puts on \
     PATH, or TRIBUNAL_CLAUDE_BIN pointing at claude.exe)";
#[cfg(not(windows))]
const SPAWN_HINT: &str = "";

#[cfg(unix)]
fn signal_of(status: &std::process::ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(status)
}

#[cfg(not(unix))]
fn signal_of(_: &std::process::ExitStatus) -> Option<i32> {
    None
}

/// The child leads its own process group (`process_group(0)`), so the
/// group id is its pid.
#[cfg(unix)]
pub(crate) async fn kill_tree(pid: u32) {
    use nix::sys::signal::{killpg, Signal};
    let pg = nix::unistd::Pid::from_raw(pid as i32);
    if killpg(pg, Signal::SIGTERM).is_err() {
        return;
    }
    tokio::time::sleep(GRACE).await;
    let _ = killpg(pg, Signal::SIGKILL);
}

/// Windows has no process groups to signal: `taskkill /T` walks the tree
/// from the child's pid, and has no graceful step a console child answers.
#[cfg(windows)]
pub(crate) async fn kill_tree(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

fn not_started(why: String) -> DispatchResult {
    DispatchResult {
        exit: Exit::SpawnFailed(why),
        output: None,
        stderr_tail: String::new(),
        tool_calls: Vec::new(),
        transcript: None,
    }
}

/// The names of the tools the stream's assistant turns called.
fn tool_calls(stream: &str) -> Vec<String> {
    stream
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["type"] == "assistant")
        .flat_map(|e| {
            e["message"]["content"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter(|c| c["type"] == "tool_use")
        .filter_map(|c| c["name"].as_str().map(str::to_owned))
        .collect()
}

impl ClaudeAgent {
    fn save_transcript(&self, label: &str, stream: &str) -> Option<PathBuf> {
        let dir = self.transcripts.as_ref()?;
        std::fs::create_dir_all(dir).ok()?;
        let name: String = label
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let mut path = dir.join(format!("{name}.jsonl"));
        let mut n = 1;
        while path.exists() {
            n += 1;
            path = dir.join(format!("{name}.{n}.jsonl"));
        }
        std::fs::write(&path, stream).ok()?;
        Some(path)
    }
}

fn tail(s: &str) -> String {
    if s.len() <= STDERR_TAIL_BYTES {
        return s.to_owned();
    }
    let mut start = s.len() - STDERR_TAIL_BYTES;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[cfg(unix)]
    #[test]
    fn cli_version_is_the_first_line_the_cli_prints() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("tribunal-cli-version-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = |name: &str, body: &str| {
            let p = dir.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        let ok = script(
            "claude-ok",
            "echo '2.1.316 (Claude Code)'; echo second line",
        );
        let failing = script("claude-fail", "echo 'oops'; exit 1");
        assert_eq!(
            ClaudeAgent::new(&ok).cli_version().as_deref(),
            Some("2.1.316 (Claude Code)")
        );
        assert_eq!(ClaudeAgent::new(&failing).cli_version(), None);
        assert_eq!(ClaudeAgent::new(dir.join("missing")).cli_version(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn req() -> DispatchRequest {
        let mut r = DispatchRequest::new("lens:1", "review this", "/tmp");
        r.model = Some("claude-sonnet-5-5".into());
        r.tools = vec!["Read".into(), "Bash".into()];
        r.schema = Some(json!({"type": "object"}));
        r
    }

    #[test]
    fn isolated_args_name_every_bound() {
        let a = ClaudeAgent::new("claude").args(&req());
        let has = |pair: [&str; 2]| a.windows(2).any(|w| w[0] == pair[0] && w[1] == pair[1]);
        assert_eq!(a[0], "-p");
        assert!(!a.iter().any(|x| x.contains("review this")));
        assert!(has(["--model", "claude-sonnet-5-5"]));
        assert!(has(["--tools", "Read,Bash"]));
        assert!(has(["--allowedTools", "Read,Bash"]));
        assert!(has(["--json-schema", r#"{"type":"object"}"#]));
        assert!(has(["--setting-sources", ""]));
        assert!(a.contains(&"--strict-mcp-config".to_owned()));
        assert!(!a.iter().any(|x| x == "--agent" || x == "--bare"));
    }

    #[test]
    fn a_system_prompt_goes_on_the_command_line_unless_a_file_carries_it() {
        let mut r = req();
        r.system_prompt = Some("short prompt".into());
        let a = ClaudeAgent::new("claude").args(&r);
        assert!(a
            .windows(2)
            .any(|w| w[0] == "--system-prompt" && w[1] == "short prompt"));

        let long = "x".repeat(SYSTEM_PROMPT_ARG_BYTES + 1);
        r.system_prompt = Some(long.clone());
        let a = ClaudeAgent::new("claude").args_with_prompt_file(&r, Some(Path::new("/t/sp.md")));
        assert!(a
            .windows(2)
            .any(|w| w[0] == "--system-prompt-file" && w[1] == "/t/sp.md"));
        assert!(!a
            .iter()
            .any(|x| x == "--system-prompt" || x.contains(&long)));
    }

    #[test]
    fn tool_calls_are_read_from_the_stream() {
        let stream = concat!(
            r#"{"type":"system","subtype":"init"}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"x"},{"type":"tool_use","name":"Bash","input":{}}]}}"#,
            "\n",
            r#"{"type":"user","message":{"content":[]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"StructuredOutput","input":{}}]}}"#,
            "\n",
            r#"{"type":"result","subtype":"success","is_error":false,"num_turns":3}"#,
            "\n",
        );
        assert_eq!(tool_calls(stream), ["Bash", "StructuredOutput"]);
        assert_eq!(CliResult::parse(stream).unwrap().num_turns, 3);
    }

    #[test]
    fn no_tools_is_an_explicit_empty_list() {
        let mut r = req();
        r.tools.clear();
        let a = ClaudeAgent::new("claude").args(&r);
        assert!(a.windows(2).any(|w| w[0] == "--tools" && w[1].is_empty()));
        assert!(!a.contains(&"--allowedTools".to_owned()));
    }

    #[test]
    fn inherit_loads_the_operator_settings_and_not_the_repository() {
        let mut agent = ClaudeAgent::new("claude");
        agent.isolation = Isolation::Inherit;
        let a = agent.args(&req());
        assert!(a
            .windows(2)
            .any(|w| w[0] == "--setting-sources" && w[1] == "user"));
        assert!(!a.contains(&"--strict-mcp-config".to_owned()));
    }

    #[test]
    fn one_request_can_inherit_while_the_agent_isolates() {
        let agent = ClaudeAgent::new("claude");
        let mut r = req();
        r.inherit_settings = true;
        let a = agent.args(&r);
        assert!(a
            .windows(2)
            .any(|w| w[0] == "--setting-sources" && w[1] == "user"));
        assert!(!a.contains(&"--strict-mcp-config".to_owned()));
        let isolated = agent.args(&req());
        assert!(isolated
            .windows(2)
            .any(|w| w[0] == "--setting-sources" && w[1].is_empty()));
    }

    #[test]
    fn children_get_a_five_minute_cache_unless_the_operator_set_one() {
        let ttl = |env: &[(OsString, OsString)]| -> Vec<String> {
            env.iter()
                .filter(|(k, _)| k == "CLAUDE_CODE_PROMPT_CACHE_TTL")
                .map(|(_, v)| v.to_string_lossy().into_owned())
                .collect()
        };
        let agent = ClaudeAgent::new("claude");
        let unset: Vec<(OsString, OsString)> = vec![("PATH".into(), "/bin".into())];
        assert_eq!(ttl(&agent.env(unset)), ["5m"]);
        let set: Vec<(OsString, OsString)> = vec![
            ("PATH".into(), "/bin".into()),
            ("CLAUDE_CODE_PROMPT_CACHE_TTL".into(), "1h".into()),
        ];
        assert_eq!(ttl(&agent.env(set)), ["1h"]);
    }

    #[test]
    fn session_login_drops_keys_and_session_vars() {
        let parent: Vec<(OsString, OsString)> = [
            ("PATH", "/bin"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_EFFORT", "high"),
            ("ANTHROPIC_API_KEY", "k"),
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
        let env = ClaudeAgent::new("claude").env(parent.clone());
        let keys: Vec<String> = env
            .iter()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            keys,
            [
                "PATH",
                "CLAUDE_CODE_USE_BEDROCK",
                "CLAUDE_CODE_PROMPT_CACHE_TTL"
            ]
        );

        let mut keyed = ClaudeAgent::new("claude");
        keyed.credentials = Credentials::ApiKey("mine".into());
        let env = keyed.env(parent);
        assert!(env
            .iter()
            .any(|(k, v)| k == "ANTHROPIC_API_KEY" && v == "mine"));
        assert_eq!(
            env.iter().filter(|(k, _)| k == "ANTHROPIC_API_KEY").count(),
            1
        );
    }

    fn lookup(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn a_child_never_gets_the_conventions_token() {
        let env = ClaudeAgent::new("claude").env([
            ("TRIBUNAL_CONVENTIONS_TOKEN".into(), "docs".into()),
            ("PATH".into(), "/bin".into()),
        ]);
        assert!(!env.iter().any(|(k, _)| k == "TRIBUNAL_CONVENTIONS_TOKEN"));
        assert!(env.contains(&("PATH".into(), "/bin".into())));
    }

    #[test]
    fn from_env_defaults_to_the_session_login_and_the_claude_binary() {
        let agent = ClaudeAgent::from_env(lookup(&[]));
        assert_eq!(agent, ClaudeAgent::new("claude"));
    }

    #[test]
    fn from_env_reads_the_binary_override() {
        let agent = ClaudeAgent::from_env(lookup(&[("TRIBUNAL_CLAUDE_BIN", "/opt/claude")]));
        assert_eq!(agent.bin, PathBuf::from("/opt/claude"));
    }

    #[test]
    fn from_env_uses_the_tribunal_api_key_when_given() {
        let agent = ClaudeAgent::from_env(lookup(&[("TRIBUNAL_ANTHROPIC_API_KEY", "sk-test")]));
        assert_eq!(agent.credentials, Credentials::ApiKey("sk-test".into()));
    }

    #[test]
    fn from_env_ignores_an_empty_tribunal_api_key() {
        let agent = ClaudeAgent::from_env(lookup(&[("TRIBUNAL_ANTHROPIC_API_KEY", "")]));
        assert_eq!(agent.credentials, Credentials::Session);
    }

    #[test]
    fn from_env_never_picks_up_an_inherited_anthropic_api_key() {
        // A key left in the operator's shell must not switch the run to API billing.
        let agent = ClaudeAgent::from_env(lookup(&[("ANTHROPIC_API_KEY", "sk-shell")]));
        assert_eq!(agent.credentials, Credentials::Session);
    }
}
