//! The panel's orchestration against the workflow's: the same scripted
//! answers go to the workflow's code (from the clean-verdict sweep to the vote
//! corrections) through a fake `agent`, and to `panel_phase` through a fake
//! `PanelIo`. Results, logs and the dispatches asked for must agree.

#![recursion_limit = "256"]

mod common;

use std::sync::Mutex;

use common::{assert_same, sample, span, Span, LOGS, LOG_FN};
use proptest::prelude::*;
use serde_json::{json, Map, Value};
use tribunal_pipeline::panel::{panel_phase, Ask, HeadJob, PanelEnv, PanelFlags, PanelIo};
use tribunal_pipeline::synthesize::Relay;

const FILES: &[&str] = &["src/a.ts", "src/b/service.ts", "web/c.tsx"];

const DEPS: &[&str] = &[
    "CLEAN-SWEEP",
    "PANEL-GATES",
    "ARTIFACT-TRUTH",
    "CLASSIFIER-RELAY",
    "ARTIFACT-KEY",
    "STRENGTHEN-MERGE",
    "DEBATE-DEGRADE",
    "COST-BREAKER",
    "SPENT-COUNTER",
    "HIGH-RISK-SURFACE",
    "PROFILE-LOOKUP",
    "OUT-OF-DIFF",
    "DIFF-SECTIONING",
];

const PRE: &[(&str, &str)] = &[
    ("const PRECEDENCE_RULE = `", "BY-DESIGN.`;"),
    ("const AGENTS_FIXED_OVERHEAD = 12;", "const AGENTS_PER_REFINE = 3.7;"),
    ("const costDegradedStages = [];", "  costDegradedStages.push({ stage, title: title || null, reason: st.reason });\n  return true;\n}"),
    ("const runSpent = spentCounter(", ";"),
    ("const findingLineAdded = (finding) =>", ";"),
    ("const externalDiffSlice = (finding) => {", "\n};"),
    ("let codexHealthy = useCodexHead;", "      markUnhealthy(v.source, v.failureReason);\n    }\n  });\n}"),
    ("function needsPanel(finding) {", "  return true;\n}"),
    ("const REFUTER_LENSES = [", "        ...(refuterAgentType ? { agentType: refuterAgentType } : {}),\n      },\n    );\n  };\n}"),
    ("const escapeXmlText = (s) =>", "const CLAUDE_BATCH_VALIDATED = true;"),
    ("function externalSourcesFor(finding) {", "  return thunks;\n}"),
    ("async function classifyArtifacts(round, label) {", "  for (const e of res.entries) map[e.key] = e;\n  return map;\n}"),
    ("const NO_PANEL_RESULT = (finding) => ({", "  refineOutcome: null,\n  escalated: false,\n});"),
];

const RET: &str = "{verified, panelInput, droppedPrePanel, cleanSweep, criticOutput, panelSkippedForBudget, refinesSkippedForBudget, costBreakerFired, costDegradedStages, voteCorrections, voteQuoteCheck, batchPanelsActive, agentsProjected, agentsProjectedUnbatched, round0ClaudeBatchSavings, refineClaudeBatchSavings, maxPanels, maxRefines, externalSlots, criticGaps: criticGaps.length, tiebreak: {ran: panelTiebreakRan, groupSize: panelTiebreakGroupSize, order: panelTiebreakOrder, skippedReason: panelTiebreakSkippedReason}, calls: globalThis.__calls.filter((c) => !c.label.startsWith('classify-') && !c.label.startsWith('heads:')).map((c) => c.label + '\\n' + c.prompt).sort(), payloads: globalThis.__payloads, errors: globalThis.__errors, logs: __LOGS__}";

