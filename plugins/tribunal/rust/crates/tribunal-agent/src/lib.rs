//! The boundary to the model: one dispatch, one child process.
//!
//! Every judgement in a review (a lens, a refuter vote, the critic, the
//! report) is a [`DispatchRequest`] handed to an [`Agent`]. [`ClaudeAgent`]
//! runs it as `claude -p` on the operator's own login; [`FakeAgent`] answers
//! from a script so the pipeline can be tested without spending a token.
//!
//! The shape follows the dispatch layer of the sibling Rust suite on purpose
//! (an agent trait, a request, a result that reports how the child ended), so
//! one implementation can replace the other later.

mod claude;
mod fake;
mod result;

use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

pub use claude::{ClaudeAgent, Isolation};
pub use fake::{FakeAgent, FakeAnswer};
pub use result::{CliResult, ModelUsage};

/// Runs one dispatch. The future is `Send` so a review can fan dispatches
/// out on a multi-threaded runtime.
pub trait Agent: Send + Sync {
    fn run(&self, req: &DispatchRequest) -> impl Future<Output = DispatchResult> + Send;
}

/// Where the child's model credentials come from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Credentials {
    /// The CLI's own login: the operator's subscription. An inherited
    /// `ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN` is removed from the child,
    /// because the CLI ranks either above the login.
    #[default]
    Session,
    /// A key the operator chose for this run.
    ApiKey(String),
}

/// One dispatch of one role.
#[derive(Debug, Clone, PartialEq)]
pub struct DispatchRequest {
    /// Shown in logs and used to name the log file, e.g. `lens:2`.
    pub label: String,
    /// The agent definition's body. `None` keeps the CLI's default prompt.
    pub system_prompt: Option<String>,
    pub prompt: String,
    /// A full model id. `None` leaves it to the CLI's default.
    pub model: Option<String>,
    /// `low`, `medium`, `high`, `xhigh` or `max`. `None` leaves the default.
    pub effort: Option<String>,
    /// The only tools the child gets, all pre-approved. Empty means none.
    pub tools: Vec<String>,
    /// A JSON Schema the answer must satisfy; the CLI re-prompts on a
    /// mismatch and returns the object as `structured_output`.
    pub schema: Option<Value>,
    /// The child's working directory: the head worktree for a reviewer.
    pub cwd: PathBuf,
    pub timeout: Duration,
    /// Load the operator's own settings, plugins and skills for this
    /// dispatch even when the agent isolates its children: the steps the
    /// orchestrating session did itself, such as composing the posted review.
    pub inherit_settings: bool,
}

impl DispatchRequest {
    #[must_use]
    pub fn new(
        label: impl Into<String>,
        prompt: impl Into<String>,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        Self {
            label: label.into(),
            system_prompt: None,
            prompt: prompt.into(),
            model: None,
            effort: None,
            tools: Vec::new(),
            schema: None,
            cwd: cwd.into(),
            inherit_settings: false,
            timeout: Duration::from_secs(30 * 60),
        }
    }
}

/// What the child did. Whether its answer is any good is the caller's call.
#[derive(Debug, Clone, PartialEq)]
pub struct DispatchResult {
    pub exit: Exit,
    /// The CLI's result object, when it printed one.
    pub output: Option<CliResult>,
    /// The tail of the child's stderr, for a failure message.
    pub stderr_tail: String,
    /// Every tool the child called, in order.
    pub tool_calls: Vec<String>,
    /// Where the child's event stream was saved, when it was.
    pub transcript: Option<PathBuf>,
}

impl DispatchResult {
    /// The schema-validated answer, when the dispatch succeeded with one.
    #[must_use]
    pub fn structured(&self) -> Option<&Value> {
        match (&self.exit, &self.output) {
            (Exit::Code(0), Some(o)) if !o.is_error => o.structured_output.as_ref(),
            _ => None,
        }
    }

    /// The free-text answer, when the dispatch succeeded with one.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        match (&self.exit, &self.output) {
            (Exit::Code(0), Some(o)) if !o.is_error => o.result.as_deref(),
            _ => None,
        }
    }

    /// Token use per model, empty when the child printed no result.
    #[must_use]
    pub fn model_usage(&self) -> BTreeMap<String, ModelUsage> {
        self.output
            .as_ref()
            .map(|o| o.model_usage.clone())
            .unwrap_or_default()
    }
}

/// How the child ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    TimedOut,
    /// A signal ended it, or its status could not be read.
    Killed(Option<i32>),
    /// No child ran, and why. Never a status of the CLI's own.
    SpawnFailed(String),
}
