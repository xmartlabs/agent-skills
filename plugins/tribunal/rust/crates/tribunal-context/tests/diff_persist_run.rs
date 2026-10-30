//! `lib/persist-run.sh` against `tribunal_context::persist_run`: the same vote
//! directory, data directory and arguments on both sides, compared on the
//! printed line, the appended `metrics.jsonl` and `inflight-runs.jsonl`, and
//! the files left in the vote directory. Only what the clock decides is
//! normalised: `ts` stamps and the elapsed seconds.
//!
//! The script's side is read from `tests/goldens/persist-run.json`: running
//! the script for every case made this the slowest test binary. The file keeps
//! the text of the scripts it was recorded from and the clock the cases were
//! generated against, so a script or generator edited without a regeneration
//! fails here; a script no longer on disk has been retired and the golden is
//! final for it. Nothing runs the script outside a regeneration. Regenerate
//! after changing either:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_persist_run`.
mod common;
#[path = "../../tribunal-core/tests/common/mod.rs"]
mod core_common;

use common::{plugin_root, scratch};
use core_common::{assert_same, sample};

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use proptest::collection::vec;
use proptest::option;
use proptest::prelude::*;
use proptest::sample::select;
use regex::Regex;
use serde_json::{json, Value};
use tribunal_context::harness_version;
use tribunal_context::persist_run::{run, PersistRun};

/// What the script reads besides itself. harness-version.mjs is left out: the
/// version it prints is normalised away, and diff_harness_version covers it.
const SCRIPTS: [&str; 3] = [
    "lib/persist-run.sh",
    "lib/reconcile-votes.mjs",
    "lib/audit-run.mjs",
];
const REGENERATE: &str =
    "regenerate: TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_persist_run";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// The clock the cases are generated against: now when recording, the
/// recorded one when replaying.
static T0: OnceLock<u64> = OnceLock::new();

fn t0() -> u64 {
    *T0.get_or_init(now)
}

#[derive(Debug, Clone)]
struct Vote {
    slot: u8,
    source: &'static str,
    verdict: &'static str,
    artifact: Option<&'static str>,
}

#[derive(Debug, Clone)]
struct Case {
    has_votes: bool,
    votes_file: u8,
    votes: Vec<Vote>,
    findings: Value,
    expected_delta: usize,
    metrics_extra: Value,
    report: Option<Value>,
    inflight: Vec<String>,
    ledger: Vec<String>,
    stale_lock: bool,
    head_sha: &'static str,
    hint: String,
    pr_repo: Option<&'static str>,
    batch: bool,
}

const ARTIFACTS: [&str; 4] = [
    "VERDICT: REFUTED\nCONFIDENCE: 0.9\n",
    "VERDICT: NOT_REFUTED\nCONFIDENCE: 0.4\n",
    "HEAD FAILURE (quota): 429 rate limit\n",
    "",
];

fn vote() -> impl Strategy<Value = Vote> {
    (
        0u8..4,
        select(vec!["codex", "gemini"]),
        select(vec!["REFUTED", "NOT_REFUTED"]),
        option::of(select(ARTIFACTS.to_vec())),
    )
        .prop_map(|(slot, source, verdict, artifact)| Vote {
            slot,
            source,
            verdict,
            artifact,
        })
}

fn odd_value() -> impl Strategy<Value = Value> {
    select(vec![
        json!(1.5),
        json!(1e20),
        json!(1e16),
        json!(1e15),
        json!(1e-7),
        json!(0.0001),
        json!(0.1),
        json!(100.0),
        json!(-0.0),
        json!(12345678901234567u64),
        json!("plain"),
        json!("é ü ñ"),
        json!("emoji 😀"),
        json!("ctl \u{7f}\u{1}\u{1f}"),
        json!("quote \" back \\ tab\t"),
        json!(null),
        json!(true),
        json!([]),
        json!({}),
        json!([1, 2.5, "x"]),
    ])
}

fn finding() -> impl Strategy<Value = Value> {
    (any::<bool>(), any::<bool>(), odd_value()).prop_map(|(blocking, adjudicated, extra)| {
        json!({"title": "t — é", "blocking": blocking, "adjudicated": adjudicated, "extra": extra})
    })
}

