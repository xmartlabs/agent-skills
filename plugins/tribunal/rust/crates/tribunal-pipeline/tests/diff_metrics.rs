//! The metrics row against the workflow's `metrics` object, built from the
//! same state.
//!
//! The workflow's side is read from `tests/goldens/span/diff_metrics.json`
//! (see `common`), with a SHA-256 of the workflow code each span came from, so
//! an edit there without regenerating fails here. Each row is over 1 KiB, so
//! the file keeps a SHA-256 of each; a failure shows the port's row and the
//! input.
//! Regenerate after changing a span, its cases or the workflow:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-pipeline --test diff_metrics`.

#![recursion_limit = "1024"]

mod common;

use common::{assert_same, sample, span, Span};
use proptest::prelude::*;
use serde_json::{json, Value};
use tribunal_pipeline::metrics::{metrics_row, MetricsScope};
use tribunal_pipeline::verify::no_panel_result;

const FILES: &[&str] = &["src/a.ts", "src/b/service.ts", "web/c.tsx"];

fn finding() -> impl Strategy<Value = Value> {
    (
        prop::sample::select(FILES),
        prop_oneof![Just(json!("critical")), Just(json!("important")), Just(json!("nit"))],
        prop_oneof![Just(json!(10)), Just(Value::Null)],
        (any::<bool>(), any::<bool>(), any::<bool>()),
        prop_oneof![Just(Value::Null), Just(json!(["correctness", "security"]))],
        prop_oneof![Just(Value::Null), Just(json!({"evidenceLocated": true, "matchKind": "exact"}))],
        prop_oneof![Just(Value::Null), Just(json!(7)), Just(json!(2.5))],
    )
        .prop_map(|(file, sev, line, (risk, beh, re), merged, res, from)| {
            // What the evidence gate reads: no anchor, a good one, one too
            // short. Taken from values already drawn, so the sample, and the
            // coverage counts below, stay what they were.
            let verified_by = match (beh, line.is_null()) {
                (false, _) => Value::Null,
                (true, false) => json!([{"file": "src/a.ts", "line": 3, "quote": "const total = sum(items);"}]),
                (true, true) => json!([{"file": "src/a.ts", "line": 3, "quote": "short"}]),
            };
            let mut f = json!({"title": "retry guard missing in handler", "file": file, "severity": sev, "description": "d", "evidence": "e", "lens": "correctness"});
            if !line.is_null() {
                f["line"] = line;
            }
            if risk {
                f["touchesHighRiskSurface"] = json!(true);
            }
            if beh {
                f["isBehaviorChanging"] = json!(1);
            }
            if re {
                f["reanchored"] = json!({"via": "verifiedBy"});
            }
            if !merged.is_null() {
                f["mergedFrom"] = merged;
                f["mergedLocations"] = json!([{"file": "src/a.ts", "line": 10}, {"file": "web/c.tsx", "line": 4}]);
            }
            if !res.is_null() {
                f["evidenceResolution"] = res;
            }
            if !from.is_null() {
                f["lineCorrectedFrom"] = from;
            }
            if !verified_by.is_null() {
                f["verifiedBy"] = verified_by;
            }
            f
        })
}

fn vote() -> impl Strategy<Value = Value> {
    (
        prop_oneof![Just("claude"), Just("codex"), Just("gemini"), Just("")],
        prop_oneof![Just("REFUTED"), Just("NOT_REFUTED")],
        prop_oneof![
            Just(Value::Null),
            Just(json!("PRE_DECIDED")),
            Just(json!("TECHNICALLY_WRONG"))
        ],
        prop_oneof![Just(Value::Null), Just(json!("not-triggered"))],
        prop_oneof![Just(Value::Null), Just(json!(true)), Just(json!("yes"))],
    )
        .prop_map(|(src, v, rt, kill, quote)| {
            let mut o = json!({"verdict": v, "confidence": 0.8});
            if !src.is_empty() {
                o["source"] = json!(src);
            }
            if !rt.is_null() {
                o["refutationType"] = rt;
                o["decisionQuestioned"] = json!("DEFERRED: x");
            }
            if !kill.is_null() {
                o["killReason"] = kill;
            }
            if !quote.is_null() {
                o["quoteFound"] = quote;
            }
            o
        })
}

