//! `tribunal audit` against a ledger built here and `gh` answers recorded
//! here, served by the compiled `tribunal-test-gh`.

mod common;

use std::ffi::OsString;

use serde_json::{json, Value};
use tribunal_context::exec::Tools;
use tribunal_context::metrics_report::RowFilter;
use tribunal_context::outcome_audit::{audit, render, AuditArgs};

const VIEW: &str = "state,mergedAt,headRefOid,reviews";

fn finding(title: &str, file: &str, line: u64, extra: Value) -> Value {
    let mut f = json!({
        "title": title, "file": file, "line": line, "posted": true, "blocking": false,
        "panelRan": false, "survived": true, "hadExternalVoter": false,
        "debate": {"fired": false, "outcome": null},
    });
    for (k, v) in extra.as_object().unwrap() {
        f[k] = v.clone();
    }
    f
}

fn row(pr: u64, sha: &str, ts: &str, engine: Option<&str>, findings: Vec<Value>) -> Value {
    let mut r = json!({
        "schemaVersion": 3, "ts": ts, "repo": "o/r", "pr": pr, "headSha": sha,
        "mode": "workflow", "posted": true, "rerunOf": null, "findings": findings,
    });
    if let Some(e) = engine {
        r["engine"] = json!(e);
    }
    r
}

fn ledger() -> Vec<Value> {
    let f1 = finding(
        "Lost write",
        "src/a.ts",
        12,
        json!({"blocking": true, "panelRan": true, "hadExternalVoter": true,
            "debate": {"fired": true, "outcome": "flipped"}}),
    );
    let f2 = finding(
        "Retry drops the job",
        "src/b.ts",
        50,
        json!({"blocking": true, "panelRan": true}),
    );
    let f3 = finding("Naming", "src/c.ts", 40, json!({}));
    let k1 = finding(
        "Killed claim",
        "src/d.ts",
        3,
        json!({"posted": false, "panelRan": true, "survived": false}),
    );
    let mut rerun = row(
        14,
        "ggg",
        "2026-10-27T10:00:00Z",
        Some("rust"),
        vec![finding("Rewritten", "y.ts", 1, json!({}))],
    );
    rerun["rerunOf"] = json!(1);
    let mut silent_rerun = row(10, "aaa", "2026-10-21T10:00:00Z", Some("rust"), vec![]);
    silent_rerun["rerunOf"] = json!(1);
    let mut no_sha = row(
        15,
        "",
        "2026-10-28T10:00:00Z",
        Some("rust"),
        vec![finding("No head", "z.ts", 1, json!({}))],
    );
    no_sha.as_object_mut().unwrap().remove("headSha");
    vec![
        row(
            10,
            "aaa",
            "2026-10-20T10:00:00Z",
            Some("rust"),
            vec![f1, f2.clone(), f3, k1],
        ),
        row(10, "bbb", "2026-10-22T10:00:00Z", Some("rust"), vec![f2]),
        row(
            11,
            "eee",
            "2026-10-25T10:00:00Z",
            None,
            vec![finding("Dismissed one", "x.ts", 5, json!({}))],
        ),
        row(
            12,
            "fff",
            "2026-10-26T10:00:00Z",
            Some("rust"),
            vec![finding("Gone", "w.ts", 1, json!({"blocking": true}))],
        ),
        row(
            13,
            "iii",
            "2026-10-01T10:00:00Z",
            Some("rust"),
            vec![finding("Old", "v.ts", 1, json!({}))],
        ),
        rerun,
        silent_rerun,
        no_sha,
        json!({"ts": "2026-07-01", "repo": "o/r", "pr": 9, "findings": [{"posted": true, "file": "a.ts"}]}),
    ]
}

fn compare(files: Value) -> String {
    json!({"status": "ahead", "files": files}).to_string()
}

