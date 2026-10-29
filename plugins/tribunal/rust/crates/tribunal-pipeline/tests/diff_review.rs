//! The Review phase's pure half against the workflow: the bot-vetting task,
//! lens labels, and the candidates from the lens answers through the
//! out-of-diff gate.

mod common;

use common::{assert_same, sample, span, span_raw, Span, LOGS, LOG_FN};
use proptest::prelude::*;
use serde_json::{json, Value};
use tribunal_pipeline::review::{
    bot_vetting_prompt, check_lenses_returned, collect_candidates, lens_labels,
};

const CANDIDATES_SPAN: Span = Span {
    start: "const lensLabels = lensPlan.map(",
    end: "re-anchored onto a changed file) across ${lensPlan.length} lenses`,\n);",
    deps: &["PROFILE-LOOKUP", "HIGH-RISK-SURFACE"],
    pre: &[],
    fns: &["log"],
    ret: "{lensLabels, lensFindings, lensNotReached, validBotFindings, allCandidates, droppedPrePanel, logs: __LOGS__}",
};

const BOT_SPAN: Span = Span {
    start: "const hasBotComments =",
    end: ": []),\n];",
    deps: &[],
    pre: &[],
    fns: &["reviewerAgent"],
    ret: "{hasBotComments, tasks: reviewTasks.map((t) => t())}",
};

const PR_FILES: &[&str] = &[
    "src/a.ts",
    "src/b/service.ts",
    "prisma/schema.prisma",
    "src/auth/guard.ts",
];
const OTHER: &[&str] = &["src/caller.ts", "lib/other.js", "", "a.ts"];

fn file() -> impl Strategy<Value = Value> {
    prop_oneof![
        3 => prop::sample::select(PR_FILES).prop_map(|f| json!(f)),
        2 => prop::sample::select(OTHER).prop_map(|f| json!(f)),
        1 => Just(Value::Null),
    ]
}

fn anchor() -> impl Strategy<Value = Value> {
    (
        file(),
        prop_oneof![Just(json!(12)), Just(json!(3.5)), Just(Value::Null)],
    )
        .prop_map(|(f, l)| json!({"file": f, "line": l, "quote": "x"}))
}

fn finding() -> impl Strategy<Value = Value> {
    (
        "[A-Za-z ]{1,20}",
        file(),
        prop_oneof![Just(json!(10)), Just(json!(0))],
        prop_oneof![Just("critical"), Just("important"), Just("nit")],
        prop::option::of(prop::collection::vec(anchor(), 0..3)),
        any::<bool>(),
    )
        .prop_map(|(title, file, line, sev, vb, lens_key)| {
            let mut f = json!({"title": title, "file": file, "line": line, "severity": sev,
                "description": "d", "evidence": "e", "isBehaviorChanging": true, "touchesHighRiskSurface": false});
            if let Some(vb) = vb {
                f["verifiedBy"] = Value::from(vb);
            }
            if lens_key {
                f["lens"] = json!("self-reported");
            }
            f
        })
}

fn lens_result() -> impl Strategy<Value = Value> {
    prop_oneof![
        1 => Just(Value::Null),
        6 => (
            prop::collection::vec(finding(), 0..4),
            prop::option::of(prop::collection::vec(prop_oneof![Just(json!(" src/x.ts ")), Just(json!("")), Just(json!(null)), Just(json!("src/a.ts"))], 0..3)),
        )
            .prop_map(|(f, nr)| {
                let mut r = json!({"findings": f});
                if let Some(nr) = nr {
                    r["notReached"] = Value::from(nr);
                }
                r
            }),
    ]
}

fn bot_comment() -> impl Strategy<Value = Value> {
    (
        prop_oneof![Just("VALID"), Just("INVALID"), Just("POLICY")],
        prop_oneof![
            Just(json!(0)),
            Just(json!(1)),
            Just(json!(7)),
            Just(json!(-1)),
            Just(json!(0.5))
        ],
        prop::option::of(file()),
        prop::option::of(prop_oneof![Just(json!(4)), Just(json!(0))]),
        prop::option::of(prop_oneof![
            Just(json!(true)),
            Just(json!(false)),
            Just(json!("true"))
        ]),
        prop::option::of(prop::collection::vec(anchor(), 0..2)),
        any::<bool>(),
    )
        .prop_map(|(class, index, file, line, ibc, vb, reason)| {
            let mut c = json!({"index": index, "classification": class, "summary": "bot says"});
            if reason {
                c["reason"] = json!("because");
            }
            if let Some(f) = file {
                c["file"] = f;
            }
            if let Some(l) = line {
                c["line"] = l;
            }
            if let Some(b) = ibc {
                c["isBehaviorChanging"] = b;
            }
            if let Some(vb) = vb {
                c["verifiedBy"] = Value::from(vb);
            }
            c
        })
}

