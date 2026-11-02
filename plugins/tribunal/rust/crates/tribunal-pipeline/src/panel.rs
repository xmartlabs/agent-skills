//! The refutation panel: the clean-verdict sweep, the completeness critic,
//! which findings open a panel, round 0 (Claude refuters, alone or batched,
//! beside the external heads), the vote-quote check, escalation, refine and
//! its re-vote, and the debate. Ports the workflow from the CLEAN-SWEEP block
//! to the `verified` results, with the decisions taken by `tribunal-core`.
//!
//! The order in which findings reach a shared decision (the refine budget,
//! the cost breaker) is the one the workflow's own scheduling gives when every
//! dispatch answers at once: findings that did not escalate decide first, in
//! order, then the escalated ones.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Map, Value};
use tribunal_core::js;
use tribunal_core::lens::diff_sectioning::any_location_added;
use tribunal_core::lens::js_ext::{to_js_string, utf16_prefix_lossy};
use tribunal_core::lens::out_of_diff::reanchor_out_of_diff;
use tribunal_core::panel::cost_breaker::{cost_breaker_state, CostInputs};
use tribunal_core::panel::debate_degrade::debate_ruling;
use tribunal_core::panel::panel_gates::{
    contested_pre_decision, needs_refine, place_survivor, should_debate, should_escalate,
    tally_votes, Tally,
};
use tribunal_core::panel::strengthen_merge::apply_strengthen;
use tribunal_core::profile::high_risk_surface::risk_path_touched;
use tribunal_core::profile::profile_lookup::risk_path_rules;
use tribunal_core::votes::artifact_truth::{apply_artifact_truth, PanelSlot};
use tribunal_core::votes::clean_sweep::{
    parse_sweep_findings, should_run_clean_sweep, sweep_family_for, SweepGateInput,
};
use tribunal_core::votes::js_ext::number_value;
use tribunal_core::votes::panel_ceiling_tiebreak::{
    panel_ceiling_tied_group, resolve_panel_ceiling_tiebreak, TiebreakResolution,
    PANEL_TIEBREAK_CAP,
};
use tribunal_core::votes::vote_quotes::{checkable_vote_quotes, vote_quote_verdicts};

use crate::panel_prompts::{
    batch_prompt, context_instruction, critic_prompt, defense_prompt, external_context_instruction,
    external_prompt, judge_prompt, refute_prompt, strengthen_prompt, sweep_prompt, tiebreak_prompt,
    REFUTER_LENSES,
};
use crate::synthesize::Relay;
use crate::verify::{
    needs_panel, no_panel_result, panel_plan, AGENTS_FIXED_OVERHEAD, AGENTS_PER_PANEL,
    AGENTS_PER_REFINE,
};

pub const EXTERNAL_BATCH_SIZE: usize = 4;
pub const CLAUDE_BATCH_SIZE: usize = 3;
pub const CLAUDE_BATCH_PANEL_THRESHOLD: usize = 8;
pub const CLAUDE_BATCH_VALIDATED: bool = true;
pub const OUTPUT_CEILING_DEFAULT: f64 = 1_200_000.0;

/// What a panel dispatch is, which picks its agent, schema and wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    Refuter,
    RefuterBatch,
    Strengthen,
    Defense,
    Judge,
    Critic,
    Tiebreak,
}

/// One external head's vote to cast.
#[derive(Debug, Clone, PartialEq)]
pub struct HeadJob {
    pub finding: Value,
    pub source: &'static str,
    pub round: u64,
    /// `externalPromptText(finding)`, the slice mark in place when the slice
    /// goes in as a file.
    pub body: String,
    pub slice_file: Option<String>,
}

/// Everything the panel does outside a Claude dispatch: the model calls go
/// through [`PanelIo::ask`], the external heads through the rest.
#[allow(async_fn_in_trait)]
pub trait PanelIo {
    /// A Claude dispatch: its schema'd answer, or its text for the defense.
    /// `None` when it returned nothing usable.
    async fn ask(&self, label: String, prompt: String, kind: Ask) -> Option<Value>;
    /// Runs the external heads and leaves their artifacts in the vote dir.
    async fn run_heads(&self, jobs: Vec<HeadJob>);
    /// `classify-artifacts` over the vote dir for `round`, as
    /// `{total, entries: [{key, failclass, verdict, confidence,
    /// confidenceDeclared}]}`. `None` when it could not run.
    async fn classify(&self, round: u64) -> Option<Value>;
    /// The anchor resolver over a vote-quote payload.
    async fn resolve_anchors(&self, payload: String, head: String) -> Relay;
    /// The clean-verdict sweep: what the Codex CLI wrote.
    async fn sweep(&self, prompt: String) -> Option<String>;
}

/// The operator's switches the panel reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PanelFlags {
    pub force_codex: bool,
    pub no_codex: bool,
    pub force_gemini: bool,
    pub no_gemini: bool,
    pub force_batch: bool,
    pub no_batch: bool,
}

/// The run's facts the panel reads.
pub struct PanelEnv<'a> {
    pub pr_number: &'a Value,
    pub repo: &'a str,
    pub pr_context: &'a Value,
    pub profile: &'a Value,
    pub combined_decisions: &'a str,
    pub retest: &'a (dyn Fn(&Value) -> bool + Sync),
    pub diff_block: &'a str,
    /// `sectionIndex`, when section-diff ran.
    pub section_index: Option<&'a Value>,
    pub diff_path: Option<&'a str>,
    pub blast_radius: Option<&'a Value>,
    pub file_count: f64,
    pub flags: PanelFlags,
    pub codex_installed: bool,
    pub gemini_installed: bool,
    pub external_cwd: Option<&'a str>,
    pub max_panels: f64,
    pub max_agents: f64,
    pub output_ceiling: f64,
    /// Output tokens spent so far, as the counter reads them.
    pub spent: &'a (dyn Fn() -> f64 + Sync),
    /// What the run spent before the panel, which the workflow's run counter
    /// (started with the run) has already counted.
    pub spent_before: f64,
}

/// What the panel leaves for the results split and the report.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PanelOutcome {
    pub panel_input: Vec<Value>,
    pub verified: Vec<Option<Value>>,
    pub dropped_pre_panel: Vec<Value>,
    pub clean_sweep: Option<Value>,
    pub critic_output: String,
    /// Wall-clock seconds the completeness critic took; 0 when it did not run.
    pub critic_seconds: f64,
    pub panel_skipped_for_budget: Vec<Value>,
    pub refines_skipped_for_budget: Vec<Value>,
    pub cost_breaker_fired: Option<Value>,
    pub cost_degraded_stages: Vec<Value>,
    pub vote_corrections: Vec<Value>,
    pub vote_quote_check: Value,
    pub batch_panels_active: bool,
    pub agents_projected: f64,
    pub agents_projected_unbatched: f64,
    pub round0_claude_batch_savings: f64,
    pub refine_claude_batch_savings: f64,
    pub max_panels: f64,
    pub max_refines: f64,
    pub external_slots: usize,
    pub critic_gaps: usize,
    pub tiebreak: Value,
    pub use_codex: bool,
    pub use_gemini: bool,
}

