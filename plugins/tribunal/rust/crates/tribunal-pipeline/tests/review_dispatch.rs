//! The reviewer dispatch: what each child is given, the order results come
//! back in, and the concurrency ceiling.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;
use tribunal_agent::{Agent, DispatchRequest, DispatchResult, Exit, FakeAgent, FakeAnswer};
use tribunal_pipeline::agent_def::AgentDef;
use tribunal_pipeline::review::{
    answer_of, bot_task, dispatch_all, dispatch_lenses, lens_task, warm_cache, ReviewerSetup,
    WARMUP_PROMPT,
};
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

/// Records when each request starts and ends. The warm-up takes longest.
struct Ordered {
    events: std::sync::Mutex<Vec<String>>,
}

impl Agent for Ordered {
    async fn run(&self, req: &DispatchRequest) -> DispatchResult {
        self.events
            .lock()
            .unwrap()
            .push(format!("start:{}", req.label));
        let ms = if req.label == "cache-warmup" { 60 } else { 5 };
        tokio::time::sleep(Duration::from_millis(ms)).await;
        self.events
            .lock()
            .unwrap()
            .push(format!("end:{}", req.label));
        DispatchResult {
            exit: Exit::Code(0),
            output: None,
            stderr_tail: String::new(),
            tool_calls: Vec::new(),
            transcript: None,
        }
    }
}

fn lens_tasks(n: usize) -> Vec<tribunal_pipeline::review::ReviewerTask> {
    (1..=n)
        .map(|i| lens_task(format!("lens:{i}"), format!("P{i}")))
        .collect()
}

#[tokio::test]
async fn the_warm_up_ends_before_any_lens_starts() {
    let agent = Ordered {
        events: std::sync::Mutex::new(Vec::new()),
    };
    let (results, warmup) = dispatch_lenses(&agent, &setup(4), &lens_tasks(3), true).await;
    let events = agent.events.lock().unwrap().clone();
    assert_eq!(&events[..2], ["start:cache-warmup", "end:cache-warmup"]);
    assert!(events[2..].iter().all(|e| e.contains("lens:")));
    assert_eq!(
        results.len(),
        3,
        "the warm-up is not among the lens results"
    );
    assert!(warmup.is_some());
}

#[tokio::test]
async fn the_warm_up_asks_what_a_lens_asks_before_its_user_message() {
    let fake = FakeAgent::new().otherwise(FakeAnswer::Structured(json!({"findings": []})));
    let setup = setup(4);
    let _ = dispatch_lenses(&fake, &setup, &lens_tasks(2), true).await;
    let calls = fake.calls();
    assert_eq!(calls.len(), 3);
    let (warm, lens) = (&calls[0], &calls[1]);
    assert_eq!(warm.label, "cache-warmup");
    assert_eq!(warm.prompt, WARMUP_PROMPT);
    assert_eq!(warm.system_prompt, lens.system_prompt);
    assert_eq!(warm.model, lens.model);
    assert_eq!(warm.effort, lens.effort);
    assert_eq!(warm.tools, lens.tools);
    assert_eq!(warm.schema, lens.schema);
    assert_eq!(warm.cwd, lens.cwd);
}

#[tokio::test]
async fn a_failed_warm_up_does_not_stop_the_lenses() {
    let fake = FakeAgent::new()
        .answer("cache-warmup", FakeAnswer::Exit(Exit::TimedOut))
        .otherwise(FakeAnswer::Structured(json!({"findings": []})));
    let (results, warmup) = dispatch_lenses(&fake, &setup(4), &lens_tasks(3), true).await;
    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|r| answer_of(r).is_some()));
    assert!(warmup.is_some());
}

#[tokio::test]
async fn no_warm_up_when_off_or_for_a_single_lens() {
    let fake = FakeAgent::new().otherwise(FakeAnswer::Structured(json!({"findings": []})));
    let (_, off) = dispatch_lenses(&fake, &setup(4), &lens_tasks(3), false).await;
    let (_, single) = dispatch_lenses(&fake, &setup(4), &lens_tasks(1), true).await;
    assert!(off.is_none() && single.is_none());
    assert!(fake.calls().iter().all(|c| c.label != "cache-warmup"));
}

#[tokio::test]
async fn warm_cache_alone_returns_the_result() {
    let fake = FakeAgent::new().otherwise(FakeAnswer::Structured(json!({"findings": []})));
    let r = warm_cache(&fake, &setup(4)).await;
    assert!(answer_of(&r).is_some());
}