fn result() -> impl Strategy<Value = Value> {
    (
        finding(),
        prop::collection::vec(vote(), 0..4),
        (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>()),
        prop_oneof![
            Just(Value::Null),
            Just(json!({"verdict": "confirmed", "diagnostic": "src/a.ts(3): TS2322: m"}))
        ],
    )
        .prop_map(|(f, votes, (ran, surv, refined, unverified), grounded)| {
            let mut v = no_panel_result(&f);
            v["votes"] = json!(votes);
            v["panelRan"] = json!(ran);
            v["survived"] = json!(surv);
            v["refined"] = json!(refined);
            v["unverified"] = json!(unverified);
            v["refuteFraction"] = json!(0.3);
            if !grounded.is_null() {
                v["grounded"] = grounded;
            }
            if ran {
                v["severityAtIntake"] = json!("important");
            }
            v
        })
}

fn scope() -> impl Strategy<Value = Value> {
    (
        prop::collection::vec(result(), 0..5),
        (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>()),
        prop_oneof![Just(Value::Null), Just(json!({"inScope": ["src/a.ts"], "unchanged": ["web/c.tsx", 3], "changed": [], "added": []}))],
        prop_oneof![Just(json!(1234)), Just(json!(0)), Just(Value::Null)],
        prop_oneof![Just(json!("")), Just(json!(" me "))],
        prop_oneof![Just(Value::Null), Just(json!("linux"))],
        prop::collection::vec(prop_oneof![Just(json!("src/x.ts")), Just(json!("")), Just(json!(4))], 0..3),
    )
        .prop_map(|(results, (codex, gemini, blast, critic), inc, diff_bytes, viewer, platform, not_reached)| {
            json!({
                "results": results,
                "tokensAtStart": 100, "tokensAtContextEnd": 300, "tokensAtReviewEnd": 900, "tokensAtVerifyEnd": 1500, "tokensAtEnd": 1800, "counterResets": 0,
                "prContext": {"headRefOid": "feedfacecafebeef", "diffBytes": diff_bytes, "additions": 10, "platform": "darwin", "viewer": viewer,
                              "inThreadDecisions": [{"category": "DEFERRED", "summary": "retries", "source": "thread", "files": "x"}]},
                "diffMode": "path", "lensReceiverCount": 2, "refuterReceiverCount": 3, "sectioningAvailable": true, "sectionCount": 3,
                "sectionedReceiverCount": 0, "fullDiffReceiverCount": 5, "deliveredBytes": 5000, "unsectionedFiles": [], "diffUnparsedHeaders": [],
                "repo": "acme/widgets", "prNumber": 77, "platform": platform, "refEvidence": {"ran": true},
                "headPin": {"ok": true, "kind": "worktree", "path": "/x"}, "pinnedSha": "abc", "fileCount": 3,
                "lensPlan": [{"focus": "f1", "files": ["src/a.ts"]}, {"focus": "f2"}], "lensAgent": "tribunal:code-review-expert",
                "profileSha256": "p", "profileSource": "s", "projectRulesSha256": null, "projectRulesSource": null,
                "conventionsManifest": {"sha256": "c"}, "lensLabels": ["correctness", null],
                "reviewResults": [{"findings": [{"title": "t"}]}, null], "lensFindings": [{"title": "t", "lens": "correctness"}],
                "lensesOverride": null, "filePartitionMode": "churn", "perGroupChurn": [10, 5],
                "useCodexHead": codex, "externalProbeAnswered": true, "useGeminiHead": gemini, "externalCwd": if codex { json!("/wt") } else { Value::Null },
                "cleanSweep": null, "verifiedAnchors": {"ran": false}, "voteQuoteCheck": {"ran": false},
                "batchPanelsActive": false, "vetBotComments": false, "verdictGuess": "COMMENT", "reportVerdictCorrected": null,
                "allCandidates": 4, "placedAside": 1, "droppedPrePanel": [{"title": "d", "file": "x.ts"}],
                "blastRelay": blast, "blastRadius": if blast { json!({"callers": [1, 2], "total": 5, "truncated": false}) } else { Value::Null },
                "dedupedPrePanel": 1, "voteCorrections": [], "crossFileClustersRejected": 0, "proximityVetoed": 0,
                "groundedAvailable": true, "groundedConfirmed": 1, "groundedRefuted": 0,
                "evidenceResolverRan": true, "evidenceLocatedCount": 3, "evidenceLineCorrectedCount": 1, "evidenceNotFoundCount": 0,
                "criticOutput": if critic { "gaps" } else { "" }, "criticGaps": 1, "relays": [],
                "priorReviews": [{"author": "rev", "state": "CHANGES_REQUESTED", "at": "2026-01-01", "summary": "s"}],
                "priorReviewGate": {"status": "clear", "ownOpenReview": null, "ownStandingReview": null},
                "fidelityCritic": {"ran": false}, "retestClaims": if critic { json!(["retry guard"]) } else { json!([]) },
                "reportAsPosted": "### Blocking\n(found by: correctness, security)\nalso at src/a.ts:10\nkillReason: not-triggered",
                "groupCount": 2, "churn": 15, "uncoveredFiles": ["lib/x.ts"], "lensNotReached": not_reached,
                "incrementalScope": inc, "priorReview": {"sha": "0123456789abcdef", "source": "tribunal"},
                "incrementalFallback": null, "allPrFiles": 3, "carriedForward": 2,
                "priorOutcome": {"stillOpen": [1], "notResurfaced": [1, 2]}, "heuristicEscalated": 1,
                "priorReExamined": [{"file": "src/a.ts", "title": "t", "verification": {"verdict": "FIXED", "quote": "q"}}],
                "maxPanels": 40, "panelSkippedForBudget": [], "tiebreak": {"ran": false, "groupSize": 0, "order": null, "skippedReason": null},
                "costBreakerFired": null, "outputCeiling": 1200000, "budgetTotal": null, "costDegradedStages": [],
                "maxAgents": 150, "agentsProjected": 24, "agentsProjectedUnbatched": 24, "round0ClaudeBatchSavings": 0, "refineClaudeBatchSavings": 0,
                "maxRefines": 11, "refinesSkippedForBudget": [], "externalSlots": 2, "artifactDir": "/tmp/votes",
            })
        })
}

