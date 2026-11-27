//! `needs_human` turns a run's coverage facts into the flag a person reads.

use serde_json::{json, Value};
use tribunal_core::profile::needs_human::{NeedsHumanConfig, SeverityFloor, Signal};
use tribunal_pipeline::needs_human::{
    meets_floor, needs_human, needs_human_line, with_needs_human_line, NeedsHuman, NeedsHumanFacts,
    Reason,
};
use tribunal_pipeline::synthesize::Report;
use tribunal_pipeline::verdict_basis::with_verdict_basis;

fn facts() -> NeedsHumanFacts<'static> {
    NeedsHumanFacts {
        lenses_unreturned: &[],
        lens_count: 4,
        unread_risk_files: &[],
        touched_risk_files: &[],
        typecheck_applies: true,
        typecheck_text: Some("0 errors"),
        panel_skipped_for_budget: &[],
        cost_breaker_fired: None,
        results: &[],
        critic_not_checked: &[],
        incremental_fallback: None,
    }
}

fn result(severity: &str, survived: bool, unverified: bool, refute_fraction: Option<f64>) -> Value {
    json!({
        "finding": {"severity": severity},
        "survived": survived,
        "panelRan": true,
        "unverified": unverified,
        "borderline": false,
        "refuteFraction": refute_fraction,
        "votes": [],
        "discardedVotes": [],
        "failedSlots": [],
    })
}

fn reasons(facts: &NeedsHumanFacts, config: &NeedsHumanConfig) -> Value {
    needs_human(facts, config).to_value()["reasons"].clone()
}

fn with(signals: &[Signal]) -> NeedsHumanConfig {
    NeedsHumanConfig {
        signals: signals.iter().copied().collect(),
        ..NeedsHumanConfig::default()
    }
}

#[test]
fn a_complete_run_stays_quiet_under_the_default_set() {
    let nh = needs_human(&facts(), &NeedsHumanConfig::default());
    assert!(!nh.raised);
    assert!(nh.reasons.is_empty());
    assert_eq!(nh.to_value(), json!({"raised": false, "reasons": []}));
}

#[test]
fn an_empty_lens_raises_it() {
    let lenses = [json!({"lens": "security", "files": ["src/widgets.ts"]})];
    let nh = needs_human(
        &NeedsHumanFacts {
            lenses_unreturned: &lenses,
            ..facts()
        },
        &NeedsHumanConfig::default(),
    );
    assert!(nh.raised);
    assert_eq!(
        nh.to_value()["reasons"],
        json!([{"signal": "lens-empty", "detail": "1 of 4 lens(es) returned nothing"}])
    );
}

#[test]
fn an_unread_risk_path_names_its_files() {
    let files = ["db/migrations/001.sql".to_owned(), "src/auth.ts".to_owned()];
    let r = reasons(
        &NeedsHumanFacts {
            unread_risk_files: &files,
            ..facts()
        },
        &NeedsHumanConfig::default(),
    );
    assert_eq!(
        r,
        json!([{"signal": "risk-path-unread", "detail": "no lens read db/migrations/001.sql, src/auth.ts"}])
    );
}

#[test]
fn a_degraded_typecheck_names_its_code() {
    let r = reasons(
        &NeedsHumanFacts {
            typecheck_text: Some("DEGRADED_NO_PINNED_TREE: no tree at the head"),
            ..facts()
        },
        &NeedsHumanConfig::default(),
    );
    assert_eq!(
        r,
        json!([{"signal": "typecheck-not-run", "detail": "DEGRADED_NO_PINNED_TREE"}])
    );
}

#[test]
fn a_typecheck_that_does_not_apply_stays_quiet() {
    let nh = needs_human(
        &NeedsHumanFacts {
            typecheck_applies: false,
            typecheck_text: Some("DEGRADED_NO_PINNED_TREE: no tree at the head"),
            ..facts()
        },
        &NeedsHumanConfig::default(),
    );
    assert!(!nh.raised);
}

