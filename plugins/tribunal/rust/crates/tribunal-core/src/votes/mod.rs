//! The vote regions: deriving external votes from their artifacts, checking
//! vote quotes, the clean-verdict sweep, the panel-ceiling tie-break, and the
//! small gates around them.

pub mod artifact_truth;
pub mod availability_check_gate;
pub mod clean_sweep;
pub mod dedup_veto;
pub mod evidence_resolver_degrade;
pub mod js_ext;
pub mod panel_ceiling_tiebreak;
pub mod vote_quotes;