fn s(v: Option<&Value>) -> String {
    to_js_string(v)
}

fn title20(f: &Value) -> String {
    utf16_prefix_lossy(&s(js::get(f, "title")), 20)
}

fn slot_of(f: &Value) -> u64 {
    js::get(f, "slotId").and_then(Value::as_u64).unwrap_or(0)
}

/// `demuxClaudeBatch`: each finding's vote from a batched answer, or an
/// abstention when the answer did not carry one for its slot.
#[must_use]
pub fn demux_claude_batch(findings: &[Value], result: Option<&Value>) -> Vec<Value> {
    let mut by_slot: HashMap<u64, Value> = HashMap::new();
    if let Some(Value::Array(verdicts)) = result.and_then(|r| js::get(r, "verdicts")) {
        let expected: HashSet<u64> = findings.iter().map(slot_of).collect();
        for v in verdicts {
            let Some(Value::Number(n)) = js::get(v, "slotId") else {
                continue;
            };
            let Some(id) = n.as_f64().filter(|x| x.fract() == 0.0 && *x >= 0.0) else {
                continue;
            };
            let id = id as u64;
            if !expected.contains(&id) || by_slot.contains_key(&id) {
                continue;
            }
            by_slot.insert(id, v.clone());
        }
    }
    findings
        .iter()
        .map(|f| {
            by_slot.get(&slot_of(f)).cloned().unwrap_or_else(|| {
                json!({
                    "source": "claude",
                    "verdict": "NOT_REFUTED",
                    "confidence": 0,
                    "slotStatus": "UNAVAILABLE",
                    "failureReason": "unparseable",
                    "reason": "batched refuter dispatch did not return a verdict for this slot",
                })
            })
        })
        .collect()
}

/// `classifierEntriesInconsistent`.
#[must_use]
pub fn classifier_entries_inconsistent(entries: &[Value]) -> Vec<Value> {
    entries
        .iter()
        .filter(|e| {
            let verdict = js::get(e, "verdict");
            let cast = verdict == Some(&json!("REFUTED")) || verdict == Some(&json!("NOT_REFUTED"));
            if (js::get(e, "failclass") == Some(&json!("none"))) != cast {
                return true;
            }
            let declared = js::get(e, "confidenceDeclared");
            if cast && !matches!(declared, Some(Value::Bool(_))) {
                return true;
            }
            !cast && !matches!(declared, None | Some(Value::Null))
        })
        .cloned()
        .collect()
}

/// `classifyArtifacts` after its relay: the truth map, or `None` with the
/// workflow's log line when the classification cannot be trusted.
#[must_use]
pub fn truth_from_classification(
    res: Option<&Value>,
    label: &str,
    logs: &mut Vec<String>,
) -> Option<Value> {
    let entries = res
        .and_then(|r| js::get(r, "entries"))
        .and_then(Value::as_array);
    let Some(entries) = entries else {
        logs.push(format!("Artifact classification ({label}) returned nothing usable — external votes keep what the wrapper agents reported, which is NOT verified."));
        return None;
    };
    let total = res.and_then(|r| js::get(r, "total"));
    if total.and_then(Value::as_f64) != Some(entries.len() as f64) {
        logs.push(format!(
            "Artifact classification ({label}) relayed {} of {} entries — discarding it rather than verifying only part of the panel.",
            entries.len(),
            s(total)
        ));
        return None;
    }
    let bad = classifier_entries_inconsistent(entries);
    if !bad.is_empty() {
        let detail = bad
            .iter()
            .map(|e| {
                let v = js::get(e, "verdict");
                let shown = match v {
                    None | Some(Value::Null) => "none".to_owned(),
                    v => s(v),
                };
                format!(
                    "{}: failclass={}, verdict={shown}",
                    s(js::get(e, "key")),
                    s(js::get(e, "failclass"))
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        logs.push(format!(
            "Artifact classification ({label}) relayed {} entr{} whose failclass and verdict disagree ({detail}) — discarding rather than installing a vote the classifier did not cast.",
            bad.len(),
            if bad.len() == 1 { "y" } else { "ies" }
        ));
        return None;
    }
    let mut map = Map::new();
    for e in entries {
        map.insert(s(js::get(e, "key")), e.clone());
    }
    Some(Value::Object(map))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Health {
    codex: bool,
    gemini: bool,
}

impl Health {
    fn sources_for(&self, finding: &Value) -> Vec<&'static str> {
        if self.codex {
            if self.gemini && js::truthy(js::get(finding, "touchesHighRiskSurface")) {
                vec!["codex", "gemini"]
            } else {
                vec!["codex"]
            }
        } else if self.gemini {
            vec!["gemini"]
        } else {
            Vec::new()
        }
    }

    fn mark(&mut self, source: &str, reason: &str, logs: &mut Vec<String>) {
        if !["timeout", "error", "unparseable", "quota", "auth"].contains(&reason) {
            return;
        }
        if source == "codex" && self.codex {
            self.codex = false;
            logs.push(format!(
                "Codex marked unhealthy after a {reason} — falling back for the rest of this run"
            ));
        } else if source == "gemini" && self.gemini {
            self.gemini = false;
            logs.push(format!(
                "Gemini marked unhealthy after a {reason} — falling back for the rest of this run"
            ));
        }
    }

    fn record(&mut self, votes: &[Value], logs: &mut Vec<String>) {
        for v in votes {
            if js::get(v, "slotStatus") == Some(&json!("UNAVAILABLE"))
                && js::truthy(js::get(v, "source"))
            {
                let src = s(js::get(v, "source"));
                let reason = s(js::get(v, "failureReason"));
                self.mark(&src, &reason, logs);
            }
        }
    }
}

/// A panel slot: who votes and, for a Claude slot, under which lens.
#[derive(Debug, Clone, PartialEq)]
struct Slot {
    source: &'static str,
    lens: Option<usize>,
}

fn slots_value(panel: &[Slot]) -> Vec<Value> {
    panel.iter().map(|p| json!({"source": p.source})).collect()
}

fn slots_truth(panel: &[Slot]) -> Vec<PanelSlot> {
    panel
        .iter()
        .map(|p| PanelSlot {
            source: Some(p.source.to_owned()),
            relayed: p.lens.is_some(),
        })
        .collect()
}

struct Breaker<'a> {
    spent: &'a (dyn Fn() -> f64 + Sync),
    ceiling: f64,
    last: f64,
    total: f64,
    fired: Option<Value>,
    stages: Vec<Value>,
}

impl<'a> Breaker<'a> {
    fn new(spent: &'a (dyn Fn() -> f64 + Sync), ceiling: f64, before: f64) -> Self {
        let last = spent();
        Self {
            spent,
            ceiling,
            last,
            total: before,
            fired: None,
            stages: Vec::new(),
        }
    }

    fn run_now(&mut self) -> f64 {
        let cur = (self.spent)();
        if cur < self.last {
            self.total += cur;
        } else {
            self.total += cur - self.last;
        }
        self.last = cur;
        self.total
    }

    fn declines(&mut self, stage: &str, title: Option<&Value>, logs: &mut Vec<String>) -> bool {
        let spent = (self.spent)();
        let run = self.run_now();
        let st = cost_breaker_state(&CostInputs {
            spent_output: Some(spent),
            run_output: Some(run),
            budget_total: None,
            budget_remaining: None,
            ceiling: Some(self.ceiling),
        });
        if !st.degrade {
            return false;
        }
        let reason = st.reason.clone().unwrap_or_default();
        if self.fired.is_none() {
            self.fired = Some(json!({
                "reason": reason,
                "spentOutput": number_value(st.spent_output),
                "runOutput": number_value(st.run_output),
                "ceiling": number_value(self.ceiling),
            }));
            logs.push(format!("COST BREAKER — {reason}. Declining every further escalation, refine and debate. This run is DEGRADED: findings keep whatever adjudication they already got, and the report must say so."));
        }
        let title = match title {
            Some(t) if js::truthy(Some(t)) => t.clone(),
            _ => Value::Null,
        };
        self.stages
            .push(json!({"stage": stage, "title": title, "reason": reason}));
        true
    }
}

/// A finding's state between round 0 and its result.
#[derive(Debug, Clone)]
struct State {
    finding: Value,
    working: Value,
    panel: Vec<Slot>,
    votes: Vec<Value>,
    tally: Option<Tally>,
    refined: bool,
    refine_outcome: Value,
    escalated: bool,
    round: u64,
}

fn tally_of(votes: &[Value], panel: &[Slot], finding: &Value, env: &PanelEnv) -> Tally {
    let file = js::get(finding, "file").and_then(Value::as_str);
    tally_votes(
        votes,
        &slots_value(panel),
        file,
        js::get(env.pr_context, "inThreadDecisions").unwrap_or(&Value::Null),
        !env.combined_decisions.is_empty(),
    )
}

fn line_added(finding: &Value, env: &PanelEnv) -> bool {
    let usable = env
        .section_index
        .is_some_and(|i| js::truthy(js::get(i, "usable")));
    let mut sections = Map::new();
    if usable {
        for sec in env
            .section_index
            .and_then(|i| js::get(i, "sections"))
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
        {
            sections.insert(s(js::get(sec, "file")), sec.clone());
        }
    }
    any_location_added(&sections, finding)
}

/// The diff a head reads: the finding's own section, the whole-diff excerpt,
/// or the instruction to read the file when the diff is not usable. Returns
/// `(inline text, slice file)`.
fn external_slice(finding: &Value, env: &PanelEnv) -> (Option<String>, Option<String>) {
    let index = env
        .section_index
        .filter(|i| js::truthy(js::get(i, "usable")));
    let Some(index) = index else {
        return (
            Some(format!(
                "No diff excerpt is provided. Open {} in your working directory (it is already checked out at the PR head commit) and read the region around the finding yourself.",
                s(js::get(finding, "file"))
            )),
            None,
        );
    };
    let sections: Vec<&Value> = js::get(index, "sections")
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let files: Vec<String> = js::get(env.pr_context, "files")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|f| s(Some(f))).collect())
        .unwrap_or_default();
    let unparsed = js::get(index, "unparsed")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let sectioned = !sections.is_empty()
        && unparsed == 0
        && files
            .iter()
            .all(|f| sections.iter().any(|sec| s(js::get(sec, "file")) == *f));
    let own = if sectioned {
        let file = s(js::get(finding, "file"));
        sections
            .iter()
            .find(|sec| s(js::get(sec, "file")) == file)
            .and_then(|sec| js::get(sec, "slice"))
            .map(|p| s(Some(p)))
    } else {
        None
    };
    (
        None,
        own.or_else(|| {
            js::get(index, "wholeSlice")
                .filter(|w| !w.is_null())
                .map(|w| s(Some(w)))
        }),
    )
}

