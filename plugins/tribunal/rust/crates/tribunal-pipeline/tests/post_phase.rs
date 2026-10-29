//! `post_phase` end to end with a scripted agent and a fake `gh`: what it
//! posts, what it refuses, and the row it patches. No test here can reach
//! GitHub: `Tools.gh` is a script in a temp directory that records its argv.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};
use tribunal_agent::{Agent, DispatchRequest, DispatchResult};
use tribunal_agent::{FakeAgent, FakeAnswer};
use tribunal_context::exec::Tools;
use tribunal_context::operator::OperatorEnv;
use tribunal_pipeline::context::{RunArgs, Runtime};
use tribunal_pipeline::post::{
    post_phase, post_saved_run, PostChoice, PostInput, PostMode, PostOutcome,
};

/// Writes `compose` into the body file the compose prompt names, and
/// `patched` on every patch round.
struct Scribe {
    compose: String,
    patched: Option<String>,
    labels: Mutex<Vec<String>>,
}

fn body_path(prompt: &str) -> Option<PathBuf> {
    let at = prompt.find("Write the final body to ")? + "Write the final body to ".len();
    let rest = &prompt[at..];
    Some(PathBuf::from(&rest[..rest.find(" with the Write tool")?]))
}

fn patched_path(prompt: &str) -> Option<PathBuf> {
    let at = prompt.find("The review body in ")? + "The review body in ".len();
    let rest = &prompt[at..];
    Some(PathBuf::from(&rest[..rest.find(" must carry")?]))
}

impl Agent for Scribe {
    async fn run(&self, req: &DispatchRequest) -> DispatchResult {
        self.labels.lock().unwrap().push(req.label.clone());
        assert!(
            req.inherit_settings,
            "{} must load the operator's skills",
            req.label
        );
        if req.label == "compose-post" {
            std::fs::write(body_path(&req.prompt).unwrap(), &self.compose).unwrap();
        } else if let (Some(p), Some(text)) = (patched_path(&req.prompt), &self.patched) {
            std::fs::write(p, text).unwrap();
        }
        FakeAgent::new()
            .otherwise(FakeAnswer::Text("DONE".into()))
            .run(req)
            .await
    }
}

struct World {
    root: PathBuf,
    rt: Runtime,
}

/// Writes an executable script and runs it once. Executing a file this
/// process has just written fails with ETXTBSY while a child another test's
/// thread forked during the write still holds the write descriptor, until
/// that child execs. One run that starts proves no such descriptor is left,
/// so the code under test cannot hit it later.
fn write_executable(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    for _ in 0..200 {
        match std::process::Command::new(path).output() {
            Ok(_) => return,
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(e) => panic!("{}: {e}", path.display()),
        }
    }
    panic!("{} stayed busy for a second", path.display());
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn world(name: &str, row: &Value) -> World {
    let root = std::env::temp_dir().join(format!("tribunal-post-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(root.join("tmp")).unwrap();
    std::fs::write(data.join("metrics.jsonl"), format!("{row}\n")).unwrap();
    std::fs::write(root.join("head"), "abc\n").unwrap();
    let gh = root.join("gh");
    write_executable(
        &gh,
        &format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> '{log}'
case "$*" in
  *headRefOid*) cat '{head}'; exit 0 ;;
  *"reviews/5 -X PUT"*) exit 0 ;;
esac
case "$1 $2" in
  "pr review") exit 0 ;;
  "pr view") echo "https://github.com/o/r/pull/77" ;;
  "api user") echo dev ;;
  "api repos/o/r/pulls/77/reviews") echo '[{{"id": 5, "body": "posted", "user": {{"login": "dev"}}, "submitted_at": "2026-10-20T10:00:00Z"}}]' ;;
  "api repos/o/r/pulls/77/reviews/5") echo '{{"id": 5, "state": "APPROVED", "body": "old"}}' ;;
  *) echo "unexpected gh $*" >&2; exit 9 ;;
