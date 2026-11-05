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
        ..AuditArgs::default()
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

fn sha(c: char) -> String {
    format!("{c}{}", "0".repeat(39))
}

fn commit_answer(sha: &str, files: Value) -> Value {
    common::call(
        &["api", &format!("repos/o/r/commits/{sha}")],
        &json!({"sha": sha, "files": files}).to_string(),
        0,
    )
}

fn range(commits: Value, files: Value) -> String {
    json!({"status": "ahead", "commits": commits, "files": files}).to_string()
}

fn candidate_rows() -> Vec<Value> {
    let plain = |t: &str, file: &str, line: u64| finding(t, file, line, json!({}));
    vec![
        row(
            20,
            "aaa",
            "2026-10-20T10:00:00Z",
            Some("rust"),
            vec![
                plain("Lost write", "src/a.ts", 12),
                plain("Stale read", "src/b.ts", 50),
                plain("Untouched", "src/e.ts", 3),
            ],
        ),
        row(
            21,
            "ddd",
            "2026-10-19T10:00:00Z",
            Some("rust"),
            vec![plain("After a merge", "src/m.ts", 5)],
        ),
        row(
            22,
            "fff",
            "2026-10-18T10:00:00Z",
            Some("rust"),
            vec![plain("After a failure", "src/f.ts", 7)],
        ),
        row(
            23,
            "jjj",
            "2026-10-17T10:00:00Z",
            Some("rust"),
            vec![plain("Unrelated merge", "src/g.ts", 5)],
        ),
    ]
}

fn candidate_recordings() -> Vec<Value> {
    let view = |n: &str, head: &str| {
        common::call(
            &["pr", "view", n, "--repo", "o/r", "--json", VIEW],
            &json!({"state": "MERGED", "mergedAt": "2026-10-23T09:00:00Z",
                "headRefOid": head, "reviews": []})
            .to_string(),
            0,
        )
    };
    let (c1, c2, c3, c4, c5, c6) = (sha('1'), sha('2'), sha('3'), sha('4'), sha('5'), sha('6'));
    let (merge, other_merge, c7) = (sha('9'), sha('8'), sha('7'));
    let one = json!([{"sha": "p"}]);
    let two = json!([{"sha": "p"}, {"sha": "q"}]);
    vec![
        view("20", "ccc"),
        common::call(
            &["api", "repos/o/r/compare/aaa...ccc"],
            &range(
                json!([
                    {"sha": c1, "parents": one},
                    {"sha": c2, "parents": one},
                    {"sha": c3, "parents": one},
                ]),
                json!([
                    {"filename": "src/a.ts", "patch": "@@ -10,5 +10,6 @@"},
                    {"filename": "src/b.ts", "patch": "@@ -30,2 +30,2 @@"},
                ]),
            ),
            0,
        ),
        commit_answer(
            &c1,
            json!([{"filename": "src/b.ts", "patch": "@@ -30,2 +30,2 @@"}]),
        ),
        commit_answer(
            &c2,
            json!([{"filename": "src/a.ts", "patch": "@@ -10,5 +10,6 @@"}]),
        ),
        view("21", "eee"),
        common::call(
            &["api", "repos/o/r/compare/ddd...eee"],
            &range(
                json!([{"sha": merge, "parents": two}, {"sha": c4, "parents": one}]),
                json!([{"filename": "src/m.ts", "patch": "@@ -4,3 +4,3 @@"}]),
            ),
            0,
        ),
        commit_answer(
            &merge,
            json!([{"filename": "src/m.ts", "patch": "@@ -1,2 +1,9 @@"}]),
        ),
        commit_answer(
            &c4,
            json!([{"filename": "src/m.ts", "patch": "@@ -4,3 +4,3 @@"}]),
        ),
        view("23", "kkk"),
        common::call(
            &["api", "repos/o/r/compare/jjj...kkk"],
            &range(
                json!([{"sha": other_merge, "parents": two}, {"sha": c7, "parents": one}]),
                json!([{"filename": "src/g.ts", "patch": "@@ -4,3 +4,3 @@"}]),
            ),
            0,
        ),
        commit_answer(
            &other_merge,
            json!([{"filename": "src/x.ts", "patch": "@@ -1 +1 @@"}]),
        ),
        commit_answer(
            &c7,
            json!([{"filename": "src/g.ts", "patch": "@@ -4,3 +4,3 @@"}]),
        ),
        view("22", "ggg"),
        common::call(
            &["api", "repos/o/r/compare/fff...ggg"],
            &range(
                json!([{"sha": c5, "parents": one}, {"sha": c6, "parents": one}]),
                json!([{"filename": "src/f.ts", "patch": "@@ -6,3 +6,3 @@"}]),
            ),
            0,
        ),
        json!({"args": ["api", format!("repos/o/r/commits/{c5}")],
            "stdout": "", "stderr": "HTTP 502\n", "code": 1}),
        commit_answer(
            &c6,
            json!([{"filename": "src/f.ts", "patch": "@@ -6,3 +6,3 @@"}]),
        ),
    ]
}

