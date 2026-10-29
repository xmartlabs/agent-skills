//! The tribunal review engine: the pipeline's decision logic, ported from the
//! marked regions of `workflow-src/tribunal.js`. Each module names the region
//! it replaces, and `tests/differential.rs` runs both on the same inputs.

pub mod artifact_key;
pub mod evidence;
pub mod js;
pub mod lens;
pub mod panel;
pub mod profile;
pub mod report;
pub mod report_verdict;
pub mod verified_gate;
pub mod votes;