fn findings() -> impl Strategy<Value = Value> {
    prop_oneof![
        8 => vec(finding(), 0..4).prop_map(Value::Array),
        1 => Just(json!(null)),
        1 => Just(json!(5)),
    ]
}

fn epoch_field() -> impl Strategy<Value = Option<Value>> {
    let base = t0() as i64;
    prop_oneof![
        3 => select(vec![100i64, 5000, 15_000, 20_000, 50_000]).prop_map(move |o| Some(json!(base - o))),
        1 => Just(Some(json!(base + 86_400))),
        1 => Just(Some(json!((base - 300) as f64 + 0.5))),
        1 => Just(Some(json!(true))),
        1 => Just(None),
    ]
}

fn ts_field() -> impl Strategy<Value = Option<Value>> {
    select(vec![
        Some(json!("2026-10-22T10:00:00Z")),
        Some(json!("2026-1-2t3:4:5z")),
        Some(json!("2026-02-30T00:00:00Z")),
        Some(json!("2026-10-22T10:00:60Z")),
        Some(json!("2026-10-22 10:00:00")),
        Some(json!(5)),
        None,
    ])
}

fn inflight_line() -> impl Strategy<Value = String> {
    let started = (
        select(vec![json!(77), json!(77.0), json!(78), json!(true)]),
        select(vec![
            None,
            Some(json!("o/r")),
            Some(json!("x/y")),
            Some(json!("")),
        ]),
        select(vec![None, Some(json!("abc")), Some(json!("def"))]),
        epoch_field(),
        ts_field(),
    )
        .prop_map(|(pr, repo, sha, epoch, ts)| {
            let mut m = serde_json::Map::new();
            m.insert("event".into(), json!("started"));
            m.insert("pr".into(), pr);
            if let Some(r) = repo {
                m.insert("repo".into(), r);
            }
            if let Some(s) = sha {
                m.insert("headSha".into(), s);
            }
            if let Some(t) = ts {
                m.insert("ts".into(), t);
            }
            if let Some(e) = epoch {
                m.insert("epoch".into(), e);
            }
            Value::Object(m).to_string()
        });
    prop_oneof![
        8 => started,
        1 => Just("{not json".to_owned()),
        1 => Just(String::new()),
        1 => Just(format!("{{\"event\":\"completed\",\"pr\":77,\"epoch\":{}}}", t0() - 10)),
        1 => Just("[1, 2]".to_owned()),
        1 => Just("  {\"event\":\"started\",\"pr\":77,\"epoch\":1}\r".to_owned()),
    ]
}

fn ledger_line() -> impl Strategy<Value = String> {
    let row = (
        select(vec![json!(77), json!(77.0), json!(78)]),
        select(vec![json!(null), json!("abc"), json!("def")]),
        select(vec![None, Some(json!("o/r")), Some(json!("x/y"))]),
    )
        .prop_map(|(pr, sha, repo)| {
            let mut v = json!({"pr": pr, "headSha": sha});
            if let Some(r) = repo {
                v["repo"] = r;
            }
            v.to_string()
        });
    prop_oneof![
        10 => row,
        1 => Just("garbage".to_owned()),
        1 => Just(String::new()),
        1 => Just("[\"not a row\"]".to_owned()),
    ]
}

fn case() -> impl Strategy<Value = Case> {
    (
        (
            any::<bool>(),
            select(vec![0u8, 0, 0, 1, 2]),
            vec(vote(), 0..4),
        ),
        (findings(), select(vec![0usize, 0, 0, 0, 1]), odd_value()),
        option::of(select(vec![json!("## Report\n— é\n"), json!(""), json!(5)])),
        (vec(inflight_line(), 0..5), vec(ledger_line(), 0..4)),
        (
            prop::bool::weighted(0.1),
            select(vec!["", "abc", " abc "]),
            select(vec![
                String::new(),
                (t0() - 700).to_string(),
                " 456 ".to_owned(),
                "abc".to_owned(),
                "-5".to_owned(),
            ]),
            select(vec![None, Some("o/r")]),
            any::<bool>(),
        ),
    )
        .prop_map(
            |(
                (has_votes, votes_file, votes),
                (findings, expected_delta, metrics_extra),
                report,
                (inflight, ledger),
                (stale_lock, head_sha, hint, pr_repo, batch),
            )| Case {
                has_votes,
                votes_file,
                votes,
                findings,
                expected_delta,
                metrics_extra,
                report,
                inflight,
                ledger,
                stale_lock,
                head_sha,
                hint,
                pr_repo,
                batch,
            },
        )
}

