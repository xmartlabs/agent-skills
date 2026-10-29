//! `run::apply_evidence` against the workflow's handling of the evidence
//! resolver's answer.

mod common;

use common::{assert_same, sample, span, Span, LOGS, LOG_FN};
use proptest::prelude::*;
use serde_json::{json, Value};
use tribunal_pipeline::run::apply_evidence;

const EVIDENCE_SPAN: Span = Span {
    start: "const parsed = evidenceRelay.ok ? evidenceRelay.data : null;",
    end: "log(evidenceResolverFailure(parsed, allCandidates.length, evidenceRelay.ok ? null : evidenceRelay.reason));\n  }",
    deps: &["LINE-CORRECTION", "EVIDENCE-RESOLVER-DEGRADE"],
    pre: &[],
    fns: &["log"],
    ret: "{allCandidates, evidenceResolverRan, evidenceLocatedCount, evidenceNotFoundCount, evidenceLineCorrectedCount, logs: __LOGS__}",
};

fn resolution() -> impl Strategy<Value = Value> {
    (
        prop_oneof![3 => Just(json!(0)), 3 => Just(json!(1)), 1 => Just(json!(2)), 1 => Just(json!(9)), 1 => Just(json!(-1)), 1 => Just(json!(1.5)), 1 => Just(json!("1"))],
        any::<bool>(),
        prop_oneof![Just("exact"), Just("normalized"), Just("not-found")],
        prop::option::weighted(0.8, prop_oneof![3 => Just(json!(12)), 1 => Just(json!(0)), 1 => Just(json!(7.5)), 1 => Just(json!(40))]),
        prop::option::weighted(0.8, prop_oneof![3 => Just(json!(30)), 1 => Just(json!(12)), 1 => Just(json!(40))]),
    )
        .prop_map(|(id, located, kind, corrected, patch)| {
            let mut r = json!({"id": id, "evidenceLocated": located, "matchKind": kind, "matchedLineKind": "added"});
            if let Some(c) = corrected {
                r["correctedLine"] = c;
            }
            if let Some(p) = patch {
                r["patchLine"] = p;
            }
            r
        })
}

fn case() -> impl Strategy<Value = Value> {
    (
        prop::collection::vec(prop_oneof![3 => Just(json!(30)), 1 => Just(json!(40)), 1 => Just(json!("30")), 1 => Just(json!(12))], 0..4),
        prop_oneof![
            4 => prop::collection::vec(resolution(), 0..5).prop_map(|r| json!({"ok": true, "data": {"ran": true, "results": r}})),
            1 => Just(json!({"ok": true, "data": {"ran": false, "reason": "diff-unreadable"}})),
            1 => Just(json!({"ok": true, "data": {"ran": false}})),
            1 => Just(json!({"ok": true, "data": {"ran": true}})),
            1 => Just(json!({"ok": false, "reason": "crc mismatch"})),
            1 => Just(json!({"ok": true, "data": null})),
        ],
    )
        .prop_map(|(lines, relay)| {
            let candidates: Vec<Value> = lines
                .into_iter()
                .map(|l| json!({"title": "t", "file": "src/a.ts", "line": l}))
                .collect();
            json!({
                "evidenceRelay": relay,
                "allCandidates": candidates,
                "evidenceResolverRan": false,
                "evidenceLocatedCount": 0,
                "evidenceNotFoundCount": 0,
                "evidenceLineCorrectedCount": 0,
                "log": LOG_FN,
            })
        })
}

fn rust_evidence(c: &Value) -> Value {
    let mut candidates: Vec<Value> = c["allCandidates"].as_array().unwrap().clone();
    let relay = &c["evidenceRelay"];
    let ok = relay["ok"] == json!(true);
    let parsed = if ok {
        Some(&relay["data"]).filter(|d| !d.is_null())
    } else {
        None
    };
    let reason = if ok { None } else { relay["reason"].as_str() };
    let mut logs = Vec::new();
    let s = apply_evidence(&mut candidates, parsed, reason, &mut logs);
    json!({
        "allCandidates": candidates,
        "evidenceResolverRan": s.ran,
        "evidenceLocatedCount": s.located,
        "evidenceNotFoundCount": s.not_found,
        "evidenceLineCorrectedCount": s.line_corrected,
        "logs": logs,
    })
}

#[test]
fn evidence_handling_matches_the_workflow() {
    let cases = sample(case());
    let ret = EVIDENCE_SPAN.ret.replace("__LOGS__", LOGS);
    let js = span(
        &Span {
            ret: &ret,
            ..EVIDENCE_SPAN
        },
        &cases,
    );
    let rust: Vec<Value> = cases.iter().map(rust_evidence).collect();
    assert_same("evidence resolver", &cases, &rust, &js);
}

#[test]
fn the_generated_cases_reach_every_branch() {
    let out: Vec<Value> = sample(case()).iter().map(rust_evidence).collect();
    assert!(
        out.iter()
            .filter(|o| o["evidenceLineCorrectedCount"].as_u64().unwrap() > 0)
            .count()
            > 20
    );
    assert!(
        out.iter()
            .filter(|o| o["evidenceResolverRan"] == false)
            .count()
            > 50
    );
    assert!(
        out.iter()
            .filter(|o| o["evidenceNotFoundCount"].as_u64().unwrap() > 0)
            .count()
            > 30
    );
}
