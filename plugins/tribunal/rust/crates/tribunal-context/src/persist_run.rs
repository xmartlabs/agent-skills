//! Port of `lib/persist-run.sh`: reconcile the external votes, run the
//! self-audit, stamp the harness version and the wall clock, and append the
//! row to `metrics.jsonl`, then print the one line the workflow reads back.
//!
//! The steps the script hands to `node` run in process here
//! ([`reconcile_votes`], [`audit_run`], [`harness_version`]); the ones it hands
//! to Python keep Python's semantics through [`py_json`]. The same files land
//! in the vote directory (`recon.json`, `report.md`, `audit.json`,
//! `wallclock.json`, `persist-result.txt`, the rewritten `metrics.json`).
//!
//! The payload-checksum gate is not ported: it checks a copy a model retyped,
//! and the native caller writes the payload files itself.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Map, Value};

use crate::py_json;
use crate::{audit_run, harness_version, inflight, reconcile_votes};

pub const ACTIVE_RUN_BOUND: i128 = 4 * 3600;
const LOCK_WAIT: Duration = Duration::from_secs(15);
const LOCK_STALE: Duration = Duration::from_secs(60);

/// The script's arguments, plus what it reads from its environment and clock.
#[derive(Debug, Clone)]
pub struct PersistRun<'a> {
    pub vote_dir: &'a Path,
    pub has_votes: bool,
    pub expected_findings: usize,
    pub pr: u64,
    /// `$5`, as given: the marker uses it raw, the row stripped.
    pub head_sha: &'a str,
    pub batch_panels: bool,
    /// `$7`, as given; only ASCII digits count.
    pub start_epoch_hint: &'a str,
    /// `PR_REPO`.
    pub repo: Option<&'a str>,
    pub data_dir: &'a Path,
    /// Where `harness-version.mjs` would hash from.
    pub plugin_root: &'a str,
    pub now: u64,
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn get<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.as_object().and_then(|m| m.get(k))
}

fn or_value(v: Option<&Value>, fallback: Value) -> Value {
    if py_json::truthy(v) {
        v.cloned().unwrap_or(fallback)
    } else {
        fallback
    }
}

fn reconcile(p: &PersistRun) {
    let vd = p.vote_dir.to_string_lossy();
    let recon = p.vote_dir.join("recon.json");
    if !p.has_votes {
        let _ = fs::write(&recon, "{\"checked\": -1}");
        return;
    }
    let Ok(bytes) = fs::read(p.vote_dir.join("recorded-votes.json")) else {
        return;
    };
    let stdin = String::from_utf8_lossy(&bytes);
    let (out, err) = match reconcile_votes::run(&[&vd, "--compact"], &stdin) {
        Ok(printed) => (printed.stdout(), String::new()),
        Err(e) => (String::new(), format!("{e}\n")),
    };
    let _ = fs::write(&recon, out);
    let _ = fs::write(p.vote_dir.join("recon.err"), err);
}

/// PYRECON: folds `recon.json` into the metrics. A `recon.json` Python cannot
/// call `.get` on, or whose `checked` it cannot compare, leaves them as they are.
fn fold_reconciliation(metrics: &mut Map<String, Value>, recon: &Value, has_votes: bool) {
    if !has_votes {
        metrics.insert("voteReconciliation".into(), Value::Null);
        metrics.insert("codexModelUsed".into(), Value::Null);
        return;
    }
    let Some(r) = recon.as_object() else { return };
    let checked = r.get("checked").cloned().unwrap_or(json!(-1));
    let Some(usable) = py_json::ge_zero(&checked) else {
        return;
    };
    if usable {
        let field = |k: &str| r.get(k).cloned().unwrap_or(Value::Null);
        metrics.insert(
            "voteReconciliation".into(),
            json!({
                "checked": field("checked"),
                "ok": field("ok"),
                "classifierFailed": or_value(r.get("classifierFailed"), json!(0)),
                "counts": or_value(r.get("counts"), json!({})),
                "mismatches": or_value(r.get("mismatches"), json!([])),
            }),
        );
        let models = or_value(r.get("codexModelsUsed"), json!([]));
        let used = match &models {
            Value::Array(a) if a.len() == 1 => a[0].clone(),
            Value::Array(a) if a.is_empty() => Value::Null,
            other => other.clone(),
        };
        metrics.insert("codexModelUsed".into(), used);
    } else {
        metrics.insert(
            "voteReconciliation".into(),
            json!({
                "checked": null, "ok": null, "counts": {}, "mismatches": [],
                "error": or_value(r.get("scriptError"), json!("reconciliation did not return a usable result")),
            }),
        );
        metrics.insert("codexModelUsed".into(), Value::Null);
    }
}