fn expected_findings(c: &Case) -> usize {
    c.findings.as_array().map_or(0, Vec::len) + c.expected_delta
}

fn metrics(c: &Case) -> Value {
    json!({
        "pr": 77,
        "repo": c.pr_repo,
        "headSha": "abc",
        "wallClockSource": null,
        "odd": c.metrics_extra,
        "findings": c.findings,
        "codexEnabled": true,
    })
}

fn lay_out(dir: &Path, c: &Case) -> (PathBuf, PathBuf) {
    let vd = dir.join("vd");
    let data = dir.join("data");
    std::fs::create_dir_all(&vd).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(vd.join("metrics.json"), metrics(c).to_string()).unwrap();
    if let Some(r) = &c.report {
        std::fs::write(vd.join("report.json"), json!({ "report": r }).to_string()).unwrap();
    }
    let votes: Vec<Value> = c
        .votes
        .iter()
        .map(|v| json!({"slotId": v.slot, "source": v.source, "verdict": v.verdict, "confidence": 0.9}))
        .collect();
    match c.votes_file {
        0 => std::fs::write(
            vd.join("recorded-votes.json"),
            Value::Array(votes).to_string(),
        )
        .unwrap(),
        1 => std::fs::write(vd.join("recorded-votes.json"), "{oops").unwrap(),
        _ => {}
    }
    for v in &c.votes {
        if let Some(a) = v.artifact {
            std::fs::write(vd.join(format!("{}-{}.out", v.slot, v.source)), a).unwrap();
            std::fs::write(
                vd.join(format!("{}-{}.meta", v.slot, v.source)),
                "CHARS: 10/10\nMODEL: gpt-x\nRUNS: 1\n",
            )
            .unwrap();
        }
    }
    if !c.inflight.is_empty() {
        std::fs::write(
            data.join("inflight-runs.jsonl"),
            c.inflight.join("\n") + "\n",
        )
        .unwrap();
    }
    if !c.ledger.is_empty() {
        std::fs::write(data.join("metrics.jsonl"), c.ledger.join("\n") + "\n").unwrap();
    }
    if c.stale_lock {
        let lock = data.join("metrics.jsonl.lock");
        std::fs::create_dir(&lock).unwrap();
        let ok = Command::new("touch")
            .args(["-t", "202001010000"])
            .arg(&lock)
            .status()
            .unwrap()
            .success();
        assert!(ok);
    }
    (vd, data)
}

fn shim_bin(root: &Path) -> PathBuf {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let shim = bin.join("tribunal-root");
    std::fs::write(
        &shim,
        format!("#!/bin/sh\necho '{}'\n", plugin_root().display()),
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    bin
}

fn script(dir: &Path, bin: &Path, c: &Case) -> String {
    let (vd, data) = lay_out(dir, c);
    let mut cmd = Command::new("bash");
    cmd.arg(plugin_root().join("lib/persist-run.sh"))
        .arg(&vd)
        .arg(if c.has_votes { "1" } else { "0" })
        .arg(expected_findings(c).to_string())
        .arg("77")
        .arg(c.head_sha)
        .arg(c.batch.to_string())
        .arg(&c.hint)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("TRIBUNAL_DATA_DIR", &data)
        .env_remove("CLAUDE_PLUGIN_ROOT")
        .env_remove("PAYLOAD_CHECKSUMS");
    match c.pr_repo {
        Some(r) => cmd.env("PR_REPO", r),
        None => cmd.env_remove("PR_REPO"),
    };
    let out = cmd.output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim_end().to_owned()
}

fn port(dir: &Path, c: &Case) -> String {
    let (vd, data) = lay_out(dir, c);
    let root = plugin_root().to_string_lossy().into_owned();
    run(&PersistRun {
        vote_dir: &vd,
        has_votes: c.has_votes,
        expected_findings: expected_findings(c),
        pr: 77,
        head_sha: c.head_sha,
        batch_panels: c.batch,
        start_epoch_hint: &c.hint,
        repo: c.pr_repo,
        data_dir: &data,
        plugin_root: &root,
        now: t0(),
    })
}

/// The `--short` harness version, which both sides write into the row and
/// which moves with every change to the harness.
fn harness() -> &'static str {
    static CELL: OnceLock<String> = OnceLock::new();
    CELL.get_or_init(|| {
        let root = plugin_root().to_string_lossy().into_owned();
        harness_version::run(&["--short"], &root)
            .map(|s| s.trim().to_owned())
            .unwrap_or_default()
    })
}

