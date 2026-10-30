//! The metrics row of a finished run: [`MetricsScope`] read off the phase
//! outcomes, the persist step that appends it, and the fields the workflow
//! leaves for a later patch (`agentCost`, `agentsActual`), which a native run
//! can fill from its own dispatch results.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{json, Map, Value};
use tribunal_agent::{DispatchResult, Exit};
use tribunal_context::inflight;
use tribunal_context::ledger::LedgerEnv;
use tribunal_context::persist_run::{self, PersistRun};
use tribunal_context::review_record;
use tribunal_core::js;
use tribunal_core::lens::coverage_gaps::kind_counts;
use tribunal_core::lens::js_ext::to_js_string;
use tribunal_core::profile::lens_foci::foci_label_for;
use tribunal_core::profile::profile_lookup::risk_path_rules;

use crate::context::{RunArgs, Runtime};
use crate::metrics::{metrics_row, MetricsScope};
use crate::persist::{
    external_votes, persist_json, reconciliation_logs, resolve_vote_reconciliation,
    row_is_this_run, self_audit, wall_clock_fields,
};
use crate::review::answer_of;
use crate::run::{DispatchPolicy, ReviewOutcome, SynthesisOutcome, VerifyOutcome};

fn output_tokens<'a>(results: impl IntoIterator<Item = &'a DispatchResult>) -> f64 {
    results
        .into_iter()
        .flat_map(|d| d.model_usage().into_values())
        .map(|u| u.output_tokens as f64)
        .sum()
}

fn array<T: Clone + Into<Value>>(v: &[T]) -> Value {
    Value::Array(v.iter().cloned().map(Into::into).collect())
}