esac
"#,
            log = root.join("gh.log").display(),
            head = root.join("head").display()
        ),
    );
    // The run write_executable made is not the code under test's.
    let _ = std::fs::remove_file(root.join("gh.log"));
    let rt = Runtime {
        plugin_root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."),
        tmpdir: root.join("tmp").to_string_lossy().into_owned(),
        tools: Tools {
            gh: gh.to_string_lossy().into_owned(),
            ..Tools::default()
        },
        operator: OperatorEnv {
            review_harness_config: None,
            home: root.to_string_lossy().into_owned(),
            cwd: root.clone(),
        },
        data_dir: data,
    };
    World { root, rt }
}

fn row(gate: &str) -> Value {
    json!({
        "pr": 77, "repo": "o/r", "headSha": "abc", "harnessVersion": "pr-harness-x",
        "reviewMode": "full", "ts": "2026-10-22T10:00:00Z", "posted": false,
        "agentsActual": 12, "agentCost": {"inputTokens": 1000, "outputTokens": 10},
        "priorOpenReviews": {"gate": gate},
        "findings": [{"title": "Lost write", "mergedLocations": [{"file": "a.ts", "line": 1}, {"file": "b.ts", "line": 9}]}],
    })
}

fn gh_log(w: &World) -> String {
    std::fs::read_to_string(w.root.join("gh.log")).unwrap_or_default()
}

async fn post(w: &World, agent: &Scribe, metrics: &Value) -> (PostOutcome, Vec<String>) {
    post_over(w, agent, metrics, false).await
}

async fn post_over(
    w: &World,
    agent: &Scribe,
    metrics: &Value,
    over_own_review: bool,
) -> (PostOutcome, Vec<String>) {
    let choice = PostChoice {
        over_own_review,
        ..PostChoice::default()
    };
    post_with(w, agent, metrics, choice).await
}

async fn post_with(
    w: &World,
    agent: &Scribe,
    metrics: &Value,
    choice: PostChoice,
) -> (PostOutcome, Vec<String>) {
    let args = RunArgs {
        pr: "77".into(),
        review_model: "m".into(),
        post: true,
        ..RunArgs::default()
    };
    let mut logs = Vec::new();
    let input = PostInput {
        metrics,
        report: "## Report\nVerdict suggestion: REQUEST CHANGES\n",
        verdict: "REQUEST CHANGES",
        head_sha: "abc",
        repo: "o/r",
        cwd: Path::new(&w.root),
        effort: None,
        choice,
    };
    let out = post_phase(agent, &w.rt, &args, &input, &mut logs).await;
    (out, logs)
}

const GOOD_BODY: &str =
    "### 🔴 Blocking\n**Lost write** `a.ts:1` also at: b.ts:9\n\n### Draft comments\n- x\n";

#[tokio::test]
async fn a_clear_gate_posts_with_footer_and_patches_the_row() {
    let r = row("clear");
    let w = world("clear", &r);
    let agent = Scribe {
        compose: GOOD_BODY.into(),
        patched: None,
        labels: Mutex::default(),
    };
    let (out, _) = post(&w, &agent, &r).await;
    assert_eq!(
        out,
        PostOutcome::Posted {
            url: "https://github.com/o/r/pull/77".into(),
            verdict: "REQUEST CHANGES".into()
        }
    );
    let log = gh_log(&w);
    assert!(log.contains("pr review 77 --repo o/r --body-file"), "{log}");
    assert!(log.contains("--request-changes"), "{log}");
    let ledger = std::fs::read_to_string(w.rt.data_dir.join("metrics.jsonl")).unwrap();
    let patched: Value = serde_json::from_str(ledger.trim()).unwrap();
    assert_eq!(patched["posted"], json!(true));
    assert_eq!(patched["verdict"], json!("REQUEST CHANGES"));
    assert_eq!(
        patched["findings"][0]["mergedLocationsMissingFromReport"],
        json!([])
    );
    assert_eq!(*agent.labels.lock().unwrap(), ["compose-post"]);
}

