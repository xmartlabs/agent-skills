//! A scripted agent: answers by label so the pipeline can be tested
//! without a model, and records every request it was given.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;

use crate::result::CliResult;
use crate::{Agent, DispatchRequest, DispatchResult, Exit};

/// What the fake answers for one label.
#[derive(Debug, Clone, PartialEq)]
pub enum FakeAnswer {
    /// A successful dispatch whose schema-validated answer is this value.
    Structured(Value),
    /// A successful dispatch that answered in plain text.
    Text(String),
    /// The child ended this way and printed no result.
    Exit(Exit),
}

#[derive(Debug, Default)]
pub struct FakeAgent {
    answers: HashMap<String, FakeAnswer>,
    /// Answer for a label with no entry. `None` makes it a spawn failure, so
    /// a test that forgot a label fails loudly.
    fallback: Option<FakeAnswer>,
    calls: Mutex<Vec<DispatchRequest>>,
}

impl FakeAgent {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn answer(mut self, label: impl Into<String>, a: FakeAnswer) -> Self {
        self.answers.insert(label.into(), a);
        self
    }

    #[must_use]
    pub fn otherwise(mut self, a: FakeAnswer) -> Self {
        self.fallback = Some(a);
        self
    }

    /// Every request received, in order.
    pub fn calls(&self) -> Vec<DispatchRequest> {
        self.calls.lock().expect("fake agent lock").clone()
    }
}

fn success(structured: Option<Value>, text: Option<String>) -> CliResult {
    CliResult {
        subtype: "success".into(),
        is_error: false,
        result: text,
        structured_output: structured,
        num_turns: 1,
        session_id: None,
        duration_ms: 0,
        total_cost_usd: 0.0,
        model_usage: Default::default(),
        permission_denials: Vec::new(),
    }
}

impl Agent for FakeAgent {
    async fn run(&self, req: &DispatchRequest) -> DispatchResult {
        self.calls
            .lock()
            .expect("fake agent lock")
            .push(req.clone());
        let answer = self
            .answers
            .get(&req.label)
            .or(self.fallback.as_ref())
            .cloned();
        let (exit, output) = match answer {
            Some(FakeAnswer::Structured(v)) => (
                Exit::Code(0),
                Some(success(Some(v.clone()), Some(v.to_string()))),
            ),
            Some(FakeAnswer::Text(t)) => (Exit::Code(0), Some(success(None, Some(t)))),
            Some(FakeAnswer::Exit(e)) => (e, None),
            None => (
                Exit::SpawnFailed(format!("fake agent has no answer for {}", req.label)),
                None,
            ),
        };
        DispatchResult {
            exit,
            output,
            stderr_tail: String::new(),
            tool_calls: Vec::new(),
            transcript: None,
        }
    }
}
