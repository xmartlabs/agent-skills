//! The read-only subcommands the skill runs through `bin/tribunal-engine`
//! before and around a review (`lost-runs`, `harness-version`), the
//! measurement modes over the ledger (`metrics`, `audit`), and the two
//! loaders templates/check.sh runs on a profile.

use std::path::PathBuf;
use std::process::{Command, Output};

fn tribunal(args: &[&str], data_dir: &std::path::Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tribunal"))
        .args(args)
        .env("TRIBUNAL_DATA_DIR", data_dir)
        .env(
            "TRIBUNAL_PLUGIN_ROOT",
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."),
        )
        .output()
        .expect("tribunal runs")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tribunal-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn lost_runs_without_a_trail() {
    let dir = scratch("no-trail");
    let full = tribunal(&["lost-runs"], &dir);
    assert!(full.status.success());
    assert!(stdout(&full).ends_with("inflight-runs.jsonl: nothing to check\n"));
    // The skill runs --recent before every review: silent when there is
    // nothing to say.
    let recent = tribunal(&["lost-runs", "--recent", "7"], &dir);
    assert!(recent.status.success());
    assert_eq!(stdout(&recent), "");
}

#[test]
fn lost_runs_warns_about_recent_losses_only() {
    let dir = scratch("trail");
    let now = tribunal_context::inflight::now_epoch();
    let stamp = |hours_ago: u64| tribunal_context::inflight::utc_stamp(now - hours_ago * 3600);
    let lines = [
        format!(
            r#"{{"event":"started","pr":7,"headSha":"a","ts":"{}"}}"#,
            stamp(30)
        ),
        format!(
            r#"{{"event":"started","pr":8,"headSha":"a","ts":"{}"}}"#,
            stamp(26)
        ),
        format!(
            r#"{{"event":"started","pr":9,"headSha":"a","ts":"{}"}}"#,
            stamp(24 * 20)
        ),
    ];
    std::fs::write(dir.join("inflight-runs.jsonl"), lines.join("\n") + "\n").unwrap();
    let recent = tribunal(&["lost-runs", "--recent", "7"], &dir);
    let out = stdout(&recent);
    assert!(recent.status.success());
    assert!(
        out.starts_with("WARNING: 2 run(s) in the last 7 day(s)"),
        "{out}"
    );
    assert!(out.contains("(PR 7, 8)"), "{out}");
    assert_eq!(out.lines().count(), 1);
    let wider = stdout(&tribunal(&["lost-runs", "--recent", "30"], &dir));
    assert!(wider.contains("(PR 7, 8, 9)"), "{wider}");
    let full = stdout(&tribunal(&["lost-runs"], &dir));
    assert!(full.contains("LOST RUNS (3):"), "{full}");

    let done = format!(
        r#"{{"event":"completed","pr":8,"headSha":"a","ts":"{}"}}"#,
        stamp(1)
    );
    std::fs::write(
        dir.join("inflight-runs.jsonl"),
        lines.join("\n") + "\n" + &done + "\n",
    )
    .unwrap();
    // A run completed after the losses: the persist step works.
    assert_eq!(stdout(&tribunal(&["lost-runs", "--recent", "7"], &dir)), "");
}

#[test]
fn harness_version_hashes_the_plugin() {
    let dir = scratch("harness");
    let short = tribunal(&["harness-version", "--short"], &dir);
    assert!(short.status.success());
    let id = stdout(&short);
    let hex = id.trim().strip_prefix("pr-harness-").expect(&id);
    assert!(
        hex.len() == 12 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "{id}"
    );
    let manifest = stdout(&tribunal(&["harness-version", "--manifest"], &dir));
    assert!(manifest.starts_with(&id));
    assert!(
        manifest.contains("  skills/review-pr/SKILL.md\n"),
        "{manifest}"
    );
}

#[test]
fn unknown_arguments_are_refused() {
    let dir = scratch("usage");
    for args in [
        &["lost-runs", "--nope"][..],
        &["harness-version", "--nope"][..],
    ] {
        let o = tribunal(args, &dir);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
    }
}

fn templates() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../templates")
}

