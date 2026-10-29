//! The evidence group: the grounded typecheck oracle, the evidence resolver's
//! line correction, the ref-evidence audit, and the PR context and prior
//! reviews the post gate reads.

pub mod fetch_pr_context;
pub mod js_ext;
pub mod line_correction;
pub mod prior_reviews;
pub mod ref_evidence_check;
pub mod tsc_oracle;
