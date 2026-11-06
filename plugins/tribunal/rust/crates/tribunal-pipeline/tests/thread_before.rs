//! `threadBefore` records the `--thread-before` cutoff a run read the PR thread
//! up to, so two arms of a measurement can be checked for the same cutoff.

use serde_json::{json, Value};
use tribunal_pipeline::context::RunArgs;
use tribunal_pipeline::metrics::{metrics_row, MetricsScope};
use tribunal_pipeline::run_metrics::thread_before_of;

fn row_for(thread_before: &str) -> Value {
    let args = RunArgs {
        thread_before: thread_before.to_owned(),
        ..RunArgs::default()
    };
    let scope = MetricsScope {
        thread_before: thread_before_of(&args),
        ..MetricsScope::default()
    };
    metrics_row(&scope).unwrap()
}

#[test]
fn the_row_records_the_cutoff_the_run_was_given() {
    let row = row_for("2026-11-05T12:34:56Z");
    assert_eq!(row["threadBefore"], json!("2026-11-05T12:34:56Z"));
}

#[test]
fn a_run_without_a_cutoff_records_null() {
    assert_eq!(row_for("")["threadBefore"], Value::Null);
    assert_eq!(row_for("   ")["threadBefore"], Value::Null);
}

#[test]
fn the_field_sits_after_pinned() {
    let row = row_for("");
    let keys: Vec<&String> = row.as_object().unwrap().keys().collect();
    let at = keys.iter().position(|k| *k == "pinned").unwrap();
    assert_eq!(keys[at + 1], "threadBefore");
}
