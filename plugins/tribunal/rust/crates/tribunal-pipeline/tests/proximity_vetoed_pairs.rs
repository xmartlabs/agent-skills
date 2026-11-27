//! The dedup step can propose merging two candidates in the same file that
//! the proximity veto then keeps apart. Each such pair is recorded, in the
//! dedup result and the metrics row, so the veto can be judged on real pairs.

use serde_json::{json, Value};
use tribunal_pipeline::metrics::{metrics_row, MetricsScope};
use tribunal_pipeline::verify::apply_dedup;

fn candidate(line: u64, title: &str, lens: &str) -> Value {
    json!({"file": "src/services/login.ts", "line": line, "title": title, "lens": lens})
}

#[test]
fn a_far_and_dissimilar_pair_is_kept_apart_and_recorded() {
    let all = vec![
        candidate(
            169,
            "Promotion committed before the session is issued",
            "scenario-question",
        ),
        candidate(
            257,
            "Code spent ahead of token write loses the login",
            "correctness",
        ),
    ];
    let mut logs = Vec::new();
    let d = apply_dedup(&all, Some(&json!({"clusters": [[0, 1]]})), &mut logs).unwrap();
    assert_eq!(d.proximity_vetoed, 1);
    assert_eq!(d.panel_candidates.len(), 2);
    assert_eq!(
        d.proximity_vetoed_pairs,
        vec![json!({
            "file": "src/services/login.ts",
            "kept": {"line": 169, "title": "Promotion committed before the session is issued", "lens": "scenario-question"},
            "vetoed": {"line": 257, "title": "Code spent ahead of token write loses the login", "lens": "correctness"},
        })]
    );
}

#[test]
fn a_close_pair_is_merged_and_records_nothing() {
    let all = vec![
        candidate(
            169,
            "Promotion committed before the session is issued",
            "scenario-question",
        ),
        candidate(
            175,
            "Code spent ahead of token write loses the login",
            "correctness",
        ),
    ];
    let mut logs = Vec::new();
    let d = apply_dedup(&all, Some(&json!({"clusters": [[0, 1]]})), &mut logs).unwrap();
    assert_eq!(d.proximity_vetoed, 0);
    assert!(d.proximity_vetoed_pairs.is_empty());
    assert_eq!(d.panel_candidates.len(), 1);
}

#[test]
fn the_row_carries_the_pairs() {
    let pairs = json!([{"file": "a.ts", "kept": {"line": 1}, "vetoed": {"line": 90}}]);
    let row = metrics_row(&MetricsScope {
        proximity_vetoed_pairs: pairs.clone(),
        ..MetricsScope::default()
    })
    .unwrap();
    assert_eq!(row["proximityVetoedPairs"], pairs);
}