fn recorded() -> Vec<Value> {
    let view = |n: &str, state: &str, merged: Value, head: &str, reviews: Value| {
        common::call(
            &["pr", "view", n, "--repo", "o/r", "--json", VIEW],
            &json!({"state": state, "mergedAt": merged, "headRefOid": head, "reviews": reviews})
                .to_string(),
            0,
        )
    };
    vec![
        view(
            "10",
            "MERGED",
            json!("2026-10-23T09:00:00Z"),
            "ccc",
            json!([{"state": "DISMISSED", "commit": {"oid": "zzz"}}]),
        ),
        common::call(
            &["api", "repos/o/r/compare/aaa...ccc"],
            &compare(json!([
                {"filename": "src/a.ts", "patch": "@@ -10,5 +10,6 @@\n ctx"},
                {"filename": "src/c.ts", "patch": "@@ -1,2 +1,3 @@\n ctx"},
                {"filename": "src/d.ts", "patch": "@@ -1,9 +1,1 @@"},
            ])),
            0,
        ),
        common::call(
            &["api", "repos/o/r/compare/bbb...ccc"],
            &compare(json!([])),
            0,
        ),
        view(
            "11",
            "OPEN",
            Value::Null,
            "eee",
            json!([{"state": "DISMISSED", "commit": {"oid": "eee"}}]),
        ),
        json!({"args": ["pr", "view", "12", "--repo", "o/r", "--json", VIEW],
            "stdout": "", "stderr": "GraphQL: Could not resolve to a PullRequest\n", "code": 1}),
        view("14", "CLOSED", Value::Null, "hhh", json!([])),
        common::call(
            &["api", "repos/o/r/compare/ggg...hhh"],
            &json!({"status": "diverged", "files": [{"filename": "y.ts"}]}).to_string(),
            0,
        ),
    ]
}

fn tools(dir: &std::path::Path) -> Tools {
    let rec = common::recordings(dir, recorded());
    Tools {
        gh: env!("CARGO_BIN_EXE_tribunal-test-gh").to_owned(),
        git: "git".to_owned(),
        env: vec![
            (OsString::from("GH_RECORDINGS"), rec.into_os_string()),
            (
                OsString::from("GH_CALL_LOG"),
                dir.join("calls.log").into_os_string(),
            ),
        ],
    }
}

fn bucket<'a>(g: &'a Value, name: &str) -> &'a Value {
    g["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["bucket"] == name)
        .unwrap_or_else(|| panic!("no bucket {name} in {g}"))
}

