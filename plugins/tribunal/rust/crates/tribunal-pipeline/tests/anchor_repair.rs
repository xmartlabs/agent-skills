//! `repair_anchor_lines` against the compiled `tribunal-test-gh`, which answers
//! only the file reads the test records. No network and no model.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tribunal_context::exec::Tools;
use tribunal_core::verified_gate::finding_problems;
use tribunal_pipeline::anchor_repair::repair_anchor_lines;
use tribunal_pipeline::synthesize::surviving_for_synthesis;

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
