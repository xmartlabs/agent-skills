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
    patch_prompt, post_phase, post_saved_run, FidelityGaps, PostChoice, PostInput, PostMode,
    PostOutcome,
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
    let end = [" must carry", " calls findings unexamined"]
        .iter()
        .filter_map(|m| rest.find(m))
        .min()?;
    Some(PathBuf::from(&rest[..end]))
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

/// One compose dispatch: write this body to the file, or only answer with it.
enum Compose {
    Write(&'static str),
    Answer(&'static str),
}

/// A composer that does not always write the file: each compose dispatch
/// takes the next step, and every patch round writes `patched`.
struct Talker {
    steps: Mutex<Vec<Compose>>,
    patched: Option<&'static str>,
    labels: Mutex<Vec<String>>,
}

impl Talker {
    fn new(mut steps: Vec<Compose>, patched: Option<&'static str>) -> Self {
        steps.reverse();
        Self {
            steps: Mutex::new(steps),
            patched,
            labels: Mutex::default(),
        }
    }
}

impl Agent for Talker {
    async fn run(&self, req: &DispatchRequest) -> DispatchResult {
        self.labels.lock().unwrap().push(req.label.clone());
        let answer = if req.label.starts_with("compose-post") {
            match self.steps.lock().unwrap().pop().expect("a compose step") {
                Compose::Write(text) => {
                    std::fs::write(body_path(&req.prompt).unwrap(), text).unwrap();
                    "DONE"
                }
                Compose::Answer(text) => text,
            }
        } else {
            if let (Some(p), Some(text)) = (patched_path(&req.prompt), self.patched) {
                std::fs::write(p, text).unwrap();
            }
            "DONE"
        };
        FakeAgent::new()
            .otherwise(FakeAnswer::Text(answer.into()))
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
  *"reviews -X POST"*) for a in "$@"; do case "$a" in body=@*) cp "${{a#body=@}}" '{posted}' ;; esac; done; exit 0 ;;
esac
case "$1 $2" in
  "pr review") prev=; for a in "$@"; do [ "$prev" = --body-file ] && cp "$a" '{posted}'; prev=$a; done; exit 0 ;;
  "pr view") echo "https://github.com/o/r/pull/77" ;;
  "api user") echo dev ;;
  "api repos/o/r/pulls/77/reviews") echo '[{{"id": 5, "body": "posted", "user": {{"login": "dev"}}, "submitted_at": "2026-10-20T10:00:00Z"}}]' ;;
  "api repos/o/r/pulls/77/reviews/5") echo '{{"id": 5, "state": "APPROVED", "body": "old"}}' ;;
  *) echo "unexpected gh $*" >&2; exit 9 ;;
esac
"#,
            log = root.join("gh.log").display(),
            head = root.join("head").display(),
            posted = root.join("posted-body.md").display()
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