fn json_line(o: &Output) -> serde_json::Value {
    serde_json::from_str(stdout(o).trim()).expect("one JSON line")
}

// templates/check.sh runs both loaders on the shipped templates, with the
// base ref passed in so nothing is fetched.
#[test]
fn load_profile_reads_a_template_without_the_network() {
    let dir = scratch("load-profile");
    let t = templates();
    let profile = t.join("review-profile.backend.json");
    let rules = t.join("review-rules.md");
    let o = tribunal(
        &[
            "load-profile",
            "--pr",
            "1",
            "--repo",
            "your-org/your-api",
            "--ref",
            "main",
            "--profile",
            profile.to_str().unwrap(),
            "--rules",
            rules.to_str().unwrap(),
        ],
        &dir,
    );
    let v = json_line(&o);
    assert_eq!(v["ok"], true, "{v}");
    assert!(o.status.success());

    let missing = dir.join("no-such-profile.json");
    let o = tribunal(
        &[
            "load-profile",
            "--repo",
            "your-org/your-api",
            "--ref",
            "main",
            "--profile",
            missing.to_str().unwrap(),
        ],
        &dir,
    );
    assert_eq!(json_line(&o)["ok"], false);
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn assemble_methodology_fills_the_template_slots() {
    let dir = scratch("assemble");
    let out = dir.join("methodology.md");
    let rules = templates().join("review-rules.md");
    let o = tribunal(
        &[
            "assemble-methodology",
            "--kind",
            "backend",
            "--rules",
            rules.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ],
        &dir,
    );
    let v = json_line(&o);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["unknown"], serde_json::json!([]));
    assert!(!v["filled"].as_array().unwrap().is_empty());
    assert!(std::fs::read_to_string(&out).unwrap().len() > 1000);

    let o = tribunal(&["assemble-methodology", "--kind", "backend"], &dir);
    assert_eq!(json_line(&o)["ok"], false);
    assert_eq!(o.status.code(), Some(1));
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn ledger_row(engine: Option<&str>, schema: Option<u64>, rerun: serde_json::Value) -> String {
    let mut r = serde_json::json!({
        "ts": "2026-10-30T10:00:00Z", "repo": "o/r", "pr": 1, "headSha": "a",
        "candidates": 2, "paneled": 1, "rerunOf": rerun,
        "findings": [{"title": "t", "file": "a.ts", "posted": false, "panelRan": true,
            "survived": false, "refuteFraction": 0.9, "threshold": 0.45,
            "votes": [{"source": "claude", "verdict": "REFUTED", "killReason": "not-triggered"}]}],
    });
    if let Some(e) = engine {
        r["engine"] = serde_json::json!(e);
        r["pluginVersion"] = serde_json::json!("115.0.0");
    }
    if let Some(v) = schema {
        r["schemaVersion"] = serde_json::json!(v);
    }
    r.to_string()
}

#[test]
fn metrics_reads_the_ledger_in_the_data_dir() {
    let dir = scratch("metrics");
    let lines = [
        ledger_row(Some("rust"), Some(3), serde_json::Value::Null),
        ledger_row(None, Some(3), serde_json::Value::Null),
        ledger_row(Some("rust"), Some(3), serde_json::json!(1)),
        ledger_row(None, None, serde_json::Value::Null),
        "not json".to_owned(),
    ];
    std::fs::write(dir.join("metrics.jsonl"), lines.join("\n") + "\n").unwrap();
    let o = tribunal(&["metrics"], &dir);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.starts_with(
        "tribunal metrics: 2 rows aggregated, 1 legacy skipped, 1 re-runs excluded (of 4 rows read, plus 1 unreadable lines)\n"
    ), "{out}");
    assert!(
        out.contains("\nrust, v91+, schemaVersion 3: 1 rows"),
        "{out}"
    );
    assert!(
        out.contains("\nworkflow, pre-v91, schemaVersion 3: 1 rows"),
        "{out}"
    );
    assert!(out.contains("not-triggered 1 of 1 (100.0%)"), "{out}");

    let j = json_line(&tribunal(&["metrics", "--json", "--repo", "o/r"], &dir));
    assert_eq!(j["rows"]["aggregated"], 2);
    assert_eq!(j["groups"][0]["killRate"]["rate"], 1.0);
    let none = json_line(&tribunal(
        &["metrics", "--json", "--since", "2026-11-01"],
        &dir,
    ));
    assert_eq!(none["rows"]["filteredOut"], 4);
}