#[tokio::test]
async fn any_other_gate_posts_nothing_and_dispatches_nothing() {
    for gate in ["own-open-review", "own-prior-review", "unknown-viewer"] {
        let r = row(gate);
        let w = world(gate, &r);
        let agent = Scribe {
            compose: GOOD_BODY.into(),
            patched: None,
            labels: Mutex::default(),
        };
        let (out, _) = post(&w, &agent, &r).await;
        assert!(
            matches!(&out, PostOutcome::Held(why) if why.contains(gate)),
            "{out:?}"
        );
        assert_eq!(gh_log(&w), "");
        assert!(agent.labels.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn a_body_missing_a_location_is_patched_before_posting() {
    let r = row("clear");
    let w = world("patch", &r);
    let agent = Scribe {
        compose: "### 🔴 Blocking\n**Lost write** `a.ts:1`\n".into(),
        patched: Some(GOOD_BODY.into()),
        labels: Mutex::default(),
    };
    let (out, logs) = post(&w, &agent, &r).await;
    assert!(matches!(out, PostOutcome::Posted { .. }), "{out:?}");
    assert_eq!(
        *agent.labels.lock().unwrap(),
        ["compose-post", "patch-post:1"]
    );
    assert!(
        logs.iter().any(|l| l.contains("patching 1 missing item")),
        "{logs:?}"
    );
}

#[tokio::test]
async fn a_body_that_stays_short_is_never_posted() {
    let r = row("clear");
    let w = world("short", &r);
    let short = "### 🔴 Blocking\n**Lost write** `a.ts:1`\n".to_owned();
    let agent = Scribe {
        compose: short.clone(),
        patched: Some(short),
        labels: Mutex::default(),
    };
    let (out, _) = post(&w, &agent, &r).await;
    assert!(
        matches!(&out, PostOutcome::Held(why) if why.contains("b.ts:9")),
        "{out:?}"
    );
    assert_eq!(gh_log(&w), "");
    assert_eq!(agent.labels.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn no_homosapiens_means_no_post() {
    let no_skill = FakeAgent::new().otherwise(FakeAnswer::Text("NO-HOMOSAPIENS".into()));
    let r = row("clear");
    let w = world("noskill", &r);
    let args = RunArgs {
        pr: "77".into(),
        post: true,
        ..RunArgs::default()
    };
    let input = PostInput {
        metrics: &r,
        report: "x",
        verdict: "COMMENT",
        head_sha: "abc",
        repo: "o/r",
        cwd: Path::new(&w.root),
        effort: None,
        choice: PostChoice::default(),
    };
    let out = post_phase(&no_skill, &w.rt, &args, &input, &mut Vec::new()).await;
    assert!(
        matches!(&out, PostOutcome::Held(why) if why.contains("homosapiens")),
        "{out:?}"
    );
    assert_eq!(gh_log(&w), "");
}

/// `--over-own-review` lifts the hold for your own earlier review and for
/// nothing else: an open review of yours or an unknown viewer still holds.
#[tokio::test]
async fn over_own_review_lifts_only_the_own_prior_review_hold() {
    let r = row("own-prior-review");
    let w = world("over-own", &r);
    let agent = Scribe {
        compose: GOOD_BODY.into(),
        patched: None,
        labels: Mutex::default(),
    };
    let (out, logs) = post_over(&w, &agent, &r, true).await;
    assert!(matches!(out, PostOutcome::Posted { .. }), "{out:?}");
    assert!(logs
        .iter()
        .any(|l| l.contains("over your own standing review")));
    for gate in ["own-open-review", "unknown-viewer"] {
        let r = row(gate);
        let w = world(&format!("over-{gate}"), &r);
        let agent = Scribe {
            compose: GOOD_BODY.into(),
            patched: None,
            labels: Mutex::default(),
        };
        let (out, _) = post_over(&w, &agent, &r, true).await;
        assert!(
            matches!(&out, PostOutcome::Held(why) if why.contains(gate)),
            "{out:?}"
        );
        assert_eq!(gh_log(&w), "");
    }
}

fn saved_run(r: &Value) -> Value {
    json!({
        "repo": "o/r",
        "verdict": "REQUEST CHANGES",
        "report": "## Report\nVerdict suggestion: REQUEST CHANGES\n",
        "metrics": r,
    })
}

/// `tribunal post` posts a saved run while the PR is still at the commit
/// it reviewed, and refuses once the PR has moved, before any dispatch.
#[tokio::test]
async fn a_saved_run_posts_only_at_the_commit_it_reviewed() {
    let r = row("own-prior-review");
    let w = world("saved", &r);
    let agent = Scribe {
        compose: GOOD_BODY.into(),
        patched: None,
        labels: Mutex::default(),
    };
    let mut logs = Vec::new();
    let over = PostChoice {
        over_own_review: true,
        ..PostChoice::default()
    };
    let out = post_saved_run(&agent, &w.rt, &saved_run(&r), over, "m", None, &mut logs).await;
    assert!(
        matches!(out, PostOutcome::Posted { .. }),
        "{out:?} {logs:?}"
    );
    assert!(gh_log(&w).contains("--request-changes"));

    let w = world("saved-moved", &r);
    std::fs::write(w.root.join("head"), "def\n").unwrap();
    let agent = Scribe {
        compose: GOOD_BODY.into(),
        patched: None,
        labels: Mutex::default(),
    };
    let out = post_saved_run(&agent, &w.rt, &saved_run(&r), over, "m", None, &mut logs).await;
    assert!(
        matches!(&out, PostOutcome::Held(why) if why.contains("head is def")),
        "{out:?}"
    );
    assert!(agent.labels.lock().unwrap().is_empty());
    assert!(!gh_log(&w).contains("pr review"));
}

/// `tribunal post --verdict` posts the review state the operator picked at
/// Step 4, not the run's suggestion, and records that verdict.
#[tokio::test]
async fn a_saved_run_posts_the_verdict_the_operator_picked() {
    let r = row("clear");
    let w = world("saved-verdict", &r);
    let agent = Scribe {
        compose: GOOD_BODY.into(),
        patched: None,
        labels: Mutex::default(),
    };
    let choice = PostChoice {
        verdict: Some("COMMENT"),
        ..PostChoice::default()
    };
    let mut logs = Vec::new();
    let out = post_saved_run(&agent, &w.rt, &saved_run(&r), choice, "m", None, &mut logs).await;
    assert!(
        matches!(&out, PostOutcome::Posted { verdict, .. } if verdict == "COMMENT"),
        "{out:?} {logs:?}"
    );
    let log = gh_log(&w);
    assert!(log.contains("--comment"), "{log}");
    assert!(!log.contains("--request-changes"), "{log}");
}

/// A run with no metrics row cannot carry a footer, so nothing is posted.
#[tokio::test]
async fn a_saved_run_without_a_row_is_not_posted() {
    let r = row("clear");
    let w = world("saved-norow", &r);
    let agent = Scribe {
        compose: GOOD_BODY.into(),
        patched: None,
        labels: Mutex::default(),
    };
    let mut run = saved_run(&r);
    run["metrics"] = Value::Null;
    let mut logs = Vec::new();
    let out = post_saved_run(
        &agent,
        &w.rt,
        &run,
        PostChoice::default(),
        "m",
        None,
        &mut logs,
    )
    .await;
    assert!(
        matches!(&out, PostOutcome::Held(why) if why.contains("no metrics row")),
        "{out:?}"
    );
    assert_eq!(gh_log(&w), "");
}

fn scribe(compose: &str, patched: Option<&str>) -> Scribe {
    Scribe {
        compose: compose.into(),
        patched: patched.map(Into::into),
        labels: Mutex::default(),
    }
}

fn mode(mode: PostMode) -> PostChoice {
    PostChoice {
        mode,
        ..PostChoice::default()
    }
}

/// `--over-standing-review` is SKILL.md's "post the full review anyway": it
/// lets the full review through every gate, an open review of yours and an
/// unknown viewer included.
#[tokio::test]
async fn over_standing_review_posts_the_full_review_past_any_gate() {
    for gate in ["own-open-review", "own-prior-review", "unknown-viewer"] {
        let r = row(gate);
        let w = world(&format!("over-standing-{gate}"), &r);
        let agent = scribe(GOOD_BODY, None);
        let choice = PostChoice {
            over_standing_review: true,
            ..PostChoice::default()
        };
        let (out, logs) = post_with(&w, &agent, &r, choice).await;
        assert!(matches!(out, PostOutcome::Posted { .. }), "{gate}: {out:?}");
        assert!(gh_log(&w).contains("--request-changes"), "{gate}");
        assert!(
            logs.iter().any(|l| l.contains("over the standing review")),
            "{logs:?}"
        );
    }
}

/// A delta is a COMMENT whatever the run suggested, and the fidelity gate
/// checks only the findings it carries: the one it leaves out (the standing
/// review already has it) costs no patch round.
#[tokio::test]
async fn a_delta_is_a_comment_checked_only_for_the_findings_it_carries() {
    let r = row("own-prior-review");
    let w = world("delta", &r);
    let agent = scribe(
        "Since your APPROVED review of 2026-10-20: nothing new.\n\n### Draft comments\n- x\n",
        None,
    );
    let (out, logs) = post_with(&w, &agent, &r, mode(PostMode::Delta)).await;
    assert_eq!(
        out,
        PostOutcome::Posted {
            url: "https://github.com/o/r/pull/77".into(),
            verdict: "COMMENT".into()
        },
        "{logs:?}"
    );
    let log = gh_log(&w);
    assert!(log.contains("--comment"), "{log}");
    assert!(!log.contains("--request-changes"), "{log}");
    assert_eq!(*agent.labels.lock().unwrap(), ["compose-post"]);
    let ledger = std::fs::read_to_string(w.rt.data_dir.join("metrics.jsonl")).unwrap();
    let patched: Value = serde_json::from_str(ledger.trim()).unwrap();
    assert_eq!(patched["verdict"], json!("COMMENT"));
}

/// A finding the delta does carry keeps every annotation, as in a full post.
#[tokio::test]
async fn a_delta_keeps_the_annotations_of_a_finding_it_carries() {
    let r = row("own-open-review");
    let w = world("delta-patch", &r);
    let agent = scribe(
        "### 🔴 Blocking\n**Lost write** `a.ts:1`\n",
        Some(GOOD_BODY),
    );
    let (out, _) = post_with(&w, &agent, &r, mode(PostMode::Delta)).await;
    assert!(matches!(out, PostOutcome::Posted { .. }), "{out:?}");
    assert_eq!(
        *agent.labels.lock().unwrap(),
        ["compose-post", "patch-post:1"]
    );
}

/// A delta or an edit needs a review of yours to stand on the PR: on a clear
/// gate or an unknown viewer nothing is dispatched and nothing is posted.
#[tokio::test]
async fn a_delta_or_an_edit_needs_a_standing_review_of_yours() {
    for gate in ["clear", "unknown-viewer"] {
        for m in [PostMode::Delta, PostMode::Edit] {
            let r = row(gate);
            let w = world(&format!("no-standing-{gate}-{m:?}"), &r);
            let agent = scribe(GOOD_BODY, None);
            let (out, _) = post_with(&w, &agent, &r, mode(m)).await;
            assert!(
                matches!(&out, PostOutcome::Held(why) if why.contains(gate)),
                "{gate} {m:?}: {out:?}"
            );
            assert_eq!(gh_log(&w), "");
            assert!(agent.labels.lock().unwrap().is_empty());
        }
    }
}

/// An edit finds your newest review, replaces its body and records the state
/// GitHub keeps, here APPROVED, whatever the run suggested.
#[tokio::test]
async fn an_edit_overwrites_your_latest_review_and_keeps_its_state() {
    let r = row("own-prior-review");
    let w = world("edit", &r);
    let agent = scribe(GOOD_BODY, None);
    let (out, logs) = post_with(&w, &agent, &r, mode(PostMode::Edit)).await;
    assert_eq!(
        out,
        PostOutcome::Posted {
            url: "https://github.com/o/r/pull/77".into(),
            verdict: "APPROVE".into()
        },
        "{logs:?}"
    );
    let log = gh_log(&w);
    assert!(log.contains("api user"), "{log}");
    assert!(
        log.contains("api repos/o/r/pulls/77/reviews/5 -X PUT"),
        "{log}"
    );
    assert!(!log.contains("pr review"), "{log}");
    let ledger = std::fs::read_to_string(w.rt.data_dir.join("metrics.jsonl")).unwrap();
    let patched: Value = serde_json::from_str(ledger.trim()).unwrap();
    assert_eq!(patched["posted"], json!(true));
    assert_eq!(patched["verdict"], json!("APPROVE"));
}