// The fake agent: every answer comes from the case's script, by label.
const AGENT: &str = r#"(globalThis.__agent = async (p, o) => {
  const S = globalThis.__script;
  globalThis.__calls.push({label: o.label, prompt: p});
  const l = o.label;
  if (l === "heads:codex:sweep") return {processed: 1, text: S.sweepText ?? "", finished: true};
  if (l === "classify-votes") return S.classify0;
  if (l === "classify-revotes") return S.classify1;
  if (l.startsWith("refute-batch:")) {
    if (S.batchNull) return null;
    const ids = l.slice(l.lastIndexOf(":") + 1).split(",");
    const v = ids.map((id) => ({...(S.votes[id] || {verdict: "NOT_REFUTED", confidence: 0.5, reason: "x", source: "claude"}), slotId: Number(id)}));
    if (S.batchMode === "drop") v.pop();
    if (S.batchMode === "dup") v.push({...v[0], verdict: v[0].verdict === "REFUTED" ? "NOT_REFUTED" : "REFUTED", reason: "dup"});
    if (S.batchMode === "bogus") v.push({...v[0], slotId: 999});
    return {verdicts: v};
  }
  if (l === "panel-ceiling-tiebreak" && S.byLabel[l] === "auto") return {order: [...p.matchAll(/"slotId": (\d+)/g)].map((m) => Number(m[1])).reverse()};
  return S.byLabel[l] ?? null;
})"#;

fn sev() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(json!("critical")),
        Just(json!("important")),
        Just(json!("nit"))
    ]
}

fn vote() -> impl Strategy<Value = Value> {
    (
        prop_oneof![3 => Just("REFUTED"), 2 => Just("NOT_REFUTED")],
        prop_oneof![Just(json!(0.95)), Just(json!(0.7)), Just(json!(0.5)), Just(json!(0.4))],
        prop_oneof![3 => Just(json!("TECHNICALLY_WRONG")), 1 => Just(json!("PRE_DECIDED")), 1 => Just(json!("NO_REFUTATION"))],
        prop_oneof![Just(json!("not-triggered")), Just(json!("incorrect-premise")), Just(Value::Null)],
        prop_oneof![Just(json!("yes")), Just(json!("no")), Just(json!("unsure")), Just(Value::Null)],
        prop_oneof![sev(), Just(Value::Null)],
        prop_oneof![5 => Just(json!("OK")), 1 => Just(json!("UNAVAILABLE"))],
        any::<bool>(),
    )
        .prop_map(|(v, c, rt, kill, intro, ps, status, quote)| {
            let mut o = json!({"verdict": v, "confidence": c, "refutationType": rt, "reason": "r `normalizeOrderId`", "source": "claude", "slotStatus": status});
            if !kill.is_null() {
                o["killReason"] = kill;
            }
            if !intro.is_null() {
                o["introducedByPr"] = intro;
            }
            if !ps.is_null() {
                o["proposedSeverity"] = ps;
            }
            if status == "UNAVAILABLE" {
                o["failureReason"] = json!("confined");
            }
            if quote {
                o["contradiction"] = json!({"file": "src/a.ts", "line": 3, "quote": "const x = 1;"});
            }
            if rt == "PRE_DECIDED" {
                o["decisionQuestioned"] = json!("DEFERRED: retries [files: src/a.ts]");
            }
            o
        })
}

fn finding(i: usize) -> impl Strategy<Value = Value> {
    (
        prop::sample::select(FILES),
        prop_oneof![Just(json!(10)), Just(json!(120)), Just(Value::Null)],
        sev(),
        (any::<bool>(), any::<bool>()),
        prop_oneof![4 => Just("correctness"), 1 => Just("bot-comments")],
    )
        .prop_map(move |(file, line, sev, (risk, beh), lens)| {
            let mut f = json!({"title": format!("F{i} defect in handler"), "file": file, "severity": sev, "description": "d", "evidence": "e", "lens": lens, "slotId": i, "confidence": 0.7});
            if !line.is_null() {
                f["line"] = line;
            }
            if risk {
                f["touchesHighRiskSurface"] = json!(true);
            }
            if beh {
                f["isBehaviorChanging"] = json!(true);
            }
            f
        })
}

