//! A finding the pre-decided check set aside carries a `REFUTED` vote with no
//! kill reason. Neither the row nor the posted body's gate asks the report for
//! one: the report lists the finding under "Pre-decided" (agent-skills #182).

use serde_json::{json, Value};
use tribunal_pipeline::metrics::{metrics_row, MetricsScope};
use tribunal_pipeline::post::fidelity_gaps;

fn vote(source: &str) -> Value {
    json!({"source": source, "verdict": "REFUTED", "killReason": null,
        "refutationType": "PRE_DECIDED", "reason": "the author answered this in the thread"})
}

fn result(source: &str) -> Value {
    json!({"survived": false, "panelRan": false, "votes": [vote(source)], "finding": {
        "title": "Old worker drops the job", "file": "src/jobs/worker.ts", "line": 12,
        "severity": "important", "lens": "correctness"}})
}

const REPORT: &str = "### Pre-decided (not re-litigated)\n- Old worker drops the job -> PRE-DECIDED: the author's reply covers it.";

#[test]
fn the_row_asks_no_kill_reason_of_a_pre_decided_finding() {
    let row = |r: Value| {
        let scope = MetricsScope {
            results: vec![r],
            report_as_posted: REPORT.to_owned(),
            ..MetricsScope::default()
        };
        metrics_row(&scope).unwrap()["findings"][0]["killReasonMissingFromReport"].clone()
    };
    assert_eq!(
        row(result(tribunal_pipeline::pre_decided::LABEL)),
        json!([])
    );
    assert_eq!(row(result("claude")), json!(["uncategorized"]));
}

#[test]
fn the_posted_body_is_not_patched_with_uncategorized() {
    let finding = |t: &str| json!({"title": "Old worker drops the job", "votes": [vote(t)]});
    let gaps = fidelity_gaps(&[finding(tribunal_pipeline::pre_decided::LABEL)], REPORT).unwrap();
    assert!(
        format!("{gaps:?}").find("uncategorized").is_none(),
        "{gaps:?}"
    );
    let gaps = fidelity_gaps(&[finding("claude")], REPORT).unwrap();
    assert!(format!("{gaps:?}").contains("uncategorized"), "{gaps:?}");
}
