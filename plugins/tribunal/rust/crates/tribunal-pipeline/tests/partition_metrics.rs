//! S11: the metrics row records the partition mode when one was chosen
//! (`groupBy` in the profile or `--partition`), churn included, and a default
//! row stays unchanged.
use serde_json::json;
use tribunal_pipeline::metrics::{metrics_row, MetricsScope};

#[test]
fn a_cohesion_run_records_its_partition_mode() {
    let s = MetricsScope {
        partition_mode: json!("cohesion"),
        ..MetricsScope::default()
    };
    let row = metrics_row(&s).unwrap();
    assert_eq!(row["partitionMode"], json!("cohesion"));
}

#[test]
fn a_churn_run_leaves_the_row_unchanged() {
    let row = metrics_row(&MetricsScope::default()).unwrap();
    assert!(row.get("partitionMode").is_none());
}

#[test]
fn a_chosen_churn_partition_is_recorded() {
    let s = MetricsScope {
        partition_mode: json!("churn"),
        ..MetricsScope::default()
    };
    let row = metrics_row(&s).unwrap();
    assert_eq!(row["partitionMode"], json!("churn"));
}