fn blast_radius() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        Just(json!({
            "changed": [{"file": "src/b/service.ts", "symbols": ["load", "save"]}],
            "callers": [
                {"file": "src/caller.ts", "line": 3, "via": "load -> wrap", "call": true, "awaited": false, "text": "load()"},
                {"file": "lib/other.js", "line": 9, "via": "missing", "call": false, "text": "x"}
            ],
        })),
    ]
}

const FOCI: &[(&str, &str)] = &[
    ("correctness", "correctness sentence"),
    ("security", "security sentence"),
    ("dataContract", "data sentence"),
];

fn case() -> impl Strategy<Value = Value> {
    (
        prop::collection::vec(
            prop_oneof![
                prop::sample::select(FOCI).prop_map(|(_, s)| json!(s)),
                Just(json!("unlabelled focus")),
                Just(Value::Null),
            ],
            1..5,
        ),
        any::<bool>(),
        prop::collection::vec(bot_comment(), 0..4),
        prop::collection::vec(prop_oneof![
            Just(json!({"path": "src/a.ts", "line": 2})),
            Just(json!({"path": "src/elsewhere.ts", "line": 0})),
            Just(json!({})),
        ], 0..3),
        blast_radius(),
        any::<bool>(),
    )
        .prop_flat_map(|(foci, has_bot, comments, bot_src, br, bot_null)| {
            let n = foci.len();
            (
                Just(foci),
                Just(has_bot),
                Just(comments),
                Just(bot_src),
                Just(br),
                Just(bot_null),
                prop::collection::vec(lens_result(), n..=n),
            )
        })
        .prop_map(|(foci, has_bot, comments, bot_src, br, bot_null, mut results)| {
            if has_bot {
                results.push(if bot_null { Value::Null } else { json!({"comments": comments}) });
            }
            let label: serde_json::Map<String, Value> =
                FOCI.iter().map(|(k, s)| ((*s).to_owned(), json!(k))).collect();
            json!({
                "lensPlan": foci.iter().map(|f| json!({"focus": f, "files": null})).collect::<Vec<_>>(),
                "FOCI_LABEL": label,
                "reviewResults": results,
                "hasBotComments": has_bot,
                "prContext": {"files": PR_FILES, "botComments": bot_src},
                "PROFILE": {"riskPaths": [{"focus": "security", "pattern": "auth"}, {"focus": "migration", "pattern": "prisma"}]},
                "blastRadius": br,
                "log": LOG_FN,
            })
        })
}

fn rust_candidates(c: &Value) -> Value {
    let foci: Vec<Value> = c["lensPlan"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["focus"].clone())
        .collect();
    let labels = lens_labels(&foci, c["FOCI_LABEL"].as_object().unwrap());
    let results: Vec<Option<Value>> = c["reviewResults"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (!r.is_null()).then(|| r.clone()))
        .collect();
    let files: Vec<String> = serde_json::from_value(c["prContext"]["files"].clone()).unwrap();
    let br = &c["blastRadius"];
    let out = collect_candidates(
        &results,
        &labels,
        c["hasBotComments"].as_bool().unwrap(),
        c["prContext"].get("botComments"),
        &files,
        (!br.is_null()).then_some(br),
        &c["PROFILE"],
    )
    .unwrap();
    json!({
        "lensLabels": labels,
        "lensFindings": out.lens_findings,
        "lensNotReached": out.lens_not_reached,
        "validBotFindings": out.valid_bot_findings,
        "allCandidates": out.all_candidates,
        "droppedPrePanel": out.dropped_pre_panel,
        "logs": out.logs,
    })
}

#[test]
fn candidates_match_the_workflow() {
    let cases = sample(case());
    let ret = CANDIDATES_SPAN.ret.replace("__LOGS__", LOGS);
    let js = span(
        &Span {
            ret: &ret,
            ..CANDIDATES_SPAN
        },
        &cases,
    );
    let rust: Vec<Value> = cases.iter().map(rust_candidates).collect();
    assert_same("review candidates", &cases, &rust, &js);
}

#[test]
fn the_generated_cases_reach_every_branch() {
    let out: Vec<Value> = sample(case()).iter().map(rust_candidates).collect();
    let count = |f: &dyn Fn(&Value) -> bool| out.iter().filter(|d| f(d)).count();
    let any_in =
        |d: &Value, k: &str, p: &dyn Fn(&Value) -> bool| d[k].as_array().unwrap().iter().any(p);
    assert!(
        count(&|d| any_in(d, "allCandidates", &|c| c["reanchored"]["via"]
            == "verifiedBy"))
            > 20
    );
    assert!(
        count(&|d| any_in(d, "allCandidates", &|c| c["reanchored"]["via"]
            == "dependents"))
            > 20
    );
    assert!(count(&|d| !d["droppedPrePanel"].as_array().unwrap().is_empty()) > 50);
    assert!(
        count(
            &|d| any_in(d, "validBotFindings", &|c| c["touchesHighRiskSurface"]
                == true)
        ) > 5
    );
    assert!(count(&|d| any_in(d, "validBotFindings", &|c| c.get("description").is_none())) > 5);
    assert!(count(&|d| !d["lensNotReached"].as_array().unwrap().is_empty()) > 50);
    assert!(
        count(&|d| any_in(d, "lensLabels", &|l| l
            .as_str()
            .unwrap()
            .starts_with("lens")))
            > 50
    );
}