fn classify(round: u64, n: usize) -> impl Strategy<Value = Value> {
    let entry = |key: String| {
        let k2 = key.clone();
        prop_oneof![
            4 => (prop_oneof![Just("REFUTED"), Just("NOT_REFUTED")], prop_oneof![Just(json!(0.9)), Just(json!(0.7)), Just(json!(0.5)), Just(json!(0.3))], any::<bool>())
                .prop_map(move |(v, c, d)| json!({"key": key.clone(), "failclass": "none", "verdict": v, "confidence": c, "confidenceDeclared": d})),
            1 => prop_oneof![Just("timeout"), Just("unparseable"), Just("quota"), Just("confined")]
                .prop_map(move |fc| json!({"key": k2.clone(), "failclass": fc, "verdict": null, "confidence": null, "confidenceDeclared": null})),
        ]
    };
    let keys: Vec<String> = (0..n)
        .map(|i| {
            if round == 0 {
                format!("{i}-codex")
            } else {
                format!("{i}r1-codex")
            }
        })
        .collect();
    let entries = keys.into_iter().map(entry).collect::<Vec<_>>();
    prop_oneof![
        6 => entries.prop_map(|e| json!({"total": e.len(), "entries": e})),
        1 => Just(Value::Null),
        1 => Just(json!({"total": 99, "entries": []})),
        1 => Just(json!({"total": 1, "entries": [{"key": "0-codex", "failclass": "none", "verdict": null}]})),
    ]
}

fn case() -> impl Strategy<Value = Value> {
    (1usize..11).prop_flat_map(|n| {
        (
            (0..n).map(finding).collect::<Vec<_>>(),
            prop::collection::vec(prop_oneof![5 => vote(), 1 => Just(Value::Null)], n),
            prop::collection::vec(prop_oneof![
                Just(Value::Null),
                any::<bool>().prop_map(|b| json!({"strengthened": b, "title": "S tightened claim", "file": "src/a.ts", "severity": "critical", "description": "sharper", "unstrengthenableReason": if b { Value::Null } else { json!("no") }})),
            ], n),
            prop::collection::vec(prop_oneof![
                Just(Value::Null),
                (prop_oneof![Just("SURVIVES"), Just("REFUTED")], any::<bool>()).prop_map(|(d, c)| json!({"decision": d, "confidence": 0.8, "rationale": "because", "changedFromPanel": c})),
            ], n),
            (classify(0, n), classify(1, n)),
            (
                prop_oneof![4 => Just(true), 1 => Just(false)], prop_oneof![1 => Just(true), 4 => Just(false)], any::<bool>(), prop_oneof![Just(0u8), Just(1), Just(2)],
                prop_oneof![Just(40.0), Just(1.0), Just(2.0)], prop_oneof![3 => Just(0.0), 1 => Just(300_000.0), 1 => Just(700_000.0)],
                any::<bool>(), prop_oneof![Just(1.0), Just(5.0)],
            ),
            (
                prop_oneof![Just(json!(null)), Just(json!("NO FINDINGS ABOVE NIT.")), Just(json!("FINDING: important | swept defect | src/a.ts:5 | it bites\nEVIDENCE: const x = 1;\nVERIFIED: src/a.ts:5 | const x = 1;")), Just(json!("garbage"))],
                prop_oneof![Just(Value::Null), Just(json!({"summary": "s", "findings": [{"title": "G gap", "file": "src/b/service.ts", "line": 40, "severity": "important", "description": "gap", "evidence": "e"}, {"title": "G2 far", "file": "other/x.ts", "line": 1, "severity": "nit"}]}))],
                prop_oneof![3 => Just(json!("auto")), 1 => Just(json!({"order": "bad"})), 1 => Just(Value::Null)],
                prop_oneof![Just(json!({"ok": true, "data": {"ran": true, "results": [{"findingIndex": 0, "status": "found"}, {"findingIndex": 1, "status": "not-found"}]}})), Just(json!({"ok": false, "data": null, "reason": "x"})), Just(json!({"ok": true, "data": {"ran": true, "results": [{"findingIndex": 0, "status": "lookup-failed"}]}}))],
                prop_oneof![Just(""), Just("- DEFERRED: retries [files: src/a.ts]")],
                prop_oneof![Just(json!(null)), Just(json!("drop")), Just(json!("dup")), Just(json!("bogus"))],
                any::<bool>(),
            ),
            (prop_oneof![4 => Just(false), 1 => Just(true)], prop_oneof![Just(150.0), Just(25.0), Just(18.0)]),
        )
    })
    .prop_map(|(mut findings, votes, strengthen, rulings, (c0, c1), (codex, no_codex, force_codex, batch, max_panels, spent, sectioned, file_count), (sweep, critic, order, relay, decisions, batch_mode, batch_null), (all_nit, max_agents))| {
        if all_nit {
            for f in &mut findings {
                f["severity"] = json!("nit");
                f["lens"] = json!("correctness");
            }
        }
        json!({
            "maxAgents": max_agents,
            "findings": findings, "votes": votes, "strengthen": strengthen, "rulings": rulings,
            "classify0": c0, "classify1": c1, "codex": codex, "noCodex": no_codex, "forceCodex": force_codex,
            "batch": batch, "maxPanels": max_panels, "spent": spent, "sectioned": sectioned, "fileCount": file_count,
            "sweepText": sweep, "critic": critic, "order": order, "relay": relay, "decisions": decisions,
            "batchMode": batch_mode, "batchNull": batch_null,
        })
    })
}