fn js_params(c: &Value) -> Value {
    let arr = |k: &str| json!(vec![json!({}); c[k].as_f64().unwrap() as usize]);
    let mut p = c.clone();
    let o = p.as_object_mut().unwrap();
    for k in [
        "allCandidates",
        "placedAside",
        "criticGaps",
        "heuristicEscalated",
        "carriedForward",
        "allPrFiles",
    ] {
        o.insert(k.into(), arr(k));
    }
    o.insert("groundedConfirmedList".into(), arr("groundedConfirmed"));
    o.insert("groundedRefutedList".into(), arr("groundedRefuted"));
    o.insert(
        "CONVENTIONS".into(),
        json!({"manifest": c["conventionsManifest"]}),
    );
    o.insert(
        "diffSections".into(),
        json!(format!("({{size: {}}})", c["sectionCount"])),
    );
    o.insert("parsedArgs".into(), json!({"platform": c["platform"]}));
    o.insert(
        "reportVerdictPin".into(),
        json!({"corrected": c["reportVerdictCorrected"]}),
    );
    o.insert(
        "runSpent".into(),
        json!(format!(
            "({{now: () => {}, resets: () => {}}})",
            c["tokensAtEnd"], c["counterResets"]
        )),
    );
    o.insert(
        "budget".into(),
        json!(format!("({{total: {}}})", c["budgetTotal"])),
    );
    for (k, v) in [
        ("profileSha256", "PROFILE_SHA256"),
        ("profileSource", "PROFILE_SOURCE"),
        ("projectRulesSha256", "PROJECT_RULES_SHA256"),
        ("projectRulesSource", "PROJECT_RULES_SOURCE"),
        ("lensAgent", "LENS_AGENT"),
        ("artifactDir", "VOTE_DIR"),
        ("maxAgents", "maxAgents"),
    ] {
        o.insert(v.into(), c[k].clone());
    }
    o.insert("lensesOverride".into(), c["lensesOverride"].clone());
    o.remove("tokensAtEnd");
    o.insert("panelTiebreakRan".into(), c["tiebreak"]["ran"].clone());
    o.insert(
        "panelTiebreakGroupSize".into(),
        c["tiebreak"]["groupSize"].clone(),
    );
    o.insert("panelTiebreakOrder".into(), c["tiebreak"]["order"].clone());
    o.insert(
        "panelTiebreakSkippedReason".into(),
        c["tiebreak"]["skippedReason"].clone(),
    );
    p
}

