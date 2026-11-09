//! `--scenario-questions`: the generators, the merge, which lens prompt answers
//! each question, the answering model, and what reaches the candidates.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;
use tribunal_agent::{FakeAgent, FakeAnswer};
use tribunal_pipeline::agent_def::AgentDef;
use tribunal_pipeline::questions::{
    question_stage, QuestionInputs, ANSWER_PARAGRAPH, GENERATOR_PROMPT, QUESTION_LENS,
};
use tribunal_pipeline::review::{ReviewerSetup, WARMUP_PROMPT};
use tribunal_pipeline::schemas;

fn setup() -> ReviewerSetup {
    ReviewerSetup {
        agent: AgentDef {
            name: "code-review-expert".into(),
            model: None,
            tools: vec!["Read".into(), "Bash".into()],
            body: "SHARED SYSTEM PROMPT".into(),
        },
        model: "claude-sonnet-5-5".into(),
        cwd: PathBuf::from("/tmp/wt"),
        timeout: Duration::from_secs(90),
        concurrency: 3,
        effort: Some("high".into()),
    }
}

#[tokio::test]
async fn questions_are_generated_merged_routed_and_answered_one_per_dispatch() {
    let fake = FakeAgent::new()
        .answer(
            "questions:1",
            FakeAnswer::Structured(json!({"questions": [
                {"file": "src/jobs/sync.ts", "line": 40, "question": "If the provider returns 503 for an hour, does the job ever retry?"},
                {"file": "src/web/Banner.tsx", "question": "If the user goes Back mid-flow, does the redirect fire again?"}
            ]})),
        )
        .answer(
            "questions:2",
            FakeAnswer::Structured(json!({"questions": [
                {"file": "src/jobs/sync.ts", "line": 41, "question": "If the provider returns 503 for an hour, does the job ever retry at all?"},
                {"file": "src/jobs/other.ts", "question": "What does the caller do when lookup returns null?"}
            ]})),
        )
        .answer(
            "question:1",
            FakeAnswer::Structured(json!({"findings": [
                {"title": "A 503 is swallowed to null, so the job never retries", "file": "src/jobs/sync.ts", "line": 40, "severity": "important", "description": "d", "evidence": "e", "isBehaviorChanging": true, "touchesHighRiskSurface": false}
            ]})),
        )
        .answer("question:2", FakeAnswer::Structured(json!({"findings": []})))
        .answer(
            "question:3",
            FakeAnswer::Structured(json!({"findings": [
                {"title": "Outside the PR", "file": "src/unrelated.ts", "line": 1, "severity": "nit", "description": "d", "evidence": "e", "isBehaviorChanging": false, "touchesHighRiskSurface": false}
            ]})),
        )
        .answer("cache-warmup", FakeAnswer::Structured(json!({"findings": []})));
    let s = setup();
    let prompts = ["LENS ONE PROMPT".to_owned(), "LENS TWO PROMPT".to_owned()];
    let files = [
        Some(vec![
            "src/jobs/sync.ts".to_owned(),
            "src/jobs/other.ts".to_owned(),
        ]),
        Some(vec!["src/web/Banner.tsx".to_owned()]),
    ];
    let pr_files = [
        "src/jobs/sync.ts".to_owned(),
        "src/jobs/other.ts".to_owned(),
        "src/web/Banner.tsx".to_owned(),
    ];
    let schema = schemas::findings();
    let out = question_stage(
        &fake,
        &QuestionInputs {
            lens_setup: &s,
            answer_model: "claude-haiku-5-5",
            samples: 2,
            lens_prompts: &prompts,
            lens_files: &files,
            lens_schema: &schema,
            generator_prompt: GENERATOR_PROMPT.to_owned(),
            pr_files: &pr_files,
            blast_radius: None,
        },
    )
    .await;

    // Two samples, four questions written, the near-repeat on sync.ts merged away.
    assert_eq!(out.generated, 4);
    let asked: Vec<&str> = out.questions.iter().map(|q| q.file.as_str()).collect();
    assert_eq!(
        asked,
        [
            "src/jobs/sync.ts",
            "src/web/Banner.tsx",
            "src/jobs/other.ts"
        ]
    );

    let calls = fake.calls();
    let gens: Vec<_> = calls
        .iter()
        .filter(|c| c.label.starts_with("questions:"))
        .collect();
    assert_eq!(gens.len(), 2);
    for g in &gens {
        assert_eq!(g.model.as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(g.prompt, GENERATOR_PROMPT);
        assert_eq!(g.system_prompt.as_deref(), Some("SHARED SYSTEM PROMPT"));
        assert_eq!(g.schema.as_ref(), Some(&schemas::scenario_questions()));
    }

    let answers: Vec<_> = calls
        .iter()
        .filter(|c| c.label.starts_with("question:"))
        .collect();
    assert_eq!(answers.len(), 3, "one dispatch per merged question");
    for a in &answers {
        assert_eq!(a.model.as_deref(), Some("claude-haiku-5-5"));
        assert_eq!(a.schema.as_ref(), Some(&schema));
        assert_eq!(a.tools, ["Read", "Bash"]);
    }
    let by_label = |l: &str| answers.iter().find(|c| c.label == l).expect("dispatched");
    let q1 = by_label("question:1");
    assert!(q1.prompt.starts_with("LENS ONE PROMPT"));
    assert!(q1.prompt.contains(ANSWER_PARAGRAPH));
    assert!(q1.prompt.ends_with("Question: src/jobs/sync.ts:40 If the provider returns 503 for an hour, does the job ever retry?\n"));
    assert!(
        by_label("question:2").prompt.starts_with("LENS TWO PROMPT"),
        "routed to the lens that has the file"
    );
    assert!(by_label("question:3").prompt.starts_with("LENS ONE PROMPT"));

    let warm = calls
        .iter()
        .find(|c| c.label == "cache-warmup")
        .expect("answers share a warm cache");
    assert_eq!(warm.model.as_deref(), Some("claude-haiku-5-5"));
    assert_eq!(warm.prompt, WARMUP_PROMPT);

    assert_eq!(out.answered, 3);
    assert_eq!(out.candidates.len(), 1);
    assert_eq!(out.dropped_out_of_diff, 1);
    let c = &out.candidates[0];
    assert_eq!(c["lens"], json!(QUESTION_LENS));
    assert_eq!(
        c["scenarioQuestion"],
        json!(
            "src/jobs/sync.ts:40 If the provider returns 503 for an hour, does the job ever retry?"
        )
    );

    let m = out.metrics();
    assert_eq!(m["samples"], json!(2));
    assert_eq!(m["answerModel"], json!("claude-haiku-5-5"));
    assert_eq!(m["generated"], json!(4));
    assert_eq!(m["asked"], json!(3));
    assert_eq!(m["answered"], json!(3));
    assert_eq!(m["candidates"], json!(1));
    assert_eq!(m["droppedOutOfDiff"], json!(1));
    let labels: Vec<&str> = out.dispatches.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(
        labels
            .iter()
            .filter(|l| l.starts_with("questions:"))
            .count(),
        2
    );
    assert_eq!(
        labels.iter().filter(|l| l.starts_with("question:")).count(),
        3
    );
    assert!(labels.contains(&"question-warmup"));
}

#[tokio::test]
async fn a_generator_that_returns_nothing_asks_nothing() {
    let fake = FakeAgent::new().answer(
        "questions:1",
        FakeAnswer::Structured(json!({"questions": []})),
    );
    let s = setup();
    let schema = schemas::findings();
    let out = question_stage(
        &fake,
        &QuestionInputs {
            lens_setup: &s,
            answer_model: "claude-haiku-5-5",
            samples: 1,
            lens_prompts: &["P".to_owned()],
            lens_files: &[None],
            lens_schema: &schema,
            generator_prompt: GENERATOR_PROMPT.to_owned(),
            pr_files: &[],
            blast_radius: None,
        },
    )
    .await;
    assert_eq!(out.questions.len(), 0);
    assert_eq!(
        fake.calls().len(),
        1,
        "no warm-up and no answer without questions"
    );
}

fn result(title: &str, lens: &str, merged: usize) -> serde_json::Value {
    json!({
        "survived": true,
        "finding": {
            "title": title, "file": "src/a.ts", "line": 3, "evidence": "x",
            "lens": lens, "mergedFrom": vec![lens; merged]
        }
    })
}

#[tokio::test]
async fn low_ranked_question_findings_move_out_of_the_verdict_and_lens_findings_never_do() {
    use tribunal_pipeline::questions::{rank_and_place, rank_prompt, LOWER_PRIORITY, RANK_PROMPT};
    let fake = FakeAgent::new().answer(
        "question-rank",
        FakeAnswer::Structured(json!({"ranking": [
            {"id": "1", "worth": 1}, {"id": "2", "worth": 1}, {"id": "3", "worth": 4},
            {"id": "4", "worth": 2}, {"id": "[5]", "worth": 1}
        ]})),
    );
    let mut surviving = vec![
        result("lens finding ranked low", "correctness", 0),
        result("question finding ranked low", QUESTION_LENS, 0),
        result("question finding ranked high", QUESTION_LENS, 0),
        result("question finding many raised", QUESTION_LENS, 5),
        result("question finding bracket id", QUESTION_LENS, 2),
    ];
    let mut aside = Vec::new();
    let prompt = rank_prompt("acme/web-app", "12", "abc123", &surviving);
    assert!(prompt.contains("[2] src/a.ts:3 | question finding ranked low"));
    let s = setup();
    let (_, summary) = rank_and_place(&fake, &s, prompt, &mut surviving, &mut aside).await;
    let kept: Vec<&str> = surviving
        .iter()
        .map(|r| r["finding"]["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        kept,
        [
            "lens finding ranked low",
            "question finding ranked high",
            "question finding many raised"
        ]
    );
    let moved: Vec<&str> = aside
        .iter()
        .map(|r| r["finding"]["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        moved,
        ["question finding ranked low", "question finding bracket id"]
    );
    assert!(aside
        .iter()
        .all(|r| r["placement"]["placement"] == json!(LOWER_PRIORITY)));
    assert_eq!(summary["ranked"], json!(5));
    assert_eq!(summary["movedToQuestionsToReview"], json!(2));
    let call = &fake.calls()[0];
    assert_eq!(call.system_prompt.as_deref(), Some(RANK_PROMPT));
    assert_eq!(call.model.as_deref(), Some("claude-sonnet-5-5"));
}

#[tokio::test]
async fn a_ranking_that_returns_nothing_moves_nothing() {
    use tribunal_pipeline::questions::{rank_and_place, rank_prompt};
    let fake = FakeAgent::new().answer(
        "question-rank",
        FakeAnswer::Structured(json!({"ranking": []})),
    );
    let mut surviving = vec![result("q", QUESTION_LENS, 0)];
    let mut aside = Vec::new();
    let prompt = rank_prompt("acme/web-app", "12", "abc123", &surviving);
    let s = setup();
    let (_, summary) = rank_and_place(&fake, &s, prompt, &mut surviving, &mut aside).await;
    assert_eq!(surviving.len(), 1);
    assert!(aside.is_empty());
    assert_eq!(summary["ranked"], json!(0));
}

#[tokio::test]
async fn a_kept_question_finding_below_worth_four_cannot_block() {
    use tribunal_pipeline::questions::{rank_and_place, rank_prompt, RANKED_BELOW_BLOCKING};
    use tribunal_pipeline::verdict_basis::verdict_basis;
    let fake = FakeAgent::new().answer(
        "question-rank",
        FakeAnswer::Structured(
            json!({"ranking": [{"id": "1", "worth": 3}, {"id": "2", "worth": 4}]}),
        ),
    );
    let mut surviving = vec![
        result("worth three", QUESTION_LENS, 0),
        result("worth four", QUESTION_LENS, 0),
    ];
    let mut aside = Vec::new();
    let prompt = rank_prompt("acme/web-app", "12", "abc123", &surviving);
    let s = setup();
    let (_, summary) = rank_and_place(&fake, &s, prompt, &mut surviving, &mut aside).await;
    assert_eq!(surviving.len(), 2, "worth 3 stays in the report");
    assert_eq!(surviving[0]["finding"][RANKED_BELOW_BLOCKING], json!(true));
    assert!(surviving[1]["finding"].get(RANKED_BELOW_BLOCKING).is_none());
    assert_eq!(summary["belowBlocking"], json!(1));
    let findings: Vec<serde_json::Value> = surviving
        .iter()
        .map(|r| {
            let mut f = r["finding"].clone();
            f["severity"] = json!("critical");
            f
        })
        .collect();
    let basis = verdict_basis(&findings).expect("the worth-four finding qualifies");
    assert!(basis.contains("worth four") && !basis.contains("worth three"));
}