#[tokio::test]
async fn proxies_per_mechanism_from_recorded_answers() {
    let dir = common::scratch("outcome-audit");
    let t = tools(&dir);
    let rows = ledger();
    let args = AuditArgs {
        filter: RowFilter::default(),
        limit: 4,
    };
    let r = audit(&t, &rows, 0, &args).await;

    let calls = std::fs::read_to_string(dir.join("calls.log")).unwrap();
    let mut calls: Vec<&str> = calls.lines().collect();
    calls.sort_unstable();
    assert_eq!(
        calls,
        [
            r#"["api","repos/o/r/compare/aaa...ccc"]"#,
            r#"["api","repos/o/r/compare/bbb...ccc"]"#,
            r#"["api","repos/o/r/compare/ggg...hhh"]"#,
            r#"["pr","view","10","--repo","o/r","--json","state,mergedAt,headRefOid,reviews"]"#,
            r#"["pr","view","11","--repo","o/r","--json","state,mergedAt,headRefOid,reviews"]"#,
            r#"["pr","view","12","--repo","o/r","--json","state,mergedAt,headRefOid,reviews"]"#,
            r#"["pr","view","14","--repo","o/r","--json","state,mergedAt,headRefOid,reviews"]"#,
        ]
    );

    assert_eq!(r["postedReruns"], 1);
    assert_eq!(
        r["prs"],
        json!({
            "withPostedFindings": 5, "queried": 4, "limit": 4,
            "skippedOverLimit": ["o/r#13"], "rowsWithoutIdentity": 1,
            "ghCalls": 7, "ghFailures": 1,
        })
    );
    assert_eq!(r["rows"]["legacySkipped"], 1);
    assert_eq!(r["rows"]["rerunsExcluded"], 2);
    let order: Vec<u64> = r["perPr"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["pr"].as_u64().unwrap())
        .collect();
    assert_eq!(order, [14, 12, 11, 10]);
    let pr10 = &r["perPr"][3];
    assert_eq!(pr10["state"], "MERGED");
    assert_eq!(pr10["reviews"], 2);
    assert_eq!(
        (&pr10["posted"], &pr10["fixed"], &pr10["mergedUntouched"]),
        (&json!(3), &json!(2), &json!(1))
    );
    assert!(r["perPr"][0]["errors"][0]
        .as_str()
        .unwrap()
        .contains("rewritten"));
    assert!(r["perPr"][1]["errors"][0]
        .as_str()
        .unwrap()
        .contains("Could not resolve"));

    let groups = r["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2);
    let rust = &groups[0];
    assert_eq!(rust["engine"], "rust");
    assert_eq!(rust["posted"], 5);
    assert_eq!(rust["duplicatesMerged"], 1);
    assert_eq!(
        bucket(rust, "all posted"),
        &json!({
            "bucket": "all posted", "findings": 5, "fixed": 2, "dismissed": 0,
            "mergedUntouched": 1, "openUntouched": 0, "closedUnmerged": 0, "unknown": 2,
            "fixedLineInHunk": 1, "fixedLineKnown": 2, "decided": 3,
            "fixedShareOfDecided": 0.6667,
        })
    );
    assert_eq!(bucket(rust, "blocking")["findings"], 3);
    assert_eq!(bucket(rust, "not blocking")["fixed"], 1);
    assert_eq!(bucket(rust, "not paneled")["findings"], 3);
    assert_eq!(bucket(rust, "paneled, external voter")["fixed"], 1);
    assert_eq!(bucket(rust, "paneled, Claude only")["mergedUntouched"], 1);
    assert_eq!(bucket(rust, "debate flipped")["fixedShareOfDecided"], 1.0);
    assert!(rust["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .all(|b| b["bucket"] != "debate held"));
    assert_eq!(
        rust["killed"],
        json!({"findings": 1, "fileTouchedAfter": 1, "unknown": 0, "untouched": 0})
    );
    let workflow = &groups[1];
    assert_eq!(workflow["engine"], "workflow");
    assert_eq!(bucket(workflow, "all posted")["dismissed"], 1);
    assert_eq!(bucket(workflow, "all posted")["fixedShareOfDecided"], 0.0);

    let text = render(&r);
    assert!(
        text.contains("These are proxies, not ground truth"),
        "{text}"
    );
    assert!(
        text.contains(
            "4 queried (--limit 4), 1 not queried over the limit (older reviews): o/r#13."
        ),
        "{text}"
    );
    assert!(
        text.contains("  o/r#10 MERGED 2026-10-23T09:00:00Z: 3 posted: 2 fixed, 1 merged untouched (2 reviews)\n"),
        "{text}"
    );
    assert!(
        text.contains("killed findings whose file changed after the review: 1 of 1 (100.0%)"),
        "{text}"
    );
}

#[tokio::test]
async fn a_ledger_with_nothing_posted_asks_gh_nothing() {
    let dir = common::scratch("outcome-audit-empty");
    let t = tools(&dir);
    let rows = vec![row(1, "a", "2026-10-20T10:00:00Z", Some("rust"), vec![])];
    let r = audit(&t, &rows, 0, &AuditArgs::default()).await;
    assert!(!dir.join("calls.log").exists());
    assert_eq!(r["prs"]["withPostedFindings"], 0);
    assert!(render(&r).contains("No PR with posted findings matches.\n"));
}
