//! The published schemas are the workflow's own constants, key order included.

mod common;

use common::{canon, span, Span};
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

#[test]
fn findings_schema_is_the_workflows() {
    assert_eq!(
        canon(&schemas::findings()),
        canon(&constant("FINDINGS_SCHEMA"))
    );
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

#[test]
fn the_harvest_rules_are_the_workflows_step_two() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(root.join("../../../workflow-src/tribunal.js")).unwrap();
    let start = src
        .find("It is the PR's human text:")
        .expect("the harvest text moved");
    let end = src[start..]
        .find("must not be conflated.")
        .expect("the harvest text moved")
        + start
        + "must not be conflated.".len();
    let theirs = src[start..end].replace("\\`", "`");
    let ours = std::fs::read_to_string(root.join("src/harvest-rules.txt")).unwrap();
    assert_eq!(ours.trim_end(), theirs);
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
