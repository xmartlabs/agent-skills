use serde_json::{json, Value};
use tribunal_pipeline::synthesize::{prior_check, prior_re_examined, PRIOR_VERIFY_CEILING};

fn four_prior() -> Value {
    json!({"findings": [
        {"file": "a.ts", "title": "one", "severity": "important"},
        {"file": "a.ts", "title": "two", "severity": "important"},
        {"file": "a.ts", "title": "three", "severity": "nit"},
        {"file": "a.ts", "title": "four", "severity": "nit"},
    ]})
}

fn check(ceiling: usize) -> (tribunal_pipeline::synthesize::PriorCheck, Vec<String>) {
    let scope = json!({"unchanged": [], "gone": [], "changed": ["a.ts"], "added": []});
    let prior = four_prior();
    let mut logs = Vec::new();
    let c = prior_check(
        Some(&scope),
        Some(&prior),
        &[],
        &json!({}),
        ceiling,
        &mut logs,
    )
    .unwrap();
    (c, logs)
}

#[test]
fn default_ceiling_verifies_all_four() {
    let (c, logs) = check(PRIOR_VERIFY_CEILING);
    assert_eq!(c.to_verify.len(), 4);
    assert!(logs.iter().all(|l| !l.contains("capped")));
}

#[test]
fn a_lower_ceiling_verifies_that_many_and_reports_the_rest_unverified() {
    let (c, logs) = check(2);
    assert_eq!(c.to_verify.len(), 2);
    assert!(
        logs.iter().any(|l| l.contains("capped at 2 of 4")),
        "{logs:?}"
    );
    let verified: Vec<Value> = c.to_verify.clone();
    let mut relog = Vec::new();
    let all = prior_re_examined(&verified, &c, &mut relog);
    assert_eq!(all.len(), 4);
    let text = all[2]["verification"].to_string();
    assert!(text.contains("ceiling (2) was reached"), "{text}");
}

#[test]
fn a_higher_ceiling_lets_more_than_six_through() {
    let scope = json!({"unchanged": [], "gone": [], "changed": ["a.ts"], "added": []});
    let findings: Vec<Value> = (0..9)
        .map(|i| json!({"file": "a.ts", "title": format!("t{i}")}))
        .collect();
    let prior = json!({ "findings": findings });
    let mut logs = Vec::new();
    let c = prior_check(Some(&scope), Some(&prior), &[], &json!({}), 9, &mut logs).unwrap();
    assert_eq!(c.to_verify.len(), 9);
    let mut logs = Vec::new();
    let c = prior_check(Some(&scope), Some(&prior), &[], &json!({}), 6, &mut logs).unwrap();
    assert_eq!(c.to_verify.len(), 6);
}
