//! Fields the engine adds to a kept finding after the panel reach the
//! metrics row, so a run can be read back without its report.

use serde_json::json;
use tribunal_pipeline::metrics::{metrics_row, MetricsScope};

#[test]
fn the_row_records_the_pre_existing_mark_and_each_sources_quote() {
    let marked = json!({"survived": true, "panelRan": false, "finding": {
    "title": "a", "file": "src/a.ts", "line": 3, "severity": "important",
    "lens": "correctness", "preExistingCode": true,
    "mergedFrom": ["correctness", "security"],
    "evidence": "[correctness] const old = legacy(x);\n[security] const tail = 2;",
    "evidenceBySource": [
        {"lens": "correctness", "evidence": "const old = legacy(x);"},
        {"lens": "security", "evidence": "const tail = 2;"}
    ]}});
    let plain = json!({"survived": true, "panelRan": false, "finding": {
        "title": "b", "file": "src/b.ts", "line": 4, "severity": "important",
        "lens": "correctness", "evidence": "const added = compute(x);"}});
    let scope = MetricsScope {
        results: vec![marked, plain],
        ..MetricsScope::default()
    };
    let row = metrics_row(&scope).unwrap();
    let f = row["findings"].as_array().unwrap();
    assert_eq!(f[0]["preExistingCode"], json!(true));
    assert_eq!(f[0]["blocking"], json!(false));
    assert_eq!(
        f[0]["evidenceBySource"][1],
        json!({"lens": "security", "evidence": "const tail = 2;"})
    );
    assert!(f[1].get("preExistingCode").is_none());
    assert!(f[1].get("evidenceBySource").is_none());
}
