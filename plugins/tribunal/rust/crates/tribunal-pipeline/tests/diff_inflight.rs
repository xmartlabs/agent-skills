//! The `started` marker against the workflow's own `MARKER_BASH`, run with a
//! fake `gh` that answers the base branch.
//!
//! The bash's side is read from `tests/goldens/inflight-marker.json`: the
//! workflow is being retired, so a run starts only the port. The file keeps
//! each case's input, the bash the span oracle cut out for it, and a SHA-256
//! of the workflow code that bash came from and of the span oracle's own
//! files, so editing either without a regeneration fails here. A source that
//! is gone from disk is retired and is not checked; the golden is final.
//! Regenerate after changing them:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-pipeline --test diff_inflight`.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{span, Span};
use serde_json::{json, Value};
use tribunal_context::harness_version::sha256_hex;
use tribunal_context::inflight::append_started;

const MARKER: Span<'static> = Span {
    start: "const RESOLVE_META_BASH = `",
    end: "STARTJSON\nfi`;",
    deps: &["PROFILE-LOOKUP"],
    pre: &[],
    fns: &[],
    ret: "RESOLVE_META_BASH + '\\n' + MARKER_BASH",
};

fn marker_bash(pinned: &str, deploy: &Value, batch: bool) -> String {
    span(
        &MARKER,
        &[json!({"PY_UTF8_BASH": "", "pinnedSha": pinned, "prNumber": 77, "repo": "acme/widgets", "PROFILE": {"deployBranches": deploy}, "forceBatchPanels": batch})],
    )
    .remove(0)
    .as_str()
    .unwrap()
    .to_owned()
}

fn lines(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| {
            let mut v: Value = serde_json::from_str(l).unwrap();
            v["ts"] = json!("T");
            v["epoch"] = json!(0);
            v
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Golden: the bash's side recorded once, so a run starts only the port.

const GOLDEN: &str = "inflight-marker.json";
const REGENERATE: &str = "regenerate with `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-pipeline --test diff_inflight`";
const WORKFLOW: &str = "workflow-src/tribunal.js";
/// The span oracle's own files, by path in the plugin.
const ORACLE_FILES: &[&str] = &[
    "rust/oracle/span-oracle.mjs",
    "skills/review-pr/eval/extract-pure-region.mjs",
];

fn regenerating() -> bool {
    std::env::var("TRIBUNAL_REGEN_GOLDENS").is_ok_and(|v| v == "1")
}

fn plugin_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

/// The workflow code the span oracle runs for `s`, cut out the way it cuts it;
/// `None` when an anchor is no longer in `src`.
fn span_source(src: &str, s: &Span) -> Option<String> {
    let cut = |start: &str, end: &str| -> Option<String> {
        let a = src.find(start)?;
        let e = a + src[a..].find(end)?;
        Some(src[a..e + end.len()].to_owned())
    };
    let mut parts = Vec::new();
    for d in s.deps {
        let (begin, end) = (
            format!("// ===== BEGIN {d} ====="),
            format!("// ===== END {d} ====="),
        );
        let mut from = 0;
        while let Some(b) = src[from..].find(&begin).map(|b| b + from) {
            let e = b + src[b..].find(&end)?;
            parts.push(src[b + begin.len()..e].to_owned());
            from = e + end.len();
        }
    }
    for (a, b) in s.pre {
        parts.push(cut(a, b)?);
    }
    parts.push(cut(s.start, s.end)?);
    Some(parts.join("\n"))
}

/// The SHA-256 of what the recorded bash came from, by path in the plugin
/// under `root`: the span's code for the workflow, the whole file otherwise.
/// A file that is gone maps to `None`: that oracle is retired.
fn sources(root: &Path) -> BTreeMap<String, Option<String>> {
    let mut out = BTreeMap::new();
    let workflow = std::fs::read_to_string(root.join(WORKFLOW)).ok();
    out.insert(
        WORKFLOW.to_owned(),
        workflow.map(|w| sha256_hex(span_source(&w, &MARKER).unwrap_or_default().as_bytes())),
    );
    for f in ORACLE_FILES {
        let sha = std::fs::read(root.join(f)).ok().map(|b| sha256_hex(&b));
        out.insert((*f).to_owned(), sha);
    }
    out
}

/// The sources still on disk that differ from `recorded`; a retired one is
/// skipped, since nothing can regenerate from it.
fn stale(recorded: &Value, now: &BTreeMap<String, Option<String>>) -> Vec<String> {
    let mut keys: Vec<&String> = recorded
        .as_object()
        .map(|o| o.keys().collect())
        .unwrap_or_default();
    keys.sort();
    if keys != now.keys().collect::<Vec<_>>() {
        return vec!["the list of sources".to_owned()];
    }
    now.iter()
        .filter(|(k, sha)| {
            sha.as_ref()
                .is_some_and(|s| recorded[k.as_str()] != json!(s))
        })
        .map(|(k, _)| k.clone())
        .collect()
}

/// The bash's side of each case: run by `record` and written to the golden
/// when regenerating, else read back after checking the sources and inputs.
fn recorded(inputs: &[Value], record: impl FnOnce() -> Vec<Value>) -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens")
        .join(GOLDEN);
    if regenerating() {
        let with: BTreeMap<String, String> = sources(&plugin_root())
            .into_iter()
            .map(|(k, sha)| {
                let sha = sha.unwrap_or_else(|| panic!("{k} is gone: nothing to regenerate from"));
                (k, sha)
            })
            .collect();
        let js = record();
        let cases: Vec<Value> = inputs
            .iter()
            .zip(&js)
            .map(|(input, js)| json!({"input": input, "js": js}))
            .collect();
        let text =
            serde_json::to_string_pretty(&json!({"recordedWith": with, "cases": cases})).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text + "\n").unwrap();
        return js;
    }
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; {REGENERATE}", path.display()));
    let golden: Value = serde_json::from_str(&text).unwrap();
    let stale = stale(&golden["recordedWith"], &sources(&plugin_root()));
    assert!(
        stale.is_empty(),
        "{} changed since {} was recorded; {REGENERATE}",
        stale.join(", "),
        path.display()
    );
    let cases = golden["cases"].as_array().unwrap();
    assert_eq!(cases.len(), inputs.len(), "the cases changed; {REGENERATE}");
    cases
        .iter()
        .zip(inputs)
        .map(|(c, input)| {
            assert_eq!(&c["input"], input, "the cases changed; {REGENERATE}");
            c["js"].clone()
        })
        .collect()
}

