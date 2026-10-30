//! The Synthesize phase's glue against the workflow: prior findings carried
//! and re-checked, the survivors' payload, the verifiedBy anchors, the side
//! payloads, the wrong-ref audit, the verdict, the report prompt and the
//! report as posted.

mod common;

use std::collections::HashSet;
use std::path::PathBuf;

use common::{assert_same, sample, span, span_raw, Span, LOGS, LOG_FN};
use proptest::prelude::*;
use serde_json::{json, Value};
use tribunal_pipeline::synthesize::{
    anchored_payload, baseline_description, finish_report, graded, missing_identifiers,
    prior_check, prior_re_examined, ref_evidence, ref_evidence_targets, side_payloads,
    surviving_for_synthesis, synthesis_prompt, verdict, verified_anchors, verify_prior_prompt,
    Relay, SynthesisInput,
};
use tribunal_pipeline::verify::no_panel_result;

const FILES: &[&str] = &["src/a.ts", "src/b/service.ts", "src/c.ts", "web/d.tsx"];

fn sev() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(json!("critical")),
        Just(json!("important")),
        Just(json!("nit")),
        Just(Value::Null)
    ]
}

fn anchor() -> impl Strategy<Value = Value> {
    (
        prop::sample::select(FILES),
        prop_oneof![Just(json!(12)), Just(json!(0)), Just(json!(2.5))],
        prop_oneof![
            Just(json!("const x = await y();")),
            Just(json!("short")),
            Just(Value::Null)
        ],
    )
        .prop_map(|(f, l, q)| json!({"file": f, "line": l, "quote": q}))
}

fn finding() -> impl Strategy<Value = Value> {
    (
        prop::sample::select(FILES),
        "[a-z]{3,8} [a-z]{3,8}",
        sev(),
        prop_oneof![Just(json!(10)), Just(Value::Null)],
        prop::option::of(prop::collection::vec(anchor(), 0..3)),
        (any::<bool>(), any::<bool>()),
    )
        .prop_map(|(f, t, s, line, vb, (beh, risk))| {
            let mut x = json!({"file": f, "title": t, "description": "d", "evidence": "e"});
            if !s.is_null() {
                x["severity"] = s;
            }
            if !line.is_null() {
                x["line"] = line;
            }
            if let Some(vb) = vb {
                x["verifiedBy"] = json!(vb);
            }
            if beh {
                x["isBehaviorChanging"] = json!(true);
            }
            if risk {
                x["touchesHighRiskSurface"] = json!(true);
            }
            x
        })
}

fn vote() -> impl Strategy<Value = Value> {
    (
        prop_oneof![Just("REFUTED"), Just("NOT_REFUTED")],
        prop_oneof![Just("claude"), Just("codex"), Just("gemini")],
        prop_oneof![
            Just(json!(
                "the guard in `src/a.ts` calls `normalizeOrderId` first"
            )),
            Just(json!("`missingHelperName` is never reached")),
            Just(json!("no citation")),
            Just(Value::Null),
        ],
        prop_oneof![Just(Value::Null), Just(json!("not-triggered"))],
        prop_oneof![Just(Value::Null), Just(json!("PRE_DECIDED"))],
    )
        .prop_map(|(v, src, reason, kill, rt)| {
            let mut o = json!({"verdict": v, "source": src, "confidence": 0.7});
            if !reason.is_null() {
                o["reason"] = reason;
            }
            if !kill.is_null() {
                o["killReason"] = kill;
            }
            if !rt.is_null() {
                o["refutationType"] = rt;
                o["decisionQuestioned"] = json!("DEFERRED: the retry");
            }
            o
        })
}

fn result() -> impl Strategy<Value = Value> {
    (
        finding(),
        prop::collection::vec(vote(), 0..3),
        (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>()),
        prop_oneof![
            Just(Value::Null),
            Just(json!({"placement": "pre-existing", "proposedSeverity": "nit", "intendedBy": {"file": "CLAUDE.md", "rule": "r"}})),
            Just(json!({"placement": null, "proposedSeverity": null}))
        ],
        prop_oneof![
            Just(Value::Null),
            Just(json!({"verdict": "confirmed", "diagnostic": "src/a.ts(3): TS2322: m"}))
        ],
    )
        .prop_map(|(f, votes, (ran, unverified, debate, possibly), placement, grounded)| {
            let mut v = no_panel_result(&f);
            v["votes"] = json!(votes);
            v["panelRan"] = json!(ran);
            v["unverified"] = json!(unverified);
            v["refuteFraction"] = json!(0.5);
            if debate {
                v["debateFired"] = json!(true);
                v["debateOutcome"] = json!("held");
                v["debateRationale"] = json!("because");
            }
            if possibly {
                v["possiblyPreDecided"] = json!({"files": ["src/c.ts"], "decisionSource": "thread"});
            }
            if !placement.is_null() {
                v["placement"] = placement;
            }
            if !grounded.is_null() {
                v["grounded"] = grounded;
            }
            v
        })
}

// ------------------------------------------------------------ prior findings

