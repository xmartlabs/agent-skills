//! The `started` marker against the workflow's own `MARKER_BASH`, run with a
//! fake `gh` that answers the base branch.

mod common;

use std::process::Command;

use common::{span, Span};
use serde_json::{json, Value};
use tribunal_context::inflight::append_started;

fn marker_bash(pinned: &str, deploy: &Value, batch: bool) -> String {
    span(
        &Span {
            start: "const RESOLVE_META_BASH = `",
            end: "STARTJSON\nfi`;",
            deps: &["PROFILE-LOOKUP"],
            pre: &[],
            fns: &[],
            ret: "RESOLVE_META_BASH + '\\n' + MARKER_BASH",
        },
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

#[test]
fn the_started_marker_is_the_workflows() {
    let root = std::env::temp_dir().join(format!("tribunal-marker-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
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
    for (n, (sha, deploy, base, batch)) in cases.iter().enumerate() {
        let gh = bin.join("gh");
        std::fs::write(&gh, format!("#!/bin/sh\necho {base}\n")).unwrap();
        std::fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let js_dir = root.join(format!("js-{n}"));
        let rs_dir = root.join(format!("rs-{n}"));
        let out = Command::new("bash")
            .arg("-c")
            .arg(marker_bash(sha, deploy, *batch))
            .env("PATH", &path)
            .env("TRIBUNAL_DATA_DIR", &js_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
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
            lines(&rs_dir.join("inflight-runs.jsonl")),
            lines(&js_dir.join("inflight-runs.jsonl")),
            "case {n}"
        );
        assert_eq!(rs_dir.is_dir(), js_dir.is_dir());
    }
    let _ = std::fs::remove_dir_all(&root);
}
