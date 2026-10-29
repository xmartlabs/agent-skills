//! The external heads, run natively: what one block of the workflow's
//! `externalRunBlock` does for a Codex or Gemini vote, and the clean-verdict
//! sweep, leaving the same artifacts in the vote directory (`<key>.prompt`,
//! `.out`, `.meta`, the `.lock` latch) for `classify_artifacts` to read.
//! `lib/gemini-exec.sh` is ported here as [`gemini_exec`].

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::classify_head_failure::classify_head_failure;

pub const EXTERNAL_HEAD_TIMEOUT_S: u64 = 150;
pub const RETRY_FAST_THRESHOLD_S: u64 = 120;
pub const SWEEP_TIMEOUT_S: u64 = 480;
const DEFAULT_CODEX_MODEL: &str = "gpt-6.1-sol";
const GEMINI_MODEL: &str = "gemini-2.5-flash";

/// Where the heads run and write.
#[derive(Debug, Clone)]
pub struct HeadSetup {
    pub vote_dir: PathBuf,
    /// The pinned checkout the heads read; `None` runs them in `fallback_cwd`.
    pub external_cwd: Option<PathBuf>,
    pub fallback_cwd: PathBuf,
    pub home: PathBuf,
    pub codex_bin: String,
    pub gemini_bin: String,
}

/// How a CLI ended. `124` is the timeout's own status.
struct Ran {
    code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn run_with_timeout(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &[(OsString, OsString)],
    timeout: Duration,
) -> Ran {
    let mut cmd = crate::proc::group_command(program);
    cmd.args(args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Ran {
                code: 127,
                stdout: Vec::new(),
                stderr: format!("{program}: {e}\n").into_bytes(),
            }
        }
    };
    let pid = child.id();
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => Ran {
            code: out.status.code().unwrap_or(1),
            stdout: out.stdout,
            stderr: out.stderr,
        },
        Ok(Err(e)) => Ran {
            code: 1,
            stdout: Vec::new(),
            stderr: e.to_string().into_bytes(),
        },
        Err(_) => {
            if let Some(pid) = pid {
                crate::proc::kill_tree(pid).await;
            }
            Ran {
                code: 124,
                stdout: Vec::new(),
                stderr: Vec::new(),
            }
        }
    }
}

/// The first `model = "…"` above the first table of a Codex config file.
fn top_level_model(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .take_while(|l| !l.trim_start().starts_with('['))
        .filter(|l| l.starts_with("model = "))
        .find_map(|l| match (l.find('"'), l.rfind('"')) {
            (Some(a), Some(b)) if b > a + 1 => Some(l[a + 1..b].to_owned()),
            _ => None,
        })
}

/// Ports `lib/codex-model.sh`: the model of `~/.codex/review.config.toml`,
/// else that of `~/.codex/config.toml`, else the default.
#[must_use]
pub fn codex_model(home: &Path) -> String {
    let dir = home.join(".codex");
    top_level_model(&dir.join("review.config.toml"))
        .or_else(|| top_level_model(&dir.join("config.toml")))
        .unwrap_or_else(|| DEFAULT_CODEX_MODEL.to_owned())
}

