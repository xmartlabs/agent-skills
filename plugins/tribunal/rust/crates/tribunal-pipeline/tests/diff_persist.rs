//! The persist step's pure parts against the workflow's own code:
//! `externalVotes`, `persistJson`, RECONCILIATION-RELAY with the logs and
//! the self-audit and wall-clock reads that follow it, and `rowIsThisRun`.
mod common;

use common::{assert_same, sample, span, Span, LOGS, LOG_FN};

use proptest::collection::vec;
use proptest::option;
use proptest::prelude::*;
use proptest::sample::select;
use serde_json::{json, Map, Value};
use tribunal_pipeline::persist::{
    external_votes, persist_json, reconciliation_logs, resolve_vote_reconciliation,
    row_is_this_run, self_audit, wall_clock_fields,
};

fn scalar() -> impl Strategy<Value = Value> {
    select(vec![
        json!(null),
        json!(0),
        json!(1),
        json!(2.5),
        json!(true),
        json!(false),
        json!(""),
        json!("x"),
        json!("claude"),
        json!("codex"),
        json!([]),
        json!({}),
    ])
}

fn vote() -> impl Strategy<Value = Value> {
    (
        option::of(select(vec![
            json!("claude"),
            json!("codex"),
            json!("gemini"),
            json!(""),
            json!(null),
        ])),
        option::of(scalar()),
        option::of(scalar()),
        option::of(scalar()),
        option::of(scalar()),
    )
        .prop_map(|(source, slot, verdict, declared, status)| {
            let mut m = Map::new();
            for (k, v) in [
                ("source", source),
                ("slotId", slot),
                ("verdict", verdict),
                ("confidence", Some(json!(0.7))),
                ("confidenceDeclared", declared),
                ("slotStatus", status),
            ] {
                if let Some(v) = v {
                    m.insert(k.into(), v);
                }
            }
            Value::Object(m)
        })
}

#[test]
fn external_votes_are_the_workflows() {
    let cases = sample(vec(
        (option::of(vec(vote(), 0..4)), option::of(scalar())).prop_map(|(votes, refined)| {
            let mut m = Map::new();
            if let Some(v) = votes {
                m.insert("allVotes".into(), Value::Array(v));
            }
            if let Some(r) = refined {
                m.insert("refined".into(), r);
            }
            Value::Object(m)
        }),
        0..4,
    ));
    let params: Vec<Value> = cases.iter().map(|r| json!({ "results": r })).collect();
    let js = span(
        &Span {
            start: "const externalVotes = results.flatMap((v) =>",
            end: "      round: v.refined ? 1 : 0,\n    })),\n);",
            deps: &[],
            pre: &[],
            fns: &[],
            ret: "externalVotes",
        },
        &params,
    );
    let rust: Vec<Value> = cases
        .iter()
        .map(|r| Value::Array(external_votes(r)))
        .collect();
    assert_same("externalVotes", &params, &rust, &js);
}

fn text() -> impl Strategy<Value = String> {
    vec(
        select(vec![
            "a",
            "é",
            "😀",
            "\u{1}",
            "\u{1f}",
            "\t",
            "\n",
            "\r",
            "\\",
            "\\u",
            "u",
            "00e9",
            "\\u12G4",
            "\\u0041",
            "\\\\u0041",
            "\u{7f}",
            " ",
        ]),
        0..8,
    )
    .prop_map(|parts| parts.concat())
}

#[test]
fn persist_json_is_the_workflows() {
    let cases = sample(
        (text(), text(), text()).prop_map(|(a, b, k)| json!({ "a": a, "nested": [b, {k: 1}] })),
    );
    let params: Vec<Value> = cases.iter().map(|v| json!({ "value": v })).collect();
    let js = span(
        &Span {
            start: "function persistSafeString(s) {",
            end: "  return JSON.stringify(value, (key, v) => (typeof v === \"string\" ? persistSafeString(v) : v));\n}",
            deps: &[],
            pre: &[],
            fns: &[],
            ret: "persistJson(value)",
        },
        &params,
    );
    let rust: Vec<Value> = cases.iter().map(|v| json!(persist_json(v))).collect();
    assert_same("persistJson", &params, &rust, &js);
}

fn kind() -> impl Strategy<Value = Value> {
    select(vec![
        json!({"kind": "verdict-mismatch"}),
        json!({"kind": "repeat-blocked"}),
        json!({"kind": "classifier-failed"}),
        json!({"kind": "orphan-artifact"}),
    ])
}

fn reconciliation() -> impl Strategy<Value = Option<Value>> {
    let rec = (
        select(vec![
            Some(json!(3)),
            Some(json!(0)),
            Some(json!(null)),
            Some(json!("3")),
            None,
        ]),
        option::of(select(vec![
            json!({}),
            json!({"repeat-blocked": 2}),
            json!({"classifier-failed": 1, "verdict-mismatch": 1}),
            json!(null),
        ])),
        option::of(vec(kind(), 0..3)),
        option::of(select(vec![json!(0), json!(1), json!(null)])),
        option::of(select(vec![json!("boom"), json!(5)])),
    )
        .prop_map(|(checked, counts, mismatches, failed, error)| {
            let mut m = Map::new();
            m.insert("ok".into(), json!(2));
            if let Some(c) = checked {
                m.insert("checked".into(), c);
            }
            if let Some(c) = counts {
                m.insert("counts".into(), c);
            }
            if let Some(x) = mismatches {
                m.insert("mismatches".into(), Value::Array(x));
            }
            if let Some(f) = failed {
                m.insert("classifierFailed".into(), f);
            }
            if let Some(e) = error {
                m.insert("error".into(), e);
            }
            Value::Object(m)
        });
    option::of(prop_oneof![8 => rec, 1 => Just(json!(null)), 1 => Just(json!("x"))])
}

