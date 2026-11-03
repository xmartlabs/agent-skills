//! The read-only subcommands the skill runs through `bin/tribunal-engine`
//! before and around a review (`lost-runs`, `harness-version`), and the two
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