/// The sectioning facts the row reports, from the section index the Context
/// phase built: `[sectioningAvailable, sectionCount, unsectionedFiles,
/// diffUnparsedHeaders]`.
fn sectioning(section_index: Option<&Value>, pr_context: &Value) -> (bool, f64, Value, Value) {
    let usable = section_index.is_some_and(|s| js::truthy(js::get(s, "usable")));
    let sections: BTreeSet<String> = if usable {
        section_index
            .and_then(|s| js::get(s, "sections"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|s| to_js_string(js::get(s, "file")))
            .collect()
    } else {
        BTreeSet::new()
    };
    let unparsed = if usable {
        section_index
            .and_then(|s| js::get(s, "unparsed"))
            .cloned()
            .unwrap_or(json!([]))
    } else {
        json!([])
    };
    let unsectioned: Vec<Value> = js::get(pr_context, "files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|f| !f.as_str().is_some_and(|f| sections.contains(f)))
        .cloned()
        .collect();
    let available = usable
        && !sections.is_empty()
        && unparsed.as_array().is_none_or(Vec::is_empty)
        && unsectioned.is_empty();
    (
        available,
        sections.len() as f64,
        Value::Array(unsectioned),
        unparsed,
    )
}

/// Every variable the workflow's `metrics` object reads, from the run's
/// outcomes.
#[must_use]
pub fn metrics_scope(
    args: &RunArgs,
    policy: &DispatchPolicy,
    review: &ReviewOutcome,
    verify: &VerifyOutcome,
    synthesis: &SynthesisOutcome,
) -> MetricsScope {
    let rc = &review.context;
    let plan = &rc.plan;
    let panel = &verify.panel;
    let prepared = &review.prepared;
    let review_end = output_tokens(&review.dispatches);
    let verify_end = review_end + output_tokens(verify.dispatches.iter().map(|(_, d)| d));
    let end = verify_end + output_tokens(synthesis.dispatches.iter().map(|(_, d)| d));
    let (sectioning_available, section_count, unsectioned, unparsed) =
        sectioning(rc.section_index.as_ref(), &rc.pr_context);
    let (re, ve) = (&review.exposure, &verify.exposure);
    let lens_plan: Vec<Value> = plan
        .lens_plan
        .iter()
        .map(|s| {
            let mut m = Map::new();
            m.insert("focus".into(), s.focus.clone());
            if let Some(f) = &s.files {
                m.insert("files".into(), json!(f));
            }
            Value::Object(m)
        })
        .collect();
    let opt = |v: &Option<Value>| v.clone().unwrap_or(Value::Null);
    MetricsScope {
        results: verify.verdicts.results.clone(),
        tokens_at_start: 0.0,
        tokens_at_context_end: 0.0,
        tokens_at_review_end: review_end,
        tokens_at_verify_end: verify_end,
        tokens_at_end: end,
        counter_resets: 0.0,
        pr_context: rc.pr_context.clone(),
        diff_mode: json!(if rc.delivery.diff_path.is_some() {
            "path"
        } else {
            "self-fetch"
        }),
        lens_receiver_count: f64::from(re.lens_receivers + ve.lens_receivers),
        refuter_receiver_count: f64::from(re.refuter_receivers + ve.refuter_receivers),
        sectioning_available,
        section_count,
        sectioned_receiver_count: f64::from(re.sectioned_receivers + ve.sectioned_receivers),
        full_diff_receiver_count: f64::from(re.full_diff_receivers + ve.full_diff_receivers),
        delivered_bytes: re.delivered_bytes + ve.delivered_bytes,
        unsectioned_files: unsectioned,
        diff_unparsed_headers: unparsed,
        repo: prepared.repo.clone(),
        pr_number: args.pr.parse::<u64>().map_or(json!(args.pr), |n| json!(n)),
        platform: Value::Null,
        ref_evidence: synthesis.ref_evidence.clone(),
        head_pin: rc.head_pin.clone(),
        pinned_sha: json!(args.head_sha.trim()),
        file_count: plan.file_count,
        lens_plan: Value::Array(lens_plan),
        lens_agent: review.lens_agent.clone(),
        profile_sha256: prepared.profile_sha256.clone(),
        profile_source: prepared.profile_source.clone(),
        project_rules_sha256: prepared.project_rules_sha256.clone(),
        project_rules_source: prepared.project_rules_source.clone(),
        conventions_manifest: prepared.conventions.manifest.clone().unwrap_or(Value::Null),
        lens_labels: array(&review.labels),
        review_results: Value::Array(
            review
                .dispatches
                .iter()
                .map(|d| answer_of(d).unwrap_or(Value::Null))
                .collect(),
        ),
        lens_findings: Value::Array(review.candidates.lens_findings.clone()),
        lenses_override: plan.lenses_override.map_or(Value::Null, |n| json!(n)),
        file_partition_mode: json!(plan.file_partition_mode),
        per_group_churn: json!(plan.per_group_churn),
        use_codex_head: panel.use_codex,
        external_probe_answered: verify.probe_answered.map_or(Value::Null, Value::Bool),
        use_gemini_head: panel.use_gemini,
        external_cwd: verify
            .external_cwd
            .clone()
            .map_or(Value::Null, Value::String),
        clean_sweep: opt(&panel.clean_sweep),
        verified_anchors: synthesis.verified_anchors.clone(),
        vote_quote_check: panel.vote_quote_check.clone(),
        batch_panels_active: panel.batch_panels_active,
        vet_bot_comments: args.bot_comments,
        verdict_guess: synthesis.verdict.guess.to_owned(),
        report_verdict_corrected: synthesis
            .report
            .corrected
            .as_ref()
            .and_then(|c| serde_json::to_value(c).ok())
            .unwrap_or(Value::Null),
        all_candidates: review.candidates.all_candidates.len() as f64,
        placed_aside: verify.verdicts.placed_aside.len() as f64,
        dropped_pre_panel: panel.dropped_pre_panel.clone(),
        blast_relay: rc.blast_radius.is_some(),
        blast_radius: rc.blast_radius.clone().unwrap_or(Value::Null),
        deduped_pre_panel: verify.dedup.deduped_pre_panel as f64,
        vote_corrections: panel.vote_corrections.clone(),
        cross_file_clusters_rejected: verify.dedup.cross_file_clusters_rejected as f64,
        proximity_vetoed: verify.dedup.proximity_vetoed as f64,
        grounded_available: verify.grounded.available,
        grounded_confirmed: verify.gate.confirmed.len() as f64,
        grounded_refuted: verify.gate.refuted.len() as f64,
        evidence_resolver_ran: review.evidence.ran,
        evidence_located_count: review.evidence.located as f64,
        evidence_line_corrected_count: review.evidence.line_corrected as f64,
        evidence_not_found_count: review.evidence.not_found as f64,
        critic_output: panel.critic_output.clone(),
        critic_gaps: panel.critic_gaps as f64,
        relays: json!([]),
        prior_reviews: rc
            .decisions
            .prior_reviews
            .iter()
            .map(|r| r.to_value())
            .collect(),
        prior_review_gate: rc.decisions.prior_review_gate.to_value(),
        fidelity_critic: json!({
            "ran": false,
            "violations": [],
            "skipped": "removed in v101: nothing acted on its flags",
        }),
        retest_claims: rc.decisions.retest_claims.clone(),
        report_as_posted: synthesis.report.as_posted.clone(),
        group_count: plan.group_count,
        churn: plan.churn,
        uncovered_files: json!(plan.uncovered_files),
        lens_not_reached: review
            .candidates
            .lens_not_reached
            .iter()
            .map(|f| json!(f))
            .collect(),
        incremental_scope: opt(&plan.incremental_scope),
        prior_review: opt(&plan.prior_review),
        incremental_fallback: opt(&plan.incremental_fallback),
        all_pr_files: plan.all_pr_files.len() as f64,
        carried_forward: synthesis.prior.outcome.carried.len() as f64,
        prior_outcome: synthesis.prior.outcome.to_json(),
        heuristic_escalated: synthesis.prior.heuristic.len() as f64,
        prior_re_examined: synthesis.prior_re_examined.clone(),
        max_panels: panel.max_panels,
        panel_skipped_for_budget: Value::Array(panel.panel_skipped_for_budget.clone()),
        tiebreak: panel.tiebreak.clone(),
        cost_breaker_fired: opt(&panel.cost_breaker_fired),
        output_ceiling: policy.output_ceiling,
        budget_total: Value::Null,
        cost_degraded_stages: Value::Array(panel.cost_degraded_stages.clone()),
        max_agents: policy.max_agents,
        agents_projected: panel.agents_projected,
        agents_projected_unbatched: panel.agents_projected_unbatched,
        round0_claude_batch_savings: panel.round0_claude_batch_savings,
        refine_claude_batch_savings: panel.refine_claude_batch_savings,
        max_refines: panel.max_refines,
        refines_skipped_for_budget: Value::Array(panel.refines_skipped_for_budget.clone()),
        external_slots: panel.external_slots as f64,
        artifact_dir: verify.vote_dir_path.to_string_lossy().into_owned(),
    }
}

/// The build that writes the row: `rust-` and a hash of the workspace's
/// sources (`build.rs`).
pub const ENGINE_VERSION: &str = env!("TRIBUNAL_ENGINE_VERSION");

/// `agentCost`'s `byKind` bucket for a dispatch label.
#[must_use]
pub fn kind_of(label: &str) -> Option<&'static str> {
    let head = label.split(':').next().unwrap_or(label);
    Some(match head {
        "lens" => "lens",
        "refute" | "refute-batch" => "refuter",
        "refine" => "strengthen",
        "defense" => "defense",
        "judge" => "adjudicator",
        "dedup-candidates" => "dedup",
        "bot-vetting" => "bot-vetting",
        "completeness-critic" => "critic",
        "synthesize" => "synthesize",
        "panel-ceiling-tiebreak" | "verify-prior" => "mechanical",
        _ => return None,
    })
}

#[derive(Default)]
struct Tally {
    agents: u64,
    turns: u64,
    fresh: u64,
    cache_read: u64,
    cache_creation: u64,
    output: u64,
    tool_uses: u64,
    cost_usd: f64,
}

impl Tally {
    fn add(&mut self, d: &DispatchResult) {
        self.agents += 1;
        self.turns += d.output.as_ref().map_or(0, |o| u64::from(o.num_turns));
        self.tool_uses += d.tool_calls.len() as u64;
        self.cost_usd += cost_of(d);
        for u in d.model_usage().into_values() {
            self.fresh += u.input_tokens;
            self.cache_read += u.cache_read_input_tokens;
            self.cache_creation += u.cache_creation_input_tokens;
            self.output += u.output_tokens;
        }
    }

    fn input(&self) -> u64 {
        self.fresh + self.cache_read + self.cache_creation
    }

    fn units(&self) -> f64 {
        let u =
            self.fresh as f64 + self.cache_creation as f64 * 1.25 + self.cache_read as f64 * 0.1;
        (u * 100.0).round() / 100.0
    }

    fn entry(&self) -> Value {
        json!({
            "agents": self.agents, "turns": self.turns, "inputTokens": self.input(),
            "outputTokens": self.output, "inputUnits": self.units(),
            "toolUses": self.tool_uses, "toolTurns": null, "toolUsesPerTurn": null,
            "costUsd": usd(self.cost_usd),
        })
    }
}

/// The CLI's own estimate of a dispatch's price, at list price: what it
/// prints as `total_cost_usd`, not a bill.
fn cost_of(d: &DispatchResult) -> f64 {
    d.output.as_ref().map_or(0.0, |o| o.total_cost_usd)
}

fn usd(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

/// One row per lens: the focus it ran, why the plan had it, and what it cost.
/// `forced` when a risk path the PR touches forces that focus, `default`
/// otherwise; `overBudget` when the lens sits past the size-based budget, so
/// it exists only because the forced foci needed more lenses than the PR's
/// size gave. A lens whose focus is not one of the profile's sentences is
/// `unknown`.
#[must_use]
pub fn lens_cost(
    foci: &[Value],
    files: &[String],
    lens_budget: f64,
    overridden: bool,
    profile: &Value,
    lenses: &[DispatchResult],
) -> Value {
    let labels = foci_label_for(profile).unwrap_or_default();
    let forced: BTreeSet<String> = risk_path_rules(profile)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| files.iter().any(|f| r.re.test(f)))
        .map(|r| r.focus)
        .collect();
    let rows = foci
        .iter()
        .zip(lenses)
        .enumerate()
        .map(|(i, (focus, d))| {
            let lens = focus
                .as_str()
                .and_then(|s| labels.get(s))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let output: u64 = d.model_usage().values().map(|u| u.output_tokens).sum();
            json!({
                "lens": lens,
                "why": if forced.contains(lens) { "forced" } else { "default" },
                "overBudget": !overridden && i as f64 >= lens_budget,
                "costUsd": usd(cost_of(d)),
                "toolUses": d.tool_calls.len(),
                "turns": d.output.as_ref().map_or(0, |o| o.num_turns),
                "outputTokens": output,
            })
        })
        .collect();
    Value::Array(rows)
}

/// `agentCost` in the schema's shape, from the CLI's own usage figures. The
/// output side is exact here; `toolTurns` is not reported by the CLI.
#[must_use]
pub fn agent_cost(dispatches: &[(&str, &DispatchResult)]) -> Value {
    let mut total = Tally::default();
    let mut by_kind: BTreeMap<&'static str, Tally> = BTreeMap::new();
    let mut by_model: BTreeMap<String, Tally> = BTreeMap::new();
    let mut unclassified = Tally::default();
    for (label, d) in dispatches {
        total.add(d);
        match kind_of(label) {
            Some(k) => by_kind.entry(k).or_default().add(d),
            None => unclassified.add(d),
        }
        let model = d
            .model_usage()
            .into_keys()
            .next()
            .unwrap_or_else(|| "unknown".to_owned());
        by_model.entry(model).or_default().add(d);
    }
    json!({
        "runId": null,
        "turns": total.turns,
        "inputTokens": total.input(),
        "outputTokens": total.output,
        "inputUnits": total.units(),
        "outputExact": true,
        "cacheRead": total.cache_read,
        "cacheCreation": total.cache_creation,
        "toolUses": total.tool_uses,
        "toolTurns": null,
        "toolUsesPerTurn": null,
        "agents": total.agents,
        "costUsd": usd(total.cost_usd),
        "missingTranscripts": 0,
        "byKind": by_kind.iter().map(|(k, t)| ((*k).to_owned(), t.entry())).collect::<Map<_, _>>(),
        "byModel": by_model.iter().map(|(k, t)| (k.clone(), t.entry())).collect::<Map<_, _>>(),
        "unclassified": {"agents": unclassified.agents, "inputTokens": unclassified.input()},
    })
}

/// What the persist step leaves: the row as the run knows it, whether the
/// last row on disk is this run's, and why not when it is not.
#[derive(Debug, Clone)]
pub struct Persisted {
    pub metrics: Value,
    pub verified: bool,
    pub detail: Option<String>,
}

/// The newest row on disk, read the way the row check reads it.
fn last_row(data_dir: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(data_dir.join("metrics.jsonl")).ok()?;
    let line = text.lines().rev().find(|l| !l.trim().is_empty())?;
    let r: Value = serde_json::from_str(line).ok()?;
    Some(json!({
        "found": true,
        "pr": js::get(&r, "pr"),
        "repo": js::get(&r, "repo"),
        "headSha": js::get(&r, "headSha"),
        "findings": js::get(&r, "findings").and_then(Value::as_array).map_or(0, Vec::len),
    }))
}

/// Why a lens dispatch came back without an answer, read from its exit and
/// what the CLI printed: `rate-limit`, `overloaded`, `auth`, `timeout`,
/// `killed`, `spawn-failed`, `no-answer` (it exited 0 with no schema'd
/// answer) or `error`.
fn failure_class(d: &DispatchResult) -> &'static str {
    let said = format!(
        "{} {}",
        d.stderr_tail,
        d.output
            .as_ref()
            .and_then(|o| o.result.as_deref())
            .unwrap_or("")
    )
    .to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| said.contains(w));
    match &d.exit {
        Exit::TimedOut => "timeout",
        Exit::Killed(_) => "killed",
        Exit::SpawnFailed(_) => "spawn-failed",
        _ if has(&["429", "rate limit", "rate_limit", "usage limit"]) => "rate-limit",
        _ if has(&["529", "overloaded"]) => "overloaded",
        _ if has(&[
            "401",
            "403",
            "authentication",
            "not logged in",
            "invalid api key",
        ]) =>
        {
            "auth"
        }
        Exit::Code(0) => "no-answer",
        Exit::Code(_) => "error",
    }
}

