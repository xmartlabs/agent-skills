//! `ClaudeAgent` against a stand-in `claude` written in shell: the arguments
//! and environment the child receives, how its result is read, and that a
//! timeout kills the whole process group.
//!
//! `live_haiku_round_trip` calls the real CLI and is ignored unless asked for:
//! `TRIBUNAL_LIVE=1 cargo test -p tribunal-agent -- --ignored`.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::json;
use tribunal_agent::{Agent, ClaudeAgent, DispatchRequest, Exit};

/// The tests run on parallel threads of one process. A child forked by one
/// test holds, until it execs, a copy of the write handle another test has
/// open on its stand-in, and Linux then refuses to start that stand-in with
/// ETXTBSY. Each test holds this from writing its stand-in to its last spawn.
static SPAWN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tribunal-agent-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn stand_in(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("claude");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[tokio::test]
async fn a_result_is_read_and_the_child_gets_a_clean_environment() {
    let _spawn = SPAWN.lock().await;
    let dir = scratch("ok");
    let bin = stand_in(
        &dir,
        r#"printf '%s\n' "$@" > args.txt
cat > stdin.txt
env > env.txt
echo 'a warning first'
echo '{"type":"result","subtype":"success","is_error":false,"structured_output":{"ok":true},"num_turns":3,"modelUsage":{"claude-sonnet-5-5":{"inputTokens":10,"outputTokens":20}}}'"#,
    );
    // Larger than a pipe buffer and than Windows' whole command line.
    let prompt = format!("review\n{}", "x".repeat(200_000));
    let mut req = DispatchRequest::new("lens:1", prompt.as_str(), &dir);
    req.tools = vec!["Read".into()];
    req.schema = Some(json!({"type": "object"}));
    std::env::set_var("CLAUDECODE", "1");
    let res = ClaudeAgent::new(bin).run(&req).await;
    assert_eq!(res.exit, Exit::Code(0));
    assert_eq!(res.structured(), Some(&json!({"ok": true})));
    assert_eq!(res.model_usage()["claude-sonnet-5-5"].output_tokens, 20);
    let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
    assert!(args.starts_with("-p\n--output-format\nstream-json\n--verbose\n"));
    assert!(!args.contains("review"));
    assert_eq!(
        std::fs::read_to_string(dir.join("stdin.txt")).unwrap(),
        prompt
    );
    let env = std::fs::read_to_string(dir.join("env.txt")).unwrap();
    assert!(!env.lines().any(|l| l.starts_with("CLAUDECODE=")));
}

#[tokio::test]
async fn an_error_exit_without_a_result_has_no_answer() {
    let _spawn = SPAWN.lock().await;
    let dir = scratch("err");
    let bin = stand_in(&dir, "echo 'boom' >&2\nexit 3");
    let res = ClaudeAgent::new(bin)
        .run(&DispatchRequest::new("x", "p", &dir))
        .await;
    assert_eq!(res.exit, Exit::Code(3));
    assert!(res.output.is_none());
    assert!(res.structured().is_none());
    assert!(res.stderr_tail.contains("boom"));
}

#[tokio::test]
async fn a_timeout_kills_the_whole_group() {
    let _spawn = SPAWN.lock().await;
    let dir = scratch("timeout");
    let bin = stand_in(&dir, "sleep 300 &\necho $! > grandchild.pid\nsleep 300");
    let mut req = DispatchRequest::new("slow", "p", &dir);
    req.timeout = Duration::from_millis(500);
    let started = Instant::now();
    let res = ClaudeAgent::new(bin).run(&req).await;
    assert_eq!(res.exit, Exit::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(20));
    let pid: i32 = std::fs::read_to_string(dir.join("grandchild.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let alive = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok();
    assert!(!alive, "the grandchild {pid} survived the timeout");
}

#[tokio::test]
async fn nothing_runs_without_a_directory_or_a_binary() {
    let _spawn = SPAWN.lock().await;
    let res = ClaudeAgent::new("/bin/true")
        .run(&DispatchRequest::new("x", "p", "/nonexistent/dir"))
        .await;
    assert!(matches!(res.exit, Exit::SpawnFailed(_)));
    let dir = scratch("nobin");
    let res = ClaudeAgent::new(dir.join("missing"))
        .run(&DispatchRequest::new("x", "p", &dir))
        .await;
    assert!(matches!(res.exit, Exit::SpawnFailed(_)));
}

#[tokio::test]
#[ignore = "calls the real claude CLI; set TRIBUNAL_LIVE=1"]
async fn live_haiku_round_trip() {
    assert_eq!(
        std::env::var("TRIBUNAL_LIVE").as_deref(),
        Ok("1"),
        "set TRIBUNAL_LIVE=1"
    );
    let dir = scratch("live");
    std::fs::write(dir.join("a.txt"), "hello world\n").unwrap();
    let mut req = DispatchRequest::new("live", "Read a.txt and give its first word.", &dir);
    req.system_prompt = Some("You read files with the Read tool and answer briefly.".into());
    req.model = Some("claude-haiku-5-5".into());
    req.tools = vec!["Read".into()];
    req.schema = Some(
        json!({"type": "object", "properties": {"word": {"type": "string"}}, "required": ["word"]}),
    );
    let res = ClaudeAgent::new("claude").run(&req).await;
    assert_eq!(res.exit, Exit::Code(0), "{}", res.stderr_tail);
    assert_eq!(res.structured(), Some(&json!({"word": "hello"})));
}