/// `GEMINI_API_KEY` from the environment, else the first
/// `export GEMINI_API_KEY=` line of `~/.zshrc`.
fn gemini_key(home: &Path) -> Option<String> {
    if let Some(k) = std::env::var("GEMINI_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
    {
        return Some(k);
    }
    let rc = std::fs::read_to_string(home.join(".zshrc")).ok()?;
    let line = rc
        .lines()
        .find(|l| l.starts_with("export GEMINI_API_KEY="))?;
    let v = &line["export GEMINI_API_KEY=".len()..];
    let v = v.strip_prefix('"').unwrap_or(v);
    let v = v.split('"').next().unwrap_or_default();
    (!v.is_empty()).then(|| v.to_owned())
}

/// Ports `gemini-exec.sh <model> <timeout> <prompt>`: the answer on stdout
/// and status 0, or a one-line reason on stderr and the script's status.
async fn gemini_exec(setup: &HeadSetup, prompt: &str, cwd: &Path, timeout: u64) -> Ran {
    let Some(key) = gemini_key(&setup.home) else {
        return Ran {
            code: 3,
            stdout: Vec::new(),
            stderr: b"gemini-exec: no GEMINI_API_KEY\n".to_vec(),
        };
    };
    let env = vec![
        (OsString::from("GEMINI_API_KEY"), OsString::from(key)),
        (
            OsString::from("GEMINI_CLI_TRUST_WORKSPACE"),
            OsString::from("true"),
        ),
    ];
    let args: Vec<String> = [
        "-m",
        GEMINI_MODEL,
        "--approval-mode",
        "plan",
        "-o",
        "text",
        "-p",
        prompt,
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    let ran = run_with_timeout(
        &setup.gemini_bin,
        &args,
        cwd,
        &env,
        Duration::from_secs(timeout),
    )
    .await;
    if ran.code == 127 {
        return Ran {
            code: 4,
            stdout: Vec::new(),
            stderr: b"gemini-exec: gemini CLI not installed\n".to_vec(),
        };
    }
    let out = String::from_utf8_lossy(&ran.stdout)
        .trim_end_matches('\n')
        .to_owned();
    if out.chars().all(char::is_whitespace) {
        return Ran {
            code: 1,
            stdout: Vec::new(),
            stderr: b"gemini-exec: empty/failed response\n".to_vec(),
        };
    }
    Ran {
        code: 0,
        stdout: format!("{out}\n").into_bytes(),
        stderr: Vec::new(),
    }
}

fn append(path: &Path, line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}

/// `grep -qiE "429|rate.?limit"`.
fn is_rate_limited(text: &[u8]) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"429|[rR][aA][tT][eE][^\n]?[lL][iI][mM][iI][tT]").unwrap())
        .is_match(&String::from_utf8_lossy(text))
}

/// A head's prompt as the workflow's heredocs assemble it: the text written
/// to `<key>.prompt`, the prompt the CLI receives (`$(cat …)`, trailing
/// newlines gone), and the part the truncation guard counts, which leaves the
/// slice file out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assembled {
    pub file_text: String,
    pub prompt: String,
    pub checked: String,
}

#[must_use]
pub fn assemble(body: &str, slice_file: Option<&str>, mark: &str) -> Assembled {
    let lines: Vec<&str> = body.split('\n').collect();
    let at = lines.iter().position(|l| *l == mark);
    let (file_text, checked) = match (slice_file, at) {
        (Some(path), Some(i)) => {
            let before = lines[..i].join("\n");
            let after = lines[i + 1..].join("\n");
            let slice = std::fs::read_to_string(path).unwrap_or_default();
            (
                format!("{before}\n{slice}\n{after}\n"),
                format!("{before}\n{after}"),
            )
        }
        _ => (format!("{body}\n"), body.to_owned()),
    };
    let prompt = file_text.trim_end_matches('\n').to_owned();
    Assembled {
        file_text,
        prompt,
        checked,
    }
}