#[test]
fn a_touched_risk_path_raises_it_even_when_read() {
    let files = ["db/migrations/001.sql".to_owned()];
    let r = reasons(
        &NeedsHumanFacts {
            touched_risk_files: &files,
            ..facts()
        },
        &NeedsHumanConfig::default(),
    );
    assert_eq!(
        r,
        json!([{"signal": "risk-path-touched", "detail": "changes db/migrations/001.sql"}])
    );
}

#[test]
fn risk_path_details_name_at_most_five_files() {
    let files: Vec<String> = (1..=8).map(|i| format!("db/m{i}.sql")).collect();
    let five = &files[..5];
    let r = reasons(
        &NeedsHumanFacts {
            unread_risk_files: &files,
            touched_risk_files: &files,
            ..facts()
        },
        &NeedsHumanConfig::default(),
    );
    assert_eq!(
        r,
        json!([
            {"signal": "risk-path-unread", "detail": "no lens read db/m1.sql, db/m2.sql, db/m3.sql, db/m4.sql, db/m5.sql and 3 more"},
            {"signal": "risk-path-touched", "detail": "changes db/m1.sql, db/m2.sql, db/m3.sql, db/m4.sql, db/m5.sql and 3 more"},
        ])
    );
    let exact = reasons(
        &NeedsHumanFacts {
            touched_risk_files: five,
            ..facts()
        },
        &NeedsHumanConfig::default(),
    );
    assert_eq!(
        exact[0]["detail"],
        "changes db/m1.sql, db/m2.sql, db/m3.sql, db/m4.sql, db/m5.sql"
    );
}

#[test]
fn an_unverified_nit_raises_it_by_default() {
    let results = [result("nit", true, true, None)];
    let f = NeedsHumanFacts {
        results: &results,
        ..facts()
    };
    assert_eq!(
        reasons(&f, &NeedsHumanConfig::default()),
        json!([{"signal": "finding-unverified", "detail": "1 finding(s) survived without a counted vote"}])
    );
    let config = NeedsHumanConfig {
        unverified_min: SeverityFloor::Important,
        ..NeedsHumanConfig::default()
    };
    assert!(!needs_human(&f, &config).raised);
}

#[test]
fn a_finding_whose_votes_were_all_discarded_raises_it() {
    let mut r = result("important", true, false, Some(0.0));
    r["votes"] = json!([{"verdict": "REFUTED"}]);
    r["discardedVotes"] = json!([{"verdict": "REFUTED"}]);
    let results = [r];
    let out = reasons(
        &NeedsHumanFacts {
            results: &results,
            ..facts()
        },
        &NeedsHumanConfig::default(),
    );
    assert_eq!(
        out,
        json!([{"signal": "finding-unverified", "detail": "1 finding(s) survived without a counted vote"}])
    );
}

#[test]
fn a_finding_with_one_counted_vote_is_verified() {
    let mut r = result("important", true, false, Some(0.0));
    r["votes"] = json!([{"verdict": "REFUTED"}, {"verdict": "NOT_REFUTED"}]);
    r["discardedVotes"] = json!([{"verdict": "REFUTED"}]);
    let results = [r];
    let nh = needs_human(
        &NeedsHumanFacts {
            results: &results,
            ..facts()
        },
        &NeedsHumanConfig::default(),
    );
    assert!(!nh.raised);
}

fn one_vote_after_failures(failed: Value) -> Value {
    let mut r = result("important", true, false, Some(0.0));
    r["votes"] = json!([{"verdict": "NOT_REFUTED"}]);
    r["failedSlots"] = failed;
    r
}

#[test]
fn a_partial_failure_raises_it_only_when_configured() {
    let results = [one_vote_after_failures(json!([2, 3]))];
    let f = NeedsHumanFacts {
        results: &results,
        ..facts()
    };
    assert!(!needs_human(&f, &NeedsHumanConfig::default()).raised);
    let config = NeedsHumanConfig {
        unverified_min_votes: 2,
        ..NeedsHumanConfig::default()
    };
    assert_eq!(
        reasons(&f, &config),
        json!([{"signal": "finding-unverified", "detail": "1 finding(s) kept with fewer than 2 counted vote(s) after refuters failed"}])
    );
}