fn utf16_20(s: &str, n: usize) -> String {
    tribunal_core::lens::js_ext::utf16_prefix_lossy(s, n)
}

/// What each label answers, for every title a run can dispatch under.
fn by_label(c: &Value) -> Map<String, Value> {
    let mut m = Map::new();
    let titles = |f: &Value| f["title"].as_str().unwrap().to_owned();
    for (i, f) in c["findings"].as_array().unwrap().iter().enumerate() {
        let t = titles(f);
        m.insert(
            format!("refute:{}:claude", utf16_20(&t, 20)),
            c["votes"][i].clone(),
        );
        m.insert(
            format!("refine:{}", utf16_20(&t, 20)),
            c["strengthen"][i].clone(),
        );
        m.insert(
            format!("judge:{}", utf16_20(&t, 20)),
            c["rulings"][i].clone(),
        );
        m.insert(
            format!("defense:{}", utf16_20(&t, 18)),
            json!("DEFENSE TEXT"),
        );
    }
    for t in ["S tightened claim", "G gap", "swept defect"] {
        m.insert(
            format!("refute:{}:claude", utf16_20(t, 20)),
            c["votes"][0].clone(),
        );
        m.insert(
            format!("judge:{}", utf16_20(t, 20)),
            c["rulings"][0].clone(),
        );
        m.insert(
            format!("refine:{}", utf16_20(t, 20)),
            c["strengthen"][0].clone(),
        );
        m.insert(
            format!("defense:{}", utf16_20(t, 18)),
            json!("DEFENSE TEXT"),
        );
    }
    m.insert("completeness-critic".into(), c["critic"].clone());
    m.insert("panel-ceiling-tiebreak".into(), c["order"].clone());
    m
}

fn section_index(c: &Value) -> Value {
    if c["sectioned"] == true {
        json!({"usable": true, "unparsed": [], "wholeSlice": "/tmp/whole.slice", "sections": FILES.iter().map(|f| json!({"file": f, "slice": format!("/tmp/{}.slice", f.len()), "added": [[1, 50]]})).collect::<Vec<_>>()})
    } else {
        Value::Null
    }
}

fn pr_context(c: &Value) -> Value {
    json!({"files": FILES, "headRefOid": "feedfacecafebeef", "inThreadDecisions": if c["decisions"] == "" { json!([]) } else { json!([{"summary": "DEFERRED: retries", "files": ["src/a.ts"], "status": "DEFERRED"}]) }})
}

fn profile() -> Value {
    json!({"riskPaths": [{"focus": "security", "pattern": "b/service"}], "crossRepoAnchors": []})
}

