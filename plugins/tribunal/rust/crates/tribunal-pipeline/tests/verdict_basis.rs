//! The line under the verdict that says which findings caused a REQUEST
//! CHANGES. It must never disagree with `verdict_for`, which stays the only
//! thing that decides the verdict.

use serde_json::{json, Value};
use tribunal_core::report::carry_forward::verdict_for;
use tribunal_pipeline::synthesize::Report;
use tribunal_pipeline::verdict_basis::{verdict_basis, with_verdict_basis};

fn finding(severity: &str, behavior: bool, risk: bool) -> Value {
    json!({
        "title": format!("{severity} finding"),
        "severity": severity,
        "isBehaviorChanging": behavior,
        "touchesHighRiskSurface": risk,
    })
}

#[test]
fn a_critical_finding_is_named() {
    let b = verdict_basis(&[finding("critical", false, false)]).unwrap();
    assert!(
        b.starts_with("Why REQUEST CHANGES: 1 finding qualify"),
        "{b}"
    );
    assert!(b.contains("\"critical finding\" (critical)"), "{b}");
}

#[test]
fn an_important_finding_says_which_flag_qualified_it() {
    let b = verdict_basis(&[finding("important", true, false)]).unwrap();
    assert!(b.contains("(changes behavior)"), "{b}");
    let b = verdict_basis(&[finding("important", false, true)]).unwrap();
    assert!(b.contains("(touches a high-risk surface)"), "{b}");
    let b = verdict_basis(&[finding("important", true, true)]).unwrap();
    assert!(
        b.contains("(changes behavior, touches a high-risk surface)"),
        "{b}"
    );
}

#[test]
fn a_flag_raised_by_a_risk_path_is_named_as_that() {
    let mut f = finding("important", false, true);
    f["riskPathSurface"] = json!(true);
    let b = verdict_basis(&[f]).unwrap();
    assert!(
        b.contains("(its file is on a risk path of the profile)"),
        "{b}"
    );
}

#[test]
fn findings_that_do_not_qualify_leave_no_line() {
    assert_eq!(verdict_basis(&[]), None);
    assert_eq!(verdict_basis(&[finding("important", false, false)]), None);
    assert_eq!(verdict_basis(&[finding("nit", true, true)]), None);
}

#[test]
fn findings_the_verdict_skips_are_skipped_here_too() {
    let mut gated = finding("critical", true, true);
    gated["verifiedByGateNote"] = json!("not cited");
    let mut unadjudicated = finding("important", true, true);
    unadjudicated["adjudicated"] = json!(false);
    assert_eq!(verdict_basis(&[gated, unadjudicated]), None);
}

#[test]
fn many_findings_are_counted_and_long_titles_cut() {
    let mut all: Vec<Value> = (0..6).map(|_| finding("important", true, false)).collect();
    all[0]["title"] = json!("x".repeat(200));
    let b = verdict_basis(&all).unwrap();
    assert!(
        b.starts_with("Why REQUEST CHANGES: 6 findings qualify"),
        "{b}"
    );
    assert!(b.contains("; and 2 more."), "{b}");
    assert!(b.contains(&format!("\"{}\"", "x".repeat(80))), "{b}");
    assert!(!b.contains(&"x".repeat(81)), "{b}");
}

#[test]
fn the_basis_exists_exactly_when_verdict_for_asks_for_changes() {
    let cases: Vec<Vec<Value>> = vec![
        vec![],
        vec![finding("nit", false, false)],
        vec![finding("important", false, false)],
        vec![finding("important", true, false)],
        vec![finding("important", false, true)],
        vec![finding("critical", false, false)],
        vec![
            finding("important", false, false),
            finding("nit", true, true),
        ],
        vec![
            finding("important", false, false),
            finding("important", true, false),
        ],
    ];
    for c in cases {
        let asks = verdict_for(Some(&c), Some(&[])) == "REQUEST CHANGES";
        assert_eq!(verdict_basis(&c).is_some(), asks, "{c:?}");
    }
}

fn report(text: &str) -> Report {
    Report {
        report: text.to_owned(),
        corrected: None,
        as_posted: text.to_owned(),
    }
}

#[test]
fn the_basis_goes_right_under_the_verdict_line() {
    let r = with_verdict_basis(
        report(
            "# Review\n\n### Verdict suggestion: REQUEST CHANGES\n\nSummary text.\n\n### Blocking",
        ),
        "Why REQUEST CHANGES: x.",
    );
    assert_eq!(
        r.report,
        "# Review\n\n### Verdict suggestion: REQUEST CHANGES\n\nWhy REQUEST CHANGES: x.\n\nSummary text.\n\n### Blocking"
    );
}

#[test]
fn the_posted_body_is_recomputed_without_the_draft_comments() {
    let r = with_verdict_basis(
        report("### Verdict suggestion: REQUEST CHANGES\n\nSummary.\n\n### Draft comments\n\nsecret draft"),
        "Why REQUEST CHANGES: x.",
    );
    assert!(r.report.contains("Why REQUEST CHANGES: x."));
    assert!(r.report.contains("secret draft"));
    assert!(r.as_posted.contains("Why REQUEST CHANGES: x."));
    assert!(!r.as_posted.contains("secret draft"));
}

#[test]
fn a_report_with_no_verdict_line_is_left_alone() {
    let before = report("No verdict heading here.");
    assert_eq!(with_verdict_basis(before.clone(), "Why: x."), before);
}