fn materialise_report(vd: &Path) {
    let Some(report) = read_json(&vd.join("report.json")) else {
        return;
    };
    if let Some(Value::String(body)) = get(&report, "report") {
        if !body.is_empty() {
            let _ = fs::write(vd.join("report.md"), body);
        }
    }
}

/// PYAUDIT.
fn fold_audit(metrics: &mut Map<String, Value>, audit: Option<Value>) {
    let audit = audit.unwrap_or_else(|| json!({}));
    let self_audit = match get(&audit, "passed") {
        Some(Value::Bool(passed)) => json!({
            "passed": passed,
            "checked": or_value(get(&audit, "checked"), json!(0)),
            "violations": or_value(get(&audit, "violations"), json!([])),
            "notes": or_value(get(&audit, "notes"), json!([])),
        }),
        _ => json!({
            "passed": null, "checked": 0,
            "violations": [{"id": "audit-unreadable", "detail": "the self-audit returned nothing usable"}],
            "notes": [],
        }),
    };
    metrics.insert("selfAudit".into(), self_audit);
}

fn epoch_of(entry: &Map<String, Value>) -> Option<i128> {
    if let Some(e) = py_json::int(entry.get("epoch")) {
        return Some(e);
    }
    match entry.get("ts") {
        Some(Value::String(ts)) => py_json::strptime_epoch(ts).map(i128::from),
        _ => None,
    }
}

/// Lines as Python's text mode yields them: universal newlines, `strip()`ed.
fn py_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split("\r\n")
        .flat_map(|l| l.split(['\r', '\n']))
        .map(|l| l.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)))
}

/// PY-WALLCLOCK's `_resolve_start`.
#[must_use]
pub fn resolve_start(
    path: &Path,
    pr: u64,
    head_sha: Option<&str>,
    hint: Option<i128>,
    end_epoch: i128,
    repo: Option<&Value>,
) -> (Option<i128>, Option<&'static str>) {
    let (mut exact, mut loose) = (Vec::new(), Vec::new());
    if let Ok(bytes) = fs::read(path) {
        let text = String::from_utf8_lossy(&bytes);
        let started = json!("started");
        let pr = json!(pr);
        for line in py_lines(&text) {
            if line.is_empty() {
                continue;
            }
            let Ok(entry) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(entry) = entry.as_object() else {
                break;
            };
            if !py_json::eq(entry.get("event"), Some(&started))
                || !py_json::eq(entry.get("pr"), Some(&pr))
            {
                continue;
            }
            if py_json::truthy(repo)
                && py_json::truthy(entry.get("repo"))
                && !py_json::eq(entry.get("repo"), repo)
            {
                continue;
            }
            let Some(found) = epoch_of(entry) else {
                continue;
            };
            if found > end_epoch {
                continue;
            }
            match (head_sha, entry.get("headSha")) {
                (Some(sha), Some(Value::String(h))) if h == sha => exact.push(found),
                _ => loose.push(found),
            }
        }
    }
    if let Some(&e) = exact.last() {
        return (Some(e), Some("inflight-exact"));
    }
    if let Some(&e) = loose.last() {
        return (Some(e), Some("inflight-pr"));
    }
    match hint {
        Some(h) => (Some(h), Some("return-value")),
        None => (None, None),
    }
}

