//! The Verify phase's glue against the workflow: the dedup prompt and its
//! answer applied, the typecheck text read, the grounded gate, which findings
//! a panel would judge and afford, and the results split.
//!
//! The workflow's side is read from `tests/goldens/span/diff_verify.json` (see
//! `common`), with a SHA-256 of the workflow code each span came from, so an
//! edit there without regenerating fails here. Results over 1 KiB are kept as
//! a SHA-256.
//! Regenerate after changing a span, its cases or the workflow:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-pipeline --test diff_verify`.

mod common;

use common::{assert_same, sample, span, Span, LOGS, LOG_FN};
use proptest::prelude::*;
use serde_json::{json, Value};
use tribunal_core::evidence::tsc_oracle::{DiagsByFile, TscDiagnostic};
use tribunal_pipeline::verify::{
    apply_dedup, apply_typecheck_result, dedup_prompt, grounded_gate, needs_panel, no_panel_result,
    panel_plan, partition_results, Grounded,
};

const FILES: &[&str] = &[
    "src/a.ts",
    "src/b/service.ts",
    "scripts/run.js",
    "web/c.tsx",
];

fn file() -> impl Strategy<Value = Value> {
    prop_oneof![
        8 => prop::sample::select(FILES).prop_map(|f| json!(f)),
        1 => Just(json!("service.ts")),
        1 => Just(json!("")),
        1 => Just(Value::Null),
    ]
}

fn title() -> impl Strategy<Value = Value> {
    let pool = |w: &'static [&'static str]| {
        prop::sample::subsequence(w, 1..4).prop_map(|w| json!(w.join(" ")))
    };
    prop_oneof![
        pool(&["missing", "guard", "null", "orderIds", "the", "rejection"]),
        pool(&["retry", "webhook", "timeout", "queue", "lost"]),
    ]
}

fn candidate() -> impl Strategy<Value = Value> {
    (
        file(),
        prop_oneof![
            Just(json!(10)),
            Just(json!(14)),
            Just(json!(200)),
            Just(json!(0)),
            Just(Value::Null)
        ],
        title(),
        prop_oneof![
            3 => Just(json!("correctness")),
            2 => Just(json!("security")),
            1 => Just(json!("bot-comments")),
            1 => Just(Value::Null),
        ],
        prop_oneof![
            Just(json!("critical")),
            Just(json!("important")),
            Just(json!("nit")),
            Just(Value::Null)
        ],
        prop_oneof![Just(json!(0.9)), Just(json!(0.2)), Just(Value::Null)],
        (any::<bool>(), any::<bool>(), any::<bool>()),
    )
        .prop_map(|(f, line, t, lens, sev, conf, (beh, risk, mech))| {
            let mut c = json!({"file": f, "title": t, "description": "d", "evidence": "e"});
            if !line.is_null() {
                c["line"] = line;
            }
            if !lens.is_null() {
                c["lens"] = lens;
            }
            if !sev.is_null() {
                c["severity"] = sev;
            }
            if !conf.is_null() {
                c["confidence"] = conf;
            }
            if beh {
                c["isBehaviorChanging"] = json!(true);
            }
            if risk {
                c["touchesHighRiskSurface"] = json!(1);
            }
            if mech {
                c["mechanicalClaim"] = json!(true);
            }
            c
        })
}

// ---------------------------------------------------------------- dedup

const DEDUP_SPAN: Span = Span {
    start: "let panelCandidates = allCandidates;",
    end: "too far apart or too dissimilar to be one defect`,\n    );\n}",
    deps: &["DEDUP-VETO"],
    pre: &[(
        "const SEVERITY_RANK = { critical: 3, important: 2, nit: 1 };",
        "      lens: m.lens || \"unknown\",\n    })),\n  };\n}",
    )],
    fns: &["log", "agent", "__setup"],
    ret: "{panelCandidates, dedupedPrePanel, crossFileClustersRejected, proximityVetoed, prompt: globalThis.__dedup ? globalThis.__dedup.prompt : null, label: globalThis.__dedup ? globalThis.__dedup.opts.label + '/' + globalThis.__dedup.opts.model : null, logs: __LOGS__}",
};

fn index(n: u32) -> impl Strategy<Value = Value> {
    prop_oneof![
        12 => (0..n.max(1)).prop_map(|i| json!(i)),
        1 => Just(json!(-1)),
        1 => Just(json!(1.5)),
        1 => Just(json!(99)),
        1 => Just(json!("1")),
    ]
}

