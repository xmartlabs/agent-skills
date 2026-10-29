//! `inflight-runs.jsonl`: the `started` line the Context phase's bookkeeping
//! appends and the `completed` line the persist step appends, byte for byte
//! as the workflow's heredocs write them, so `eval/lost-runs.mjs` pairs them.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// `${TRIBUNAL_DATA_DIR:-$HOME/.claude/tribunal-data}`.
#[must_use]
pub fn data_dir(env_dir: Option<&str>, home: &str) -> PathBuf {
    match env_dir.filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(format!("{home}/.claude/tribunal-data")),
    }
}

/// `date -u +%Y-%m-%dT%H:%M:%SZ` for an epoch.
#[must_use]
pub fn utc_stamp(epoch: u64) -> String {
    let days = epoch / 86_400;
    let secs = epoch % 86_400;
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(mo <= 2);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

#[must_use]
pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A bash `case` pattern: `*`, `?` and literal characters.
fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    fn go(p: &[char], t: &[char]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some('*') => (0..=t.len()).any(|i| go(&p[1..], &t[i..])),
            Some('?') => !t.is_empty() && go(&p[1..], &t[1..]),
            Some(c) => t.first() == Some(c) && go(&p[1..], &t[1..]),
        }
    }
    go(&p, &t)
}

fn append(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{line}")
}

fn json_or_null(s: Option<&str>) -> String {
    match s.filter(|s| !s.is_empty()) {
        Some(s) => format!("\"{s}\""),
        None => "null".to_owned(),
    }
}

/// The bookkeeping's marker: the `started` line, unless the base branch is a
/// deploy branch. Returns the epoch it recorded (the run's `startEpoch`).
#[allow(clippy::too_many_arguments)]
pub fn append_started(
    dir: &Path,
    pr: u64,
    repo: &str,
    head_sha: Option<&str>,
    batch_panels_requested: bool,
    base_ref: &str,
    deploy_branches: &[String],
    epoch: u64,
) -> Option<u64> {
    let pattern_hit = if deploy_branches.is_empty() {
        glob("__tribunal_no_deploy_branch__", base_ref)
    } else {
        deploy_branches.iter().any(|b| glob(b, base_ref))
    };
    let _ = std::fs::create_dir_all(dir);
    if pattern_hit {
        return None;
    }
    let line = format!(
        "{{\"event\":\"started\",\"pr\":{pr},\"repo\":\"{repo}\",\"headSha\":{},\"batchPanelsRequested\":{batch_panels_requested},\"ts\":\"{}\",\"epoch\":{epoch}}}",
        json_or_null(head_sha),
        utc_stamp(epoch)
    );
    append(&dir.join("inflight-runs.jsonl"), &line)
        .ok()
        .map(|()| epoch)
}

/// The persist step's `completed` line.
///
/// # Errors
/// When the file cannot be appended to.
pub fn append_completed(
    dir: &Path,
    pr: u64,
    repo: Option<&str>,
    head_sha: Option<&str>,
    batch_panels: bool,
    epoch: u64,
) -> std::io::Result<()> {
    let line = format!(
        "{{\"event\":\"completed\",\"pr\":{pr},\"repo\":{},\"headSha\":{},\"batchPanels\":{batch_panels},\"ts\":\"{}\"}}",
        json_or_null(repo),
        json_or_null(head_sha),
        utc_stamp(epoch)
    );
    append(&dir.join("inflight-runs.jsonl"), &line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_are_utc() {
        assert_eq!(utc_stamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_stamp(1_792_714_363), "2026-10-23T00:12:43Z");
        assert_eq!(utc_stamp(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn deploy_branches_skip_the_marker() {
        let dir = std::env::temp_dir().join(format!("tribunal-inflight-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let deploy = vec!["staging".to_owned(), "release-*".to_owned()];
        assert_eq!(
            append_started(&dir, 7, "o/r", None, false, "staging", &deploy, 5),
            None
        );
        assert_eq!(
            append_started(&dir, 7, "o/r", None, false, "release-1", &deploy, 5),
            None
        );
        assert_eq!(
            append_started(&dir, 7, "o/r", Some("abc"), true, "development", &deploy, 5),
            Some(5)
        );
        append_completed(&dir, 7, None, Some("abc"), false, 6).unwrap();
        let text = std::fs::read_to_string(dir.join("inflight-runs.jsonl")).unwrap();
        assert_eq!(
            text,
            "{\"event\":\"started\",\"pr\":7,\"repo\":\"o/r\",\"headSha\":\"abc\",\"batchPanelsRequested\":true,\"ts\":\"1970-01-01T00:00:05Z\",\"epoch\":5}\n{\"event\":\"completed\",\"pr\":7,\"repo\":null,\"headSha\":\"abc\",\"batchPanels\":false,\"ts\":\"1970-01-01T00:00:06Z\"}\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
