//! The row's merged-location and merged-from checks hold a finding to what
//! the report prints for it (agent-skills #182): a question moved to
//! "Questions to review" is printed one line each, and a merged finding's
//! extra lines are printed with a shortened path.

use serde_json::{json, Value};
use tribunal_pipeline::metrics::{metrics_row, MetricsScope};

fn merged(title: &str) -> Value {
    json!({"survived": true, "panelRan": false, "finding": {
    "title": title, "file": "src/components/widgets.ts", "line": 212,
    "severity": "important", "lens": "scenario-question",
    "mergedFrom": ["correctness", "scenario-question"],
    "mergedLocations": [
        {"file": "src/components/widgets.ts", "line": 212},
        {"file": "src/components/widgets.ts", "line": 203}
    ]}})
}

fn row(results: Vec<Value>, report: &str) -> Value {
    let scope = MetricsScope {
        results,
        report_as_posted: report.to_owned(),
        ..MetricsScope::default()
    };
    metrics_row(&scope).unwrap()
}

#[test]
fn a_question_to_review_is_not_held_to_also_at_or_found_by() {
    let mut q = merged("Cache read races the write");
    q["placement"] = json!({"placement": "lower-priority-question", "worth": 2, "support": 1});
    let r = row(
        vec![q],
        "- Cache read races the write `widgets.ts:212` -> QUESTION TO REVIEW",
    );
    let f = &r["findings"][0];
    assert_eq!(f["mergedLocationsMissingFromReport"], json!([]));
    assert_eq!(f["mergedFromAnnotationMissingFromReport"], json!([]));
}

#[test]
fn a_reported_finding_still_needs_its_extra_line() {
    let r = row(
        vec![merged("Cache read races the write")],
        "- Cache read races the write `widgets.ts:212` (found by: correctness, scenario-question)",
    );
    assert_eq!(
        r["findings"][0]["mergedLocationsMissingFromReport"],
        json!(["src/components/widgets.ts:203"])
    );
    let r = row(
        vec![merged("Cache read races the write")],
        "- Cache read races the write `widgets.ts:212` (found by: correctness, scenario-question)\n  - also at: `widgets.ts:203`",
    );
    assert_eq!(
        r["findings"][0]["mergedLocationsMissingFromReport"],
        json!([])
    );
}