fn clusters(n: u32) -> impl Strategy<Value = Value> {
    prop_oneof![
        10 => prop::collection::vec(
            prop_oneof![
                6 => prop::collection::vec(index(n), 0..5).prop_map(Value::Array),
                1 => Just(json!(3)),
            ],
            0..4
        )
        .prop_map(|c| json!({"clusters": c})),
        1 => Just(json!({"clusters": null})),
        1 => Just(json!({"clusters": "ab"})),
        1 => Just(json!({})),
        1 => Just(Value::Null),
    ]
}

fn dedup_case() -> impl Strategy<Value = Value> {
    prop_oneof![1 => 0usize..2, 6 => 2usize..7]
        .prop_flat_map(|n| (prop::collection::vec(candidate(), n), clusters(n as u32)))
        .prop_map(|(mut all, answer)| {
            for (i, c) in all.iter_mut().enumerate() {
                if i % 3 != 2 {
                    c["file"] = json!("src/a.ts");
                }
                if i % 2 == 1 {
                    c["line"] = json!(200);
                }
            }
            json!({"allCandidates": all, "answer": answer})
        })
}

fn rust_dedup(c: &Value) -> Value {
    let all = c["allCandidates"].as_array().unwrap();
    let mut logs = Vec::new();
    if all.len() < 2 {
        return json!({"panelCandidates": all, "dedupedPrePanel": 0, "crossFileClustersRejected": 0, "proximityVetoed": 0, "prompt": null, "label": null, "logs": []});
    }
    let answer = (!c["answer"].is_null()).then_some(&c["answer"]);
    let d = apply_dedup(all, answer, &mut logs).unwrap();
    json!({
        "panelCandidates": d.panel_candidates,
        "dedupedPrePanel": d.deduped_pre_panel,
        "crossFileClustersRejected": d.cross_file_clusters_rejected,
        "proximityVetoed": d.proximity_vetoed,
        "prompt": dedup_prompt(all),
        "label": "dedup-candidates/haiku",
        "logs": logs,
    })
}

fn dedup_params(c: &Value) -> Value {
    json!({
        "allCandidates": c["allCandidates"],
        "log": LOG_FN,
        "agent": "async (p, o) => { globalThis.__dedup = {prompt: p, opts: o}; return globalThis.__answer; }",
        "__setup": format!("(globalThis.__dedup = null, globalThis.__answer = {})", c["answer"]),
        "DEDUP_SCHEMA": {},
        "SCRIBE_AGENT": "tribunal:tribunal-scribe",
    })
}

#[test]
fn dedup_matches_the_workflow() {
    let cases = sample(dedup_case());
    let ret = DEDUP_SPAN.ret.replace("__LOGS__", LOGS);
    let params: Vec<Value> = cases.iter().map(dedup_params).collect();
    let js = span(
        &Span {
            ret: &ret,
            ..DEDUP_SPAN
        },
        &params,
    );
    let rust: Vec<Value> = cases.iter().map(rust_dedup).collect();
    assert_same("dedup", &cases, &rust, &js);
}

#[test]
fn the_generated_dedup_cases_reach_every_branch() {
    let out: Vec<Value> = sample(dedup_case()).iter().map(rust_dedup).collect();
    let count = |k: &str| {
        out.iter()
            .filter(|d| d[k].as_u64().unwrap_or(0) > 0)
            .count()
    };
    assert!(
        count("dedupedPrePanel") > 30,
        "{}",
        count("dedupedPrePanel")
    );
    assert!(count("crossFileClustersRejected") > 30);
    assert!(
        count("proximityVetoed") > 10,
        "{}",
        count("proximityVetoed")
    );
    let merged_bot = out.iter().filter(|d| {
        d["panelCandidates"].as_array().unwrap().iter().any(|c| {
            c["mergedFrom"]
                .as_array()
                .is_some_and(|m| m.iter().any(|l| l == "unknown"))
        })
    });
    assert!(merged_bot.count() > 3);
}

// -------------------------------------------------------------- typecheck

