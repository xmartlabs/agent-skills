//! The published schemas are the workflow's own constants, key order included,
//! and the harvest rules are its step two.
//!
//! The workflow's side is read from `tests/goldens/span/diff_schemas.json`:
//! each constant as the span oracle evaluated it, and the step-two text,
//! with a SHA-256 of the workflow code each came from (see `common`).
//! Regenerate after changing them:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-pipeline --test diff_schemas`.

mod common;

use common::{canon, recorded_value, span, Span};
use serde_json::json;
use tribunal_context::harness_version::sha256_hex;
use tribunal_pipeline::schemas;

fn constant(name: &str) -> serde_json::Value {
    let start = format!("const {name} = {{");
    span(
        &Span {
            start: &start,
            end: "\n};",
            deps: &[],
            pre: &[],
            fns: &[],
            ret: name,
        },
        &[serde_json::json!({})],
    )
    .remove(0)
}

/// Declared difference: the port asks each `notReached` entry for its kind,
/// `{surface, file, kind}`, where the Workflow takes a plain string. The rest
/// of the schema is the Workflow's.
#[test]
fn findings_schema_is_the_workflows_but_for_not_reached() {
    let mut ours = schemas::findings();
    let theirs = constant("FINDINGS_SCHEMA");
    let item = &ours["properties"]["notReached"]["items"];
    assert_eq!(item["required"], serde_json::json!(["surface", "kind"]));
    assert_eq!(
        theirs["properties"]["notReached"]["items"],
        serde_json::json!({"type": "string"})
    );
    ours["properties"]["notReached"] = theirs["properties"]["notReached"].clone();
    assert_eq!(canon(&ours), canon(&theirs));
}

#[test]
fn bot_vetting_schema_is_the_workflows() {
    assert_eq!(
        canon(&schemas::bot_vetting()),
        canon(&constant("BOT_VETTING_SCHEMA"))
    );
}

#[test]
fn thread_harvest_schema_takes_the_workflows_two_fields() {
    let ours = schemas::thread_harvest();
    let decisions = span(
        &Span {
            start: "const PR_CONTEXT_SCHEMA = {",
            end: "\n};",
            deps: &[],
            pre: &[],
            fns: &[],
            ret: "PR_CONTEXT_SCHEMA.properties.inThreadDecisions",
        },
        &[serde_json::json!({})],
    )
    .remove(0);
    let summaries = span(
        &Span {
            start: "const FETCH_PR_SCHEMA = {",
            end: "\n};",
            deps: &[],
            pre: &[],
            fns: &[],
            ret: "FETCH_PR_SCHEMA.properties.priorReviewSummaries",
        },
        &[serde_json::json!({
            "RELAY_FILE_SCHEMA": {"properties": {}},
            "PR_CONTEXT_SCHEMA": {"properties": {"inThreadDecisions": null}},
        })],
    )
    .remove(0);
    assert_eq!(
        canon(&ours["properties"]["inThreadDecisions"]),
        canon(&decisions)
    );
    assert_eq!(
        canon(&ours["properties"]["priorReviewSummaries"]),
        canon(&summaries)
    );
}

const HARVEST: (&str, &str) = ("It is the PR's human text:", "must not be conflated.");

/// The workflow's step two, from its first sentence to its last.
fn harvest_text(src: &str) -> String {
    let start = src.find(HARVEST.0).expect("the harvest text moved");
    let end = src[start..]
        .find(HARVEST.1)
        .expect("the harvest text moved")
        + start
        + HARVEST.1.len();
    src[start..end].replace("\\`", "`")
}

#[test]
fn the_harvest_rules_are_the_workflows_step_two() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workflow = std::fs::read_to_string(root.join("../../../workflow-src/tribunal.js")).ok();
    let now = workflow
        .as_deref()
        .map(|w| sha256_hex(harvest_text(w).as_bytes()));
    let theirs = recorded_value(
        "harvestRules",
        &[("workflow-src/tribunal.js", now)],
        &json!(HARVEST),
        || json!(harvest_text(workflow.as_deref().unwrap())),
    );
    let ours = std::fs::read_to_string(root.join("src/harvest-rules.txt")).unwrap();
    assert_eq!(ours.trim_end(), theirs.as_str().unwrap());
}

fn constant_with(name: &str, pre: &[(&str, &str)]) -> serde_json::Value {
    let start = format!("const {name} = {{");
    span(
        &Span {
            start: &start,
            end: "\n};",
            deps: &[],
            pre,
            fns: &[],
            ret: name,
        },
        &[serde_json::json!({})],
    )
    .remove(0)
}

#[test]
fn panel_schemas_are_the_workflows() {
    let refutation = ("const REFUTATION_SCHEMA = {", "\n};");
    let findings = ("const FINDINGS_SCHEMA = {", "\n};");
    for (ours, name) in [
        (schemas::refutation(), "REFUTATION_SCHEMA"),
        (schemas::strengthen(), "STRENGTHEN_SCHEMA"),
        (schemas::adjudicator(), "ADJUDICATOR_SCHEMA"),
        (schemas::panel_tiebreak(), "PANEL_TIEBREAK_SCHEMA"),
        (schemas::dedup(), "DEDUP_SCHEMA"),
    ] {
        assert_eq!(canon(&ours), canon(&constant(name)), "{name}");
    }
    assert_eq!(
        canon(&schemas::batch_refutation()),
        canon(&constant_with("BATCH_REFUTATION_SCHEMA", &[refutation]))
    );
    assert_eq!(
        canon(&schemas::critic()),
        canon(&constant_with("CRITIC_SCHEMA", &[findings]))
    );
}