fn self_audit_value() -> impl Strategy<Value = Option<Value>> {
    option::of(select(vec![
        json!({"passed": true, "checked": 17, "violations": []}),
        json!({"passed": false, "checked": 17, "violations": [{"id": "a", "detail": "b"}, {"id": "c", "detail": 4}]}),
        json!({"passed": null, "checked": 0, "violations": []}),
        json!({"checked": 3}),
        json!(null),
        json!("x"),
    ]))
}

fn bookkeeping() -> impl Strategy<Value = Value> {
    let full = (
        reconciliation(),
        self_audit_value(),
        option::of(select(vec![json!(42), json!("42"), json!(null)])),
        option::of(select(vec![json!("note"), json!(3), json!(null)])),
        option::of(select(vec![json!("inflight-exact"), json!(false)])),
    )
        .prop_map(|(rec, audit, secs, note, source)| {
            let mut m = Map::new();
            for (k, v) in [
                ("reconciliation", rec),
                ("selfAudit", audit),
                ("wallClockSeconds", secs),
                ("wallClockNote", note),
                ("wallClockSource", source),
            ] {
                if let Some(v) = v {
                    m.insert(k.into(), v);
                }
            }
            Value::Object(m)
        });
    prop_oneof![9 => full, 1 => Just(json!(null))]
}

#[test]
fn the_relay_reads_are_the_workflows() {
    let cases = sample((bookkeeping(), any::<bool>()));
    let params: Vec<Value> = cases
        .iter()
        .map(|(b, has)| {
            json!({
                "bookkeeping": b, "hasExternalVotes": has, "VOTE_DIR": "/tmp/votes",
                "metrics": {}, "log": LOG_FN,
            })
        })
        .collect();
    let ret = format!("{{metrics, logs: {LOGS}}}");
    let js = span(
        &Span {
            start: "const voteReconciliation = resolveVoteReconciliation(",
            end: "    ? bookkeeping.wallClockSource\n    : null;",
            deps: &["RECONCILIATION-RELAY"],
            pre: &[],
            fns: &["log"],
            ret: &ret,
        },
        &params,
    );
    let rust: Vec<Value> = cases
        .iter()
        .map(|(b, has)| {
            let mut logs = Vec::new();
            let b = Some(b).filter(|b| !b.is_null());
            let rec = resolve_vote_reconciliation(b.and_then(|b| b.get("reconciliation")), *has);
            reconciliation_logs(&rec, *has, "/tmp/votes", &mut logs);
            let mut metrics = Map::new();
            metrics.insert("voteReconciliation".into(), rec);
            metrics.insert("selfAudit".into(), self_audit(b, &mut logs));
            for (k, v) in wall_clock_fields(b) {
                metrics.insert(k.into(), v);
            }
            json!({ "metrics": metrics, "logs": logs })
        })
        .collect();
    assert_same("reconciliation relay", &params, &rust, &js);
}

#[test]
fn row_is_this_run_is_the_workflows() {
    let check = option::of(
        (
            option::of(select(vec![json!(true), json!(false), json!(null)])),
            select(vec![json!(77), json!(78), json!("77")]),
            option::of(select(vec![json!("o/r"), json!("x/y"), json!(null)])),
            select(vec![json!("abc"), json!(null), json!("")]),
            select(vec![json!(2), json!(3), json!(2.0)]),
        )
            .prop_map(|(found, pr, repo, sha, findings)| {
                let mut m = Map::new();
                if let Some(f) = found {
                    m.insert("found".into(), f);
                }
                m.insert("pr".into(), pr);
                if let Some(r) = repo {
                    m.insert("repo".into(), r);
                }
                m.insert("headSha".into(), sha);
                m.insert("findings".into(), findings);
                Value::Object(m)
            }),
    );
    let cases = sample((check, select(vec!["abc", ""])));
    let params: Vec<Value> = cases
        .iter()
        .map(|(c, sha)| {
            json!({
                "check": c, "e": {"pr": 77, "repo": "o/r", "headSha": sha, "findings": 2},
            })
        })
        .collect();
    let js = span(
        &Span {
            start: "function rowIsThisRun(check, { pr, repo, headSha, findings }) {",
            end: "    check.findings === findings\n  );\n}",
            deps: &[],
            pre: &[],
            fns: &[],
            ret: "rowIsThisRun(check, e)",
        },
        &params,
    );
    let rust: Vec<Value> = cases
        .iter()
        .map(|(c, sha)| {
            json!(row_is_this_run(
                c.as_ref(),
                &json!(77),
                "o/r",
                Some(*sha),
                2
            ))
        })
        .collect();
    assert_same("rowIsThisRun", &params, &rust, &js);
}
