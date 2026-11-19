//! S11: the metrics row records the cohesion partition, and only it.
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