fn clockless(text: &str) -> String {
    static RES: OnceLock<Vec<(Regex, &str)>> = OnceLock::new();
    let res = RES.get_or_init(|| {
        vec![
            (Regex::new(r#""ts": ?"[^"]*""#).unwrap(), r#""ts":"T""#),
            (
                Regex::new(r#""wallClockSeconds": ?-?\d+"#).unwrap(),
                r#""wallClockSeconds":N"#,
            ),
            (Regex::new(r"\d+s exceeds").unwrap(), "Ns exceeds"),
            (Regex::new(r"end epoch \d+").unwrap(), "end epoch E"),
            (
                Regex::new(r#"Traceback \(most recent call last\):(?:\\n|\n)(?:.*?(?:\\n|\n))*?(\w+Error: )"#).unwrap(),
                "$1",
            ),
        ]
    });
    let text = res.iter().fold(text.to_owned(), |t, (re, with)| {
        re.replace_all(&t, *with).into_owned()
    });
    if harness().is_empty() {
        return text;
    }
    text.replace(
        &format!(r#""harnessVersion": "{}""#, harness()),
        r#""harnessVersion": "H""#,
    )
}

/// What a side left behind, as text with the clock taken out.
fn observe(dir: &Path, stdout: &str) -> Value {
    let vd = dir.join("vd");
    let data = dir.join("data");
    let read = |p: PathBuf| std::fs::read_to_string(p).ok().map(|t| clockless(&t));
    let mut files: BTreeMap<String, Value> = BTreeMap::new();
    for name in [
        "recon.json",
        "report.md",
        "audit.json",
        "wallclock.json",
        "persist-result.txt",
    ] {
        files.insert(name.into(), json!(read(vd.join(name))));
    }
    let mut names: Vec<String> = std::fs::read_dir(&vd)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.ends_with(".err"))
        .collect();
    names.sort();
    json!({
        "stdout": clockless(stdout),
        "metrics.jsonl": read(data.join("metrics.jsonl")),
        "inflight": read(data.join("inflight-runs.jsonl")),
        "lockLeft": data.join("metrics.jsonl.lock").exists(),
        "files": files,
        "names": names,
    })
}

fn describe(c: &Case) -> Value {
    json!({
        "hasVotes": c.has_votes, "votesFile": c.votes_file,
        "votes": c.votes.iter().map(|v| format!("{}-{} {} {:?}", v.slot, v.source, v.verdict, v.artifact)).collect::<Vec<_>>(),
        "findings": c.findings, "expectedDelta": c.expected_delta, "extra": c.metrics_extra,
        "report": c.report, "inflight": c.inflight, "ledger": c.ledger, "staleLock": c.stale_lock,
        "headSha": c.head_sha, "hint": c.hint, "prRepo": c.pr_repo, "batch": c.batch,
    })
}

struct Outcome {
    input: Value,
    rust: Value,
    js: Value,
}

fn par_map<T: Sync, R: Send>(cases: &[T], f: impl Fn(usize, &T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<R>>> = cases.iter().map(|_| Mutex::new(None)).collect();
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(case) = cases.get(i) else { break };
                *slots[i].lock().unwrap() = Some(f(i, case));
            });
        }
    });
    slots
        .into_iter()
        .map(|s| s.into_inner().unwrap().unwrap())
        .collect()
}

fn golden_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/persist-run.json")
}