fn head_job(finding: &Value, source: &'static str, round: u64, env: &PanelEnv) -> HeadJob {
    let files: Vec<String> = js::get(env.pr_context, "files")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|f| s(Some(f))).collect())
        .unwrap_or_default();
    let (text, file) = external_slice(finding, env);
    let body = external_prompt(
        finding,
        &files,
        &external_context_instruction(env.combined_decisions),
        text.as_deref(),
    );
    HeadJob {
        finding: finding.clone(),
        source,
        round,
        body,
        slice_file: file,
    }
}

/// The panel, from the clean-verdict sweep to each finding's result.
///
/// # Errors
/// Where the workflow throws.
#[allow(clippy::too_many_lines)]
pub async fn panel_phase<I: PanelIo>(
    io: &I,
    env: &PanelEnv<'_>,
    panel_input_in: Vec<Value>,
    dropped_pre_panel: Vec<Value>,
    logs: &mut Vec<String>,
) -> Result<PanelOutcome, String> {
    let mut out = PanelOutcome {
        dropped_pre_panel,
        ..PanelOutcome::default()
    };
    let flags = env.flags;
    let use_codex = env.codex_installed && (!flags.no_codex || flags.force_codex);
    let use_gemini = env.gemini_installed && flags.force_gemini;
    out.use_codex = use_codex;
    out.use_gemini = use_gemini;
    let pr_files: Vec<String> = js::get(env.pr_context, "files")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|f| s(Some(f))).collect())
        .unwrap_or_default();
    let mut health = Health {
        codex: use_codex,
        gemini: use_gemini,
    };
    let retest = env.retest;
    let mut panel_input = panel_input_in;

    // --- Clean-verdict sweep ---
    let preliminary = panel_input
        .iter()
        .filter(|f| needs_panel(f, retest(f)))
        .count();
    let gate = should_run_clean_sweep(&SweepGateInput {
        panel_eligible_count: preliminary,
        codex_installed: use_codex,
        codex_healthy: health.codex,
        external_cwd: env.external_cwd,
    });
    let family = sweep_family_for(&gate);
    if preliminary == 0 && family.is_none() {
        out.clean_sweep = Some(json!({
            "ran": false, "family": null, "skippedReason": gate.reason,
            "clean": null, "unparseable": false, "candidatesRaised": 0,
        }));
        logs.push(format!(
            "Clean-verdict sweep skipped: {}",
            s(Some(&json!(gate.reason)))
        ));
    } else if let Some(family) = family {
        logs.push(
            "Zero findings reached a panel — dispatching the clean-verdict Codex sweep".into(),
        );
        let text = io
            .sweep(sweep_prompt(
                env.diff_path,
                &pr_files,
                env.external_cwd.unwrap_or_default(),
            ))
            .await;
        let parsed = parse_sweep_findings(text.as_deref());
        let rules = risk_path_rules(env.profile)?;
        let base = panel_input.len();
        let raised: Vec<Value> = parsed
            .findings
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let fv = serde_json::to_value(f).unwrap_or(Value::Null);
                let mut m = fv.as_object().cloned().unwrap_or_default();
                m.insert("slotId".into(), json!(base + i));
                m.insert("lens".into(), json!(format!("{family}-sweep")));
                m.insert("description".into(), json!(f.why));
                m.insert("confidence".into(), json!(0.6));
                m.insert("severityAtIntake".into(), json!(f.severity));
                m.insert("isBehaviorChanging".into(), json!(false));
                m.insert(
                    "touchesHighRiskSurface".into(),
                    json!(risk_path_touched(Some(&json!(f.file)), Some(&rules))),
                );
                m.insert("forcedPanel".into(), json!(false));
                m.insert("mergedFrom".into(), json!([]));
                m.insert("mergedLocations".into(), json!([]));
                Value::Object(m)
            })
            .collect();
        let n = raised.len();
        panel_input.extend(raised);
        let head_failure = parsed
            .head_failure
            .as_ref()
            .map(|h| serde_json::to_value(h).unwrap_or(Value::Null));
        out.clean_sweep = Some(json!({
            "ran": true, "family": family, "skippedReason": null,
            "clean": parsed.clean, "unparseable": parsed.unparseable,
            "headFailure": head_failure, "candidatesRaised": n,
        }));
        logs.push(if let Some(h) = &parsed.head_failure {
            format!("Clean-verdict sweep could not run: codex {} ({}) — recorded as failed, NOT as agreement", h.class, h.reason)
        } else if parsed.unparseable {
            "Clean-verdict sweep returned no readable answer — recorded as failed, NOT as agreement".into()
        } else if n > 0 {
            format!("Clean-verdict sweep raised {n} candidate(s) — routing through the ordinary panel path")
        } else {
            "Clean-verdict sweep confirms the clean verdict (second model family)".into()
        });
    }

    // --- Completeness critic ---
    if env.file_count > 2.0 {
        let critic_started = std::time::Instant::now();
        let critic = io
            .ask(
                "completeness-critic".into(),
                critic_prompt(
                    env.pr_number,
                    env.repo,
                    js::get(env.pr_context, "headRefOid"),
                    &pr_files,
                    &panel_input,
                ),
                Ask::Critic,
            )
            .await;
        out.critic_seconds = critic_started.elapsed().as_secs_f64();
        out.critic_output = critic
            .as_ref()
            .and_then(|c| js::get(c, "summary"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let same_place = |a: &Value, b: &Value| {
            let fa = s(js::get(a, "file"));
            let fb = s(js::get(b, "file"));
            let la = tribunal_core::lens::js_ext::to_number(js::get(a, "line"));
            let lb = tribunal_core::lens::js_ext::to_number(js::get(b, "line"));
            let num = |x: f64| if x.is_nan() || x == 0.0 { 0.0 } else { x };
            (js::get(a, "file") == js::get(b, "file") || fa.ends_with(&fb) || fb.ends_with(&fa))
                && (num(la) - num(lb)).abs() <= 5.0
        };
        let mut gaps: Vec<Value> = Vec::new();
        let found = critic
            .as_ref()
            .and_then(|c| js::get(c, "findings"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for g in &found {
            let Some(kept) = reanchor_out_of_diff(g, &pr_files, env.blast_radius)? else {
                let mut d = Map::new();
                for k in ["title", "file", "severity"] {
                    if let Some(v) = js::get(g, k) {
                        d.insert(k.into(), v.clone());
                    }
                }
                d.insert("reason".into(), json!("completeness-critic gap out-of-diff (file not in this PR, and nothing ties it to a changed file)"));
                out.dropped_pre_panel.push(Value::Object(d));
                continue;
            };
            if panel_input.iter().any(|f| same_place(f, &kept))
                || gaps.iter().any(|f| same_place(f, &kept))
            {
                continue;
            }
            gaps.push(kept);
        }
        let rules = risk_path_rules(env.profile)?;
        let base = panel_input.len();
        let raised: Vec<Value> = gaps
            .into_iter()
            .enumerate()
            .map(|(i, g)| {
                let mut m = g.as_object().cloned().unwrap_or_default();
                m.insert("slotId".into(), json!(base + i));
                m.insert("lens".into(), json!("completeness-critic"));
                let conf = match js::get(&g, "confidence") {
                    Some(Value::Number(n)) => Value::Number(n.clone()),
                    _ => json!(0.6),
                };
                m.insert("confidence".into(), conf);
                m.insert(
                    "severityAtIntake".into(),
                    js::get(&g, "severity").cloned().unwrap_or(Value::Null),
                );
                let risk = js::truthy(js::get(&g, "touchesHighRiskSurface"))
                    || risk_path_touched(js::get(&g, "file"), Some(&rules));
                m.insert("touchesHighRiskSurface".into(), json!(risk));
                m.insert("forcedPanel".into(), json!(false));
                m.insert("mergedFrom".into(), json!([]));
                m.insert("mergedLocations".into(), json!([]));
                Value::Object(m)
            })
            .collect();
        out.critic_gaps = raised.len();
        if !raised.is_empty() {
            logs.push(format!(
                "Completeness critic raised {} gap(s) — routing through the ordinary panel path",
                raised.len()
            ));
        }
        panel_input.extend(raised);
    }

    // --- Which findings open a panel ---
    let plan = panel_plan(&panel_input, retest, env.max_panels, env.max_agents);
    let mut bound = plan.bound.clone();
    let mut skipped = plan.skipped_for_budget.clone();
    let priority = |f: &Value| {
        let mut p = 0;
        if retest(f) {
            p += 8;
        }
        if js::get(f, "severity") == Some(&json!("critical")) {
            p += 4;
        }
        if js::truthy(js::get(f, "touchesHighRiskSurface")) {
            p += 2;
        }
        if js::truthy(js::get(f, "isBehaviorChanging"))
            || js::get(f, "lens") == Some(&json!("bot-comments"))
        {
            p += 1;
        }
        p
    };
    let summaries: Vec<Value> = plan
        .eligible
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mut m = Map::new();
            for k in [
                "slotId",
                "title",
                "file",
                "severity",
                "touchesHighRiskSurface",
                "isBehaviorChanging",
            ] {
                if let Some(v) = js::get(f, k) {
                    m.insert(k.into(), v.clone());
                }
            }
            m.insert("priority".into(), json!(priority(f)));
            m.insert("i".into(), json!(i));
            Value::Object(m)
        })
        .collect();
    let mut tiebreak = json!({"ran": false, "groupSize": 0, "order": null, "skippedReason": null});
    let max_panels = std::num::NonZeroUsize::new(plan.max_panels as usize)
        .unwrap_or(std::num::NonZeroUsize::MIN);
    if let Some(tie) = panel_ceiling_tied_group(&summaries, max_panels) {
        if tie.tied_group.len() as i64 > tie.slots_for_tied_group {
            tiebreak["groupSize"] = json!(tie.tied_group.len());
            if tie.tied_group.len() > PANEL_TIEBREAK_CAP {
                let reason = format!(
                    "tied group of {} exceeds the {PANEL_TIEBREAK_CAP}-finding cap",
                    tie.tied_group.len()
                );
                logs.push(format!(
                    "Panel ceiling tie-break skipped — {reason}; keeping emission order."
                ));
                tiebreak["skippedReason"] = json!(reason);
            } else {
                let ranking = io
                    .ask(
                        "panel-ceiling-tiebreak".into(),
                        tiebreak_prompt(
                            &tie.tied_group,
                            tie.slots_for_tied_group.max(0) as usize,
                            &tie.boundary_priority,
                        ),
                        Ask::Tiebreak,
                    )
                    .await;
                let order = ranking.as_ref().and_then(|r| js::get(r, "order"));
                match resolve_panel_ceiling_tiebreak(
                    &tie.tied_group,
                    tie.slots_for_tied_group,
                    order,
                ) {
                    TiebreakResolution::Split { winning_ids, .. } => {
                        tiebreak["ran"] = json!(true);
                        tiebreak["order"] = order.cloned().unwrap_or(Value::Null);
                        let included: Vec<Value> = tie
                            .guaranteed_ids
                            .iter()
                            .chain(&winning_ids)
                            .cloned()
                            .collect();
                        let is_in = |f: &Value| {
                            let id = js::get(f, "slotId");
                            included.iter().any(|x| {
                                Some(x) == id
                                    || (x.as_f64().is_some()
                                        && x.as_f64() == id.and_then(Value::as_f64))
                            })
                        };
                        bound = plan.eligible.iter().filter(|f| is_in(f)).cloned().collect();
                        skipped = plan
                            .eligible
                            .iter()
                            .filter(|f| !is_in(f))
                            .map(|f| {
                                let mut m = Map::new();
                                for k in ["title", "file", "severity"] {
                                    if let Some(v) = js::get(f, k) {
                                        m.insert(k.into(), v.clone());
                                    }
                                }
                                Value::Object(m)
                            })
                            .collect();
                        logs.push(format!(
                            "Panel ceiling tie-break ranked {} tied finding(s) for the last {} slot(s).",
                            tie.tied_group.len(),
                            tie.slots_for_tied_group
                        ));
                    }
                    TiebreakResolution::Rejected(reason) => {
                        tiebreak["skippedReason"] = json!(reason);
                        logs.push(format!("Panel ceiling tie-break response was unusable ({reason}) — keeping emission order."));
                    }
                }
            }
        }
    }
    out.tiebreak = tiebreak;
    let bound_ids: HashSet<u64> = bound.iter().map(slot_of).collect();

    let batch = !flags.no_batch
        && (flags.force_batch
            || (CLAUDE_BATCH_VALIDATED && bound.len() >= CLAUDE_BATCH_PANEL_THRESHOLD));
    out.batch_panels_active = batch;
    let projected_refines = plan.max_refines.min(bound.len() as f64);
    let round0_eligible = if batch {
        bound
            .iter()
            .filter(|f| !health.sources_for(f).is_empty() && !flags.force_codex)
            .count()
    } else {
        0
    };
    let round0_savings = if round0_eligible > 0 {
        round0_eligible - round0_eligible.div_ceil(CLAUDE_BATCH_SIZE)
    } else {
        0
    };
    let refine_savings = if batch && projected_refines > 0.0 {
        projected_refines - (projected_refines / CLAUDE_BATCH_SIZE as f64).ceil()
    } else {
        0.0
    };
    let unbatched = (AGENTS_FIXED_OVERHEAD
        + bound.len() as f64 * AGENTS_PER_PANEL
        + projected_refines * AGENTS_PER_REFINE)
        .round();
    let projected = (AGENTS_FIXED_OVERHEAD
        + bound.len() as f64 * AGENTS_PER_PANEL
        + projected_refines * AGENTS_PER_REFINE
        - round0_savings as f64
        - refine_savings)
        .round();
    out.agents_projected = projected;
    out.agents_projected_unbatched = unbatched;
    out.round0_claude_batch_savings = round0_savings as f64;
    out.refine_claude_batch_savings = refine_savings;
    out.max_panels = plan.max_panels;
    out.max_refines = plan.max_refines;
    logs.push(format!(
        "Agent budget: ceiling {}, projected ~{} ({} panels x {} + up to {} refines x {} + {} fixed{}). The projection is a model calibrated on one measured run, not a counter — compare it against this run's real total and correct the constants if it drifts.",
        js::number_to_string(env.max_agents),
        js::number_to_string(projected),
        bound.len(),
        js::number_to_string(AGENTS_PER_PANEL),
        js::number_to_string(plan.max_refines),
        js::number_to_string(AGENTS_PER_REFINE),
        js::number_to_string(AGENTS_FIXED_OVERHEAD),
        if batch {
            format!(
                " − {round0_savings} round-0 + {} refine Claude-batch savings (unbatched estimate was ~{})",
                js::number_to_string(refine_savings),
                js::number_to_string(unbatched)
            )
        } else {
            String::new()
        }
    ));
    if !skipped.is_empty() {
        logs.push(format!(
            "PANEL CEILING — {} findings earned a panel but only {} fit the budget. {} were NOT adjudicated and cannot be reported as blocking: {}. Raise it with maxAgents if this PR deserves the spend.",
            plan.eligible.len(),
            js::number_to_string(plan.max_panels),
            skipped.len(),
            skipped.iter().map(|f| s(js::get(f, "title"))).collect::<Vec<_>>().join("; ")
        ));
    }
    out.panel_skipped_for_budget = skipped;

    // --- Round 0 ---
    let build_panel = |f: &Value, health: &Health| -> Vec<Slot> {
        let externals = health.sources_for(f);
        let claudes = if externals.is_empty() || flags.force_codex {
            2
        } else {
            1
        };
        (0..claudes)
            .map(|l| Slot {
                source: "claude",
                lens: Some(l),
            })
            .chain(externals.into_iter().map(|e| Slot {
                source: e,
                lens: None,
            }))
            .collect()
    };
    let panels: HashMap<u64, Vec<Slot>> = bound
        .iter()
        .map(|f| (slot_of(f), build_panel(f, &health)))
        .collect();
    let external_slots: usize = bound.iter().map(|f| health.sources_for(f).len()).sum();
    out.external_slots = external_slots;
    let mut jobs0 = Vec::new();
    for source in ["codex", "gemini"] {
        for f in &bound {
            if health.sources_for(f).contains(&source) {
                jobs0.push(head_job(f, source, 0, env));
            }
        }
    }
    if !jobs0.is_empty() {
        let drivers = ["codex", "gemini"]
            .iter()
            .map(|src| {
                jobs0
                    .iter()
                    .filter(|j| j.source == *src)
                    .count()
                    .div_ceil(EXTERNAL_BATCH_SIZE)
            })
            .sum::<usize>();
        logs.push(format!(
            "External heads: {external_slots} CLI invocation(s) across {drivers} batched driver(s) — same number of independent votes, {} fewer agents.",
            external_slots - drivers
        ));
    }
    let ctx = context_instruction(env.combined_decisions);
    let refute = |f: &Value, lens: usize| {
        io.ask(
            format!("refute:{}:claude", title20(f)),
            refute_prompt(
                env.pr_number,
                env.repo,
                env.diff_block,
                &ctx,
                f,
                REFUTER_LENSES[lens],
            ),
            Ask::Refuter,
        )
    };
    let refute_batch = |group: Vec<Value>, lens: usize| {
        let ids = group
            .iter()
            .map(|f| s(js::get(f, "slotId")))
            .collect::<Vec<_>>()
            .join(",");
        let label = format!(
            "refute-batch:{}:{ids}",
            utf16_prefix_lossy(REFUTER_LENSES[lens], 12)
        );
        let prompt = batch_prompt(
            &group,
            REFUTER_LENSES[lens],
            env.pr_number,
            env.repo,
            env.diff_block,
            &ctx,
        );
        async move { (group, io.ask(label, prompt, Ask::RefuterBatch).await) }
    };
    let pool_a: Vec<Value> = if batch {
        bound
            .iter()
            .filter(|f| !health.sources_for(f).is_empty() && !flags.force_codex)
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    let pool_ids: HashSet<u64> = pool_a.iter().map(slot_of).collect();
    let groups: Vec<Vec<Value>> = pool_a
        .chunks(CLAUDE_BATCH_SIZE)
        .map(<[Value]>::to_vec)
        .collect();
    if !groups.is_empty() {
        logs.push(format!(
            "Claude refuters: {} vote(s) across {} batched call(s) — same number of independent votes, {} fewer agents.",
            pool_a.len(),
            groups.len(),
            pool_a.len() - groups.len()
        ));
    }
    let solo: Vec<&Value> = bound
        .iter()
        .filter(|f| !pool_ids.contains(&slot_of(f)))
        .collect();
    let solo_votes = async {
        join_all(solo.iter().map(|f| {
            let panel = &panels[&slot_of(f)];
            async move {
                join_all(panel.iter().map(|p| async move {
                    match p.lens {
                        Some(l) => refute(f, l).await.unwrap_or(Value::Null),
                        None => Value::Null,
                    }
                }))
                .await
            }
        }))
        .await
    };
    let (_, batch_results, solo_votes) = tokio::join!(
        io.run_heads(jobs0),
        join_all(groups.into_iter().map(|g| refute_batch(g, 0))),
        solo_votes
    );
    let mut round0: HashMap<u64, Vec<Value>> = HashMap::new();
    for (f, votes) in solo.iter().zip(solo_votes) {
        round0.insert(slot_of(f), votes);
    }
    for (group, result) in batch_results {
        for (f, vote) in group
            .iter()
            .zip(demux_claude_batch(&group, result.as_ref()))
        {
            let panel = &panels[&slot_of(f)];
            let votes = panel
                .iter()
                .enumerate()
                .map(|(i, _)| if i == 0 { vote.clone() } else { Value::Null })
                .collect();
            round0.insert(slot_of(f), votes);
        }
    }
    let mut states: Vec<Option<State>> = panel_input
        .iter()
        .map(|f| {
            let id = slot_of(f);
            (bound_ids.contains(&id))
                .then(|| {
                    round0.get(&id).map(|votes| State {
                        finding: f.clone(),
                        working: f.clone(),
                        panel: panels[&id].clone(),
                        votes: votes.clone(),
                        tally: None,
                        refined: false,
                        refine_outcome: Value::Null,
                        escalated: false,
                        round: 0,
                    })
                })
                .flatten()
        })
        .collect();

    // --- Vote quotes and round-0 artifacts ---
    let quote_states: Vec<Value> = states
        .iter()
        .zip(&panel_input)
        .map(|(st, f)| match st {
            Some(st) => json!({"finding": st.finding, "votes": st.votes, "skipped": false}),
            None => json!({"finding": f, "skipped": true}),
        })
        .collect();
    let quotes = checkable_vote_quotes(Some(&quote_states));
    let head = js::get(env.pr_context, "headRefOid");
    let classify0 = async {
        if external_slots > 0 {
            Some(io.classify(0).await)
        } else {
            None
        }
    };
    let quote_check = async {
        if !quotes.is_empty() && js::truthy(head) {
            let payload = js::stringify(
                &Value::Array(quotes.iter().map(|q| json!({"verifiedBy": [serde_json::to_value(&q.anchor).unwrap_or(Value::Null)]})).collect()),
                "",
            );
            Some(io.resolve_anchors(payload, s(head)).await)
        } else {
            None
        }
    };
    let (classified0, relay) = tokio::join!(classify0, quote_check);
    let truth0 = match classified0 {
        Some(res) => truth_from_classification(res.as_ref(), "classify-votes", logs),
        None => None,
    };
    out.vote_quote_check = match relay {
        None => {
            json!({"ran": false, "reason": if quotes.is_empty() { "no-quoted-refutation" } else { "no-head-ref" }, "checked": quotes.len()})
        }
        Some(relay) => {
            let data = if relay.ok { relay.data.clone() } else { None };
            match data
                .as_ref()
                .and_then(|d| js::get(d, "results"))
                .filter(|r| r.is_array())
            {
                None => {
                    let reason = relay
                        .reason
                        .clone()
                        .filter(|r| !r.is_empty())
                        .or_else(|| {
                            data.as_ref()
                                .and_then(|d| js::get(d, "reason"))
                                .filter(|r| js::truthy(Some(r)))
                                .map(|r| s(Some(r)))
                        })
                        .unwrap_or_else(|| "no-relay".into());
                    json!({"ran": false, "reason": reason, "checked": quotes.len()})
                }
                Some(results) => {
                    let v = vote_quote_verdicts(Some(results), quotes.len())?;
                    for (k, q) in quotes.iter().enumerate() {
                        let Some(found) = v.found[k] else { continue };
                        if let Some(Some(st)) = states.get_mut(q.s) {
                            if let Some(vote) = st.votes.get_mut(q.i) {
                                if let Some(m) = vote.as_object_mut() {
                                    m.insert("quoteFound".into(), json!(found));
                                }
                            }
                        }
                    }
                    let not_found = v.found.iter().filter(|f| **f == Some(false)).count();
                    logs.push(format!(
                        "vote-quote check: {}",
                        if v.broken_lookup {
                            "lookup failed, nothing set aside".to_owned()
                        } else {
                            format!(
                                "{not_found} of {} quote(s) not at head, set aside",
                                quotes.len()
                            )
                        }
                    ));
                    json!({
                        "ran": true, "checked": quotes.len(), "notFound": not_found,
                        "unchecked": v.found.iter().filter(|f| f.is_none()).count(),
                        "suspectLookup": v.broken_lookup,
                    })
                }
            }
        }
    };

    // --- Round 0 tallies, escalation and the refine budget ---
    let mut breaker = Breaker::new(env.spent, env.output_ceiling, env.spent_before);
    let mut corrections: Vec<Value> = Vec::new();
    let mut refine_budget = plan.max_refines;
    let mut refine = Vec::new();
    let mut escalate = Vec::new();
    let refine_check = |i: usize,
                        states: &Vec<Option<State>>,
                        breaker: &mut Breaker,
                        budget: &mut f64,
                        refine: &mut Vec<usize>,
                        skipped: &mut Vec<Value>,
                        logs: &mut Vec<String>| {
        let st = states[i].as_ref().unwrap();
        if !needs_refine(st.tally.as_ref().unwrap()) {
            return;
        }
        if breaker.declines("refine", js::get(&st.working, "title"), logs) {
            return;
        }
        if *budget <= 0.0 {
            let mut m = Map::new();
            for k in ["title", "file"] {
                if let Some(v) = js::get(&st.working, k) {
                    m.insert(k.into(), v.clone());
                }
            }
            skipped.push(Value::Object(m));
            return;
        }
        *budget -= 1.0;
        refine.push(i);
    };
    let wants_escalation = |st: &State| {
        let claude_r0 = st.panel.iter().filter(|p| p.source == "claude").count();
        js::truthy(should_escalate(st.tally.as_ref().unwrap(), &st.finding, claude_r0).as_ref())
    };
    if batch {
        for st in states.iter_mut().flatten() {
            st.votes = apply_artifact_truth(
                &st.votes,
                &slots_truth(&st.panel),
                truth0.as_ref(),
                slot_of(&st.finding),
                0,
                &mut corrections,
            );
            health.record(&st.votes, logs);
            st.tally = Some(tally_of(&st.votes, &st.panel, &st.finding, env));
        }
        for (i, st) in states.iter().enumerate() {
            let Some(st) = st else { continue };
            if wants_escalation(st)
                && !breaker.declines("escalation", js::get(&st.finding, "title"), logs)
            {
                escalate.push(i);
            }
        }
        let groups: Vec<Vec<usize>> = escalate
            .chunks(CLAUDE_BATCH_SIZE)
            .map(<[usize]>::to_vec)
            .collect();
        let results = join_all(groups.iter().map(|g| {
            refute_batch(
                g.iter()
                    .map(|&i| states[i].as_ref().unwrap().finding.clone())
                    .collect(),
                1,
            )
        }))
        .await;
        if !groups.is_empty() {
            logs.push(format!(
                "Escalation votes: {} vote(s) across {} batched call(s) — {} fewer agents.",
                escalate.len(),
                groups.len(),
                escalate.len() - groups.len()
            ));
        }
        for (g, (group, result)) in groups.iter().zip(results) {
            for (&i, vote) in g.iter().zip(demux_claude_batch(&group, result.as_ref())) {
                add_escalation(states[i].as_mut().unwrap(), vote, env);
            }
        }
        for i in 0..states.len() {
            if states[i].is_some() {
                refine_check(
                    i,
                    &states,
                    &mut breaker,
                    &mut refine_budget,
                    &mut refine,
                    &mut out.refines_skipped_for_budget,
                    logs,
                );
            }
        }
    } else {
        for i in 0..states.len() {
            let Some(st) = states[i].as_mut() else {
                continue;
            };
            st.votes = apply_artifact_truth(
                &st.votes,
                &slots_truth(&st.panel),
                truth0.as_ref(),
                slot_of(&st.finding),
                0,
                &mut corrections,
            );
            health.record(&st.votes, logs);
            st.tally = Some(tally_of(&st.votes, &st.panel, &st.finding, env));
            if wants_escalation(st)
                && !breaker.declines("escalation", js::get(&st.finding, "title"), logs)
            {
                escalate.push(i);
            } else {
                refine_check(
                    i,
                    &states,
                    &mut breaker,
                    &mut refine_budget,
                    &mut refine,
                    &mut out.refines_skipped_for_budget,
                    logs,
                );
            }
        }
        let votes = join_all(
            escalate
                .iter()
                .map(|&i| refute(&states[i].as_ref().unwrap().finding, 1)),
        )
        .await;
        for (&i, vote) in escalate.iter().zip(votes) {
            add_escalation(
                states[i].as_mut().unwrap(),
                vote.unwrap_or(Value::Null),
                env,
            );
        }
        for &i in &escalate {
            refine_check(
                i,
                &states,
                &mut breaker,
                &mut refine_budget,
                &mut refine,
                &mut out.refines_skipped_for_budget,
                logs,
            );
        }
    }
    let strengthened = join_all(refine.iter().map(|&i| {
        let st = states[i].as_ref().unwrap();
        let label = format!("refine:{}", title20(&st.working));
        let prompt = strengthen_prompt(
            env.pr_number,
            env.repo,
            env.diff_block,
            &st.working,
            &st.tally.as_ref().unwrap().valid,
        );
        io.ask(label, prompt, Ask::Strengthen)
    }))
    .await;
    let mut jobs1 = Vec::new();
    for (&i, answer) in refine.iter().zip(&strengthened) {
        let st = states[i].as_mut().unwrap();
        let applied = apply_strengthen(&st.working, answer.as_ref());
        if applied.outcome != "strengthened" {
            logs.push(format!(
                "Refine produced nothing for \"{}\" ({}) — the original finding is kept unchanged and the re-vote runs against it, recorded as {}, NOT as a strengthened finding.",
                utf16_prefix_lossy(&s(js::get(&applied.finding, "title")), 60),
                s(Some(&applied.reason)),
                applied.outcome
            ));
        }
        st.working = applied.finding;
        st.refined = true;
        st.refine_outcome = json!(applied.outcome);
        st.round = 1;
        let sources = health.sources_for(&st.working);
        st.panel = std::iter::once(Slot {
            source: "claude",
            lens: Some(0),
        })
        .chain(sources.iter().map(|&e| Slot {
            source: e,
            lens: None,
        }))
        .collect();
        for &src in &sources {
            jobs1.push(head_job(&st.working, src, 1, env));
        }
        st.votes = vec![Value::Null; st.panel.len()];
        st.tally = None;
    }
    let mut jobs1_sorted = Vec::new();
    for source in ["codex", "gemini"] {
        jobs1_sorted.extend(jobs1.iter().filter(|j| j.source == source).cloned());
    }
    if batch {
        let groups: Vec<Vec<usize>> = refine
            .chunks(CLAUDE_BATCH_SIZE)
            .map(<[usize]>::to_vec)
            .collect();
        if !groups.is_empty() {
            logs.push(format!(
                "Refine re-votes: {} Claude vote(s) across {} batched call(s) — {} fewer agents.",
                refine.len(),
                groups.len(),
                refine.len() - groups.len()
            ));
        }
        let (results, ()) = tokio::join!(
            join_all(groups.iter().map(|g| {
                refute_batch(
                    g.iter()
                        .map(|&i| states[i].as_ref().unwrap().working.clone())
                        .collect(),
                    0,
                )
            })),
            io.run_heads(jobs1_sorted)
        );
        for (g, (group, result)) in groups.iter().zip(results) {
            for (&i, vote) in g.iter().zip(demux_claude_batch(&group, result.as_ref())) {
                states[i].as_mut().unwrap().votes[0] = vote;
            }
        }
    } else {
        let (votes, ()) = tokio::join!(
            join_all(
                refine
                    .iter()
                    .map(|&i| refute(&states[i].as_ref().unwrap().working, 0))
            ),
            io.run_heads(jobs1_sorted)
        );
        for (&i, vote) in refine.iter().zip(votes) {
            states[i].as_mut().unwrap().votes[0] = vote.unwrap_or(Value::Null);
        }
    }
    let truth1 = if refine.is_empty() {
        None
    } else {
        let res = io.classify(1).await;
        truth_from_classification(res.as_ref(), "classify-revotes", logs)
    };

    // --- Each finding's result ---
    let mut finals: Vec<(usize, State, Tally)> = Vec::new();
    let mut debates = Vec::new();
    for (i, st) in states.iter().enumerate() {
        let Some(st) = st else { continue };
        let mut st = st.clone();
        let tally = if st.round == 1 {
            st.votes = apply_artifact_truth(
                &st.votes,
                &slots_truth(&st.panel),
                truth1.as_ref(),
                slot_of(&st.finding),
                1,
                &mut corrections,
            );
            health.record(&st.votes, logs);
            tally_of(&st.votes, &st.panel, &st.finding, env)
        } else {
            st.tally.clone().unwrap()
        };
        if should_debate(&tally) && !breaker.declines("debate", js::get(&st.working, "title"), logs)
        {
            debates.push(finals.len());
        }
        finals.push((i, st, tally));
    }
    let rulings = join_all(debates.iter().map(|&k| {
        let (_, st, tally) = &finals[k];
        async move {
            let defense = io
                .ask(
                    format!(
                        "defense:{}",
                        utf16_prefix_lossy(&s(js::get(&st.working, "title")), 18)
                    ),
                    defense_prompt(
                        env.pr_number,
                        env.repo,
                        env.diff_block,
                        &st.working,
                        &tally.valid,
                    ),
                    Ask::Defense,
                )
                .await;
            let ruling = io
                .ask(
                    format!("judge:{}", title20(&st.working)),
                    judge_prompt(
                        env.diff_block,
                        &st.working,
                        &tally.valid,
                        defense.as_ref(),
                        tally.survived,
                    ),
                    Ask::Judge,
                )
                .await;
            ruling
        }
    }))
    .await;
    let mut ruled: HashMap<usize, Value> = HashMap::new();
    for (&k, ruling) in debates.iter().zip(rulings) {
        let (_, st, tally) = &finals[k];
        let r = debate_ruling(ruling.as_ref(), tally);
        if !r.adjudicated {
            logs.push(format!(
                "Adjudicator returned nothing for \"{}\" — the debate fired and produced no ruling, so the finding keeps the panel's mechanical outcome ({}) and is recorded as unadjudicated, NOT as a debate that held.",
                utf16_prefix_lossy(&s(js::get(&st.working, "title")), 60),
                if tally.survived { "SURVIVES" } else { "REFUTED" }
            ));
        }
        ruled.insert(
            k,
            json!({"survived": r.survived, "outcome": r.outcome, "rationale": r.rationale}),
        );
    }
    let mut verified: Vec<Option<Value>> = panel_input
        .iter()
        .map(|f| Some(no_panel_result(f)))
        .collect();
    for (k, (i, st, tally)) in finals.iter().enumerate() {
        let debate = ruled.get(&k);
        let survived = debate.map_or(tally.survived, |d| d["survived"] == true);
        let possibly = if survived {
            contested_pre_decision(
                &tally.discarded_votes,
                &st.finding,
                js::get(env.pr_context, "inThreadDecisions").unwrap_or(&Value::Null),
                &pr_files,
            )
            .map(|c| serde_json::to_value(c).unwrap_or(Value::Null))
            .unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        let placement = if survived {
            let added =
                line_added(&st.working, env) || js::truthy(js::get(&st.working, "reanchored"));
            serde_json::to_value(place_survivor(&tally.valid, &st.working, added))
                .unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        let all_votes: Vec<Value> = st
            .votes
            .iter()
            .filter(|v| js::truthy(Some(v)))
            .map(|v| {
                let mut m = v.as_object().cloned().unwrap_or_default();
                m.insert("slotId".into(), json!(slot_of(&st.finding)));
                Value::Object(m)
            })
            .collect();
        verified[*i] = Some(json!({
            "finding": st.working,
            "severityAtIntake": js::get(&st.finding, "severity"),
            "survived": survived,
            "panelRan": true,
            "hadIndependentVoter": tally.had_independent_voter,
            "hadExternalVoter": tally.had_external_voter,
            "failedSlots": tally.failed_slots,
            "failedSlotsDetail": tally.failed_slots_detail,
            "discardedVotes": tally.discarded_votes,
            "possiblyPreDecided": possibly,
            "placement": placement,
            "unverified": tally.unverified,
            "borderline": tally.borderline,
            "refuteFraction": if tally.unverified { Value::Null } else { number_value(tally.refute_fraction) },
            "votes": tally.valid,
            "allVotes": all_votes,
            "preDecided": tally.pre_decided,
            "debateFired": debate.is_some(),
            "debateOutcome": debate.map_or(Value::Null, |d| d["outcome"].clone()),
            "debateRationale": debate.map_or(Value::Null, |d| d["rationale"].clone()),
            "refined": st.refined,
            "refineOutcome": st.refine_outcome,
            "escalated": st.escalated,
        }));
    }
    if !corrections.is_empty() {
        let mut by_kind = Map::new();
        for c in &corrections {
            let k = s(js::get(c, "kind"));
            let n = by_kind.get(&k).and_then(Value::as_u64).unwrap_or(0);
            by_kind.insert(k, json!(n + 1));
        }
        logs.push(format!(
            "Vote correction: {} external vote(s) did not match the CLI output they claim to relay and were replaced by the artifact — {}. The tally below uses what the CLIs actually returned.",
            corrections.len(),
            js::stringify(&Value::Object(by_kind), "")
        ));
    }
    out.vote_corrections = corrections;
    out.cost_breaker_fired = breaker.fired;
    out.cost_degraded_stages = breaker.stages;
    out.verified = verified;
    out.panel_input = panel_input;
    Ok(out)
}

fn add_escalation(st: &mut State, vote: Value, env: &PanelEnv) {
    st.panel.push(Slot {
        source: "claude",
        lens: None,
    });
    let vote = if js::truthy(Some(&vote)) {
        let mut m = vote.as_object().cloned().unwrap_or_default();
        m.insert("fromEscalation".into(), json!(true));
        Value::Object(m)
    } else {
        vote
    };
    st.votes.push(vote);
    st.tally = Some(tally_of(&st.votes, &st.panel, &st.finding, env));
    st.escalated = true;
}

/// Runs every future at once and returns the outputs in order.
pub async fn join_all<F: std::future::Future>(futs: impl IntoIterator<Item = F>) -> Vec<F::Output> {
    let mut set: Vec<_> = futs.into_iter().map(Box::pin).collect();
    let mut out: Vec<Option<F::Output>> = (0..set.len()).map(|_| None).collect();
    let mut pending: Vec<usize> = (0..set.len()).collect();
    std::future::poll_fn(|cx| {
        pending.retain(|&i| match set[i].as_mut().poll(cx) {
            std::task::Poll::Ready(v) => {
                out[i] = Some(v);
                false
            }
            std::task::Poll::Pending => true,
        });
        if pending.is_empty() {
            std::task::Poll::Ready(())
        } else {
            std::task::Poll::Pending
        }
    })
    .await;
    out.into_iter()
        .map(|o| o.expect("every future completed"))
        .collect()
}