#[test]
fn a_planned_lone_refuter_stays_quiet() {
    let results = [one_vote_after_failures(json!([]))];
    let config = NeedsHumanConfig {
        unverified_min_votes: 2,
        ..NeedsHumanConfig::default()
    };
    let nh = needs_human(
        &NeedsHumanFacts {
            results: &results,
            ..facts()
        },
        &config,
    );
    assert!(!nh.raised);
}

#[test]
fn a_killed_borderline_blocker_raises_it() {
    let mut r = result("important", false, false, Some(0.47));
    r["finding"]["isBehaviorChanging"] = json!(true);
    let results = [r];
    let out = reasons(
        &NeedsHumanFacts {
            results: &results,
            ..facts()
        },
        &NeedsHumanConfig::default(),
    );
    assert_eq!(
        out,
        json!([{"signal": "finding-borderline", "detail": "1 finding(s) decided by a close vote (1 killed)"}])
    );
}

#[test]
fn the_band_comes_from_the_config() {
    let mut r = result("critical", true, false, Some(0.52));
    r["votes"] = json!([{"verdict": "NOT_REFUTED"}]);
    let results = [r];
    let f = NeedsHumanFacts {
        results: &results,
        ..facts()
    };
    let narrow = NeedsHumanConfig {
        borderline_band: 0.05,
        ..NeedsHumanConfig::default()
    };
    assert!(!needs_human(&f, &narrow).raised);
    let wide = NeedsHumanConfig {
        borderline_band: 0.1,
        ..NeedsHumanConfig::default()
    };
    assert_eq!(
        reasons(&f, &wide),
        json!([{"signal": "finding-borderline", "detail": "1 finding(s) decided by a close vote (0 killed)"}])
    );
}

#[test]
fn panel_cut_joins_both_causes() {
    let skipped = [json!({"title": "a"}), json!({"title": "b"})];
    let breaker = json!({"reason": "cost"});
    let r = reasons(
        &NeedsHumanFacts {
            panel_skipped_for_budget: &skipped,
            cost_breaker_fired: Some(&breaker),
            ..facts()
        },
        &NeedsHumanConfig::default(),
    );
    assert_eq!(
        r,
        json!([{"signal": "panel-cut", "detail": "2 finding(s) skipped the panel for budget; cost breaker fired"}])
    );
}

#[test]
fn opt_in_signals_stay_quiet_by_default() {
    let not_checked = ["x".to_owned()];
    let fallback = json!({"reason": "base moved"});
    let f = NeedsHumanFacts {
        critic_not_checked: &not_checked,
        incremental_fallback: Some(&fallback),
        ..facts()
    };
    assert!(!needs_human(&f, &NeedsHumanConfig::default()).raised);
    assert_eq!(
        reasons(
            &f,
            &with(&[Signal::CriticNotChecked, Signal::IncrementalFallback])
        ),
        json!([
            {"signal": "critic-not-checked", "detail": "the critic did not check 1 item(s)"},
            {"signal": "incremental-fallback", "detail": "base moved"},
        ])
    );
    let bare = json!({});
    let f = NeedsHumanFacts {
        incremental_fallback: Some(&bare),
        ..facts()
    };
    assert_eq!(
        reasons(&f, &with(&[Signal::IncrementalFallback])),
        json!([{"signal": "incremental-fallback", "detail": "incremental review fell back"}])
    );
}

#[test]
fn reasons_come_in_signal_order() {
    let lenses = [json!({"lens": "security", "files": []})];
    let touched = ["src/auth.ts".to_owned()];
    let skipped = [json!({})];
    let nh = needs_human(
        &NeedsHumanFacts {
            lenses_unreturned: &lenses,
            touched_risk_files: &touched,
            panel_skipped_for_budget: &skipped,
            ..facts()
        },
        &NeedsHumanConfig::default(),
    );
    let ids: Vec<&str> = nh.reasons.iter().map(|r| r.signal.as_str()).collect();
    assert_eq!(ids, ["lens-empty", "panel-cut", "risk-path-touched"]);
}

#[test]
fn a_signal_left_out_of_the_config_stays_quiet() {
    let lenses = [json!({"lens": "security", "files": []})];
    let f = NeedsHumanFacts {
        lenses_unreturned: &lenses,
        ..facts()
    };
    assert!(!needs_human(&f, &with(&[])).raised);
}