#[test]
fn audit_with_nothing_posted_needs_no_gh() {
    let dir = scratch("audit");
    std::fs::write(
        dir.join("metrics.jsonl"),
        ledger_row(Some("rust"), Some(3), serde_json::Value::Null) + "\n",
    )
    .unwrap();
    let o = tribunal(&["audit", "--limit", "5"], &dir);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("These are proxies, not ground truth"), "{out}");
    assert!(out.contains("0 queried (--limit 5)"), "{out}");
    let j = json_line(&tribunal(&["audit", "--json"], &dir));
    assert_eq!(j["prs"]["limit"], 20);
}

#[test]
fn an_unreadable_ledger_fails_with_a_tribunal_line() {
    let dir = scratch("no-ledger");
    for sub in ["metrics", "audit"] {
        let o = tribunal(&[sub], &dir);
        assert_eq!(o.status.code(), Some(1), "{sub}");
        assert!(
            stderr(&o).starts_with("tribunal: cannot read "),
            "{sub}: {}",
            stderr(&o)
        );
    }
    for args in [
        &["metrics", "--limit", "3"][..],
        &["audit", "--limit", "0"][..],
        &["audit", "--repo"][..],
    ] {
        assert_eq!(tribunal(args, &dir).status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn compare_reads_two_arm_files() {
    let dir = scratch("compare");
    let arm = |name: &str, found: bool| {
        let rows: Vec<_> = (0..6)
            .flat_map(|u| {
                (0..8).map(move |r| {
                    serde_json::json!({"unit": format!("p{u}"), "run": r, "item": "d", "found": found})
                })
            })
            .collect();
        serde_json::json!({"arm": name, "detections": rows}).to_string()
    };
    let (a, b) = (dir.join("a.json"), dir.join("b.json"));
    std::fs::write(&a, arm("base", false)).unwrap();
    std::fs::write(&b, arm("edit", true)).unwrap();
    let (a, b) = (a.to_str().unwrap(), b.to_str().unwrap());

    let o = tribunal(
        &["compare", "--a", a, "--b", b, "--json", "--seed", "7"],
        &dir,
    );
    assert!(o.status.success(), "{}", stderr(&o));
    let j = json_line(&o);
    assert_eq!(j["recall"]["verdict"], "b-higher");
    assert_eq!(j["seed"], 7);
    assert_eq!(j["cost"]["recorded"], false);

    let text = stdout(&tribunal(&["compare", "--a", a, "--b", b], &dir));
    assert!(
        text.starts_with("tribunal compare: A (base) against B (edit)\n"),
        "{text}"
    );
    assert!(text.contains("B minus A  +100.0 points"), "{text}");
    assert!(text.contains("verdict: B found more"), "{text}");

    let missing = tribunal(&["compare", "--a", a], &dir);
    assert_eq!(missing.status.code(), Some(2));
    let nofile = tribunal(&["compare", "--a", a, "--b", "no-such.json"], &dir);
    assert_eq!(nofile.status.code(), Some(1));
    assert!(
        stderr(&nofile).contains("no-such.json"),
        "{}",
        stderr(&nofile)
    );
    for bad in [["--resamples", "x"], ["--seed", "-1"]] {
        let o = tribunal(&["compare", "--a", a, "--b", b, bad[0], bad[1]], &dir);
        assert_eq!(o.status.code(), Some(2), "{bad:?}");
    }
}
