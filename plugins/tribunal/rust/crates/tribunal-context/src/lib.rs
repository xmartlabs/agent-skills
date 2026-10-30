//! The context phase of a review: the repo's profile, conventions and
//! methodology, the PR and its thread, the incremental baseline and the
//! dependents block. Each module ports one script from
//! `skills/review-pr/scripts/` or `lib/`, and its differential test runs the
//! script and the port against the same recorded `gh`.

pub mod assemble_methodology;
pub mod ast_bindings;
pub mod attention_facts;
pub mod audit_run;
pub mod blast_radius;
pub mod check_report_fidelity;
pub mod classify_artifacts;
pub mod classify_head_failure;
pub mod classify_verdict;
pub mod convention_sources;
pub mod exec;
pub mod fetch_pr_context;
pub mod grounded_typecheck;
pub mod harness_version;
pub mod head_worktree;
pub mod heads;
pub mod import_adjacency;
pub mod incremental_scope;
pub mod inflight;
pub mod js_engine;
pub mod ledger;
pub mod load_conventions;
pub mod load_profile;
pub mod operator;
pub mod persist_run;
pub mod post_footer;
pub mod post_review;
pub mod proc;
pub mod py_json;
pub mod reconcile_votes;
pub mod resolve_baseline;
pub mod resolve_evidence_line;
pub mod resolve_verified_anchors;
pub mod review_record;
pub mod section_diff;
pub mod typescript_fallback;
