//! The reviewer dispatch: what each child is given, the order results come
//! back in, and the concurrency ceiling.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;
use tribunal_agent::{Agent, DispatchRequest, DispatchResult, Exit, FakeAgent, FakeAnswer};
use tribunal_pipeline::agent_def::AgentDef;
use tribunal_pipeline::review::{answer_of, bot_task, dispatch_all, lens_task, ReviewerSetup};
use tribunal_pipeline::schemas;

fn setup(concurrency: usize) -> ReviewerSetup {
    ReviewerSetup {
        agent: AgentDef {
            name: "code-review-expert".into(),
            model: Some("claude-sonnet-5-5".into()),
            tools: vec!["Read".into(), "Bash".into()],
            body: "You review code.".into(),
        },
        model: "claude-sonnet-5-5".into(),
        cwd: PathBuf::from("/tmp/wt"),
        timeout: Duration::from_secs(90),
        concurrency,
        effort: None,
    }
}

#[tokio::test]
async fn every_reviewer_gets_the_definition_the_model_and_its_schema() {
    let fake = FakeAgent::new()
        .answer(
            "lens:1",
            FakeAnswer::Structured(json!({"findings": [], "1": "x", "a": "y"})),
        )
        .answer("bot-vetting", FakeAnswer::Exit(Exit::TimedOut));
    let tasks = [
        lens_task("lens:1".into(), "P1".into()),
        bot_task("P2".into()),
    ];
    let results = dispatch_all(&fake, &setup(4), &tasks).await;
    let calls = fake.calls();
    assert_eq!(calls.len(), 2);
    for (call, (label, prompt, schema)) in calls.iter().zip([
        ("lens:1", "P1", schemas::findings()),
        ("bot-vetting", "P2", schemas::bot_vetting()),
    ]) {
        assert_eq!(call.label, label);
        assert_eq!(call.prompt, prompt);
        assert_eq!(call.system_prompt.as_deref(), Some("You review code."));
        assert_eq!(call.model.as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(call.tools, ["Read", "Bash"]);
        assert_eq!(call.cwd, PathBuf::from("/tmp/wt"));
        assert_eq!(call.timeout, Duration::from_secs(90));
        assert_eq!(call.schema.as_ref(), Some(&schema));
    }
    let lens = answer_of(&results[0]).unwrap();
    assert_eq!(
        lens.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["1", "findings", "a"],
        "an answer's keys come back in JavaScript's order"
    );
    assert_eq!(answer_of(&results[1]), None);
}

struct Slow {
    in_flight: AtomicUsize,
    peak: AtomicUsize,
}

impl Agent for Slow {
    async fn run(&self, req: &DispatchRequest) -> DispatchResult {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        let delay = 30 - req.label.len() as u64;
        tokio::time::sleep(Duration::from_millis(delay)).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        DispatchResult {
            exit: Exit::Code(7),
            output: None,
            stderr_tail: req.label.clone(),
            tool_calls: Vec::new(),
            transcript: None,
        }
    }
}

#[tokio::test]
async fn results_keep_task_order_under_the_concurrency_ceiling() {
    let slow = Slow {
        in_flight: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
    };
    let tasks: Vec<_> = (0..9)
        .map(|i| lens_task(format!("lens:{}{}", i, "x".repeat(i)), String::from("p")))
        .collect();
    let results = dispatch_all(&slow, &setup(3), &tasks).await;
    let labels: Vec<_> = results.iter().map(|r| r.stderr_tail.clone()).collect();
    let expected: Vec<_> = tasks.iter().map(|t| t.label.clone()).collect();
    assert_eq!(labels, expected);
    assert_eq!(slow.peak.load(Ordering::SeqCst), 3);
}