fn js_params(c: &Value) -> Value {
    let script = json!({
        "votes": c["votes"].as_array().unwrap().iter().enumerate().map(|(i, v)| (i.to_string(), v.clone())).collect::<Map<_, _>>(),
        "byLabel": by_label(c), "classify0": c["classify0"], "classify1": c["classify1"],
        "sweepText": c["sweepText"], "batchMode": c["batchMode"], "batchNull": c["batchNull"],
    });
    let idx = section_index(c);
    let sections = if idx.is_null() {
        "new Map()".to_owned()
    } else {
        format!("new Map({}.map((s) => [s.file, s]))", idx["sections"])
    };
    json!({
        "__setup": format!("(globalThis.__spent = 0, globalThis.__script = {script}, globalThis.__calls = [], globalThis.__payloads = [], globalThis.__errors = [], globalThis.__relay = {})", c["relay"]),
        "agent": AGENT,
        "reviewerAgent": "(p, o) => globalThis.__agent(p, o)",
        "relayAgent": "async () => globalThis.__relay",
        "verifiedWrite": "(f, payload) => { globalThis.__payloads.push(payload); return []; }",
        "noteBashSize": "(l, t) => t", "bashBlocksText": "(b) => b.join('\\n')", "crossRepoFlags": "() => ''", "relayFilePipe": "() => ''",
        "externalDriver": "() => async () => null",
        "parallel": "async (ts) => Promise.all(ts.map((t) => Promise.resolve().then(() => t()).catch((e) => { globalThis.__errors.push(String(e && e.stack || e)); return null; })))",
        "withConventions": "(p) => p", "refuterDiffBlockFor": "() => 'DIFF BLOCK'",
        "isRetestTarget": "() => false",
        "budget": format!("({{spent: () => (globalThis.__spent = (globalThis.__spent ?? 0) + {}), total: null, remaining: () => Infinity}})", c["spent"]),
        "diffSections": sections,
        "log": LOG_FN,
        "RELAY_FILE_SCRIPT": "", "RELAY_RULES": "", "RUN_BLOCKS_RULE": "",
        "prNumber": 77, "repo": "acme/widgets", "prContext": pr_context(c), "PROFILE": profile(),
        "combinedDecisions": c["decisions"], "reviewModel": "m", "refuterAgentType": "r",
        "REFUTATION_SCHEMA": {"properties": {}, "required": []}, "STRENGTHEN_SCHEMA": {}, "ADJUDICATOR_SCHEMA": {}, "FINDINGS_SCHEMA": {"properties": {"findings": {}}},
        "STRENGTHEN_AGENT": "s", "LENS_AGENT": "l", "SCRIBE_AGENT": "c", "MECHANICAL": {},
        "maxPanelsArg": c["maxPanels"], "maxAgents": c["maxAgents"], "outputCeiling": 1_200_000,
        "panelInput": c["findings"], "droppedPrePanel": [], "blastRadius": null, "fileCount": c["fileCount"],
        "noBatchPanels": c["batch"] == 1, "forceBatchPanels": c["batch"] == 2, "forceCodex": c["forceCodex"],
        "useCodexHead": c["codex"] == true && (c["noCodex"] != true || c["forceCodex"] == true), "useGeminiHead": false,
        "externalCwd": "/wt", "diffPath": "/tmp/x.patch", "VOTE_DIR": "/tmp/votes", "SWEEP_TIMEOUT_S": 480,
        "diffUsable": !idx.is_null(), "sectioningAvailable": !idx.is_null(), "sectionIndex": idx,
    })
}

struct FakeIo {
    script: Value,
    labels: Map<String, Value>,
    calls: Mutex<Vec<String>>,
    payloads: Mutex<Vec<String>>,
}

impl PanelIo for FakeIo {
    async fn ask(&self, label: String, prompt: String, _kind: Ask) -> Option<Value> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{label}\n{prompt}"));
        if let Some(ids) = label.strip_prefix("refute-batch:") {
            if self.script["batchNull"] == true {
                return None;
            }
            let ids = &ids[ids.rfind(':').unwrap() + 1..];
            let mut v: Vec<Value> = ids
                .split(',')
                .map(|id| {
                    let base = self.script["votes"].get(id.parse::<usize>().unwrap()).filter(|v| !v.is_null()).cloned()
                        .unwrap_or(json!({"verdict": "NOT_REFUTED", "confidence": 0.5, "reason": "x", "source": "claude"}));
                    let mut m = base.as_object().cloned().unwrap();
                    m.insert("slotId".into(), json!(id.parse::<u64>().unwrap()));
                    Value::Object(m)
                })
                .collect();
            match self.script["batchMode"].as_str() {
                Some("drop") => {
                    v.pop();
                }
                Some("dup") => {
                    let mut d = v[0].clone();
                    d["verdict"] = json!(if d["verdict"] == "REFUTED" {
                        "NOT_REFUTED"
                    } else {
                        "REFUTED"
                    });
                    d["reason"] = json!("dup");
                    v.push(d);
                }
                Some("bogus") => {
                    let mut b = v[0].clone();
                    b["slotId"] = json!(999);
                    v.push(b);
                }
                _ => {}
            }
            return Some(json!({"verdicts": v}));
        }
        if label == "panel-ceiling-tiebreak" && self.labels.get(&label) == Some(&json!("auto")) {
            let re = regex::Regex::new(r#""slotId": (\d+)"#).unwrap();
            let mut ids: Vec<Value> = re
                .captures_iter(&prompt)
                .map(|c| json!(c[1].parse::<u64>().unwrap()))
                .collect();
            ids.reverse();
            return Some(json!({"order": ids}));
        }
        self.labels.get(&label).filter(|v| !v.is_null()).cloned()
    }

