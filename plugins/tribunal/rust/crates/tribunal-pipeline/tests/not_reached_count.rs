//! `lensNotReachedCount` counts every `notReached` entry the lenses filed,
//! whatever its kind. `lensBudgetSkips` holds the same number under an older
//! name that reads as budget stops only.

use serde_json::json;
use tribunal_pipeline::metrics::{metrics_row, MetricsScope};

fn row_with(entries: Vec<serde_json::Value>) -> serde_json::Value {
    let scope = MetricsScope {
        lens_not_reached: entries,
        ..MetricsScope::default()
    };
    metrics_row(&scope).unwrap()
}

#[test]
fn the_count_includes_every_kind_not_only_budget() {
    let row = row_with(vec![
        json!("the job wiring (budget)"),
        json!("running the suite (needs-execution)"),
        json!("the FE consumer (outside-pr)"),
    ]);
    assert_eq!(row["lensNotReachedCount"], json!(3));
    assert_eq!(row["lensBudgetSkips"], json!(3));
}

#[test]
fn an_empty_list_counts_zero() {
    let row = row_with(Vec::new());
    assert_eq!(row["lensNotReachedCount"], json!(0));
}

#[test]
fn the_new_field_sits_after_the_old_one() {
    let row = row_with(Vec::new());
    let keys: Vec<&String> = row.as_object().unwrap().keys().collect();
    let at = keys.iter().position(|k| *k == "lensBudgetSkips").unwrap();
    assert_eq!(keys[at + 1], "lensNotReachedCount");
}
