//! Differential tests: each ported function and its JavaScript region run on
//! the same inputs, and any difference fails.
//!
//! Inputs are generated, not hand-picked, and lean on the places where
//! JavaScript and Rust disagree by default: UTF-16 length, the whitespace set
//! of `trim`, line terminators other than `\n`, non-ASCII case folding, and
//! JSON values of the wrong type. The generator is deterministic, so a failure
//! reproduces.

mod common;

use common::*;
use proptest::prelude::*;
use serde_json::{json, Value};
use std::path::PathBuf;

use tribunal_core::{artifact_key, report_verdict, verified_gate};

#[test]
fn artifact_key_matches_js() {
    let cases = sample((
        proptest::option::of(0u64..40),
        prop::sample::select(vec!["codex", "gemini", "claude"]),
        proptest::option::of(0u64..4),
    ));
    let calls: Vec<Value> = cases
        .iter()
        .map(|(s, src, r)| json!({"fn": "artifactKey", "args": [s, src, r]}))
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|(s, src, r)| json!(artifact_key::artifact_key(*s, src, *r)))
        .collect();
    let js = oracle("ARTIFACT-KEY", &["artifactKey"], &[], &calls);
    assert_same("artifactKey", &calls, &rust, &js);
}

fn edge_text(len: std::ops::Range<usize>) -> impl Strategy<Value = String> {
    let chars = prop::sample::select(vec![
        'a', 'Z', 'é', '😀', ' ', '\t', '\n', '\u{FEFF}', '\u{0085}', '\u{3000}', '\u{00A0}', 'ß',
    ]);
    prop::collection::vec(chars, len).prop_map(|v| v.into_iter().collect())
}

fn any_scalar() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        Just(json!(true)),
        Just(json!(false)),
        Just(json!(0)),
        Just(json!(1)),
        Just(json!("")),
        Just(json!("x")),
    ]
}

fn anchor() -> impl Strategy<Value = Value> {
    let file = prop_oneof![
        Just(None),
        Just(Some(json!("prisma/schema.prisma"))),
        Just(Some(json!(""))),
        Just(Some(json!(7))),
        Just(Some(Value::Null)),
    ];
    let line = prop_oneof![
        Just(None),
        Just(Some(json!(1374))),
        Just(Some(json!(1))),
        Just(Some(json!(0))),
        Just(Some(json!(-2))),
        Just(Some(json!(3.5))),
        Just(Some(json!(3.0))),
        Just(Some(json!("12"))),
    ];
    // Most quotes sit next to the 10-unit threshold, where trim and UTF-16
    // length decide the outcome.
    let quote = prop_oneof![
        1 => Just(None),
        5 => edge_text(8..13).prop_map(|s| Some(json!(s))),
        2 => edge_text(0..16).prop_map(|s| Some(json!(s))),
        1 => Just(Some(json!(42))),
    ];
    let object = (file, line, quote).prop_map(|(f, l, q)| {
        let mut m = serde_json::Map::new();
        if let Some(f) = f {
            m.insert("file".into(), f);
        }
        if let Some(l) = l {
            m.insert("line".into(), l);
        }
        if let Some(q) = q {
            m.insert("quote".into(), q);
        }
        Value::Object(m)
    });
    prop_oneof![8 => object, 1 => any_scalar(), 1 => Just(json!([]))]
}

fn finding() -> impl Strategy<Value = Value> {
    let severity = prop_oneof![
        Just(None),
        Just(Some(json!("nit"))),
        Just(Some(json!("important"))),
        Just(Some(json!("critical"))),
        Just(Some(json!(5))),
    ];
    let adjudicated = prop_oneof![
        3 => Just(Some(json!(true))),
        1 => Just(Some(json!(false))),
        1 => Just(Some(json!("true"))),
        1 => Just(Some(json!(1))),
        1 => Just(None),
    ];
    let verified_by = prop_oneof![
        1 => Just(None),
        1 => Just(Some(Value::Null)),
        1 => Just(Some(json!([]))),
        1 => Just(Some(json!("x"))),
        4 => prop::collection::vec(anchor(), 1..4).prop_map(|v| Some(Value::Array(v))),
    ];
    let object = (
        any::<bool>(),
        severity,
        adjudicated,
        verified_by,
        any::<bool>(),
    )
        .prop_map(|(title_first, s, a, v, note)| {
            let mut m = serde_json::Map::new();
            if title_first {
                m.insert("title".into(), json!("t"));
            }
            if let Some(a) = a {
                m.insert("adjudicated".into(), a);
            }
            if let Some(s) = s {
                m.insert("severity".into(), s);
            }
            if let Some(v) = v {
                m.insert("verifiedBy".into(), v);
            }
            if note {
                m.insert("verifiedByGateNote".into(), json!("earlier"));
            }
            m.insert("file".into(), json!("src/a.ts"));
            Value::Object(m)
        });
    prop_oneof![9 => object, 1 => any_scalar()]
}