/// The three wall-clock fields.
#[must_use]
pub fn wall_clock(start: Option<i128>, end: i128) -> (Option<i128>, Option<String>) {
    let Some(start) = start else {
        return (
            None,
            Some(
                "no start epoch: neither the inflight trail nor the dispatch return carried one"
                    .into(),
            ),
        );
    };
    let seconds = end - start;
    if seconds < 0 {
        return (
            None,
            Some(format!(
                "refused: start epoch {start} is after end epoch {end}"
            )),
        );
    }
    let note = (seconds > ACTIVE_RUN_BOUND).then(|| {
        format!("elapsed, not active: {seconds}s exceeds the {ACTIVE_RUN_BOUND}s active-run bound")
    });
    (Some(seconds), note)
}

struct Lock(std::path::PathBuf);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

fn lock(path: &Path) -> Result<Lock, String> {
    let lock = std::path::PathBuf::from(format!("{}.lock", path.display()));
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        match fs::create_dir(&lock) {
            Ok(()) => return Ok(Lock(lock)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => {}
        }
        let stale = fs::metadata(&lock)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age > LOCK_STALE);
        if stale && fs::remove_dir(&lock).is_ok() {
            continue;
        }
        if Instant::now() > deadline {
            return Err(format!(
                "could not lock {} within 15 s, nothing written",
                path.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// How many rows already record this PR at this head, or Python's
/// `AttributeError` for a row that is not an object.
fn prior_rows(path: &Path, rec: &Map<String, Value>) -> Result<usize, String> {
    let Ok(bytes) = fs::read(path) else {
        return Ok(0);
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut prior = 0;
    for line in py_lines(&text) {
        if line.is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(row) = row.as_object() else {
            return Err(format!(
                "AttributeError: '{}' object has no attribute 'get'",
                py_json::type_name(&row)
            ));
        };
        let same_repo = !py_json::truthy(row.get("repo"))
            || !py_json::truthy(rec.get("repo"))
            || py_json::eq(row.get("repo"), rec.get("repo"));
        if py_json::eq(row.get("pr"), rec.get("pr"))
            && py_json::eq(row.get("headSha"), rec.get("headSha"))
            && same_repo
        {
            prior += 1;
        }
    }
    Ok(prior)
}

/// PYPERSIST: what it leaves in `persist-result.txt`.
fn append_row(p: &PersistRun, mut rec: Map<String, Value>, harness: Option<String>) -> String {
    let vd = p.vote_dir;
    rec.insert("harnessVersion".into(), json!(harness));
    let got = match rec.get("findings") {
        Some(f) if py_json::truthy(Some(f)) => match py_json::len(f) {
            Some(n) => n,
            None => {
                return format!(
                    "TypeError: object of type '{}' has no len()",
                    py_json::type_name(f)
                )
            }
        },
        _ => 0,
    };
    if got != p.expected_findings {
        return format!(
            "REFUSED: record carries {got} findings, expected {} — the JSON was truncated in transit, nothing written",
            p.expected_findings
        );
    }
    rec.insert("ts".into(), json!(inflight::utc_stamp(p.now)));
    let hint = p.start_epoch_hint.trim();
    let hint = (!hint.is_empty() && hint.bytes().all(|b| b.is_ascii_digit()))
        .then(|| hint.parse::<i128>().ok())
        .flatten();
    let head_sha = Some(p.head_sha.trim()).filter(|s| !s.is_empty());
    let repo = rec
        .get("repo")
        .filter(|r| py_json::truthy(Some(r)))
        .cloned();
    let end = i128::from(p.now);
    let (start, source) = resolve_start(
        &p.data_dir.join("inflight-runs.jsonl"),
        p.pr,
        head_sha,
        hint,
        end,
        repo.as_ref(),
    );
    let (seconds, note) = wall_clock(start, end);
    let num = |x: Option<i128>| x.map_or(Value::Null, |n| json!(n as i64));
    rec.insert("wallClockSeconds".into(), num(seconds));
    rec.insert("wallClockNote".into(), json!(note));
    rec.insert("wallClockSource".into(), json!(source));
    let wall = json!({
        "wallClockSeconds": rec["wallClockSeconds"],
        "wallClockNote": rec["wallClockNote"],
        "wallClockSource": rec["wallClockSource"],
    });
    let _ = fs::write(vd.join("wallclock.json"), py_json::dumps(&wall));

    let path = p.data_dir.join("metrics.jsonl");
    let _guard = match lock(&path) {
        Ok(g) => g,
        Err(e) => return e,
    };
    let prior = match prior_rows(&path, &rec) {
        Ok(n) => n,
        Err(e) => return e,
    };
    rec.insert(
        "rerunOf".into(),
        if prior > 0 { json!(prior) } else { Value::Null },
    );
    let line = format!("{}\n", py_json::dumps(&Value::Object(rec)));
    let written = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
    match written {
        Ok(()) => "ok".into(),
        Err(e) => format!("OSError: {e}"),
    }
}

/// The whole script. Returns its stdout line, without the newline.
#[must_use]
pub fn run(p: &PersistRun) -> String {
    let vd = p.vote_dir;
    let _ = fs::create_dir_all(p.data_dir);
    reconcile(p);

    let metrics_path = vd.join("metrics.json");
    let Some(Value::Object(mut metrics)) = read_json(&metrics_path) else {
        return py_json::dumps(&json!({
            "reconciliation": null, "selfAudit": null,
            "persisted": "metrics.json is missing or not a JSON object, nothing written",
            "wallClockSeconds": null, "wallClockNote": null, "wallClockSource": null,
        }));
    };
    let recon = read_json(&vd.join("recon.json")).unwrap_or_else(|| json!({}));
    fold_reconciliation(&mut metrics, &recon, p.has_votes);
    let text = py_json::dumps(&Value::Object(metrics.clone()));
    let _ = fs::write(&metrics_path, &text);

    if vd.join("report.json").is_file() {
        materialise_report(vd);
    }

    let vd_str = vd.to_string_lossy();
    let (audit_out, audit_err) = match audit_run::run(&[&vd_str], &text) {
        Ok(a) => (format!("{}\n", a.to_line()), String::new()),
        Err(e) => (String::new(), format!("{e}\n")),
    };
    let _ = fs::write(vd.join("audit.json"), &audit_out);
    let _ = fs::write(vd.join("audit.err"), audit_err);
    fold_audit(&mut metrics, serde_json::from_str(&audit_out).ok());
    let _ = fs::write(
        &metrics_path,
        py_json::dumps(&Value::Object(metrics.clone())),
    );

    let harness = harness_version::run(&["--short"], p.plugin_root)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());

    let persisted = append_row(p, metrics.clone(), harness);
    let _ = fs::write(vd.join("persist-result.txt"), format!("{persisted}\n"));

    if persisted.lines().any(|l| l.starts_with("ok")) {
        let _ = inflight::append_completed(
            p.data_dir,
            p.pr,
            p.repo.filter(|r| !r.is_empty()),
            Some(p.head_sha).filter(|s| !s.is_empty()),
            p.batch_panels,
            p.now,
        );
    }

    let wall = read_json(&vd.join("wallclock.json")).unwrap_or_else(|| json!({}));
    let field = |v: &Value, k: &str| get(v, k).cloned().unwrap_or(Value::Null);
    let metrics = Value::Object(metrics);
    py_json::dumps(&json!({
        "reconciliation": field(&metrics, "voteReconciliation"),
        "selfAudit": field(&metrics, "selfAudit"),
        "persisted": persisted.trim(),
        "wallClockSeconds": field(&wall, "wallClockSeconds"),
        "wallClockNote": field(&wall, "wallClockNote"),
        "wallClockSource": field(&wall, "wallClockSource"),
    }))
}