const PRIOR_SPAN: Span = Span {
    start: "const priorOutcome = classifyPriorFindings(",
    end: ".join(\", \")}`,\n  );",
    deps: &["CARRY-FORWARD"],
    pre: &[],
    fns: &["log", "agent", "parallel", "diffBody", "__setup"],
    ret:
        "{priorOutcome, priorToVerify, priorReExamined, calls: globalThis.__calls, logs: __LOGS__}",
};

fn prior_finding() -> impl Strategy<Value = Value> {
    (
        prop_oneof![6 => prop::sample::select(FILES), 1 => Just("gone.ts")],
        "[a-z]{3,8} [a-z]{3,8}",
        sev(),
        prop_oneof![Just(json!(10)), Just(json!(90))],
    )
        .prop_map(|(f, t, s, l)| json!({"file": f, "title": t, "severity": s, "line": l}))
}

fn answer() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(
            json!({"verdict": "FIXED", "quote": "if (!orderId) return null; // guard", "reason": "r"})
        ),
        Just(json!({"verdict": "FIXED", "reason": "no quote"})),
        Just(
            json!({"verdict": "STILL_OPEN", "quote": "const x = await risky(input);", "reason": "r"})
        ),
        Just(json!({"verdict": "CANNOT_TELL", "reason": "r"})),
        Just(Value::Null),
        Just(json!("THROW")),
    ]
}

fn prior_case() -> impl Strategy<Value = Value> {
    (
        prop::collection::vec(prior_finding(), 0..20),
        prop::sample::subsequence(FILES, 0..4),
        prop::collection::vec(prior_finding(), 0..3),
        prop::collection::vec(answer(), 20),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(prior, changed, current, answers, scoped, adjacency)| {
            let unchanged: Vec<&str> = FILES.iter().filter(|f| !changed.contains(f)).copied().collect();
            let scope = if scoped {
                json!({"unchanged": unchanged, "gone": ["gone.ts"], "changed": changed, "added": [], "inScope": changed})
            } else {
                Value::Null
            };
            let adj = if adjacency {
                json!({"src/c.ts": [{"file": "src/a.ts", "matchConfidence": "high"}]})
            } else {
                json!({})
            };
            json!({
                "incrementalScope": scope,
                "priorReview": {"sha": "0123456789abcdef", "findings": prior, "source": "tribunal"},
                "current": current,
                "importAdjacency": adj,
                "answers": answers,
            })
        })
}

fn rust_prior(c: &Value) -> Value {
    let mut logs = Vec::new();
    let scope = (!c["incrementalScope"].is_null()).then_some(&c["incrementalScope"]);
    let check = prior_check(
        scope,
        Some(&c["priorReview"]),
        c["current"].as_array().unwrap(),
        &c["importAdjacency"],
        &mut logs,
    )
    .unwrap();
    let mut calls = Vec::new();
    let mut verified = Vec::new();
    for (i, p) in check.to_verify.iter().enumerate() {
        calls.push(json!({
            "prompt": verify_prior_prompt(p, &c["priorReview"], Some(&json!("feedfacecafebeef")), "acme/widgets", "DIFF\n"),
            "label": format!("verify-prior:{}", p["file"].as_str().unwrap_or("").rsplit('/').next().unwrap()),
        }));
        let a = &c["answers"][i];
        verified.push(if a == "THROW" {
            tribunal_pipeline::synthesize::dispatch_failed(p)
        } else {
            graded(p, (!a.is_null()).then_some(a))
        });
    }
    let re = prior_re_examined(&verified, &check, &mut logs);
    json!({
        "priorOutcome": check.outcome.to_json(),
        "priorToVerify": check.to_verify,
        "priorReExamined": re,
        "calls": calls,
        "logs": logs,
    })
}

#[test]
fn prior_findings_are_carried_and_rechecked_as_the_workflow_does() {
    let cases = sample(prior_case());
    let ret = PRIOR_SPAN.ret.replace("__LOGS__", LOGS);
    let params: Vec<Value> = cases
        .iter()
        .map(|c| {
            json!({
                "incrementalScope": c["incrementalScope"],
                "priorReview": c["priorReview"],
                "surviving": c["current"].as_array().unwrap().iter().map(|f| json!({"finding": f})).collect::<Vec<_>>(),
                "placedAside": [],
                "importAdjacency": c["importAdjacency"],
                "prContext": {"headRefOid": "feedfacecafebeef"},
                "repo": "acme/widgets",
                "reviewModel": "m",
                "VERIFIER_AGENT": "tribunal:finding-verifier",
                "log": LOG_FN,
                "agent": "async (p, o) => { globalThis.__calls.push({prompt: p, label: o.label}); const a = globalThis.__answers.shift(); if (a === 'THROW') throw new Error('x'); return a; }",
                "parallel": "async (ts) => Promise.all(ts.map((t) => t().catch(() => null)))",
                "diffBody": "(t) => 'DIFF\\n' + t",
                "__setup": format!("(globalThis.__calls = [], globalThis.__answers = {})", c["answers"]),
            })
        })
        .collect();
    let js = span(
        &Span {
            ret: &ret,
            ..PRIOR_SPAN
        },
        &params,
    );
    let rust: Vec<Value> = cases.iter().map(rust_prior).collect();
    assert_same("prior findings", &cases, &rust, &js);
}