#[test]
fn a_retired_source_is_not_checked_but_a_changed_one_is() {
    let root = std::env::temp_dir().join(format!("tribunal-marker-retired-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let gone = sources(&root);
    assert!(gone.values().all(Option::is_none));
    let recorded = json!(gone
        .keys()
        .map(|k| (k, "recorded"))
        .collect::<BTreeMap<_, _>>());
    assert!(stale(&recorded, &gone).is_empty());
    for (i, f) in [WORKFLOW, ORACLE_FILES[0]].into_iter().enumerate() {
        let at = root.join(f);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(&at, "edited").unwrap();
        assert_eq!(stale(&recorded, &sources(&root)).len(), i + 1, "{f}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_started_marker_is_the_workflows() {
    let root = std::env::temp_dir().join(format!("tribunal-marker-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let cases = [
        (
            "abc123",
            json!(["staging", "production"]),
            "development",
            false,
        ),
        ("abc123", json!(["staging", "production"]), "staging", true),
        ("abc123", json!([]), "main", true),
        ("abc123", json!(["release-*"]), "release-7", false),
    ];
    let inputs: Vec<Value> = cases
        .iter()
        .map(|(sha, deploy, base, batch)| {
            json!({"sha": sha, "deploy": deploy, "base": base, "batch": batch})
        })
        .collect();
    let js = recorded(&inputs, || {
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        cases
            .iter()
            .enumerate()
            .map(|(n, (sha, deploy, base, batch))| {
                let gh = bin.join("gh");
                std::fs::write(&gh, format!("#!/bin/sh\necho {base}\n")).unwrap();
                std::fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
                let js_dir = root.join(format!("js-{n}"));
                let bash = marker_bash(sha, deploy, *batch);
                let out = Command::new("bash")
                    .arg("-c")
                    .arg(&bash)
                    .env("PATH", &path)
                    .env("TRIBUNAL_DATA_DIR", &js_dir)
                    .output()
                    .unwrap();
                assert!(
                    out.status.success(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
                json!({"bash": bash, "lines": lines(&js_dir.join("inflight-runs.jsonl")), "dirExists": js_dir.is_dir()})
            })
            .collect()
    });
    for (n, ((sha, deploy, base, batch), js)) in cases.iter().zip(&js).enumerate() {
        let rs_dir = root.join(format!("rs-{n}"));
        let branches: Vec<String> = deploy
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b.as_str().unwrap().to_owned())
            .collect();
        append_started(
            &rs_dir,
            77,
            "acme/widgets",
            Some(sha),
            *batch,
            base,
            &branches,
            1,
        );
        assert_eq!(
            json!(lines(&rs_dir.join("inflight-runs.jsonl"))),
            js["lines"],
            "case {n}"
        );
        assert_eq!(json!(rs_dir.is_dir()), js["dirExists"], "case {n}");
    }
    let _ = std::fs::remove_dir_all(&root);
}