    async fn run_heads(&self, _jobs: Vec<HeadJob>) {}

    async fn classify(&self, round: u64) -> Option<Value> {
        let v = &self.script[if round == 0 { "classify0" } else { "classify1" }];
        (!v.is_null()).then(|| v.clone())
    }

    async fn resolve_anchors(&self, payload: String, _head: String) -> Relay {
        self.payloads.lock().unwrap().push(payload);
        let r = &self.script["relay"];
        Relay {
            ok: r["ok"] == true,
            data: (!r["data"].is_null()).then(|| r["data"].clone()),
            reason: r.get("reason").and_then(Value::as_str).map(str::to_owned),
        }
    }

    async fn sweep(&self, _prompt: String) -> Option<String> {
        Some(self.script["sweepText"].as_str().unwrap_or("").to_owned())
    }
}

fn rust_panel(c: &Value, rt: &tokio::runtime::Runtime) -> Value {
    let io = FakeIo {
        script: c.clone(),
        labels: by_label(c),
        calls: Mutex::new(Vec::new()),
        payloads: Mutex::new(Vec::new()),
    };
    let ctx = pr_context(c);
    let prof = profile();
    let idx = section_index(c);
    let step = c["spent"].as_f64().unwrap();
    let total = Mutex::new(0.0f64);
    let spent_fn = move || {
        let mut t = total.lock().unwrap();
        *t += step;
        *t
    };
    let retest = |_: &Value| false;
    let env = PanelEnv {
        pr_number: &json!(77),
        repo: "acme/widgets",
        pr_context: &ctx,
        profile: &prof,
        combined_decisions: c["decisions"].as_str().unwrap(),
        retest: &retest,
        diff_block: "DIFF BLOCK",
        section_index: (!idx.is_null()).then_some(&idx),
        diff_path: Some("/tmp/x.patch"),
        blast_radius: None,
        file_count: c["fileCount"].as_f64().unwrap(),
        flags: PanelFlags {
            force_codex: c["forceCodex"] == true,
            no_codex: c["noCodex"] == true,
            force_batch: c["batch"] == 2,
            no_batch: c["batch"] == 1,
            ..PanelFlags::default()
        },
        codex_installed: c["codex"] == true,
        gemini_installed: false,
        external_cwd: Some("/wt"),
        max_panels: c["maxPanels"].as_f64().unwrap(),
        max_agents: c["maxAgents"].as_f64().unwrap(),
        output_ceiling: 1_200_000.0,
        spent: &spent_fn,
        spent_before: 0.0,
    };
    let mut logs = Vec::new();
    let out = rt
        .block_on(panel_phase(
            &io,
            &env,
            c["findings"].as_array().unwrap().clone(),
            Vec::new(),
            &mut logs,
        ))
        .unwrap();
    let mut calls = io.calls.into_inner().unwrap();
    calls.sort();
    json!({
        "verified": out.verified, "panelInput": out.panel_input, "droppedPrePanel": out.dropped_pre_panel,
        "cleanSweep": out.clean_sweep, "criticOutput": out.critic_output,
        "panelSkippedForBudget": out.panel_skipped_for_budget, "refinesSkippedForBudget": out.refines_skipped_for_budget,
        "costBreakerFired": out.cost_breaker_fired, "costDegradedStages": out.cost_degraded_stages,
        "voteCorrections": out.vote_corrections, "voteQuoteCheck": out.vote_quote_check,
        "batchPanelsActive": out.batch_panels_active, "agentsProjected": out.agents_projected,
        "agentsProjectedUnbatched": out.agents_projected_unbatched, "round0ClaudeBatchSavings": out.round0_claude_batch_savings,
        "refineClaudeBatchSavings": out.refine_claude_batch_savings, "maxPanels": out.max_panels, "maxRefines": out.max_refines,
        "externalSlots": out.external_slots, "criticGaps": out.critic_gaps,
        "tiebreak": out.tiebreak, "calls": calls, "payloads": io.payloads.into_inner().unwrap(), "errors": [], "logs": logs,
    })
}

