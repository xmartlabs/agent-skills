//! With the refutation panel off, a candidate that repeats a decision from the
//! PR's thread goes to "Pre-decided" instead of the verdict. The case is the
//! shape of a real run: the author answered a wrap-around question as
//! BY-DESIGN, a lens raised it again with severity important, and with every
//! candidate surviving it became the only blocker of a REQUEST CHANGES.

use std::path::PathBuf;

use serde_json::{json, Value};
use tribunal_agent::{FakeAgent, FakeAnswer};
use tribunal_pipeline::agent_def::AgentDef;
use tribunal_pipeline::pre_decided::{check, citable_decisions, prompt, LABEL};
use tribunal_pipeline::schemas;
use tribunal_pipeline::synthesize::side_payloads;
use tribunal_pipeline::verify::{confirmed_result, partition_results};

const FILE: &str = "src/jobs/digest.schedule.ts";
const SUMMARY: &str = "Author declined a wrap-aware window check in the digest job: production windows never wrap the year, and a job-local rule could disagree with order checkout.";

fn scribe() -> AgentDef {
    AgentDef {
        name: "tribunal-scribe".into(),
        model: None,
        tools: vec!["Read".into()],
        body: "SCRIBE".into(),
    }
}

fn decisions() -> Value {
    json!([
        {"category": "BY-DESIGN", "summary": SUMMARY, "source": "dev, 2026-11-06", "files": [FILE]},
        {"category": "RESOLVED", "summary": "Fixed the null widget id in abc123", "source": "dev, 2026-11-06", "files": []}
    ])
}

fn wrap_finding() -> Value {
    json!({
        "title": "isOpenWindow is always false for windows that wrap the year",
        "file": FILE,
        "line": 117,
        "severity": "important",
        "description": "windowFor never returns open when the window starts after it ends in the calendar year. Confirm the live rows do not wrap.",
        "evidence": "const isOpenWindow = windowFor(schedule, now) === 'open';",
        "isBehaviorChanging": true,
        "touchesHighRiskSurface": false,
    })
}

fn other_finding() -> Value {
    json!({
        "title": "Widget schema accepts nickname on archived widgets",
        "file": "src/widgets/widget.types.ts",
        "line": 77,
        "severity": "nit",
        "description": "d",
        "evidence": "nickname: z.string().nullable().optional()",
        "isBehaviorChanging": false,
        "touchesHighRiskSurface": false,
    })
}

fn answer(items: Value) -> FakeAgent {
    FakeAgent::new().answer(LABEL, FakeAnswer::Structured(json!({ "items": items })))
}

async fn run(fake: &FakeAgent, candidates: Vec<Value>) -> (Vec<Value>, Vec<Value>, Vec<String>) {
    let mut logs = Vec::new();
    let d = decisions();
    let (kept, decided, _) = check(
        fake,
        &scribe(),
        candidates,
        Some(&d),
        "",
        "sonnet",
        None,
        &PathBuf::from("/tmp/wt"),
        &mut logs,
    )
    .await;
    (kept, decided, logs)
}

#[tokio::test]
async fn a_finding_that_repeats_a_by_design_reply_leaves_the_verdict() {
    let fake = answer(json!([
        {"index": 0, "preDecided": true, "decisionQuestioned": SUMMARY, "reason": "Repeats the author's wrap-around answer."},
        {"index": 1, "preDecided": false, "decisionQuestioned": "", "reason": "Different defect."}
    ]));
    let (kept, decided, logs) = run(&fake, vec![wrap_finding(), other_finding()]).await;

    assert_eq!(kept, vec![other_finding()]);
    assert_eq!(decided.len(), 1);

    // What the rest of the run makes of it: the kept one survives as before,
    // the decided one is pre-decided and out of the survivors the verdict reads.
    let confirmed: Vec<Value> = kept
        .iter()
        .map(|c| json!({"finding": c, "grounded": {"verdict": null, "diagnostic": null}}))
        .collect();
    let verified: Vec<Option<Value>> = decided.into_iter().map(Some).collect();
    let mut plogs = Vec::new();
    let v = partition_results(&verified, &confirmed, 0, &mut plogs);
    let surviving: Vec<&Value> = v.surviving.iter().map(|r| &r["finding"]["title"]).collect();
    assert_eq!(surviving, vec![&other_finding()["title"]]);
    assert_eq!(v.pre_decided.len(), 1);
    assert_eq!(v.pre_decided[0]["finding"]["file"], json!(FILE));
    assert!(v.tech_refuted.is_empty());

    let side = side_payloads(
        &v.placed_aside,
        &v.pre_decided,
        &v.tech_refuted,
        &v.surviving,
    );
    assert_eq!(side.pre_decided[0]["decisionQuestioned"], json!(SUMMARY));
    assert!(
        logs.iter().any(|l| l.contains("1 pre-decided, 1 kept")),
        "{logs:?}"
    );

    let calls = fake.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].model.as_deref(), Some("sonnet"));
    assert_eq!(calls[0].schema.as_ref(), Some(&schemas::pre_decided()));
    assert!(calls[0].prompt.contains(SUMMARY));
    // RESOLVED needs the code read at HEAD, so this check never offers it.
    assert!(!calls[0].prompt.contains("null widget id"));
}