fn script_texts() -> Value {
    let texts: BTreeMap<&str, String> = SCRIPTS
        .iter()
        .map(|s| (*s, std::fs::read_to_string(plugin_root().join(s)).unwrap()))
        .collect();
    json!(texts)
}

fn regenerating() -> bool {
    std::env::var("TRIBUNAL_REGEN_GOLDENS").is_ok_and(|v| v == "1")
}

/// The scripts in `recorded` (`{path: text}`) or `scripts` whose text under
/// `root` is not what was recorded. A script no longer on disk was retired
/// with the Workflow and its golden is final, so it is skipped; when
/// `scripts[0]`, the one the cases run, is gone, the rest are only what it
/// ran and are skipped too.
fn stale_scripts(recorded: &Value, scripts: &[&str], root: &Path) -> Vec<String> {
    let on_disk = |s: &str| match std::fs::read_to_string(root.join(s)) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => panic!("{s}: {e}"),
    };
    if on_disk(scripts[0]).is_none() {
        return Vec::new();
    }
    let mut names: BTreeSet<String> = scripts.iter().map(|s| (*s).to_owned()).collect();
    names.extend(
        recorded
            .as_object()
            .into_iter()
            .flat_map(|m| m.keys().cloned()),
    );
    names
        .into_iter()
        .filter(|s| on_disk(s).is_some_and(|text| recorded.get(s) != Some(&json!(text))))
        .collect()
}

/// The golden, checked against the scripts on disk; `None` when regenerating.
fn recorded() -> Option<Value> {
    if regenerating() {
        return None;
    }
    let path = golden_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; {REGENERATE}", path.display()));
    let golden: Value = serde_json::from_str(&text).unwrap();
    let stale = stale_scripts(&golden["scripts"], &SCRIPTS, &plugin_root());
    assert!(
        stale.is_empty(),
        "{stale:?} changed since {} was recorded; {REGENERATE}",
        path.display()
    );
    Some(golden)
}

fn outcomes() -> &'static [Outcome] {
    static CELL: OnceLock<Vec<Outcome>> = OnceLock::new();
    CELL.get_or_init(|| {
        let golden = recorded();
        if let Some(g) = &golden {
            let clock = g["t0"].as_u64().expect("the golden records t0");
            assert!(
                T0.set(clock).is_ok() || t0() == clock,
                "the clock was read before the golden"
            );
        }
        t0();
        let cases = sample(case());
        let inputs: Vec<Value> = cases.iter().map(describe).collect();
        let root = scratch("persist-run");
        let js: Vec<Value> = match &golden {
            Some(g) => {
                let cases = g["cases"].as_array().unwrap();
                assert_eq!(
                    cases.len(),
                    inputs.len(),
                    "the generator changed; {REGENERATE}"
                );
                cases
                    .iter()
                    .zip(&inputs)
                    .map(|(c, input)| {
                        assert_eq!(&c["input"], input, "the generator changed; {REGENERATE}");
                        c["js"].clone()
                    })
                    .collect()
            }
            None => {
                let bin = shim_bin(&root);
                let js = par_map(&cases, |i, c| {
                    let dir = root.join(format!("{i}/js"));
                    let out = script(&dir, &bin, c);
                    observe(&dir, &out)
                });
                let cases: Vec<Value> = inputs
                    .iter()
                    .zip(&js)
                    .map(|(input, js)| json!({"input": input, "js": js}))
                    .collect();
                let text = serde_json::to_string_pretty(
                    &json!({"scripts": script_texts(), "t0": t0(), "cases": cases}),
                )
                .unwrap();
                assert!(
                    !text.contains(&*root.to_string_lossy()),
                    "the script's output names the scratch directory, which the golden cannot keep"
                );
                std::fs::write(golden_path(), text + "\n").unwrap();
                js
            }
        };
        let rust = par_map(&cases, |i, c| {
            let dir = root.join(format!("{i}/rs"));
            let out = port(&dir, c);
            observe(&dir, &out)
        });
        let _ = std::fs::remove_dir_all(&root);
        inputs
            .into_iter()
            .zip(rust)
            .zip(js)
            .map(|((input, rust), js)| Outcome { input, rust, js })
            .collect()
    })
}