#[test]
fn the_generated_prior_cases_reach_every_branch() {
    let out: Vec<Value> = sample(prior_case()).iter().map(rust_prior).collect();
    let n = |f: &dyn Fn(&Value) -> bool| out.iter().filter(|o| f(o)).count();
    let any_log = |s: &'static str| {
        move |o: &Value| {
            o["logs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l.as_str().unwrap().contains(s))
        }
    };
    assert!(
        n(&any_log("verification capped")) > 10,
        "{}",
        n(&any_log("verification capped"))
    );
    assert!(n(&any_log("re-checked")) > 100);
    assert!(n(&|o| !o["priorOutcome"]["carried"].as_array().unwrap().is_empty()) > 50);
    assert!(
        n(&|o| !o["priorOutcome"]["moot"].as_array().unwrap().is_empty()) > 5,
        "moot {}",
        n(&|o| !o["priorOutcome"]["moot"].as_array().unwrap().is_empty())
    );
    assert!(
        n(&|o| o["priorToVerify"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["escalatedByImportHeuristic"] == true))
            > 20
    );
    assert!(
        n(&|o| o["priorReExamined"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["verification"]["reason"] == "the verification dispatch failed"))
            > 20
    );
}

// ------------------------------------------------------- the survivors' rows

const SURVIVING_SPAN: Span = Span {
    start: "const survivingFindingsForSynthesis = verifiedGateDemoteUnanchored(",
    end: "      reason: vote.reason,\n    })),\n  })),\n);",
    deps: &["VERIFIED-GATE"],
    pre: &[],
    fns: &[],
    ret: "survivingFindingsForSynthesis",
};

#[test]
fn survivors_are_shaped_as_the_workflow_shapes_them() {
    let cases: Vec<Value> = sample(prop::collection::vec(result(), 0..4))
        .into_iter()
        .map(|r| json!({"surviving": r}))
        .collect();
    let js = span(&SURVIVING_SPAN, &cases);
    let mut rust: Vec<Value> = cases
        .iter()
        .map(|c| json!(surviving_for_synthesis(c["surviving"].as_array().unwrap())))
        .collect();
    // Declared difference: the port says how many refuters proposed the lower
    // severity, a field the Workflow does not write. It is present exactly
    // when a proposal is, and is compared away here.
    let mut counted = 0;
    for f in rust.iter_mut().flat_map(|r| r.as_array_mut().unwrap()) {
        let votes = f
            .as_object_mut()
            .unwrap()
            .shift_remove("panelProposedSeverityVotes");
        assert_eq!(
            votes.is_some(),
            f["panelProposedSeverity"].is_string(),
            "{f}"
        );
        counted += usize::from(votes.is_some());
    }
    assert!(counted > 20, "{counted}");
    assert_same("survivingFindingsForSynthesis", &cases, &rust, &js);
    let demoted = rust
        .iter()
        .filter(|r| {
            r.as_array()
                .unwrap()
                .iter()
                .any(|f| f.get("verifiedByGateNote").is_some())
        })
        .count();
    assert!(demoted > 50, "{demoted}");
}

// ------------------------------------------------------------- the anchors

const ANCHORS_SPAN: Span = Span {
    start: "let verifiedAnchors = null;",
    end: "went unchecked. This is NOT a finding that their quotes check out.`,\n      );\n    }\n  }\n}",
    deps: &[],
    pre: &[],
    fns: &["anchorResolverRelay", "log", "__setup"],
    ret: "{verifiedAnchors, payload: globalThis.__payload, logs: __LOGS__}",
};

fn relay() -> impl Strategy<Value = Value> {
    let status = prop_oneof![
        Just("found"),
        Just("found-shifted"),
        Just("not-found"),
        Just("file-not-at-ref"),
        Just("ref-not-found"),
        Just("lookup-failed"),
    ];
    let results = prop::collection::vec(
        (
            status,
            prop::sample::select(FILES),
            prop_oneof![Just(json!(3)), Just(Value::Null)],
        )
            .prop_map(|(s, f, l)| json!({"status": s, "file": f, "claimedLine": l})),
        0..4,
    );
    prop_oneof![
        6 => (results, any::<bool>(), prop_oneof![Just(Value::Null), Just(json!(2))], prop_oneof![Just(Value::Null), Just(json!(1)), Just(json!(0))])
            .prop_map(|(r, suspect, anchors, unresolved)| {
                let mut d = json!({"ran": true, "results": r, "suspectLookup": suspect});
                if !anchors.is_null() { d["anchors"] = anchors; }
                if !unresolved.is_null() { d["unresolved"] = unresolved; d["resolved"] = json!(1); }
                json!({"ok": true, "data": d})
            }),
        1 => Just(json!({"ok": true, "data": {"ran": false, "reason": "FATAL: x"}})),
        1 => Just(json!({"ok": true, "data": {"ran": false}})),
        1 => Just(json!({"ok": false, "data": null, "reason": "checksum mismatch"})),
        1 => Just(json!({"ok": false, "data": null})),
    ]
}

