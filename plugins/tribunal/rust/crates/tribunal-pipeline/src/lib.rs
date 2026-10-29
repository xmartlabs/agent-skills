//! The review pipeline: what the workflow's phases do between the decision
//! regions, run natively. Context modules come from `tribunal-context`, the
//! decisions from `tribunal-core`, and every model call goes through
//! `tribunal-agent`.

pub mod agent_def;
pub mod context;
pub mod decisions;
pub mod live;
pub mod metrics;
pub mod panel;
pub mod panel_prompts;
pub mod persist;
pub mod plan;
pub mod post;
pub mod prompts;
pub mod review;
pub mod run;
pub mod run_metrics;
pub mod schemas;
pub mod selfcheck;
pub mod synthesize;
pub mod verify;