#[test]
fn persist_run_matches_the_script_on_generated_cases() {
    let out = outcomes();
    let as_text = |v: &Value| json!(v.to_string());
    let inputs: Vec<Value> = out.iter().map(|o| o.input.clone()).collect();
    let rust: Vec<Value> = out.iter().map(|o| as_text(&o.rust)).collect();
    let js: Vec<Value> = out.iter().map(|o| as_text(&o.js)).collect();
    assert_same("persist-run", &inputs, &rust, &js);
}

#[test]
fn the_generated_cases_reach_every_outcome() {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for o in outcomes() {
        let s = o.js["stdout"].as_str().unwrap_or("");
        let jsonl = o.js["metrics.jsonl"].as_str().unwrap_or("");
        let mut hit =
            |k: &'static str, cond: bool| *seen.entry(k).or_default() += usize::from(cond);
        hit("persisted ok", s.contains(r#""persisted": "ok""#));
        hit("refused truncated", s.contains("REFUSED: record carries"));
        hit("len TypeError", s.contains("has no len()"));
        hit("ledger AttributeError", s.contains("AttributeError"));
        hit("inflight-exact", s.contains("inflight-exact"));
        hit("inflight-pr", s.contains("inflight-pr"));
        hit("return-value", s.contains("return-value"));
        hit("no start epoch", s.contains("no start epoch"));
        hit("elapsed note", s.contains("elapsed, not active"));
        hit(
            "rerunOf",
            jsonl.contains(r#""rerunOf": 1"#) || jsonl.contains(r#""rerunOf": 2"#),
        );
        hit(
            "reconciled",
            s.contains(r#""reconciliation": {"checked": "#) && !s.contains(r#""checked": null"#),
        );
        hit(
            "recon error",
            s.contains("reconciliation did not return a usable result"),
        );
        hit("no votes", s.contains(r#""reconciliation": null"#));
        hit(
            "codex model",
            jsonl.contains(r#""codexModelUsed": "gpt-x""#),
        );
        hit("report.md", !o.js["files"]["report.md"].is_null());
        hit(
            "audit violation",
            s.contains("blocking-without-adjudication"),
        );
        hit(
            "stale lock cleared",
            o.input["staleLock"] == json!(true) && s.contains(r#""persisted": "ok""#),
        );
        hit(
            "completed marker",
            o.js["inflight"]
                .as_str()
                .is_some_and(|t| t.contains("\"completed\",\"pr\":77,")),
        );
        hit(
            "float repr",
            jsonl.contains("1e+16") || jsonl.contains("1e-07"),
        );
        hit("ensure_ascii", jsonl.contains("\\u00e9"));
    }
    let thin: Vec<_> = seen.iter().filter(|(_, n)| **n < 3).collect();
    assert!(
        thin.is_empty(),
        "branches under 3 hits: {thin:?}\nall: {seen:?}"
    );
}

/// The staleness check on a tree of its own: a script edited since the
/// recording is stale, one deleted (retired with the Workflow) is not, and
/// with the script the cases run deleted nothing is.
#[test]
fn a_retired_script_is_not_stale() {
    let root = scratch("persist-run-retired");
    std::fs::create_dir_all(root.join("lib")).unwrap();
    let write = |name: &str, text: &str| std::fs::write(root.join(name), text).unwrap();
    write("lib/run.sh", "run");
    write("lib/dep.mjs", "dep edited");
    let recorded = json!({"lib/run.sh": "run", "lib/dep.mjs": "dep", "lib/gone.mjs": "gone"});
    let scripts = ["lib/run.sh", "lib/dep.mjs", "lib/gone.mjs"];
    assert_eq!(stale_scripts(&recorded, &scripts, &root), ["lib/dep.mjs"]);
    write("lib/dep.mjs", "dep");
    assert!(stale_scripts(&recorded, &scripts, &root).is_empty());
    write("lib/new.mjs", "new");
    assert_eq!(
        stale_scripts(&recorded, &["lib/run.sh", "lib/new.mjs"], &root),
        ["lib/new.mjs"]
    );
    std::fs::remove_file(root.join("lib/run.sh")).unwrap();
    write("lib/dep.mjs", "dep edited again");
    assert!(stale_scripts(&recorded, &scripts, &root).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}