/// `CHARS: <bytes on disk>/<units expected>`: `tr -d '[:space:]' | wc -c`
/// over the written text, and the JavaScript `\s`-stripped length.
fn chars_line(checked: &str) -> String {
    let bytes = format!("{checked}\n")
        .bytes()
        .filter(|b| !matches!(b, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r'))
        .count();
    let units: usize = checked
        .chars()
        .filter(|c| {
            !tribunal_core::js::is_js_whitespace(*c) && !tribunal_core::js::is_line_terminator(*c)
        })
        .map(char::len_utf16)
        .sum();
    format!("CHARS: {bytes}/{units}")
}

/// Casts one external vote: `source` is `codex` or `gemini`, `key` the
/// artifact key, `high_risk` picks Codex's reasoning effort. Returns `false`
/// when the latch shows the block already ran.
pub async fn run_head(
    setup: &HeadSetup,
    key: &str,
    source: &str,
    assembled: &Assembled,
    high_risk: bool,
) -> bool {
    let prompt = assembled.prompt.as_str();
    let _ = std::fs::create_dir_all(&setup.vote_dir);
    let file = |ext: &str| setup.vote_dir.join(format!("{key}.{ext}"));
    let (prompt_path, out, meta, lock) = (file("prompt"), file("out"), file("meta"), file("lock"));
    let _ = std::fs::write(&prompt_path, &assembled.file_text);
    if std::fs::create_dir(&lock).is_err() {
        append(&meta, "BLOCKED: 1");
        return false;
    }
    let cwd = match &setup.external_cwd {
        Some(dir) if !dir.is_dir() => {
            let _ = std::fs::write(
                &out,
                format!("HARNESS FAULT: review worktree missing — head not invoked for {key}\n"),
            );
            return true;
        }
        Some(dir) => dir.clone(),
        None => setup.fallback_cwd.clone(),
    };
    append(&meta, &chars_line(&assembled.checked));
    let mut runs = 0;
    let codex_args = |model: &str| -> Vec<String> {
        let mut a: Vec<String> = [
            "exec",
            "--sandbox",
            "read-only",
            "--skip-git-repo-check",
            "--ephemeral",
            "--ignore-user-config",
            "-m",
            model,
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        if let Some(dir) = &setup.external_cwd {
            a.extend(["-C".to_owned(), dir.to_string_lossy().into_owned()]);
        }
        a.extend([
            "-c".to_owned(),
            format!(
                "model_reasoning_effort={}",
                if high_risk { "high" } else { "medium" }
            ),
            "-o".to_owned(),
            out.to_string_lossy().into_owned(),
            prompt.to_owned(),
        ]);
        a
    };
    let model = codex_model(&setup.home);
    if source == "codex" {
        append(&meta, &format!("MODEL: {model}"));
    }
    let once = || async {
        let started = Instant::now();
        let ran = if source == "codex" {
            run_with_timeout(
                &setup.codex_bin,
                &codex_args(&model),
                &cwd,
                &[],
                Duration::from_secs(EXTERNAL_HEAD_TIMEOUT_S),
            )
            .await
        } else {
            let r = gemini_exec(setup, prompt, &cwd, EXTERNAL_HEAD_TIMEOUT_S).await;
            let mut text = r.stdout.clone();
            text.extend_from_slice(&r.stderr);
            let _ = std::fs::write(&out, &text);
            r
        };
        (ran, started.elapsed())
    };
    let (mut ran, mut elapsed) = once().await;
    runs += 1;
    let retry_source = |ran: &Ran| -> Vec<u8> {
        if source == "codex" {
            ran.stderr.clone()
        } else {
            std::fs::read(&out).unwrap_or_default()
        }
    };
    if ran.code != 0
        && elapsed.as_secs() < RETRY_FAST_THRESHOLD_S
        && is_rate_limited(&retry_source(&ran))
    {
        (ran, elapsed) = once().await;
        runs += 1;
    }
    let _ = elapsed;
    append(&meta, &format!("RUNS: {runs}"));
    if ran.code != 0 && ran.code != 124 {
        let hf = classify_head_failure(&String::from_utf8_lossy(&retry_source(&ran)));
        append(
            &meta,
            &format!("HEAD-FAILURE: {}: {}", hf.class.as_str(), hf.reason),
        );
        let previous = std::fs::read(&out).unwrap_or_default();
        let mut text = format!("{}\n", hf.artifact_line()).into_bytes();
        text.extend(previous);
        let _ = std::fs::write(&out, text);
    }
    true
}

/// The clean-verdict sweep: one Codex run over the diff, its answer (or the
/// head-failure line) read back from its artifact.
pub async fn run_sweep(setup: &HeadSetup, prompt: &str) -> String {
    let dir = setup.vote_dir.join("sweep");
    let _ = std::fs::create_dir_all(&dir);
    let out = dir.join("sweep-codex.out");
    let meta = dir.join("sweep-codex.meta");
    if std::fs::create_dir(dir.join("sweep.lock")).is_err() {
        return std::fs::read_to_string(&out).unwrap_or_default();
    }
    let model = codex_model(&setup.home);
    append(&meta, &format!("MODEL: {model}"));
    let cwd = setup
        .external_cwd
        .clone()
        .unwrap_or_else(|| setup.fallback_cwd.clone());
    let args: Vec<String> = vec![
        "exec".into(),
        "--sandbox".into(),
        "read-only".into(),
        "--skip-git-repo-check".into(),
        "--ephemeral".into(),
        "--ignore-user-config".into(),
        "-m".into(),
        model,
        "-C".into(),
        cwd.to_string_lossy().into_owned(),
        "-c".into(),
        "model_reasoning_effort=medium".into(),
        "-o".into(),
        out.to_string_lossy().into_owned(),
        prompt.to_owned(),
    ];
    let ran = run_with_timeout(
        &setup.codex_bin,
        &args,
        &cwd,
        &[],
        Duration::from_secs(SWEEP_TIMEOUT_S),
    )
    .await;
    if ran.code == 124 {
        append(&meta, "FAILCLASS: timeout");
    } else if ran.code != 0 {
        let hf = classify_head_failure(&String::from_utf8_lossy(&ran.stderr));
        append(&meta, &format!("FAILCLASS: {}", hf.class.as_str()));
        let _ = std::fs::write(&out, format!("{}\n", hf.artifact_line()));
    }
    append(&meta, "RUNS: 1");
    std::fs::read_to_string(&out).unwrap_or_default()
}

/// `command -v <program>`.
#[must_use]
pub fn installed(program: &str) -> bool {
    crate::proc::find_in_path(program).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limits_are_recognised() {
        assert!(is_rate_limited(b"HTTP 429 Too Many Requests"));
        assert!(is_rate_limited(b"Rate limit reached"));
        assert!(is_rate_limited(b"rate-limited by upstream"));
        assert!(is_rate_limited(b"ratelimit"));
        assert!(!is_rate_limited(b"generated code"));
    }

    #[test]
    fn the_codex_model_comes_from_its_config() {
        let dir = std::env::temp_dir().join(format!("tribunal-heads-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".codex")).unwrap();
        assert_eq!(codex_model(&dir), DEFAULT_CODEX_MODEL);
        std::fs::write(
            dir.join(".codex/config.toml"),
            "# x\nmodel = \"gpt-x\"\nmodel = \"y\"\n",
        )
        .unwrap();
        assert_eq!(codex_model(&dir), "gpt-x");
        std::fs::write(
            dir.join(".codex/review.config.toml"),
            "model_reasoning_effort = \"low\"\nmodel = \"gpt-r\"\n",
        )
        .unwrap();
        assert_eq!(codex_model(&dir), "gpt-r", "the review profile wins");
        std::fs::write(
            dir.join(".codex/review.config.toml"),
            "approval_policy = \"never\"\n",
        )
        .unwrap();
        assert_eq!(
            codex_model(&dir),
            "gpt-x",
            "a profile without a model falls through"
        );
        let _ = std::fs::remove_file(dir.join(".codex/review.config.toml"));
        std::fs::write(
            dir.join(".codex/config.toml"),
            "[profiles.review]\nmodel = \"no\"\n",
        )
        .unwrap();
        assert_eq!(
            codex_model(&dir),
            DEFAULT_CODEX_MODEL,
            "a model inside a table is not top-level"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