/// The lens dispatches that returned no answer, by lens and cause: the
/// row's `lensFailures`.
fn lens_failures(labels: &[String], lenses: &[DispatchResult]) -> Value {
    Value::Array(
        labels
            .iter()
            .zip(lenses)
            .filter(|(_, d)| d.structured().is_none())
            .map(|(l, d)| json!({"lens": l, "cause": failure_class(d)}))
            .collect(),
    )
}

/// The end of the Synthesize phase: build the row, write the payload files,
/// run `persist-run`, fold what it prints back into the row, and check the
/// row landed.
///
/// # Errors
/// When the row cannot be built or the vote directory cannot be written.
pub fn persist_phase(
    rt: &Runtime,
    args: &RunArgs,
    policy: &DispatchPolicy,
    review: &ReviewOutcome,
    verify: &VerifyOutcome,
    synthesis: &SynthesisOutcome,
    logs: &mut Vec<String>,
) -> Result<Persisted, String> {
    let scope = metrics_scope(args, policy, review, verify, synthesis);
    let mut metrics = metrics_row(&scope)?;
    let lenses = review.context.plan.lens_plan.len();
    let labelled: Vec<(&str, &DispatchResult)> = review
        .dispatches
        .iter()
        .enumerate()
        .map(|(i, d)| (if i < lenses { "lens" } else { "bot-vetting" }, d))
        .chain(verify.dispatches.iter().map(|(l, d)| (l.as_str(), d)))
        .chain(synthesis.dispatches.iter().map(|(l, d)| (l.as_str(), d)))
        .collect();
    metrics["agentCost"] = agent_cost(&labelled);
    let plan = &review.context.plan;
    let foci: Vec<Value> = plan.lens_plan.iter().map(|s| s.focus.clone()).collect();
    metrics["lensCost"] = lens_cost(
        &foci,
        &plan.files,
        plan.lens_budget,
        plan.lenses_override.is_some(),
        &review.prepared.profile,
        &review.dispatches[..lenses.min(review.dispatches.len())],
    );
    metrics["engine"] = json!("rust");
    metrics["engineVersion"] = json!(ENGINE_VERSION);
    metrics["pluginVersion"] =
        tribunal_context::post_footer::own_plugin_version(&rt.plugin_root.to_string_lossy())
            .unwrap_or(Value::Null);
    metrics["lensNotReachedKinds"] = kind_counts(&review.candidates.lens_gaps);
    metrics["unreadRiskFiles"] = json!(synthesis.unread_risk_files);
    metrics["lensFailures"] = lens_failures(
        &review.labels,
        &review.dispatches[..lenses.min(review.dispatches.len())],
    );
    // `agentsActual` stays null through the self-audit, as the workflow's row
    // does, and is patched in afterwards the way SKILL.md's Step 4 patches it.
    let agent_count = labelled.len();

    let vd = &verify.vote_dir_path;
    std::fs::create_dir_all(vd).map_err(|e| format!("cannot create {}: {e}", vd.display()))?;
    let write = |name: &str, body: String| {
        std::fs::write(vd.join(name), body).map_err(|e| format!("cannot write {name}: {e}"))
    };
    let votes = external_votes(&verify.verdicts.results);
    let has_votes = !votes.is_empty();
    let expected = js::get(&metrics, "findings")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    write("metrics.json", persist_json(&metrics))?;
    write(
        "report.json",
        persist_json(&json!({ "report": synthesis.report.report })),
    )?;
    if has_votes {
        write("recorded-votes.json", persist_json(&Value::Array(votes)))?;
    }

    let head_sha = js::get(&review.context.pr_context, "headRefOid")
        .and_then(Value::as_str)
        .unwrap_or("");
    let start_hint = review
        .context
        .plan
        .start_epoch
        .map_or(String::new(), |e| format!("{}", e as u64));
    let pr: u64 = args
        .pr
        .parse()
        .map_err(|_| format!("PR number {} is not a number", args.pr))?;
    let plugin_root = rt.plugin_root.to_string_lossy();
    let line = persist_run::run(&PersistRun {
        vote_dir: vd,
        has_votes,
        expected_findings: expected,
        pr,
        head_sha,
        batch_panels: verify.panel.batch_panels_active,
        start_epoch_hint: &start_hint,
        repo: Some(review.prepared.repo.as_str()),
        data_dir: &rt.data_dir,
        plugin_root: &plugin_root,
        now: inflight::now_epoch(),
    });
    let bookkeeping: Option<Value> = serde_json::from_str(&line).ok();
    let b = bookkeeping.as_ref();

    let rec = resolve_vote_reconciliation(b.and_then(|b| js::get(b, "reconciliation")), has_votes);
    reconciliation_logs(&rec, has_votes, &vd.to_string_lossy(), logs);
    metrics["voteReconciliation"] = rec;
    metrics["selfAudit"] = self_audit(b, logs);
    for (k, v) in wall_clock_fields(b) {
        metrics[k] = v;
    }
    let persisted = b
        .and_then(|b| js::get(b, "persisted"))
        .cloned()
        .unwrap_or(json!("reconcile-audit-persist returned nothing usable"));

    let check = last_row(&rt.data_dir);
    let pr_value = json!(pr);
    let verified = row_is_this_run(
        check.as_ref(),
        &pr_value,
        &review.prepared.repo,
        Some(head_sha),
        expected,
    );
    let detail = (!verified).then(|| {
        format!(
            "expected pr {pr} / {expected} findings, no recent row on disk matches; the newest reports {}. persist-metrics returned: {}",
            js::stringify(check.as_ref().unwrap_or(&Value::Null), ""),
            tribunal_core::lens::js_ext::utf16_prefix_lossy(&to_js_string(Some(&persisted)), 400)
        )
    });
    if let Some(d) = &detail {
        logs.push(format!(
            "METRICS ROW NOT VERIFIED — {d}. The run's measurement is incomplete; the returned metrics object is the authoritative copy."
        ));
    } else {
        let ledger = LedgerEnv {
            home: rt.operator.home.clone(),
            cwd: rt.operator.cwd.to_string_lossy().into_owned(),
            data_dir: Some(rt.data_dir.to_string_lossy().into_owned()),
            project_dir: None,
            lock_wait: tribunal_context::ledger::lock_wait(),
        };
        let patched = review_record::run(
            &[
                "--pr",
                args.pr.as_str(),
                "--repo",
                review.prepared.repo.as_str(),
                "--head-sha",
                head_sha,
                "--posted",
                "false",
                "--verdict",
                synthesis.verdict.guess,
                "--agent-count",
                agent_count.to_string().as_str(),
            ]
            .map(str::to_owned),
            &ledger,
        );
        if patched.code == 0 {
            metrics["agentsActual"] = json!(agent_count);
        } else {
            logs.push(format!(
                "agentsActual not patched onto the row: {}",
                patched.stderr.trim()
            ));
        }
    }
    Ok(Persisted {
        metrics,
        verified,
        detail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tribunal_agent::{CliResult, Exit, ModelUsage};

    fn dispatch(cost: f64, tools: usize, output: u64) -> DispatchResult {
        DispatchResult {
            exit: Exit::Code(0),
            output: Some(CliResult {
                subtype: "success".into(),
                is_error: false,
                result: None,
                structured_output: None,
                num_turns: 3,
                session_id: None,
                duration_ms: 0,
                total_cost_usd: cost,
                model_usage: BTreeMap::from([(
                    "claude-sonnet-5-5".to_owned(),
                    ModelUsage {
                        input_tokens: 0,
                        output_tokens: output,
                        cache_read_input_tokens: 0,
                        cache_creation_input_tokens: 0,
                        cost_usd: cost,
                    },
                )]),
                permission_denials: Vec::new(),
            }),
            stderr_tail: String::new(),
            tool_calls: vec!["Read".to_owned(); tools],
            transcript: None,
        }
    }

    fn profile() -> Value {
        json!({
            "foci": {"correctness": "c", "security": "s", "migration": "m"},
            "riskPaths": [{"focus": "migration", "pattern": "^tests/signup"}],
        })
    }

    #[test]
    fn agent_cost_adds_up_dollars_per_kind() {
        let (a, b, c) = (
            dispatch(0.25, 1, 10),
            dispatch(0.5, 2, 20),
            dispatch(0.125, 0, 5),
        );
        let cost = agent_cost(&[("lens", &a), ("lens", &b), ("synthesize", &c)]);
        assert_eq!(cost["costUsd"], json!(0.875));
        assert_eq!(cost["byKind"]["lens"]["costUsd"], json!(0.75));
        assert_eq!(cost["byKind"]["synthesize"]["costUsd"], json!(0.125));
    }

    #[test]
    fn lens_cost_names_each_lens_and_why_it_ran() {
        let foci = [json!("m"), json!("s"), json!("c")];
        let files = ["tests/signup.spec.ts".to_owned()];
        let lenses = [
            dispatch(0.3, 8, 100),
            dispatch(0.2, 5, 50),
            dispatch(0.11119, 21, 70),
        ];
        let rows = lens_cost(&foci, &files, 2.0, false, &profile(), &lenses);
        assert_eq!(
            rows,
            json!([
                {"lens": "migration", "why": "forced", "overBudget": false, "costUsd": 0.3,
                 "toolUses": 8, "turns": 3, "outputTokens": 100},
                {"lens": "security", "why": "default", "overBudget": false, "costUsd": 0.2,
                 "toolUses": 5, "turns": 3, "outputTokens": 50},
                {"lens": "correctness", "why": "default", "overBudget": true, "costUsd": 0.1112,
                 "toolUses": 21, "turns": 3, "outputTokens": 70},
            ])
        );
    }

    #[test]
    fn an_overridden_count_is_never_over_budget() {
        let foci = [json!("c"), json!("x")];
        let lenses = [dispatch(0.1, 1, 1), dispatch(0.1, 1, 1)];
        let rows = lens_cost(&foci, &[], 1.0, true, &profile(), &lenses);
        assert_eq!(rows[1]["overBudget"], json!(false));
        assert_eq!(rows[1]["lens"], json!("unknown"));
    }
}
