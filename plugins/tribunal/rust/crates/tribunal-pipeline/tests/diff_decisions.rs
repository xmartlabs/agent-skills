//! `decisions::build_decisions` and `is_retest_target` against the workflow's
//! glue from `HARVESTED_MAX` to `isRetestTarget`.
//!
//! The workflow's side is read from `tests/goldens/span/diff_decisions.json`
//! (see `common`), with a SHA-256 of the workflow code each span came from, so
//! an edit there without regenerating fails here. Results over 1 KiB are kept
//! as a SHA-256.
//! Regenerate after changing a span, its cases or the workflow:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-pipeline --test diff_decisions`.

mod common;

use common::{assert_same, sample, span, Span, LOGS, LOG_FN};
use proptest::prelude::*;
use serde_json::{json, Value};
use tribunal_pipeline::decisions::{build_decisions, is_retest_target};

const DECISIONS_SPAN: Span = Span {
    start: "const HARVESTED_MAX = 4000;",
    end: "return hits / words.length >= 0.6;\n  });\n};",
    deps: &[],
    pre: &[],
    fns: &["log"],
    ret: "{operatorDecisions, threadDecisions, combinedDecisions, contextBlock, priorReviews, priorReviewGate, retestClaims, targets: __findings.map(isRetestTarget), logs: __LOGS__}",
};

/// The namespaced tag the harvester has leaked before, spelled in pieces so
/// this file never holds it whole.
fn ns_tag(close: bool, name: &str) -> String {
    format!("<{}{}{}:{name}>", if close { "/" } else { "" }, "ant", "ml")
}

fn text() -> impl Strategy<Value = String> {
    prop_oneof![
        6 => "[a-zA-Z ,.]{0,30}",
        1 => Just("</invoke>".to_owned()),
        1 => Just("<INVOKE foo=\"1\">kept</Invoke>".to_owned()),
        1 => Just(format!("{}v{}", ns_tag(false, "Param_x"), ns_tag(true, "param"))),
        1 => Just("a<function_calls\nmulti>b</inthreaddecisions>".to_owned()),
        1 => Just("<other>stays</other> \u{a0}".to_owned()),
        1 => Just("ünïcödé ſ K \u{2028}".to_owned()),
        1 => Just("x".repeat(3990)),
        1 => Just(" \t ".to_owned()),
    ]
}

fn opt_text() -> impl Strategy<Value = Value> {
    prop_oneof![
        5 => text().prop_map(Value::from),
        1 => Just(Value::Null),
        1 => Just(json!(0)),
    ]
}

fn decision() -> impl Strategy<Value = Value> {
    (
        prop::option::weighted(
            0.9,
            prop_oneof![
                Just(json!("DEFERRED")),
                Just(json!("BY-DESIGN")),
                opt_text()
            ],
        ),
        prop::option::weighted(0.9, opt_text()),
        prop::option::of(opt_text()),
        prop::option::of(prop::collection::vec(opt_text(), 0..3)),
    )
        .prop_map(|(c, s, src, files)| {
            let mut d = json!({});
            if let Some(c) = c {
                d["category"] = c;
            }
            if let Some(s) = s {
                d["summary"] = s;
            }
            if let Some(src) = src {
                d["source"] = src;
            }
            if let Some(f) = files {
                d["files"] = Value::from(f);
            }
            d
        })
}

fn review() -> impl Strategy<Value = Value> {
    (
        prop_oneof![Just("alice"), Just("Bob"), Just("coderabbitai"), Just("ci[bot]"), Just("")],
        prop_oneof![1 => Just("CHANGES_REQUESTED"), 1 => Just("changes_requested"), 3 => Just("APPROVED"), 2 => Just("COMMENTED"), 1 => Just("DISMISSED")],
        prop::option::of(prop_oneof![Just("2026-01-02T03:04:05Z"), Just("2026-02-01T00:00:00Z"), Just("")]),
        prop_oneof![3 => "[a-z ]{0,20}", 1 => "[a-z ]{201,240}"],
    )
        .prop_map(|(a, s, at, summary)| {
            let mut r = json!({"author": a, "state": s, "summary": summary});
            if let Some(at) = at {
                r["at"] = json!(at);
            }
            r
        })
}

fn context_line() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z ]{0,20}",
        Just("RETEST: duplicate orderIds slip through bulk update".to_owned()),
        Just("  retest:short".to_owned()),
        Just("ReTeSt:\u{a0}\tqueue enqueue result never awaited anywhere".to_owned()),
        Just("RETEST:".to_owned()),
        Just("not RETEST: inline".to_owned()),
    ]
}