async fn post_with<A: Agent>(
    w: &World,
    agent: &A,
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
        cross_repo_note: "",
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
    assert!(log.contains("api repos/o/r/pulls/77/reviews -X POST -f commit_id=abc -f event=REQUEST_CHANGES -F body=@"), "{log}");
    assert!(log.contains("event=REQUEST_CHANGES"), "{log}");
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

/// The footer reads `lensSamples` off the row; an older row has none.
#[tokio::test]
async fn the_posted_footer_names_the_lens_samples_of_the_row() {
    for (name, samples, segment) in [
        (
            "samples-3",
            Some(3),
            Some("full review · 3 lens samples · 12 agents"),
        ),
        (
            "samples-1",
            Some(1),
            Some("full review · 1 lens sample · 12 agents"),
        ),
        ("samples-none", None, None),
    ] {
        let mut r = row("clear");
        if let Some(k) = samples {
            r["lensSamples"] = json!(k);
        }
        let w = world(name, &r);
        let agent = Scribe {
            compose: GOOD_BODY.into(),
            patched: None,
            labels: Mutex::default(),
        };
        let (out, _) = post(&w, &agent, &r).await;
        assert!(matches!(out, PostOutcome::Posted { .. }), "{out:?}");
        let body = std::fs::read_to_string(w.root.join("posted-body.md")).unwrap();
        let footer = body.lines().last().unwrap();
        match segment {
            Some(s) => assert!(footer.contains(s), "{footer}"),
            None => assert!(
                footer.contains("full review · 12 agents") && !footer.contains("lens sample"),
                "{footer}"
            ),
        }
    }
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

/// A composer that answers with the body instead of writing it: the answer
/// becomes the body, and the fidelity gate still patches what it lacks.
#[tokio::test]
async fn a_body_given_as_the_answer_is_posted_through_the_fidelity_gate() {
    let r = row("clear");
    let w = world("answered", &r);
    let agent = Talker::new(
        vec![Compose::Answer(
            "## Peer review\n\n### 🔴 Blocking\n**Lost write** `a.ts:1`\n\nDONE",
        )],
        Some(GOOD_BODY),
    );
    let (out, logs) = post_with(&w, &agent, &r, PostChoice::default()).await;
    assert!(
        matches!(out, PostOutcome::Posted { .. }),
        "{out:?} {logs:?}"
    );
    assert_eq!(
        *agent.labels.lock().unwrap(),
        ["compose-post", "patch-post:1"]
    );
    assert!(
        logs.iter()
            .any(|l| l.contains("compose-post answered with the body")),
        "{logs:?}"
    );
    let posted = std::fs::read_to_string(w.root.join("posted-body.md")).unwrap();
    assert!(
        posted.contains("**Lost write** `a.ts:1` also at: b.ts:9"),
        "{posted}"
    );
}

/// The answered body is posted as answered when it already passes the gate,
/// without the `DONE` the composer ended it with.
#[tokio::test]
async fn an_answered_body_that_passes_the_gate_is_posted_as_answered() {
    let r = row("clear");
    let w = world("answered-clean", &r);
    let agent = Talker::new(
        vec![Compose::Answer(
            "### 🔴 Blocking\n**Lost write** `a.ts:1` also at: b.ts:9\n\n### Draft comments\n- x\nDONE",
        )],
        None,
    );
    let (out, _) = post_with(&w, &agent, &r, PostChoice::default()).await;
    assert!(matches!(out, PostOutcome::Posted { .. }), "{out:?}");
    assert_eq!(*agent.labels.lock().unwrap(), ["compose-post"]);
    let posted = std::fs::read_to_string(w.root.join("posted-body.md")).unwrap();
    assert!(
        posted.contains("**Lost write** `a.ts:1` also at: b.ts:9"),
        "{posted}"
    );
    assert!(!posted.contains("DONE"), "{posted}");
}

/// A compose step that neither writes a body nor answers with one is asked
/// once more, and the second attempt's file is posted.
#[tokio::test]
async fn a_compose_step_without_a_body_is_asked_once_more() {
    let r = row("clear");
    let w = world("retried", &r);
    let agent = Talker::new(
        vec![Compose::Answer("DONE"), Compose::Write(GOOD_BODY)],
        None,
    );
    let (out, _) = post_with(&w, &agent, &r, PostChoice::default()).await;
    assert!(matches!(out, PostOutcome::Posted { .. }), "{out:?}");
    assert_eq!(
        *agent.labels.lock().unwrap(),
        ["compose-post", "compose-post:2"]
    );
}

/// Two compose attempts with no body and no body in the answers: nothing is
/// posted, and the hold carries both answers.
#[tokio::test]
async fn no_body_after_two_compose_attempts_is_never_posted() {
    let r = row("clear");
    let w = world("nobody", &r);
    let agent = Talker::new(
        vec![
            Compose::Answer("DONE"),
            Compose::Answer("I could not write the file."),
        ],
        None,
    );
    let (out, _) = post_with(&w, &agent, &r, PostChoice::default()).await;
    assert!(
        matches!(&out, PostOutcome::Held(why)
            if why.contains("wrote no body in 2 attempts") && why.contains("I could not write the file.")),
        "{out:?}"
    );
    assert_eq!(gh_log(&w), "");
    assert_eq!(
        *agent.labels.lock().unwrap(),
        ["compose-post", "compose-post:2"]
    );
}

fn demoted_row() -> Value {
    let mut r = row("clear");
    r["findings"] = json!([{
        "title": "Demoted write", "file": "a.ts", "line": 12,
        "votes": [{"verdict": "NOT_REFUTED"}],
        "verifiedByGateNote": "Downgraded: no verifiedBy",
    }]);
    r
}

const UNEXAMINED_BODY: &str =
    "### Nits\n**Demoted write** `a.ts:12`\nNOT ADJUDICATED: no refuter examined this claim\n";
const EXAMINED_BODY: &str = "### Nits\n**Demoted write** `a.ts:12`\nPanel: 1 NOT_REFUTED vote.\n";

#[tokio::test]
async fn the_unexamined_marker_on_a_gate_demoted_finding_is_patched_out() {
    let r = demoted_row();
    let w = world("unexamined-patched", &r);
    let agent = Scribe {
        compose: UNEXAMINED_BODY.into(),
        patched: Some(EXAMINED_BODY.into()),
        labels: Mutex::default(),
    };
    let (out, logs) = post(&w, &agent, &r).await;
    assert!(matches!(out, PostOutcome::Posted { .. }), "{out:?}");
    assert_eq!(
        *agent.labels.lock().unwrap(),
        ["compose-post", "patch-post:1"]
    );
    assert!(
        logs.iter()
            .any(|l| l.contains("patching 0 missing item(s) and 2 unexamined marker(s)")),
        "{logs:?}"
    );
    let posted = std::fs::read_to_string(w.root.join("posted-body.md")).unwrap();
    assert!(!posted.contains("NOT ADJUDICATED"), "{posted}");
}

#[tokio::test]
async fn a_review_that_keeps_the_unexamined_marker_is_held() {
    let r = demoted_row();
    let w = world("unexamined-kept", &r);
    let agent = Scribe {
        compose: UNEXAMINED_BODY.into(),
        patched: Some(UNEXAMINED_BODY.into()),
        labels: Mutex::default(),
    };
    let (out, _) = post(&w, &agent, &r).await;
    assert!(
        matches!(&out, PostOutcome::Held(why)
            if why.contains("Demoted write") && why.contains("NOT ADJUDICATED")),
        "{out:?}"
    );
    assert_eq!(gh_log(&w), "");
    assert_eq!(agent.labels.lock().unwrap().len(), 4);
}

#[test]
fn the_patch_prompt_names_what_to_add_and_what_to_remove() {
    let body = Path::new("/tmp/body.md");
    let add = FidelityGaps {
        missing: vec!["Lost write: also at b.ts:9".into()],
        unexamined: vec![],
    };
    assert_eq!(
        patch_prompt(body, &add),
        "The review body in /tmp/body.md must carry these items and does not:\n- Lost write: also at b.ts:9\n\nEdit the file so each one appears, in the finding it belongs to. Change nothing else, keep the `### Draft comments` section, and do not add a verdict banner or a footer. Answer with the single word DONE."
    );

    let both = FidelityGaps {
        unexamined: vec!["Demoted write: \"NOT ADJUDICATED\"".into()],
        ..add
    };
    let p = patch_prompt(body, &both);
    assert!(p.contains("- Lost write: also at b.ts:9"), "{p}");
    assert!(p.contains("- Demoted write: \"NOT ADJUDICATED\""), "{p}");
    assert!(p.contains("remove that wording"), "{p}");

    let remove = FidelityGaps {
        missing: vec![],
        ..both
    };
    let p = patch_prompt(body, &remove);
    assert!(!p.contains("must carry these items"), "{p}");
    assert!(
        p.starts_with("The review body in /tmp/body.md calls findings unexamined"),
        "{p}"
    );
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
        cross_repo_note: "",
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
    assert!(gh_log(&w).contains("event=REQUEST_CHANGES"));

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
    assert!(!gh_log(&w).contains("-X POST"));
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
    assert!(log.contains("event=COMMENT"), "{log}");
    assert!(!log.contains("event=REQUEST_CHANGES"), "{log}");
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
        assert!(gh_log(&w).contains("event=REQUEST_CHANGES"), "{gate}");
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
    assert!(log.contains("event=COMMENT"), "{log}");
    assert!(!log.contains("event=REQUEST_CHANGES"), "{log}");
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
    assert!(!log.contains("-X POST"), "{log}");
    let ledger = std::fs::read_to_string(w.rt.data_dir.join("metrics.jsonl")).unwrap();
    let patched: Value = serde_json::from_str(ledger.trim()).unwrap();
    assert_eq!(patched["posted"], json!(true));
    assert_eq!(patched["verdict"], json!("APPROVE"));
}

/// The posted body is composed from the report less what only the operator
/// acts on: the panel-off and sweep-off lines, and a suggestion to approve
/// when the review is posted in another state.
#[test]
fn the_posted_report_drops_operator_lines_and_an_unposted_approval() {
    use tribunal_pipeline::post::posted_report;
    use tribunal_pipeline::synthesize::{
        PANEL_OFF_BY_DEFAULT, PANEL_OFF_BY_FLAG, SWEEP_OFF_BY_DEFAULT, SWEEP_OFF_BY_FLAG,
    };
    let report = format!(
        "## Peer review\n\n### Verdict suggestion: APPROVE\n\n{SWEEP_OFF_BY_DEFAULT}\n\n{PANEL_OFF_BY_FLAG}\n\n### 🔴 Blocking\nNone.\n"
    );
    let posted = posted_report(&report, "COMMENT");
    assert_eq!(posted, "## Peer review\n\n### 🔴 Blocking\nNone.\n");
    for line in [
        PANEL_OFF_BY_DEFAULT,
        PANEL_OFF_BY_FLAG,
        SWEEP_OFF_BY_DEFAULT,
        SWEEP_OFF_BY_FLAG,
    ] {
        let report = format!("a\n\n{line}\n\nb\n");
        assert_eq!(posted_report(&report, "APPROVE"), "a\n\nb\n", "{line}");
    }
    // Posted as an approval, the suggestion matches the state and stays.
    assert!(posted_report(&report, "APPROVE").contains("### Verdict suggestion: APPROVE"));
    // A suggestion to request changes is information on a comment, and stays.
    let changes = "### Verdict suggestion: REQUEST CHANGES\n\nOne blocker.\n";
    assert_eq!(posted_report(changes, "COMMENT"), changes);
}

/// The posted body has a section for the report's questions to review, and
/// the composer is told to keep each finding in the report's own section:
/// without both, questions landed under Should fix while the findings status
/// counted none.
#[test]
fn the_compose_prompts_keep_questions_out_of_should_fix() {
    use std::path::Path;
    use tribunal_pipeline::post::{compose_prompt, delta_prompt, edit_prompt};
    let (r, o) = (Path::new("/r.md"), Path::new("/o.md"));
    for prompt in [
        compose_prompt("1", "o/r", "abc", r, o),
        edit_prompt("1", "o/r", "abc", r, o),
        delta_prompt("1", "o/r", "abc", r, o),
    ] {
        assert!(
            prompt.contains("`### ⚪ Questions to review, not counted`"),
            "{prompt}"
        );
        assert!(prompt.contains("only the report's Blocking items go under Blocking and only its Should fix items under Should fix"), "{prompt}");
        assert!(
            prompt.contains("never moves up to Blocking or Should fix"),
            "{prompt}"
        );
        assert!(
            prompt.contains("`### 🟣 Pre-existing, not blocking`"),
            "{prompt}"
        );
        assert!(
            !prompt.contains("{sections}") && !prompt.contains("{section_rule}"),
            "{prompt}"
        );
    }
}