fn anchors_case() -> impl Strategy<Value = Value> {
    (
        prop::collection::vec(finding(), 0..4),
        prop_oneof![4 => Just(json!("feedfacecafebeef")), 1 => Just(Value::Null)],
        relay(),
    )
        .prop_map(|(f, head, relay)| json!({"findings": f, "head": head, "relay": relay}))
}

fn rust_anchors(c: &Value) -> Value {
    let mut logs = Vec::new();
    let findings = c["findings"].as_array().unwrap();
    let (n, payload) = anchored_payload(findings);
    let r = &c["relay"];
    let relay = Relay {
        ok: r["ok"] == true,
        data: (!r["data"].is_null()).then(|| r["data"].clone()),
        reason: r.get("reason").and_then(Value::as_str).map(str::to_owned),
    };
    let head = (!c["head"].is_null()).then_some(&c["head"]);
    let ran = n > 0 && head.is_some();
    let va = verified_anchors(n, head, "acme/widgets", ran.then_some(&relay), &mut logs);
    json!({"verifiedAnchors": va, "payload": if ran { json!(payload) } else { Value::Null }, "logs": logs})
}

#[test]
fn anchors_read_as_the_workflow_reads_them() {
    let cases = sample(anchors_case());
    let ret = ANCHORS_SPAN.ret.replace("__LOGS__", LOGS);
    let params: Vec<Value> = cases
        .iter()
        .map(|c| {
            json!({
                "survivingFindingsForSynthesis": c["findings"],
                "prContext": {"headRefOid": c["head"]},
                "repo": "acme/widgets",
                "anchorResolverRelay": "async (payload) => { globalThis.__payload = payload; return globalThis.__relay; }",
                "log": LOG_FN,
                "__setup": format!("(globalThis.__payload = null, globalThis.__relay = {})", c["relay"]),
            })
        })
        .collect();
    let js = span(
        &Span {
            ret: &ret,
            ..ANCHORS_SPAN
        },
        &params,
    );
    let rust: Vec<Value> = cases.iter().map(rust_anchors).collect();
    assert_same("verifiedAnchors", &cases, &rust, &js);
    let logs: Vec<String> = rust
        .iter()
        .flat_map(|r| {
            r["logs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l.as_str().unwrap().to_owned())
        })
        .collect();
    for s in [
        "LOOKUP failed",
        "could NOT be found",
        "all ",
        "relay-not-verified",
        "dispatch-returned-nothing",
        "FATAL",
        "no-results-array",
        "no head ref",
    ] {
        assert!(logs.iter().filter(|l| l.contains(s)).count() > 5, "{s}");
    }
}

// --------------------------------------------------------- side payloads

const SIDE_SPAN: Span = Span {
    start: "const placedAsideForSynthesis = placedAside.map((v) => ({",
    end: "    rationale: v.debateRationale || \"\",\n  }));",
    deps: &[],
    pre: &[],
    fns: &[],
    ret: "{placedAsideForSynthesis, preDecidedForSynthesis, baselineDescription, techRefutedForSynthesis, debateOutcomesForSynthesis}",
};

fn prior_review() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        Just(
            json!({"sha": "0123456789abcdef", "source": "tribunal", "ts": "2026-10-20T10:00:00Z"})
        ),
        Just(json!({"sha": "0123456789abcdef", "source": "tribunal"})),
        Just(
            json!({"sha": "abcdef0123456789", "source": "human-review", "reviewState": "CHANGES_REQUESTED", "reviewer": "someone"})
        ),
        Just(
            json!({"sha": "abcdef0123456789", "source": "human-review", "reviewState": "APPROVED"})
        ),
    ]
}

#[test]
fn side_payloads_match_the_workflow() {
    let cases: Vec<Value> = sample((
        prop::collection::vec(result(), 0..3),
        prop::collection::vec(result(), 0..3),
        prop::collection::vec(result(), 0..3),
        prop::collection::vec(result(), 0..3),
        prior_review(),
    ))
    .into_iter()
    .map(|(mut a, b, c, d, p)| {
        for v in &mut a {
            if v.get("placement").is_none() {
                v["placement"] = json!({"placement": "required-by-convention", "intendedBy": {"file": "CLAUDE.md", "rule": "r"}});
            }
        }
        json!({"placedAside": a, "preDecided": b, "techRefuted": c, "surviving": d, "priorReview": p})
    })
    .collect();
    let js = span(&SIDE_SPAN, &cases);
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| {
            let arr = |k: &str| c[k].as_array().unwrap().clone();
            let s = side_payloads(&arr("placedAside"), &arr("preDecided"), &arr("techRefuted"), &arr("surviving"));
            let p = (!c["priorReview"].is_null()).then_some(&c["priorReview"]);
            json!({
                "placedAsideForSynthesis": s.placed_aside, "preDecidedForSynthesis": s.pre_decided,
                "baselineDescription": baseline_description(p),
                "techRefutedForSynthesis": s.tech_refuted, "debateOutcomesForSynthesis": s.debate_outcomes,
            })
        })
        .collect();
    assert_same("side payloads", &cases, &rust, &js);
}

// ------------------------------------------------------ wrong-ref evidence