#[test]
fn verified_gate_matches_js() {
    let lists = sample(prop::collection::vec(finding(), 0..5));
    let exports = [
        "verifiedGateFindingProblems",
        "verifiedGateDemoteUnanchored",
    ];

    let calls: Vec<Value> = lists
        .iter()
        .map(|l| json!({"fn": "verifiedGateDemoteUnanchored", "args": [l]}))
        .collect();
    let rust: Vec<Value> = lists
        .iter()
        .map(|l| Value::Array(verified_gate::demote_unanchored(l)))
        .collect();
    let js = oracle("VERIFIED-GATE", &exports, &[], &calls);
    assert_same("verifiedGateDemoteUnanchored", &calls, &rust, &js);

    let findings: Vec<&Value> = lists.iter().flatten().collect();
    let calls: Vec<Value> = findings
        .iter()
        .map(|f| json!({"fn": "verifiedGateFindingProblems", "args": [f]}))
        .collect();
    let rust: Vec<Value> = findings
        .iter()
        .map(|f| json!(verified_gate::finding_problems(f)))
        .collect();
    let js = oracle("VERIFIED-GATE", &exports, &[], &calls);
    assert_same("verifiedGateFindingProblems", &calls, &rust, &js);
}

fn report_body() -> impl Strategy<Value = String> {
    let line = prop::sample::select(vec![
        "## Title",
        "x## Title",
        "### Verdict suggestion: APPROVE",
        "### Verdict suggestion: COMMENT, unfinished",
        "#### verdict SUGGESTION: request changes",
        "##\tVerdict suggestion: approve",
        "# Verdict suggestion: REQUEST CHANGES or APPROVE",
        "####### Verdict suggestion: APPROVE",
        "### Verdict \u{017F}uggestion: APPROVE",
        "### Verdict suggestion: stra\u{00DF}e",
        "### Verdict suggestion:",
        " ### Verdict suggestion: APPROVE",
        "text with Verdict suggestion: COMMENT",
        "",
    ]);
    let terminator = prop::sample::select(vec!["\n", "\r\n", "\r", "\u{2028}", "\u{2029}"]);
    prop::collection::vec((line, terminator), 0..7)
        .prop_map(|parts| parts.into_iter().map(|(l, t)| format!("{l}{t}")).collect())
}

#[test]
fn report_verdict_matches_js() {
    let cases = sample((
        proptest::option::weighted(0.95, report_body()),
        prop::sample::select(vec!["APPROVE", "COMMENT", "REQUEST CHANGES"]),
    ));
    let calls: Vec<Value> = cases
        .iter()
        .map(|(body, v)| json!({"fn": "pinReportVerdict", "args": [body, v]}))
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|(body, v)| json!(report_verdict::pin_report_verdict(body.as_deref(), v)))
        .collect();
    let js = oracle("REPORT-VERDICT", &["pinReportVerdict"], &[], &calls);
    assert_same("pinReportVerdict", &calls, &rust, &js);
}

/// The recorded reports in the harness data directory, which lives outside
/// this repository. Run with `TRIBUNAL_HARNESS_DATA=<dir> cargo test -- --ignored`.
#[test]
#[ignore = "needs TRIBUNAL_HARNESS_DATA"]
fn report_verdict_matches_js_on_recorded_reports() {
    let root = std::env::var("TRIBUNAL_HARNESS_DATA").expect("TRIBUNAL_HARNESS_DATA is not set");
    let dir = PathBuf::from(root).join("tribunal/fixtures/report-verdict");
    let bodies: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect();
    assert!(
        !bodies.is_empty(),
        "no recorded reports in {}",
        dir.display()
    );
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for body in &bodies {
        for v in ["APPROVE", "COMMENT", "REQUEST CHANGES"] {
            calls.push(json!({"fn": "pinReportVerdict", "args": [body, v]}));
            rust.push(json!(report_verdict::pin_report_verdict(Some(body), v)));
        }
    }
    let js = oracle("REPORT-VERDICT", &["pinReportVerdict"], &[], &calls);
    assert_same("pinReportVerdict (recorded)", &calls, &rust, &js);
}