const TYPECHECK_SPAN: Span = Span {
    start: "let groundedAvailable = false;",
    end: "with diagnostics`,\n    );\n  }\n}",
    deps: &["TSC-ORACLE"],
    pre: &[],
    fns: &["log"],
    ret: "(applyTypecheckResult(text), {groundedAvailable, diagsByFile, covered: [...groundedCovered], logs: __LOGS__})",
};

fn diag_line() -> impl Strategy<Value = String> {
    (
        prop_oneof![
            4 => prop::sample::select(FILES).prop_map(str::to_owned),
            1 => Just("src/other.ts".to_owned()),
            1 => (0u32..25).prop_map(|i| format!("lib/m{i}.ts")),
        ],
        1u32..300,
        prop_oneof![Just("TS2345"), Just("TS2307"), Just("TS2304")],
    )
        .prop_map(|(f, l, code)| format!("{f}({l},3): error {code}: Something is wrong."))
}

fn typecheck_text() -> impl Strategy<Value = String> {
    let marker = prop_oneof![
        3 => (0u32..40, prop::collection::vec(
            prop_oneof![
                prop::sample::select(FILES).prop_map(str::to_owned),
                Just("/abs/x.ts".to_owned()),
                Just("README.md".to_owned()),
                Just("src/z.mts".to_owned()),
            ], 0..5))
            .prop_map(|(n, files)| format!("\n---COVERED--- {n}\n{}", files.join("\n"))),
        1 => Just(String::new()),
        1 => Just("\n---COVERED---\n src/a.ts ".to_owned()),
    ];
    let body = prop_oneof![
        4 => prop::collection::vec(diag_line(), 1..30).prop_map(|l| l.join("\n")),
        1 => Just((0..22).map(|i| format!("lib/m{i}.ts(1,1): error TS2307: Cannot find module 'x'.")).collect::<Vec<_>>().join("\n")),
        2 => Just("NO_DIAGNOSTICS".to_owned()),
        1 => Just("NO_TSCONFIG".to_owned()),
        1 => Just("DEGRADED_SCHEMA_DRIFT".to_owned()),
        1 => Just("DEGRADED_NO_WORKTREE".to_owned()),
        1 => Just("something else".to_owned()),
    ];
    (
        prop_oneof![Just(""), Just("CREATED_WORKTREE /tmp/wt\n")],
        body,
        marker,
    )
        .prop_map(|(a, b, c)| format!("{a}{b}{c}"))
}

fn typecheck_case() -> impl Strategy<Value = Value> {
    (typecheck_text(), prop::sample::subsequence(FILES, 0..4))
        .prop_map(|(text, files)| json!({"text": text, "files": files}))
}

fn files_of(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap().to_owned())
        .collect()
}

fn rust_typecheck(c: &Value) -> Value {
    let mut logs = Vec::new();
    let g = apply_typecheck_result(
        c["text"].as_str().unwrap(),
        &files_of(&c["files"]),
        &mut logs,
    )
    .unwrap();
    json!({"groundedAvailable": g.available, "diagsByFile": g.diags_by_file.to_value(), "covered": g.covered, "logs": logs})
}

#[test]
fn typecheck_text_reads_as_the_workflow_reads_it() {
    let cases = sample(typecheck_case());
    let ret = TYPECHECK_SPAN.ret.replace("__LOGS__", LOGS);
    let params: Vec<Value> = cases
        .iter()
        .map(|c| json!({"text": c["text"], "prContext": {"files": c["files"]}, "panelCandidates": [], "log": LOG_FN}))
        .collect();
    let js = span(
        &Span {
            ret: &ret,
            ..TYPECHECK_SPAN
        },
        &params,
    );
    let rust: Vec<Value> = cases.iter().map(rust_typecheck).collect();
    assert_same("applyTypecheckResult", &cases, &rust, &js);
}

#[test]
fn the_generated_typecheck_cases_reach_every_branch() {
    let out: Vec<Value> = sample(typecheck_case())
        .iter()
        .map(rust_typecheck)
        .collect();
    let with = |s: &str| {
        out.iter()
            .filter(|o| o["logs"][0].as_str().unwrap_or("").contains(s))
            .count()
    };
    for s in [
        "no TS typecheck",
        "different Prisma schema",
        "degraded/unrecognized",
        "node_modules resolution broken",
        "coverage is unknown",
        "covers 0 of",
        "with diagnostics",
    ] {
        assert!(with(s) > 5, "{s}: {}", with(s));
    }
}