const MAIN: Span = Span {
    start: "let cleanSweep = null;",
    end: "The tally below uses what the CLIs actually returned.`,\n  );\n}",
    deps: DEPS,
    pre: PRE,
    fns: &[
        "__setup",
        "agent",
        "reviewerAgent",
        "relayAgent",
        "verifiedWrite",
        "noteBashSize",
        "bashBlocksText",
        "crossRepoFlags",
        "relayFilePipe",
        "externalDriver",
        "parallel",
        "withConventions",
        "refuterDiffBlockFor",
        "isRetestTarget",
        "budget",
        "diffSections",
        "log",
    ],
    ret: RET,
};

#[test]
fn the_panel_runs_as_the_workflow_runs_it() {
    let cases = sample(case());
    let ret = RET.replace("__LOGS__", LOGS);
    let params: Vec<Value> = cases.iter().map(js_params).collect();
    let js = span(&Span { ret: &ret, ..MAIN }, &params);
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let rust: Vec<Value> = cases.iter().map(|c| rust_panel(c, &rt)).collect();
    if let Some(dir) = std::env::var_os("TRIBUNAL_DUMP") {
        let dir = std::path::PathBuf::from(dir);
        let first = (0..cases.len()).find(|&i| common::canon(&rust[i]) != common::canon(&js[i]));
        if let Some(i) = first {
            std::fs::write(dir.join("case.json"), cases[i].to_string()).unwrap();
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
    assert_same("panel", &cases, &rust, &js);
}

#[test]
fn the_generated_panels_reach_every_branch() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let out: Vec<Value> = sample(case()).iter().map(|c| rust_panel(c, &rt)).collect();
    let n = |f: &dyn Fn(&Value) -> bool| out.iter().filter(|o| f(o)).count();
    let any_v = |o: &Value, k: &str| {
        o["verified"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v[k] == true)
    };
    let has_log = |o: &Value, s: &str| {
        o["logs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l.as_str().unwrap().contains(s))
    };
    let checks: Vec<(&str, usize)> = vec![
        ("escalated", n(&|o| any_v(o, "escalated"))),
        ("refined", n(&|o| any_v(o, "refined"))),
        ("debate", n(&|o| any_v(o, "debateFired"))),
        (
            "refuted",
            n(&|o| {
                o["verified"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v["panelRan"] == true && v["survived"] == false)
            }),
        ),
        (
            "survived",
            n(&|o| {
                o["verified"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v["panelRan"] == true && v["survived"] == true)
            }),
        ),
        ("batch", n(&|o| o["batchPanelsActive"] == true)),
        ("breaker", n(&|o| !o["costBreakerFired"].is_null())),
        (
            "refine-skipped",
            n(&|o| !o["refinesSkippedForBudget"].as_array().unwrap().is_empty()),
        ),
        (
            "panel-skipped",
            n(&|o| !o["panelSkippedForBudget"].as_array().unwrap().is_empty()),
        ),
        ("tiebreak", n(&|o| o["tiebreak"]["ran"] == true)),
        ("sweep", n(&|o| o["cleanSweep"]["ran"] == true)),
        (
            "sweep-raised",
            n(&|o| o["cleanSweep"]["candidatesRaised"].as_u64() > Some(0)),
        ),
        (
            "critic-gap",
            n(&|o| has_log(o, "Completeness critic raised")),
        ),
        (
            "critic-drop",
            n(&|o| !o["droppedPrePanel"].as_array().unwrap().is_empty()),
        ),
        ("quote-check", n(&|o| o["voteQuoteCheck"]["ran"] == true)),
        ("unhealthy", n(&|o| has_log(o, "marked unhealthy"))),
        ("classify-discarded", n(&|o| has_log(o, "discarding"))),
        (
            "placed",
            n(&|o| {
                o["verified"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v["placement"]["placement"].is_string())
            }),
        ),
        (
            "pre-decided",
            n(&|o| {
                o["verified"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v["preDecided"] == true)
            }),
        ),
    ];
    let low: Vec<String> = checks
        .iter()
        .filter(|(_, c)| *c <= 5)
        .map(|(n, c)| format!("{n}={c}"))
        .collect();
    assert!(low.is_empty(), "{checks:?}");
}
