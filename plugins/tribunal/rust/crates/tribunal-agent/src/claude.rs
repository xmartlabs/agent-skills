//! `claude -p` as a managed child: its own process group, a timeout, and a
//! kill path that takes the whole group, tool subprocesses included.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
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

/// Variables a parent Claude Code session sets for itself. A child that
/// inherits them believes it is part of that session.
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
    /// Only the working directory's project settings and no MCP server: the
    /// operator's global CLAUDE.md, plugins, skills and servers stay out.
    /// Measured on 2.1.308, a one-file task loaded 4,570 input tokens this way
    /// against 188,430 inheriting everything.
    #[default]
    Isolated,
    /// Everything the operator's own sessions load, as a Workflow subagent
    /// does. Kept to compare the two.
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

    /// The command line, without the binary. `--agent` is never used: on
    /// 2.1.308 it drops `--json-schema`, so the agent body goes in as the
    /// system prompt instead. The output is the event stream, whose last
    /// event is the result object `--output-format json` prints alone. The
    /// prompt goes on stdin, not here: Windows caps a whole command line at
    /// 32,767 characters and Linux caps one argument at 128 KiB.
    #[must_use]
    pub fn args(&self, req: &DispatchRequest) -> Vec<String> {
        let mut a = vec![
            "-p".to_owned(),
            "--output-format".to_owned(),
            "stream-json".to_owned(),
            "--verbose".to_owned(),
            "--no-session-persistence".to_owned(),
        ];
        if let Some(sp) = &req.system_prompt {
            a.extend(["--system-prompt".to_owned(), sp.clone()]);
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
        if self.isolation == Isolation::Isolated && !req.inherit_settings {
            a.extend([
                "--setting-sources".to_owned(),
                "project".to_owned(),
                "--strict-mcp-config".to_owned(),
            ]);
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
                !SESSION_VARS.contains(&k.as_ref())
                    && !(drop_keys && (k == "ANTHROPIC_API_KEY" || k == "ANTHROPIC_AUTH_TOKEN"))
            })
            .collect();
        if let Credentials::ApiKey(key) = &self.credentials {
            env.retain(|(k, _)| k != "ANTHROPIC_API_KEY");
            env.push(("ANTHROPIC_API_KEY".into(), key.into()));
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
        let mut cmd = Command::new(&self.bin);
        cmd.args(self.args(req))
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

/// Rust looks a bare name up in PATH as `<name>.exe` only, and refuses most
/// arguments for a `.cmd`, so the npm install's `claude.cmd` cannot run here.
#[cfg(windows)]
const SPAWN_HINT: &str =
    " (tribunal needs claude.exe from the native installer, or TRIBUNAL_CLAUDE_BIN pointing at it)";
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
        assert!(has(["--setting-sources", "project"]));
        assert!(a.contains(&"--strict-mcp-config".to_owned()));
        assert!(!a.iter().any(|x| x == "--agent" || x == "--bare"));
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
    fn inherit_loads_the_operator_settings() {
        let mut agent = ClaudeAgent::new("claude");
        agent.isolation = Isolation::Inherit;
        let a = agent.args(&req());
        assert!(!a.contains(&"--setting-sources".to_owned()));
    }

    #[test]
    fn one_request_can_inherit_while_the_agent_isolates() {
        let agent = ClaudeAgent::new("claude");
        let mut r = req();
        r.inherit_settings = true;
        let a = agent.args(&r);
        assert!(!a.contains(&"--setting-sources".to_owned()));
        assert!(!a.contains(&"--strict-mcp-config".to_owned()));
        assert!(agent.args(&req()).contains(&"--setting-sources".to_owned()));
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
        assert_eq!(keys, ["PATH", "CLAUDE_CODE_USE_BEDROCK"]);

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
}