// ----------------------------------------------------------- grounded gate

const GATE_SPAN: Span = Span {
    start: "const groundedConfirmedList = [];",
    end: "panelInput = panelInput.map((finding, i) => ({ ...finding, slotId: i }));",
    deps: &["TSC-ORACLE"],
    pre: &[],
    fns: &["log", "groundedCovered"],
    ret: "{groundedConfirmedList, groundedRefutedList, panelInput, logs: __LOGS__}",
};

fn diags() -> impl Strategy<Value = Value> {
    prop::collection::vec(
        (
            prop::sample::select(FILES),
            prop_oneof![Just(9.0), Just(16.0), Just(100.0)],
        ),
        0..5,
    )
    .prop_map(|ds| {
        let mut m = serde_json::Map::new();
        for (f, l) in ds {
            let e = m.entry(f.to_owned()).or_insert_with(|| json!([]));
            e.as_array_mut()
                .unwrap()
                .push(json!({"file": f, "line": l, "code": "TS2345", "message": "m"}));
        }
        Value::Object(m)
    })
}

fn gate_case() -> impl Strategy<Value = Value> {
    (
        prop::collection::vec(candidate(), 0..6),
        any::<bool>(),
        diags(),
        prop::sample::subsequence(FILES, 0..4),
    )
        .prop_map(|(c, avail, d, cov)| json!({"panelCandidates": c, "groundedAvailable": avail, "diagsByFile": d, "covered": cov}))
}

fn diags_by_file(v: &Value) -> DiagsByFile {
    DiagsByFile::from_entries(
        v.as_object()
            .unwrap()
            .iter()
            .map(|(k, ds)| {
                (
                    k.clone(),
                    ds.as_array()
                        .unwrap()
                        .iter()
                        .map(|d| TscDiagnostic {
                            file: d["file"].as_str().unwrap().to_owned(),
                            line: d["line"].as_f64().unwrap(),
                            code: d["code"].as_str().unwrap().to_owned(),
                            message: d["message"].as_str().unwrap().to_owned(),
                        })
                        .collect(),
                )
            })
            .collect(),
    )
}

fn rust_gate(c: &Value) -> Value {
    let grounded = Grounded {
        available: c["groundedAvailable"].as_bool().unwrap(),
        diags_by_file: diags_by_file(&c["diagsByFile"]),
        covered: files_of(&c["covered"]),
    };
    let mut logs = Vec::new();
    let g = grounded_gate(
        c["panelCandidates"].as_array().unwrap(),
        &grounded,
        FILES
            .iter()
            .map(|f| (*f).to_owned())
            .collect::<Vec<_>>()
            .as_slice(),
        &mut logs,
    )
    .unwrap();
    json!({"groundedConfirmedList": g.confirmed, "groundedRefutedList": g.refuted, "panelInput": g.panel_input, "logs": logs})
}

#[test]
fn grounded_gate_matches_the_workflow() {
    let cases = sample(gate_case());
    let ret = GATE_SPAN.ret.replace("__LOGS__", LOGS);
    let params: Vec<Value> = cases
        .iter()
        .map(|c| {
            json!({
                "panelCandidates": c["panelCandidates"],
                "groundedAvailable": c["groundedAvailable"],
                "diagsByFile": c["diagsByFile"],
                "groundedCovered": format!("new Set({})", c["covered"]),
                "prContext": {"files": FILES},
                "log": LOG_FN,
            })
        })
        .collect();
    let js = span(
        &Span {
            ret: &ret,
            ..GATE_SPAN
        },
        &params,
    );
    let rust: Vec<Value> = cases.iter().map(rust_gate).collect();
    assert_same("grounded gate", &cases, &rust, &js);
}

#[test]
fn the_generated_gate_cases_reach_every_branch() {
    let out: Vec<Value> = sample(gate_case()).iter().map(rust_gate).collect();
    let n = |k: &str| {
        out.iter()
            .filter(|o| !o[k].as_array().unwrap().is_empty())
            .count()
    };
    assert!(
        n("groundedConfirmedList") > 30,
        "{}",
        n("groundedConfirmedList")
    );
    assert!(
        n("groundedRefutedList") > 10,
        "{}",
        n("groundedRefutedList")
    );
    let basename = out.iter().filter(|o| {
        o["groundedConfirmedList"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["finding"]["file"] == "service.ts")
    });
    assert!(basename.count() > 2);
}