#[test]
fn bot_vetting_task_matches_the_workflow() {
    let params = json!({
        "vetBotComments": true,
        "prContext": {"botComments": [{"path": "a"}], "botCommentsPath": "/tmp/tribunal-context-12/bot-comments.json"},
        "lensPlan": [],
        "reviewerAgent": "(prompt, opts) => ({prompt, label: opts.label, phase: opts.phase, schema: opts.schema, model: opts.model, agentType: opts.agentType})",
        "prNumber": 12,
        "repo": "acme/widgets-api",
        "BOT_VETTING_SCHEMA": "bot-vetting",
        "reviewModel": "claude-sonnet-5-5",
        "LENS_AGENT": "tribunal:code-review-expert",
    });
    let js = span(&BOT_SPAN, &[params]).remove(0);
    assert_eq!(js["hasBotComments"], json!(true));
    let task = &js["tasks"][0];
    assert_eq!(task["label"], "bot-vetting");
    assert_eq!(task["schema"], "bot-vetting");
    assert_eq!(
        task["prompt"].as_str().unwrap(),
        bot_vetting_prompt(
            "12",
            "acme/widgets-api",
            "/tmp/tribunal-context-12/bot-comments.json"
        )
    );
}

// ------------------------------------------------- lenses that came back empty

const UNRETURNED_SPAN: Span = Span {
    start: "const lensesUnreturned = unreturnedLenses(",
    end: "the verdict cannot be APPROVE`,\n  );\n}",
    deps: &["REPORT-BANNERS"],
    pre: &[],
    fns: &["log"],
    ret: "{lensesUnreturned, logs: __LOGS__}",
};

#[test]
fn the_empty_lens_check_matches_the_workflow() {
    let slot = prop_oneof![
        Just(Value::Null),
        Just(json!(["src/a.ts"])),
        Just(json!(["src/a.ts", "src/b/service.ts"])),
        Just(json!([])),
    ];
    let answer = prop_oneof![
        2 => Just(Value::Null),
        3 => Just(json!({"findings": []})),
    ];
    let cases: Vec<Value> = sample(prop::collection::vec((slot, answer), 0..5).prop_map(
        |lenses| {
            let plan: Vec<Value> = lenses
                .iter()
                .map(|(files, _)| {
                    if files.is_null() {
                        json!({"focus": "x"})
                    } else {
                        json!({"focus": "x", "files": files})
                    }
                })
                .collect();
            let results: Vec<Value> = lenses.iter().map(|(_, a)| a.clone()).collect();
            let labels: Vec<Value> = (0..lenses.len())
                .map(|i| json!(format!("lens{}", i + 1)))
                .collect();
            json!({"lensPlan": plan, "reviewResults": results, "lensLabels": labels})
        },
    ));
    let ret = UNRETURNED_SPAN.ret.replace("__LOGS__", LOGS);
    let params: Vec<Value> = cases
        .iter()
        .map(|c| {
            let mut p = c.clone();
            p["log"] = json!(LOG_FN);
            p
        })
        .collect();
    let js: Vec<Value> = span_raw(
        &Span {
            ret: &ret,
            ..UNRETURNED_SPAN
        },
        &params,
    )
    .into_iter()
    .map(|r| match r {
        Ok(v) => json!({"ok": v}),
        Err(e) => json!({"err": e}),
    })
    .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| {
            let plan_files: Vec<Option<Vec<String>>> = c["lensPlan"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.get("files").and_then(Value::as_array).map(|f| f.iter().map(|x| x.as_str().unwrap().to_owned()).collect()))
                .collect();
            let results: Vec<Option<Value>> = c["reviewResults"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| (!r.is_null()).then(|| r.clone()))
                .collect();
            let labels: Vec<String> = c["lensLabels"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l.as_str().unwrap().to_owned())
                .collect();
            match check_lenses_returned(&plan_files, &results, &labels) {
                Ok((unreturned, log)) => json!({"ok": {"lensesUnreturned": unreturned, "logs": log.into_iter().collect::<Vec<_>>()}}),
                Err(e) => json!({"err": e}),
            }
        })
        .collect();
    assert_same("empty lenses", &cases, &rust, &js);
    let thrown = rust.iter().filter(|r| r.get("err").is_some()).count();
    let partial = rust
        .iter()
        .filter(|r| {
            r["ok"]["lensesUnreturned"]
                .as_array()
                .is_some_and(|u| !u.is_empty())
        })
        .count();
    assert!(
        thrown > 10 && partial > 10,
        "thrown {thrown}, partial {partial}"
    );
}