#[tokio::test]
async fn without_the_check_the_same_finding_survives() {
    // The behaviour this replaces: skip_refuters confirmed every candidate.
    let mut plogs = Vec::new();
    let v = partition_results(
        &[],
        &[json!({"finding": wrap_finding(), "grounded": {"verdict": null, "diagnostic": null}})],
        0,
        &mut plogs,
    );
    assert_eq!(v.surviving.len(), 1);
    assert!(v.pre_decided.is_empty());
    assert_eq!(
        confirmed_result(&json!({"finding": wrap_finding()}))["survived"],
        json!(true)
    );
}

#[tokio::test]
async fn a_citation_of_a_decision_about_another_file_is_rejected() {
    let fake = answer(json!([
        {"index": 0, "preDecided": true, "decisionQuestioned": SUMMARY, "reason": "Same topic."}
    ]));
    let (kept, decided, logs) = run(&fake, vec![other_finding()]).await;
    assert_eq!(kept, vec![other_finding()]);
    assert!(decided.is_empty());
    assert!(
        logs.iter().any(|l| l.contains("1 citation(s) rejected")),
        "{logs:?}"
    );
}

#[tokio::test]
async fn an_empty_quote_or_a_missing_answer_keeps_the_finding() {
    let fake = answer(json!([
        {"index": 0, "preDecided": true, "decisionQuestioned": "  ", "reason": "r"}
    ]));
    let (kept, decided, _) = run(&fake, vec![wrap_finding()]).await;
    assert_eq!((kept.len(), decided.len()), (1, 0));

    let fake = FakeAgent::new();
    let (kept, decided, logs) = run(&fake, vec![wrap_finding()]).await;
    assert_eq!((kept.len(), decided.len()), (1, 0));
    assert!(logs.iter().any(|l| l.contains("no answer")), "{logs:?}");
}

#[tokio::test]
async fn nothing_is_asked_without_a_citable_decision() {
    let fake = FakeAgent::new();
    let only_resolved =
        json!([{"category": "RESOLVED", "summary": "s", "source": "dev", "files": []}]);
    let mut logs = Vec::new();
    let (kept, decided, res) = check(
        &fake,
        &scribe(),
        vec![wrap_finding()],
        Some(&only_resolved),
        "",
        "sonnet",
        None,
        &PathBuf::from("/tmp/wt"),
        &mut logs,
    )
    .await;
    assert_eq!((kept.len(), decided.len()), (1, 0));
    assert!(res.is_none());
    assert!(fake.calls().is_empty());
}

#[test]
fn the_operator_context_and_its_precedence_reach_the_prompt() {
    let citable = citable_decisions(Some(&decisions()));
    assert_eq!(citable.as_array().map(Vec::len), Some(1));
    let p = prompt(
        &[wrap_finding()],
        &citable,
        "REOPENED: the wrap-around check",
    )
    .unwrap();
    assert!(p.contains("[OPERATOR"), "{p}");
    assert!(p.contains("REOPENED: the wrap-around check"), "{p}");
    assert!(p.contains("AUTHORITY IS ORDERED"), "{p}");
    assert!(prompt(&[], &citable, "").is_none());
}