// ------------------------------------------------- needsPanel and the budget

const NEEDS_PANEL: (&str, &str) = ("function needsPanel(finding) {", "  return true;\n}");
const AGENT_CONSTANTS: (&str, &str) = (
    "const AGENTS_FIXED_OVERHEAD = 12;",
    "const AGENTS_PER_REFINE = 3.7;",
);
const RETEST_STUB: &str = "(f) => f.retest === true";

const PLAN_SPAN: Span = Span {
    start: "const panelEligible = panelInput.filter((f) => needsPanel(f));",
    end: ".map((f) => ({ title: f.title, file: f.file, severity: f.severity }));",
    deps: &[],
    pre: &[NEEDS_PANEL, AGENT_CONSTANTS],
    fns: &["isRetestTarget"],
    ret: "{panelEligible, refineReserve, panelBudget, maxPanels, maxRefines, panelBound, panelSkippedForBudget}",
};

fn plan_case() -> impl Strategy<Value = Value> {
    (
        prop::collection::vec((candidate(), any::<bool>()), 0..40),
        prop_oneof![Just(40.0), Just(1.0), Just(3.0), Just(7.0)],
        prop_oneof![Just(150.0), Just(20.0), Just(40.0), Just(13.0)],
    )
        .prop_map(|(fs, maxp, maxa)| {
            let input: Vec<Value> = fs
                .into_iter()
                .enumerate()
                .map(|(i, (mut f, retest))| {
                    f["slotId"] = json!(i);
                    if retest && i % 3 == 0 {
                        f["retest"] = json!(true);
                    }
                    f
                })
                .collect();
            json!({"panelInput": input, "maxPanelsArg": maxp, "maxAgents": maxa})
        })
}

fn rust_plan(c: &Value) -> Value {
    let retest = |f: &Value| f["retest"] == json!(true);
    let p = panel_plan(
        c["panelInput"].as_array().unwrap(),
        &retest,
        c["maxPanelsArg"].as_f64().unwrap(),
        c["maxAgents"].as_f64().unwrap(),
    );
    json!({
        "panelEligible": p.eligible, "refineReserve": p.refine_reserve, "panelBudget": p.panel_budget,
        "maxPanels": p.max_panels, "maxRefines": p.max_refines, "panelBound": p.bound,
        "panelSkippedForBudget": p.skipped_for_budget,
    })
}

#[test]
fn panel_plan_matches_the_workflow() {
    let cases = sample(plan_case());
    let params: Vec<Value> = cases
        .iter()
        .map(|c| json!({"panelInput": c["panelInput"], "maxPanelsArg": c["maxPanelsArg"], "maxAgents": c["maxAgents"], "isRetestTarget": RETEST_STUB}))
        .collect();
    let js = span(&PLAN_SPAN, &params);
    let rust: Vec<Value> = cases.iter().map(rust_plan).collect();
    assert_same("panel plan", &cases, &rust, &js);
}

#[test]
fn needs_panel_matches_the_workflow() {
    let cases = sample(candidate().prop_flat_map(|c| (Just(c), any::<bool>())))
        .into_iter()
        .map(|(c, r)| json!({"finding": c, "retest": r}))
        .collect::<Vec<_>>();
    let js = span(
        &Span {
            start: NEEDS_PANEL.0,
            end: NEEDS_PANEL.1,
            deps: &[],
            pre: &[],
            fns: &["isRetestTarget"],
            ret: "needsPanel(finding)",
        },
        &cases
            .iter()
            .map(|c| json!({"finding": c["finding"], "isRetestTarget": format!("() => {}", c["retest"])}))
            .collect::<Vec<_>>(),
    );
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| json!(needs_panel(&c["finding"], c["retest"].as_bool().unwrap())))
        .collect();
    assert_same("needsPanel", &cases, &rust, &js);
}

#[test]
fn the_generated_plan_cases_reach_every_branch() {
    let out: Vec<Value> = sample(plan_case()).iter().map(rust_plan).collect();
    let skipped = out
        .iter()
        .filter(|o| !o["panelSkippedForBudget"].as_array().unwrap().is_empty())
        .count();
    assert!(skipped > 100, "{skipped}");
    let cases = sample(plan_case());
    let budget_bound = cases
        .iter()
        .zip(&out)
        .filter(|(c, o)| o["panelBudget"].as_f64() < c["maxPanelsArg"].as_f64())
        .count();
    assert!(budget_bound > 50, "{budget_bound}");
    let zero_refines = out.iter().filter(|o| o["maxRefines"] == json!(0.0)).count();
    assert!(zero_refines > 20, "{zero_refines}");
}