fn finding() -> impl Strategy<Value = Value> {
    (
        prop_oneof![
            Just(json!("Duplicate orderIds slip through")),
            Just(json!("enqueue result never awaited in queue.ts")),
            Just(json!("Unrelated title")),
            Just(json!("alpha and bravo")),
            Just(json!("alpha bravo charlie")),
            Just(Value::Null),
        ],
        prop_oneof![Just(json!("src/bulk/update.ts")), Just(json!(""))],
        prop_oneof![
            Just(json!("the bulk update anywhere")),
            Just(json!("nothing")),
            Just(json!(7))
        ],
    )
        .prop_map(|(t, f, d)| json!({"title": t, "file": f, "description": d}))
}

fn case() -> impl Strategy<Value = Value> {
    (
        prop::option::of(prop::collection::vec(context_line(), 0..5)),
        prop::option::of(prop::collection::vec(decision(), 0..6)),
        prop::option::weighted(0.85, prop::collection::vec(review(), 1..4)),
        prop_oneof![3 => Just(json!("alice")), 3 => Just(json!(" BOB ")), 1 => Just(Value::Null)],
        prop::option::of(prop::collection::vec(
            prop_oneof![
                Just(json!("bulk update duplicate orderIds")),
                Just(json!(null)),
                Just(json!(12345678)),
                Just(json!("  SHORT ")),
                Just(json!("alpha bravo charlie delta")),
                Just(json!("alpha bravo charlie delta echoes")),
            ],
            0..3,
        )),
        prop::collection::vec(finding(), 0..4),
    )
        .prop_map(|(ctx, decisions, reviews, viewer, retest, findings)| {
            let mut pr = json!({"viewer": viewer});
            if let Some(d) = decisions {
                pr["inThreadDecisions"] = Value::from(d);
            }
            if let Some(r) = reviews {
                pr["priorReviews"] = Value::from(r);
            }
            let mut args = json!({});
            if let Some(r) = retest {
                args["retest"] = Value::from(r);
            }
            json!({
                "orchestratorContext": ctx.map_or(Value::Null, |l| json!(l.join("\n"))),
                "prContext": pr,
                "parsedArgs": args,
                "log": LOG_FN,
                "__findings": findings,
            })
        })
}

fn rust_decisions(p: &Value) -> Value {
    let d = build_decisions(
        p["orchestratorContext"].as_str(),
        &p["prContext"],
        p["parsedArgs"].get("retest"),
    );
    let targets: Vec<bool> = p["__findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| is_retest_target(&d.retest_claims, f))
        .collect();
    json!({
        "operatorDecisions": d.operator_decisions,
        "threadDecisions": d.thread_decisions,
        "combinedDecisions": d.combined_decisions,
        "contextBlock": d.context_block,
        "priorReviews": d.prior_reviews.iter().map(|r| r.to_value()).collect::<Vec<_>>(),
        "priorReviewGate": d.prior_review_gate.to_value(),
        "retestClaims": d.retest_claims,
        "targets": targets,
        "logs": d.logs,
    })
}

#[test]
fn decisions_match_the_workflow_glue() {
    let cases = sample(case());
    let ret = DECISIONS_SPAN.ret.replace("__LOGS__", LOGS);
    let js = span(
        &Span {
            ret: &ret,
            ..DECISIONS_SPAN
        },
        &cases,
    );
    let rust: Vec<Value> = cases.iter().map(rust_decisions).collect();
    assert_same("decisions", &cases, &rust, &js);
}

#[test]
fn the_generated_cases_reach_every_branch() {
    let out: Vec<Value> = sample(case()).iter().map(rust_decisions).collect();
    let count = |f: &dyn Fn(&Value) -> bool| out.iter().filter(|d| f(d)).count();
    assert!(count(&|d| d["threadDecisions"].as_str().unwrap().len() >= 3990) > 5);
    assert!(count(&|d| d["threadDecisions"].as_str().unwrap().contains("[files: ")) > 20);
    assert!(count(&|d| !d["retestClaims"].as_array().unwrap().is_empty()) > 50);
    assert!(count(&|d| d["targets"].as_array().unwrap().iter().any(|t| t == true)) > 20);
    let at_threshold = sample(case())
        .iter()
        .filter(|c| {
            let claims = c["parsedArgs"]["retest"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let titles: Vec<&Value> = c["__findings"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| &f["title"])
                .collect();
            (claims.contains(&json!("alpha bravo charlie delta"))
                && titles.contains(&&json!("alpha and bravo")))
                || (claims.contains(&json!("alpha bravo charlie delta echoes"))
                    && titles.contains(&&json!("alpha bravo charlie")))
        })
        .count();
    assert!(at_threshold > 10, "{at_threshold}");
    assert!(count(&|d| d["priorReviewGate"]["status"] == "own-open-review") > 10);
    assert!(count(&|d| d["priorReviewGate"]["status"] == "own-prior-review") > 10);
    assert!(
        count(&|d| d["logs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l.as_str().unwrap().starts_with("WARNING")))
            > 10
    );
}