#[test]
fn the_metrics_row_is_the_workflows() {
    let cases = sample(scope());
    let params: Vec<Value> = cases.iter().map(js_params).collect();
    let js = span(
        &Span {
            start: "const paneled = results.filter((v) => v.panelRan);",
            end: "  selfAudit: { passed: null, checked: 0, violations: [] },\n};",
            deps: &[
                "LENS-YIELD",
                "MERGED-LOCATIONS-CHECK",
                "REPORT-FIDELITY-CHECKS",
                "VERIFIED-GATE",
            ],
            pre: &[
                ("function mergeNotChecked(...lists) {", "\n}"),
                (
                    "const AGENTS_FIXED_OVERHEAD = 12;",
                    "const AGENTS_PER_REFINE = 3.7;",
                ),
                ("const LENS_TOOL_CALL_BUDGET = ", ";"),
                ("const CLAUDE_BATCH_SIZE = ", ";"),
                ("const T_KILL = ", ";"),
                ("const isRetestTarget = (finding) => {", "\n};"),
            ],
            fns: &["CONVENTIONS", "diffSections", "runSpent", "budget"],
            ret: "metrics",
        },
        &params
            .iter()
            .map(|p| {
                let mut p = p.clone();
                for k in ["diffSections", "runSpent", "budget"] {
                    let src = p[k].as_str().unwrap().to_owned();
                    p[k] = json!(src);
                }
                p["CONVENTIONS"] = json!(format!("({})", p["CONVENTIONS"]));
                p
            })
            .collect::<Vec<_>>(),
    );
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| {
            let s: MetricsScope = serde_json::from_value(c.clone()).unwrap();
            metrics_row(&s).unwrap()
        })
        .collect();
    if let Some(dir) = std::env::var_os("TRIBUNAL_DUMP") {
        let dir = std::path::PathBuf::from(dir);
        if let Some(i) = (0..cases.len()).find(|&i| !common::same(&rust[i], &js[i])) {
            std::fs::write(
                dir.join("rust.json"),
                serde_json::to_string_pretty(&rust[i]).unwrap(),
            )
            .unwrap();
            std::fs::write(
                dir.join("js.json"),
                serde_json::to_string_pretty(&js[i]).unwrap(),
            )
            .unwrap();
        }
    }
    assert_same("metrics", &cases, &rust, &js);
    let paneled = rust
        .iter()
        .filter(|r| r["crossModelDisagreement"]["codexVsClaudeMajority"].is_boolean())
        .count();
    let incremental = rust
        .iter()
        .filter(|r| r["reviewMode"] == "incremental")
        .count();
    let corrected = rust
        .iter()
        .filter(|r| {
            r["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f.get("lineCorrectedFrom").is_some())
        })
        .count();
    let forced = rust
        .iter()
        .filter(|r| {
            r["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["forcedPanel"] == true)
        })
        .count();
    assert!(
        paneled > 50 && incremental > 50 && corrected > 30 && forced > 30,
        "{paneled} {incremental} {corrected} {forced}"
    );
    // Both sides of the evidence gate on the row: demoted (issue #46) and not.
    let findings = || {
        rust.iter()
            .flat_map(|r| r["findings"].as_array().cloned().unwrap_or_default())
    };
    let demoted = findings()
        .filter(|f| f["verifiedByGateNote"].is_string() && f["blocking"] == false)
        .count();
    let anchored_blocking = findings()
        .filter(|f| f["verifiedByGateNote"].is_null() && f["blocking"] == true)
        .count();
    let demoted_but_blocking = findings()
        .filter(|f| f["verifiedByGateNote"].is_string() && f["blocking"] == true)
        .count();
    assert!(
        demoted > 30 && anchored_blocking > 30 && demoted_but_blocking == 0,
        "{demoted} {anchored_blocking} {demoted_but_blocking}"
    );
}