// -------------------------------------------------------------- the results

const NO_PANEL: (&str, &str) = (
    "const NO_PANEL_RESULT = (finding) => ({",
    "  refineOutcome: null,\n  escalated: false,\n});",
);

const RESULTS_SPAN: Span = Span {
    start: "const confirmedResults = groundedConfirmedList.map((c) => ({",
    end: "${droppedPrePanel.length} gated`,\n);",
    deps: &[],
    pre: &[NO_PANEL],
    fns: &["log"],
    ret: "{results, placedAside, surviving, preDecided, techRefuted, noPanel: NO_PANEL_RESULT({title: 'x'}), logs: __LOGS__}",
};

fn result() -> impl Strategy<Value = Value> {
    (
        candidate(),
        any::<bool>(),
        prop_oneof![Just(Value::Null), Just(json!({"files": ["src/a.ts"]}))],
        prop_oneof![
            Just(Value::Null),
            Just(json!({"placement": "pre-existing"})),
            Just(json!({"placement": null}))
        ],
        any::<bool>(),
    )
        .prop_map(|(f, survived, possibly, placement, pre)| {
            let mut v = no_panel_result(&f);
            v["survived"] = json!(survived);
            if !possibly.is_null() {
                v["possiblyPreDecided"] = possibly;
            }
            if !placement.is_null() {
                v["placement"] = placement;
            }
            v["preDecided"] = json!(pre);
            v
        })
}

fn results_case() -> impl Strategy<Value = Value> {
    (
        prop::collection::vec(prop_oneof![5 => result(), 1 => Just(Value::Null)], 0..6),
        prop::collection::vec(candidate(), 0..3),
        0usize..4,
    )
        .prop_map(|(verified, confirmed, dropped)| {
            let confirmed: Vec<Value> = confirmed
                .into_iter()
                .map(|f| json!({"finding": f, "grounded": {"verdict": "confirmed", "diagnostic": "src/a.ts(3): TS1: m"}}))
                .collect();
            json!({"verified": verified, "groundedConfirmedList": confirmed, "dropped": dropped})
        })
}

fn rust_results(c: &Value) -> Value {
    let verified: Vec<Option<Value>> = c["verified"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (!v.is_null()).then(|| v.clone()))
        .collect();
    let mut logs = Vec::new();
    let r = partition_results(
        &verified,
        c["groundedConfirmedList"].as_array().unwrap(),
        c["dropped"].as_u64().unwrap() as usize,
        &mut logs,
    );
    json!({
        "results": r.results, "placedAside": r.placed_aside, "surviving": r.surviving,
        "preDecided": r.pre_decided, "techRefuted": r.tech_refuted,
        "noPanel": no_panel_result(&json!({"title": "x"})), "logs": logs,
    })
}

#[test]
fn results_split_as_the_workflow_splits_them() {
    let cases = sample(results_case());
    let ret = RESULTS_SPAN.ret.replace("__LOGS__", LOGS);
    let params: Vec<Value> = cases
        .iter()
        .map(|c| {
            json!({
                "verified": c["verified"],
                "groundedConfirmedList": c["groundedConfirmedList"],
                "droppedPrePanel": vec![json!({}); c["dropped"].as_u64().unwrap() as usize],
                "log": LOG_FN,
            })
        })
        .collect();
    let js = span(
        &Span {
            ret: &ret,
            ..RESULTS_SPAN
        },
        &params,
    );
    let rust: Vec<Value> = cases.iter().map(rust_results).collect();
    assert_same("results", &cases, &rust, &js);
}

#[test]
fn the_generated_results_reach_every_bucket() {
    let out: Vec<Value> = sample(results_case()).iter().map(rust_results).collect();
    for k in ["placedAside", "surviving", "preDecided", "techRefuted"] {
        let n = out
            .iter()
            .filter(|o| !o[k].as_array().unwrap().is_empty())
            .count();
        assert!(n > 50, "{k}: {n}");
    }
}