fn candidate_tools(dir: &std::path::Path) -> Tools {
    let rec = common::recordings(dir, candidate_recordings());
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

fn lead<'a>(r: &'a Value, title: &str) -> &'a Value {
    r["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["title"] == title)
        .unwrap_or_else(|| panic!("no candidate {title} in {r}"))
}

#[tokio::test]
async fn candidates_name_the_first_commit_that_touched_the_file() {
    let dir = common::scratch("outcome-audit-candidates");
    let t = candidate_tools(&dir);
    let args = AuditArgs {
        candidates: true,
        ..AuditArgs::default()
    };
    let r = audit(&t, &candidate_rows(), 0, &args).await;

    let calls = std::fs::read_to_string(dir.join("calls.log")).unwrap();
    let calls: Vec<&str> = calls.lines().collect();
    let asked = |what: &str| calls.iter().any(|c| c.contains(what));
    for c in ['1', '2', '9', '4', '5', '6', '8', '7'] {
        assert!(asked(&format!("commits/{}", sha(c))), "{calls:?}");
    }
    assert!(
        !asked(&sha('3')),
        "a commit past the last wanted file: {calls:?}"
    );
    assert_eq!(r["prs"]["ghCalls"], 16);

    let a = lead(&r, "Lost write");
    assert_eq!(a["outcome"], "fixed");
    assert_eq!(a["lineInChangedHunk"], true);
    assert_eq!(
        a["firstTouch"],
        json!({"commit": sha('2'), "hunkCoversLine": true, "exact": true})
    );
    assert_eq!(
        (&a["repo"], &a["pr"], &a["file"], &a["line"]),
        (&json!("o/r"), &json!(20), &json!("src/a.ts"), &json!(12))
    );

    let b = lead(&r, "Stale read");
    assert_eq!(
        b["firstTouch"],
        json!({"commit": sha('1'), "hunkCoversLine": false, "exact": true})
    );

    let e = lead(&r, "Untouched");
    assert_eq!(e["outcome"], "mergedUntouched");
    assert_eq!(e["firstTouch"], Value::Null);

    let m = lead(&r, "After a merge");
    assert_eq!(
        m["firstTouch"],
        json!({"commit": sha('4'), "hunkCoversLine": null, "exact": false})
    );
    let g = lead(&r, "Unrelated merge");
    assert_eq!(
        g["firstTouch"],
        json!({"commit": sha('7'), "hunkCoversLine": true, "exact": true})
    );
    let f = lead(&r, "After a failure");
    assert_eq!(
        f["firstTouch"],
        json!({"commit": sha('6'), "hunkCoversLine": null, "exact": false})
    );

    let text = render(&r);
    assert!(
        text.contains("A candidate is a lead, not a label"),
        "{text}"
    );
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle} in {text}"))
    };
    assert!(
        text.contains(r#"o/r#20 src/a.ts:12 [not blocking] "Lost write": fixed by 20000000, its hunk covers the line"#),
        "{text}"
    );
    assert!(
        text.contains("fixed by 10000000, line outside its hunk"),
        "{text}"
    );
    assert!(
        text.contains("fixed by 40000000, line not comparable"),
        "{text}"
    );
    assert!(
        at(r#""Lost write""#) < at(r#""Stale read""#)
            && at(r#""Stale read""#) < at(r#""Untouched""#),
        "{text}"
    );
}

#[tokio::test]
async fn candidates_are_absent_unless_asked_for() {
    let dir = common::scratch("outcome-audit-no-candidates");
    let t = candidate_tools(&dir);
    let r = audit(&t, &candidate_rows(), 0, &AuditArgs::default()).await;
    assert!(r.get("candidates").is_none());
    assert!(!render(&r).contains("Candidates for a labelled corpus"));
    let calls = std::fs::read_to_string(dir.join("calls.log")).unwrap();
    assert!(!calls.contains("/commits/"), "{calls}");
    assert_eq!(r["prs"]["ghCalls"], 8);
}