const REF_SPAN: Span = Span {
    start: "let refEvidence = { ran: false, checkedVotes: 0, suspectVotes: 0, suspects: [] };",
    end: "} else refEvidence = { ran: true, checkedVotes: 0, suspectVotes: 0, suspects: [] };\n}",
    deps: &["REF-EVIDENCE-CHECK"],
    pre: &[],
    fns: &["agent", "log"],
    ret: "{refEvidence, logs: __LOGS__}",
};

fn checkout() -> PathBuf {
    let root = std::env::temp_dir().join(format!("tribunal-refev-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let wt = root.join("wt");
    std::fs::create_dir_all(wt.join("src/deep")).unwrap();
    std::fs::create_dir_all(wt.join(".hidden")).unwrap();
    std::fs::create_dir_all(root.join("elsewhere")).unwrap();
    std::fs::write(
        wt.join("src/deep/x.ts"),
        "export function normalizeOrderId() {}\n",
    )
    .unwrap();
    std::fs::write(wt.join(".hidden/y.txt"), "mentions src/a.ts here\n").unwrap();
    std::fs::write(root.join("elsewhere/z.ts"), "missingHelperName\n").unwrap();
    std::os::unix::fs::symlink(root.join("elsewhere"), wt.join("linked")).unwrap();
    std::fs::write(
        root.join("outside.ts"),
        "normalizeOrderId missingHelperName\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(root.join("outside.ts"), wt.join("src/lnk.ts")).unwrap();
    wt
}

#[test]
fn the_wrong_ref_audit_matches_the_workflow_and_its_grep() {
    let wt = checkout();
    let cases: Vec<Value> = sample((prop::collection::vec(result(), 0..4), any::<bool>()))
        .into_iter()
        .map(|(r, ok)| json!({"results": r, "ok": ok}))
        .collect();
    let ret = REF_SPAN.ret.replace("__LOGS__", LOGS);
    let params: Vec<Value> = cases
        .iter()
        .map(|c| {
            json!({
                "results": c["results"],
                "headPin": {"ok": c["ok"], "path": wt.to_string_lossy()},
                "prContext": {"headRefOid": "feedfacecafebeef"},
                "agent": "async (p) => { const cp = process.getBuiltinModule('node:child_process'); return cp.execSync(p.slice(p.indexOf('\\n\\n') + 2), {shell: '/bin/bash'}).toString(); }",
                "log": LOG_FN,
                "MECHANICAL": {},
            })
        })
        .collect();
    let js = span(
        &Span {
            ret: &ret,
            ..REF_SPAN
        },
        &params,
    );
    let head = json!("feedfacecafebeef");
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| {
            let mut logs = Vec::new();
            let re = if c["ok"] == true {
                let (to_check, all) = ref_evidence_targets(c["results"].as_array().unwrap());
                let missing: HashSet<String> = missing_identifiers(&wt, &all);
                ref_evidence(&to_check, &missing, Some(&head), &mut logs)
            } else {
                json!({"ran": false, "checkedVotes": 0, "suspectVotes": 0, "suspects": []})
            };
            json!({"refEvidence": re, "logs": logs})
        })
        .collect();
    assert_same("refEvidence", &cases, &rust, &js);
    let suspects = rust
        .iter()
        .filter(|r| r["refEvidence"]["suspectVotes"].as_u64() > Some(0))
        .count();
    let clean = rust
        .iter()
        .filter(|r| {
            r["refEvidence"]["checkedVotes"].as_u64() > Some(0)
                && r["refEvidence"]["suspectVotes"] == 0
        })
        .count();
    assert!(suspects > 30 && clean > 20, "{suspects} {clean}");
    let _ = std::fs::remove_dir_all(wt.parent().unwrap());
}

// --------------------------------------------------------------- the verdict

const VERDICT_SPAN: Span = Span {
    start: "const unresolvedPriorBlockers = unresolvedPriorBlockersFrom(",
    end: "    : verdictGuessRaw;",
    deps: &["CARRY-FORWARD", "HIGH-RISK-SURFACE", "PROFILE-LOOKUP"],
    pre: &[],
    fns: &[],
    ret: "{unresolvedPriorBlockers, verdictGuessRaw, sweepUnconfirmed, verdictGuess}",
};

fn prior_with_verification() -> impl Strategy<Value = Value> {
    (
        prior_finding(),
        prop_oneof![
            Just(Value::Null),
            Just(json!("FIXED")),
            Just(json!("CANNOT_TELL"))
        ],
    )
        .prop_map(|(mut p, v)| {
            if !v.is_null() {
                p["verification"] = json!({"verdict": v});
            }
            if p["severity"] == "critical" {
                p["blocking"] = json!(true);
            }
            p
        })
}

fn verdict_case() -> impl Strategy<Value = Value> {
    (
        prop::collection::vec(prior_with_verification(), 0..3),
        prop::collection::vec(prior_with_verification(), 0..3),
        prop::collection::vec(result(), 0..4),
        prop_oneof![
            Just(Value::Null),
            Just(json!({"unparseable": true})),
            Just(json!({"ran": false, "skippedReason": "no-pinned-head"})),
            Just(json!({"ran": true, "clean": true}))
        ],
        prop_oneof![3 => Just(0usize), 1 => Just(1usize), 1 => Just(2usize)],
    )
        .prop_map(|(carried, re, results, sweep, unreturned): (Vec<Value>, Vec<Value>, Vec<Value>, Value, usize)| {
            let unreturned: Vec<Value> = (0..unreturned).map(|i| json!({"lens": format!("lens{i}"), "files": null})).collect();
            json!({"carried": carried, "re": re, "surviving": surviving_for_synthesis(&results), "cleanSweep": sweep, "lensesUnreturned": unreturned})
        })
}

#[test]
fn the_verdict_matches_the_workflow() {
    let profile = json!({"riskPaths": [{"focus": "security", "pattern": "b/service"}, {"focus": "money", "pattern": "web/"}]});
    let cases = sample(verdict_case());
    let params: Vec<Value> = cases
        .iter()
        .map(|c| json!({"carriedForward": c["carried"], "priorReExamined": c["re"], "survivingFindingsForSynthesis": c["surviving"], "PROFILE": profile, "cleanSweep": c["cleanSweep"], "lensesUnreturned": c["lensesUnreturned"]}))
        .collect();
    let js = span(&VERDICT_SPAN, &params);
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| {
            let sweep = (!c["cleanSweep"].is_null()).then_some(&c["cleanSweep"]);
            let v = verdict(
                c["carried"].as_array().unwrap(),
                c["re"].as_array().unwrap(),
                c["surviving"].as_array().unwrap(),
                &profile,
                sweep,
                c["lensesUnreturned"].as_array().unwrap().len(),
            )
            .unwrap();
            json!({"unresolvedPriorBlockers": v.unresolved_prior_blockers, "verdictGuessRaw": v.raw, "sweepUnconfirmed": v.sweep_unconfirmed, "verdictGuess": v.guess})
        })
        .collect();
    assert_same("verdict", &cases, &rust, &js);
    for v in ["APPROVE", "COMMENT", "REQUEST CHANGES"] {
        assert!(
            rust.iter().filter(|r| r["verdictGuessRaw"] == v).count() > 30,
            "{v}"
        );
    }
    assert!(
        rust.iter()
            .filter(|r| r["verdictGuessRaw"] == "APPROVE" && r["verdictGuess"] == "COMMENT")
            .count()
            > 10
    );
}

// -------------------------------------------------------- the report prompt

const SYNTH_SPAN: Span = Span {
    start: "let report = await agent(",
    end: "{ label: \"synthesize\", phase: \"Synthesize\", model: reviewModel, agentType: SCRIBE_AGENT },\n);",
    deps: &[],
    pre: &[],
    fns: &["agent"],
    ret: "globalThis.__prompt",
};

fn synth_case() -> impl Strategy<Value = Value> {
    let list = || prop::collection::vec(finding(), 0..2);
    (
        (prior_review(), any::<bool>(), any::<bool>(), list(), list(), list(), list()),
        (list(), list(), list(), list(), list(), list()),
        (
            prop_oneof![Just(Value::Null), Just(json!({"comments": [{"index": 0, "classification": "VALID"}]})), Just(json!({}))],
            any::<bool>(),
            prop_oneof![Just(""), Just("The critic says nothing is missing.")],
            prop_oneof![Just(Value::Null), Just(json!({"ran": true, "clean": true, "unparseable": false}))],
            prop_oneof![Just(Value::Null), Just(json!({"reason": "output ceiling", "spentOutput": 1200000}))],
            0usize..4,
            prop_oneof![Just("APPROVE"), Just("COMMENT"), Just("REQUEST CHANGES")],
        ),
    )
        .prop_map(|((prior, scoped, fallback, carried, moot, still, re), (surv, pre, placed, tech, dropped, debates), (bot, vet, critic, sweep, cost, stages, verdict))| {
            let human = prior["source"] == "human-review";
            let scope = if scoped && !prior.is_null() {
                json!({"inScope": ["src/a.ts", null, "src/c.ts"], "unchanged": ["web/d.tsx"]})
            } else {
                Value::Null
            };
            json!({
                "prNumber": 77,
                "prContext": {"title": "Fix the {{title}} thing", "author": "dev", "jiraTicket": if fallback { json!("TICKET-1") } else { Value::Null }},
                "incrementalFallback": if fallback { json!({"expected": true, "baseline": "0011223344556677", "source": if human { "human-review" } else { "tribunal" }, "reason": "2 path(s) absent"}) } else { Value::Null },
                "incrementalScope": scope,
                "allPrFiles": ["src/a.ts", "src/b/service.ts", "src/c.ts", "web/d.tsx"],
                "priorReview": prior,
                "carriedForward": carried, "mootFindings": moot,
                "priorOutcome": {"stillOpen": still},
                "priorReExamined": re,
                "survivingFindingsForSynthesis": surv,
                "priorReviews": [{"author": "rev", "state": "CHANGES_REQUESTED", "at": null, "summary": "s"}],
                "preDecidedForSynthesis": pre, "placedAsideForSynthesis": placed,
                "techRefutedForSynthesis": tech, "droppedPrePanel": dropped,
                "groundedRefutedList": debates.iter().map(|f| json!({"finding": f})).collect::<Vec<_>>(),
                "debateOutcomesForSynthesis": [],
                "botVetting": bot, "vetBotComments": vet, "criticOutput": critic,
                "cleanSweep": sweep, "refinesSkippedForBudget": if stages > 2 { json!([{"title": "t"}]) } else { json!([]) },
                "costBreakerFired": cost, "costDegradedStages": vec![json!({}); stages],
                "verdictGuess": verdict,
            })
        })
}

fn rust_synth(c: &Value) -> String {
    let arr = |k: &str| c[k].as_array().unwrap().clone();
    let opt = |k: &str| (!c[k].is_null()).then_some(&c[k]);
    let prior = opt("priorReview");
    let baseline = baseline_description(prior);
    synthesis_prompt(&SynthesisInput {
        pr_number: &c["prNumber"],
        pr_context: &c["prContext"],
        repo: "acme/widgets",
        incremental_fallback: opt("incrementalFallback"),
        incremental_scope: opt("incrementalScope"),
        all_pr_files: c["allPrFiles"].as_array().unwrap().len(),
        baseline_description: &baseline,
        prior_review: prior,
        carried_forward: &arr("carriedForward"),
        moot: &arr("mootFindings"),
        still_open: c["priorOutcome"]["stillOpen"].as_array().unwrap(),
        prior_re_examined: &arr("priorReExamined"),
        surviving: &arr("survivingFindingsForSynthesis"),
        prior_reviews: &c["priorReviews"],
        pre_decided: &arr("preDecidedForSynthesis"),
        placed_aside: &arr("placedAsideForSynthesis"),
        tech_refuted: &arr("techRefutedForSynthesis"),
        dropped_pre_panel: &arr("droppedPrePanel"),
        grounded_refuted: &arr("groundedRefutedList"),
        debate_outcomes: &arr("debateOutcomesForSynthesis"),
        bot_vetting: opt("botVetting"),
        vet_bot_comments: c["vetBotComments"] == true,
        critic_output: c["criticOutput"].as_str().unwrap(),
        clean_sweep: opt("cleanSweep"),
        refines_skipped_for_budget: &arr("refinesSkippedForBudget"),
        cost_breaker_fired: opt("costBreakerFired"),
        cost_degraded_stages: c["costDegradedStages"].as_array().unwrap().len(),
        verdict: c["verdictGuess"].as_str().unwrap(),
    })
}

#[test]
fn the_report_prompt_is_the_workflows() {
    let cases = sample(synth_case());
    let params: Vec<Value> = cases
        .iter()
        .map(|c| {
            let mut p = c.clone();
            p["repo"] = json!("acme/widgets");
            p["baselineDescription"] = json!(baseline_description(
                (!c["priorReview"].is_null()).then_some(&c["priorReview"])
            ));
            p["T_KILL"] = json!(0.45);
            p["reviewModel"] = json!("m");
            p["SCRIBE_AGENT"] = json!("s");
            p["agent"] = json!("async (p) => { globalThis.__prompt = p; return 'r'; }");
            p
        })
        .collect();
    let js = span(&SYNTH_SPAN, &params);
    let rust: Vec<Value> = cases.iter().map(|c| json!(rust_synth(c))).collect();
    assert_same("synthesis prompt", &cases, &rust, &js);
    let has = |s: &str| {
        rust.iter()
            .filter(|r| r.as_str().unwrap().contains(s))
            .count()
    };
    for s in [
        "THIS IS AN INCREMENTAL RE-REVIEW",
        "COULD NOT BE APPLIED",
        "MOOT —",
        "STILL OPEN —",
        "RE-CHECKED —",
        "COST BREAKER",
        "CLEAN VERDICT SWEEP",
        "refine_skipped_for_budget",
        "### Carried forward",
        "turned off with",
        "(not run — small PR)",
        "human review, so there",
    ] {
        assert!(has(s) > 10, "{s}: {}", has(s));
    }
}

// ------------------------------------------------------ the report as posted

const FINISH_SPAN: Span = Span {
    start: "const reportVerdictPin =",
    end: "const reportAsPosted = stripDraftCommentsFromBody(report);",
    deps: &["REPORT-VERDICT", "DRAFT-COMMENTS-STRIP", "REPORT-BANNERS"],
    pre: &[],
    fns: &["log"],
    ret: "{report, corrected: reportVerdictPin.corrected, reportAsPosted, logs: __LOGS__}",
};

fn report_text() -> impl Strategy<Value = String> {
    let verdict_line = prop_oneof![
        Just("### Verdict suggestion: APPROVE\n"),
        Just("### Verdict suggestion: COMMENT\n"),
        Just("#### Verdict suggestion: APPROVE or COMMENT\n"),
        Just(""),
        Just("### Verdict suggestion: APPROVE\n### Verdict suggestion: COMMENT\n"),
    ];
    let drafts = prop_oneof![
        Just(""),
        Just("\n### Draft comments (ready to paste)\n- one\n- two\n"),
        Just("\n### Draft comments\n- x\n\n### After\nmore\n")
    ];
    (
        prop_oneof![Just("## Peer review: PR #7 — t\n\n"), Just("")],
        verdict_line,
        Just("### Blocking\n1. x\n"),
        drafts,
    )
        .prop_map(|(a, b, c, d)| format!("{a}{b}{c}{d}"))
}

#[test]
fn the_posted_report_matches_the_workflow() {
    let cases: Vec<Value> = sample((
        prop_oneof![5 => report_text().prop_map(Value::String), 1 => Just(json!("   "))],
        prop_oneof![Just("APPROVE"), Just("COMMENT"), Just("REQUEST CHANGES")],
        any::<bool>(),
        prop_oneof![
            Just(json!({"ran": false, "checkedVotes": 0, "suspectVotes": 0, "suspects": []})),
            Just(json!({"ran": true, "checkedVotes": 2, "suspectVotes": 0, "suspects": []})),
            Just(json!({"ran": true, "checkedVotes": 2, "suspectVotes": 1, "suspects": [{"source": "codex", "missing": ["fooBarBazQux", "src/x.ts"]}]})),
        ],
        prop_oneof![
            3 => Just(json!([])),
            1 => Just(json!([{"lens": "correctness", "files": ["src/a.ts", "src/b.ts"]}])),
            1 => Just(json!([{"lens": "security", "files": null}, {"lens": "lens3", "files": []}])),
        ],
    ))
    .into_iter()
    .map(|(r, v, ok, re, unreturned)| json!({"report": r, "verdictGuess": v, "headPin": {"ok": ok}, "refEvidence": re, "prContext": {"headRefOid": "feedfacecafebeef"}, "lensesUnreturned": unreturned, "lensPlan": [{}, {}, {}]}))
    .collect();
    let ret = FINISH_SPAN.ret.replace("__LOGS__", LOGS);
    let params: Vec<Value> = cases
        .iter()
        .map(|c| {
            let mut p = c.clone();
            p["log"] = json!(LOG_FN);
            p
        })
        .collect();
    let js = span(
        &Span {
            ret: &ret,
            ..FINISH_SPAN
        },
        &params,
    );
    let head = json!("feedfacecafebeef");
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| {
            let mut logs = Vec::new();
            let r = finish_report(
                c["report"].as_str(),
                c["verdictGuess"].as_str().unwrap(),
                c["headPin"]["ok"] == true,
                &c["refEvidence"],
                Some(&head),
                c["lensesUnreturned"].as_array().unwrap(),
                c["lensPlan"].as_array().unwrap().len(),
                &mut logs,
            )
            .unwrap();
            json!({"report": r.report, "corrected": r.corrected, "reportAsPosted": r.as_posted, "logs": logs})
        })
        .collect();
    assert_same("report as posted", &cases, &rust, &js);
    let bannered: Vec<&Value> = rust
        .iter()
        .filter(|r| {
            r["reportAsPosted"]
                .as_str()
                .unwrap()
                .contains("returned nothing**")
        })
        .collect();
    let with_drafts = cases
        .iter()
        .zip(&rust)
        // Drafts as the last section: a banner appended after them would go
        // with them in the strip, which is what withBanners exists to avoid.
        .filter(|(c, _)| {
            let r = c["report"].as_str().unwrap();
            r.contains("### Draft comments") && !r.contains("### After")
        })
        .filter(|(c, _)| !c["lensesUnreturned"].as_array().unwrap().is_empty())
        .filter(|(_, r)| {
            r["reportAsPosted"]
                .as_str()
                .unwrap()
                .contains("returned nothing**")
        })
        .count();
    assert!(
        bannered.len() > 20 && with_drafts > 10,
        "{} {with_drafts}",
        bannered.len()
    );
}

