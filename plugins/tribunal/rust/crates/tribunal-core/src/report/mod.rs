//! The report group: what the posted review must contain and how the previous
//! review's findings carry into it. One module per region of
//! `workflow-src/tribunal.js`; `js_ext` holds the JavaScript value semantics
//! they share beyond `crate::js`.

pub mod carry_forward;
pub mod draft_comments_strip;
pub mod js_ext;
pub mod merged_locations_check;
pub mod report_fidelity_checks;