#[test]
fn meets_floor_follows_the_verdict_rule() {
    assert!(meets_floor(
        &json!({"severity": "critical"}),
        SeverityFloor::Blocking
    ));
    assert!(meets_floor(
        &json!({"severity": "important", "touchesHighRiskSurface": true}),
        SeverityFloor::Blocking
    ));
    assert!(!meets_floor(
        &json!({"severity": "important"}),
        SeverityFloor::Blocking
    ));
    assert!(meets_floor(
        &json!({"severity": "important"}),
        SeverityFloor::Important
    ));
    assert!(!meets_floor(
        &json!({"severity": "nit"}),
        SeverityFloor::Important
    ));
    assert!(meets_floor(&json!({"severity": "nit"}), SeverityFloor::Nit));
    assert!(!meets_floor(
        &json!({"severity": "important"}),
        SeverityFloor::Critical
    ));
}

fn raised(details: &[(Signal, &str)]) -> NeedsHuman {
    NeedsHuman {
        raised: true,
        reasons: details
            .iter()
            .map(|(signal, detail)| Reason {
                signal: *signal,
                detail: (*detail).to_owned(),
            })
            .collect(),
    }
}

fn report(text: &str) -> Report {
    Report {
        report: text.to_owned(),
        corrected: None,
        as_posted: text.to_owned(),
    }
}

const LENS: (Signal, &str) = (Signal::LensEmpty, "1 of 4 lens(es) returned nothing");

#[test]
fn the_line_goes_directly_under_the_verdict() {
    let r = with_needs_human_line(
        report("## Review\n\n### Verdict suggestion: COMMENT\n\nBody"),
        &raised(&[LENS]),
    );
    let lines: Vec<&str> = r.report.split('\n').collect();
    assert_eq!(
        lines[4],
        "**A person should review this PR:** 1 of 4 lens(es) returned nothing (lens-empty)."
    );
    assert_eq!(lines[3], "");
    assert_eq!(r.as_posted, r.report);
}

#[test]
fn the_line_sits_above_the_verdict_basis() {
    let r = with_verdict_basis(
        report("### Verdict suggestion: REQUEST CHANGES\n\nBody"),
        "Why REQUEST CHANGES: x.",
    );
    let r = with_needs_human_line(r, &raised(&[LENS]));
    let human = r.report.find("A person should review").unwrap();
    assert!(human < r.report.find("Why REQUEST CHANGES").unwrap());
}

#[test]
fn a_quiet_flag_leaves_the_report_unchanged() {
    let before = report("### Verdict suggestion: COMMENT\n\nBody");
    let quiet = NeedsHuman {
        raised: false,
        reasons: Vec::new(),
    };
    assert_eq!(with_needs_human_line(before.clone(), &quiet), before);
    assert_eq!(needs_human_line(&quiet), None);
}

#[test]
fn a_report_with_no_verdict_line_is_left_alone() {
    let before = report("No verdict heading here.");
    assert_eq!(
        with_needs_human_line(before.clone(), &raised(&[LENS])),
        before
    );
}

#[test]
fn several_reasons_are_joined_with_semicolons() {
    let line = needs_human_line(&raised(&[
        LENS,
        (Signal::RiskPathTouched, "changes db/migrations/001.sql"),
    ]))
    .unwrap();
    assert_eq!(
        line,
        "**A person should review this PR:** 1 of 4 lens(es) returned nothing (lens-empty); changes db/migrations/001.sql (risk-path-touched)."
    );
}

#[test]
fn the_value_round_trips() {
    let nh = raised(&[LENS, (Signal::PanelCut, "cost breaker fired")]);
    assert_eq!(NeedsHuman::from_value(&nh.to_value()), Some(nh));
    let quiet = NeedsHuman {
        raised: false,
        reasons: Vec::new(),
    };
    assert_eq!(NeedsHuman::from_value(&quiet.to_value()), Some(quiet));
    assert_eq!(NeedsHuman::from_value(&json!({"raised": true})), None);
    assert_eq!(
        NeedsHuman::from_value(
            &json!({"raised": true, "reasons": [{"signal": "nope", "detail": "x"}]})
        ),
        None
    );
}