// A synthesis that returned nothing stops the run in both engines, with the
// same message, instead of the TypeError the draft strip used to throw.
const NO_REPORT_SPAN: Span = Span {
    start: "if (typeof report !== \"string\") {",
    end: "const reportAsPosted = stripDraftCommentsFromBody(report);",
    deps: &["REPORT-VERDICT", "DRAFT-COMMENTS-STRIP", "REPORT-BANNERS"],
    pre: &[],
    fns: &["log"],
    ret: "{reportAsPosted}",
};

#[test]
fn an_empty_synthesis_stops_both_engines() {
    let base = json!({"verdictGuess": "APPROVE", "headPin": {"ok": true}, "refEvidence": {"ran": false}, "prContext": {"headRefOid": "feedface"}, "lensesUnreturned": [], "lensPlan": [{}], "log": LOG_FN});
    let mut empty = base.clone();
    empty["report"] = Value::Null;
    let mut written = base.clone();
    written["report"] = json!("## Review\n\n### Verdict suggestion: APPROVE\n");
    let js = span_raw(&NO_REPORT_SPAN, &[empty, written]);
    let rust = |report: Option<&str>| {
        finish_report(
            report,
            "APPROVE",
            true,
            &json!({"ran": false}),
            None,
            &[],
            1,
            &mut Vec::new(),
        )
    };
    assert_eq!(js[0], Err("the synthesis returned no report".to_owned()));
    assert_eq!(
        rust(None).map(|r| r.as_posted),
        Err("the synthesis returned no report".to_owned())
    );
    assert!(js[1].is_ok(), "{:?}", js[1]);
    assert!(rust(Some("## Review\n\n### Verdict suggestion: APPROVE\n")).is_ok());
}
