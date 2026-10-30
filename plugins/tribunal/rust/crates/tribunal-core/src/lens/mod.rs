//! The lens group: what a review lens is given (its diff sections, the
//! dependents and attention blocks, its prompt) and what the run records about
//! it afterwards (budget gaps, yield, the out-of-diff gate).

pub mod coverage_gaps;
pub mod dependents_block;
pub mod diff_sectioning;
pub mod diff_usable;
pub mod js_ext;
pub mod lens_budget;
pub mod lens_prompt;
pub mod lens_yield;
pub mod out_of_diff;
