//! `repair_anchor_lines` against the compiled `tribunal-test-gh`, which answers
//! only the file reads the test records. No network and no model.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tribunal_context::exec::Tools;
use tribunal_core::verified_gate::finding_problems;
use tribunal_pipeline::anchor_repair::{anchor_from_evidence, repair_anchor_lines, repair_anchors};
use tribunal_pipeline::synthesize::surviving_for_synthesis;
use tribunal_pipeline::verify::{confirmed_result, merge_candidates, skip_refuters, GroundedGate};

const REPO: &str = "acme/widgets";
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const FILE: &str =
    "export function underLimit(count: number, limit: number) {\n  if (limit <= 0) return false;\n  return count <= limit;\n}\n";
const QUOTE: &str = "  if (limit <= 0) return false;";

fn test_gh() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let gh = exe
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join(format!("tribunal-test-gh{}", std::env::consts::EXE_SUFFIX));
    assert!(
        gh.is_file(),
        "{} is missing: run `cargo build -p tribunal-context --bin tribunal-test-gh`, or `cargo test --workspace`",
        gh.display()
    );
    gh
}

struct Setup {
    dir: PathBuf,
    tools: Tools,
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn setup(name: &str, serve_file: bool) -> Setup {
    let endpoint = format!("repos/{REPO}/contents/src/limit.ts?ref={HEAD}");
    if serve_file {
        setup_with(name, &[(endpoint.as_str(), FILE)])
    } else {
        setup_with(name, &[])
    }
}

/// A `gh` that serves each `(endpoint, file text)` and nothing else.
fn setup_with(name: &str, files: &[(&str, &str)]) -> Setup {
    let dir = std::env::temp_dir().join(format!("tribunal-anchor-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let calls: Vec<Value> = files
        .iter()
        .map(|(endpoint, text)| {
            json!({
                "args": ["api", "-H", "Accept: application/vnd.github.raw", endpoint],
                "stdout": text, "stderr": "", "code": 0
            })
        })
        .collect();
    let recordings = dir.join("gh.json");
    std::fs::write(&recordings, json!({"calls": calls}).to_string()).unwrap();
    let tools = Tools {
        gh: test_gh().to_string_lossy().into_owned(),
        env: vec![
            (OsString::from("TRIBUNAL_TEST_GH"), OsString::new()),
            (OsString::from("GH_RECORDINGS"), recordings.into_os_string()),
        ],
        ..Tools::default()
    };
    Setup { dir, tools }
}

fn verified(quote: &str) -> Vec<Option<Value>> {
    vec![Some(json!({"finding": {
        "title": "t", "severity": "important", "adjudicated": true,
        "verifiedBy": [{"file": "src/limit.ts", "line": 0, "quote": quote}],
    }}))]
}

fn problems(v: &[Option<Value>]) -> Vec<String> {
    finding_problems(&v[0].as_ref().unwrap()["finding"])
}

#[tokio::test]
async fn a_quote_found_in_the_file_at_head_gets_its_line_and_passes_the_gate() {
    let s = setup("found", true);
    let mut v = verified(QUOTE);
    assert_eq!(problems(&v).len(), 1, "the unrepaired finding must fail");
    let mut logs = Vec::new();
    repair_anchor_lines(&s.tools, HEAD, REPO, &[], &mut v, &mut logs).await;
    assert_eq!(
        v[0].as_ref().unwrap()["finding"]["verifiedBy"][0]["line"],
        2
    );
    assert!(problems(&v).is_empty(), "{:?}", problems(&v));
    assert!(logs[0].contains("completed 1 of 1"), "{logs:?}");
}

#[tokio::test]
async fn a_quote_that_is_not_in_the_file_stays_demoted() {
    let s = setup("missing", true);
    let mut v = verified("  return count < limit + 1000; // not in the file");
    let mut logs = Vec::new();
    repair_anchor_lines(&s.tools, HEAD, REPO, &[], &mut v, &mut logs).await;
    assert_eq!(
        v[0].as_ref().unwrap()["finding"]["verifiedBy"][0]["line"],
        0
    );
    assert_eq!(
        problems(&v),
        ["verifiedBy[0].line is not a positive integer"]
    );
    assert!(logs[0].contains("completed 0 of 1"), "{logs:?}");
}

#[tokio::test]
async fn a_failed_file_read_leaves_the_anchor_as_it_was() {
    let s = setup("unserved", false);
    let mut v = verified(QUOTE);
    let mut logs = Vec::new();
    repair_anchor_lines(&s.tools, HEAD, REPO, &[], &mut v, &mut logs).await;
    assert_eq!(
        v[0].as_ref().unwrap()["finding"]["verifiedBy"][0]["line"],
        0
    );
    assert_eq!(problems(&v).len(), 1);
}

#[tokio::test]
async fn nothing_is_read_when_every_line_is_already_valid() {
    let s = setup("valid", false);
    let mut v = vec![Some(json!({"finding": {
        "title": "t", "severity": "important", "adjudicated": true,
        "verifiedBy": [{"file": "src/limit.ts", "line": 2, "quote": QUOTE}],
    }}))];
    let mut logs = Vec::new();
    repair_anchor_lines(&s.tools, HEAD, REPO, &[], &mut v, &mut logs).await;
    assert!(logs.is_empty(), "{logs:?}");
}

fn line_of(v: &[Option<Value>]) -> Value {
    v[0].as_ref().unwrap()["finding"]["verifiedBy"][0]["line"].clone()
}

#[tokio::test]
async fn a_repaired_verdict_goes_through_the_report_gate_without_a_note() {
    let s = setup("gate", true);
    let verdict = json!({
        "finding": {"title": "t", "severity": "important", "verifiedBy": [
            {"file": "src/limit.ts", "line": 0, "quote": QUOTE}]},
        "panelRan": true, "unverified": false, "survived": true, "votes": [],
    });
    let before = surviving_for_synthesis(std::slice::from_ref(&verdict));
    assert_eq!(before[0]["adjudicated"], json!(false));
    assert!(before[0]["verifiedByGateNote"]
        .as_str()
        .unwrap()
        .contains("positive integer"));

    let mut v = vec![Some(verdict)];
    repair_anchor_lines(&s.tools, HEAD, REPO, &[], &mut v, &mut Vec::new()).await;
    let rows: Vec<Value> = v.into_iter().flatten().collect();
    let after = surviving_for_synthesis(&rows);
    assert_eq!(after[0]["adjudicated"], json!(true));
    assert!(after[0].get("verifiedByGateNote").is_none());
}

#[tokio::test]
async fn an_anchor_into_a_sibling_repo_is_read_from_that_repo() {
    let endpoint = "repos/acme/web/contents/src/button.ts?ref=main";
    let s = setup_with(
        "sibling",
        &[(endpoint, "one\nexport const Button = () => null;\n")],
    );
    let mut v = vec![Some(json!({"finding": {
        "title": "t", "severity": "important", "adjudicated": true,
        "verifiedBy": [{"file": "web/src/button.ts", "line": 0,
                        "quote": "export const Button = () => null;"}],
    }}))];
    let cross = ["--cross-repo".to_owned(), "acme/web@main".to_owned()];
    repair_anchor_lines(&s.tools, HEAD, REPO, &cross, &mut v, &mut Vec::new()).await;
    assert_eq!(line_of(&v), 2);
}

#[tokio::test]
async fn a_quote_that_appears_twice_gets_its_first_line() {
    let endpoint = format!("repos/{REPO}/contents/src/limit.ts?ref={HEAD}");
    let twice = format!("one\n{QUOTE}\nthree\nfour\n{QUOTE}\n");
    let s = setup_with("twice", &[(endpoint.as_str(), twice.as_str())]);
    let mut v = verified(QUOTE);
    repair_anchor_lines(&s.tools, HEAD, REPO, &[], &mut v, &mut Vec::new()).await;
    assert_eq!(line_of(&v), 2);
}

fn evidenced_verdict(severity: &str, kind: &str, line: Value) -> Value {
    json!({
        "finding": {
            "title": "t", "severity": severity, "file": "src/limit.ts",
            "evidence": format!("+{QUOTE}"),
            "evidenceResolution": {"matchKind": kind,
                                   "evidenceLocated": kind != "not-found",
                                   "resolvedLine": line},
        },
        "panelRan": true, "unverified": false, "survived": true, "votes": [],
    })
}

fn note_of(row: &Value) -> Option<String> {
    row.get("verifiedByGateNote")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[tokio::test]
async fn located_evidence_gives_a_finding_without_verified_by_an_anchor_the_gate_accepts() {
    let s = setup("evidence-located", false);
    let unanchored = evidenced_verdict("important", "exact", json!(2));
    let before = surviving_for_synthesis(std::slice::from_ref(&unanchored));
    assert_eq!(before[0]["adjudicated"], json!(false));
    assert!(note_of(&before[0])
        .unwrap()
        .contains("verifiedBy is missing or empty"));

    let mut v = vec![Some(unanchored)];
    assert_eq!(anchor_from_evidence(&mut v), 1);
    let mut logs = Vec::new();
    repair_anchor_lines(&s.tools, HEAD, REPO, &[], &mut v, &mut logs).await;
    assert!(
        logs.is_empty(),
        "nothing is left for the resolver: {logs:?}"
    );

    let rows: Vec<Value> = v.into_iter().flatten().collect();
    let after = surviving_for_synthesis(&rows);
    assert_eq!(after[0]["adjudicated"], json!(true));
    assert_eq!(note_of(&after[0]), None);
    assert_eq!(
        after[0]["verifiedBy"],
        json!([{"file": "src/limit.ts", "line": 2, "quote": QUOTE.trim()}])
    );
}

#[tokio::test]
async fn ambiguous_evidence_a_nit_and_a_cited_finding_stay_as_they_were() {
    let cited = json!([{"file": "src/limit.ts", "line": 3, "quote": "  return count <= limit;"}]);
    let mut with_anchor = evidenced_verdict("important", "exact", json!(2));
    with_anchor["finding"]["verifiedBy"] = cited.clone();
    let rows = [
        evidenced_verdict("important", "ambiguous", Value::Null),
        evidenced_verdict("nit", "exact", json!(2)),
        with_anchor,
    ];
    let mut v: Vec<Option<Value>> = rows.iter().cloned().map(Some).collect();
    assert_eq!(anchor_from_evidence(&mut v), 0);
    assert_eq!(v, rows.iter().cloned().map(Some).collect::<Vec<_>>());

    let kept: Vec<Value> = v.into_iter().flatten().collect();
    let out = surviving_for_synthesis(&kept);
    assert_eq!(out[0]["adjudicated"], json!(false));
    assert!(note_of(&out[0]).is_some());
    assert_eq!(note_of(&out[1]), None);
    assert_eq!(out[2]["verifiedBy"], cited);
}

#[tokio::test]
async fn the_anchor_of_a_merged_finding_is_found_in_the_file_at_head() {
    let s = setup("evidence-merged", true);
    let member = |lens: &str| {
        json!({
            "title": "t", "severity": "important", "file": "src/limit.ts", "line": 2,
            "lens": lens, "evidence": format!("+{QUOTE}\n+  return count <= limit;"),
            "evidenceResolution": {"matchKind": "exact", "evidenceLocated": true,
                                   "resolvedLine": 2},
        })
    };
    let (a, b) = (member("correctness"), member("errorHandling"));
    let merged = merge_candidates(&[&a, &b]);
    assert!(merged["evidence"]
        .as_str()
        .unwrap()
        .starts_with("[correctness] "));
    let mut v = vec![Some(json!({"finding": merged}))];
    assert_eq!(anchor_from_evidence(&mut v), 1);
    let anchors = v[0].as_ref().unwrap()["finding"]["verifiedBy"].clone();
    assert_eq!(anchors[0]["quote"], QUOTE.trim());

    let payload = serde_json::to_string(&json!([{"title": "t", "verifiedBy": anchors}])).unwrap();
    let out = tribunal_context::resolve_verified_anchors::run(
        &["-", HEAD, "--repo", REPO, "--relay"],
        &payload,
        &s.tools,
    )
    .await
    .unwrap();
    assert_eq!(out["results"][0]["status"], "found", "{out}");
}

/// Evidence the resolver did not find in the diff: the quote is unchanged code
/// of the file, as in a finding about a check placed after an old one.
async fn outside_the_diff(name: &str, file: Option<&str>) -> (Vec<Value>, Vec<String>) {
    let endpoint = format!("repos/{REPO}/contents/src/limit.ts?ref={HEAD}");
    let s = match file {
        Some(text) => setup_with(name, &[(endpoint.as_str(), text)]),
        None => setup_with(name, &[]),
    };
    let unanchored = evidenced_verdict("important", "not-found", Value::Null);
    let before = surviving_for_synthesis(std::slice::from_ref(&unanchored));
    assert!(note_of(&before[0])
        .unwrap()
        .contains("verifiedBy is missing or empty"));
    let mut v = vec![Some(unanchored)];
    assert_eq!(anchor_from_evidence(&mut v), 1);
    let mut logs = Vec::new();
    repair_anchor_lines(&s.tools, HEAD, REPO, &[], &mut v, &mut logs).await;
    let rows: Vec<Value> = v.into_iter().flatten().collect();
    (surviving_for_synthesis(&rows), logs)
}

#[tokio::test]
async fn evidence_outside_the_diff_found_in_the_file_at_head_passes_the_gate() {
    let (out, logs) = outside_the_diff("outside-found", Some(FILE)).await;
    assert_eq!(out[0]["adjudicated"], json!(true), "{logs:?}");
    assert_eq!(note_of(&out[0]), None);
    assert_eq!(
        out[0]["verifiedBy"],
        json!([{"file": "src/limit.ts", "line": 2, "quote": QUOTE.trim()}])
    );
    assert!(logs[0].contains("completed 1 of 1"), "{logs:?}");
}

#[tokio::test]
async fn evidence_outside_the_diff_and_absent_from_the_file_stays_demoted() {
    let other = "export const unrelated = true;\n";
    let (out, logs) = outside_the_diff("outside-absent", Some(other)).await;
    assert_eq!(out[0]["adjudicated"], json!(false));
    assert!(note_of(&out[0]).is_some());
    assert!(logs[0].contains("completed 0 of 1"), "{logs:?}");
}

#[tokio::test]
async fn evidence_outside_the_diff_with_a_failed_read_stays_demoted() {
    let (out, _) = outside_the_diff("outside-unread", None).await;
    assert_eq!(out[0]["adjudicated"], json!(false));
    assert!(note_of(&out[0]).is_some());
}

/// The default run skips the refutation panel: every candidate goes to the
/// gate's `confirmed` list through `skip_refuters`, and the report gate judges
/// those rows too. The repairs must reach them, not only the panel's verdicts.
async fn without_the_panel(
    name: &str,
    serve_file: bool,
    finding: Value,
) -> (Vec<Value>, Vec<Value>, Vec<String>) {
    let s = setup(name, serve_file);
    let mut gate = GroundedGate {
        panel_input: vec![finding],
        ..GroundedGate::default()
    };
    assert_eq!(skip_refuters(&mut gate), 1);
    let before: Vec<Value> = gate.confirmed.iter().map(confirmed_result).collect();
    let before = surviving_for_synthesis(&before);
    let mut verified = Vec::new();
    let mut logs = Vec::new();
    repair_anchors(
        &s.tools,
        Some(HEAD),
        REPO,
        &[],
        &mut verified,
        &mut gate.confirmed,
        &mut logs,
    )
    .await;
    assert!(verified.is_empty());
    assert_eq!(gate.confirmed.len(), 1);
    let rows: Vec<Value> = gate.confirmed.iter().map(confirmed_result).collect();
    (before, surviving_for_synthesis(&rows), logs)
}

#[tokio::test]
async fn with_the_panel_off_a_line_zero_anchor_is_completed_and_passes_the_gate() {
    let finding = json!({
        "title": "t", "severity": "important", "file": "src/limit.ts",
        "verifiedBy": [{"file": "src/limit.ts", "line": 0, "quote": QUOTE}],
    });
    let (before, after, logs) = without_the_panel("no-panel-line", true, finding).await;
    assert!(note_of(&before[0])
        .unwrap()
        .contains("verifiedBy[0].line is not a positive integer"));
    assert_eq!(after[0]["adjudicated"], json!(true), "{logs:?}");
    assert_eq!(note_of(&after[0]), None);
    assert_eq!(after[0]["verifiedBy"][0]["line"], json!(2));
    assert!(logs[0].contains("completed 1 of 1"), "{logs:?}");
}

#[tokio::test]
async fn with_the_panel_off_located_evidence_anchors_a_finding_without_verified_by() {
    let finding = evidenced_verdict("important", "exact", json!(2))["finding"].clone();
    let (before, after, logs) = without_the_panel("no-panel-evidence", false, finding).await;
    assert!(note_of(&before[0])
        .unwrap()
        .contains("verifiedBy is missing or empty"));
    assert_eq!(after[0]["adjudicated"], json!(true), "{logs:?}");
    assert_eq!(note_of(&after[0]), None);
    assert!(logs[0].contains("anchored 1 finding"), "{logs:?}");
}

#[tokio::test]
async fn both_lists_keep_their_order_and_length() {
    let s = setup("both-lists", true);
    let mut verified = verified(QUOTE);
    verified.push(None);
    let mut confirmed = vec![
        json!({"finding": {"title": "a", "severity": "nit"}, "grounded": {}}),
        json!({"finding": {"title": "b", "severity": "important", "adjudicated": true,
                            "verifiedBy": [{"file": "src/limit.ts", "line": 0, "quote": QUOTE}]},
               "grounded": {}}),
    ];
    let mut logs = Vec::new();
    repair_anchors(
        &s.tools,
        Some(HEAD),
        REPO,
        &[],
        &mut verified,
        &mut confirmed,
        &mut logs,
    )
    .await;
    assert_eq!(verified.len(), 2);
    assert!(verified[1].is_none());
    assert_eq!(
        verified[0].as_ref().unwrap()["finding"]["verifiedBy"][0]["line"],
        2
    );
    assert_eq!(confirmed.len(), 2);
    assert_eq!(confirmed[0]["finding"]["title"], "a");
    assert_eq!(confirmed[1]["finding"]["verifiedBy"][0]["line"], 2);
    assert!(logs[0].contains("completed 2 of 2"), "{logs:?}");
}
