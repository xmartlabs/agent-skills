//! A run from the operator's arguments to the Review phase's candidates: the
//! steps of [`crate::context`], one dispatch per planned lens (and the bot
//! pass), the candidates through the out-of-diff gate, and the evidence
//! resolver.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use tribunal_agent::{Agent, DispatchRequest, DispatchResult, Exit};
use tribunal_context::grounded_typecheck::{self, remove_tree, TypecheckRun};
use tribunal_context::head_worktree::{
    build_head_worktree, mktemp_then_rmdir, parse_head_worktree,
};
use tribunal_context::heads::{self, HeadSetup};
use tribunal_context::{resolve_evidence_line, resolve_verified_anchors};
use tribunal_core::evidence::line_correction::apply_line_correction;
use tribunal_core::js;
use tribunal_core::lens::coverage_gaps::{coverage_banner, unread_risk_files};
use tribunal_core::lens::js_ext::{to_js_string, utf16_prefix_lossy};
use tribunal_core::profile::lens_foci::foci_label_for;
use tribunal_core::profile::lens_routing::lens_agent_for;
use tribunal_core::profile::profile_lookup::risk_path_rules;
use tribunal_core::report::draft_comments_strip::strip_draft_comments_from_body;
use tribunal_core::votes::evidence_resolver_degrade::evidence_resolver_failure;

use crate::agent_def::{self, AgentDef};
use crate::context::{
    context_phase, prepare, reviewer_cwd, ContextOutcome, Prepared, ReviewContext, RunArgs, Runtime,
};
use crate::decisions::is_retest_target;
use crate::live::{LiveIo, PanelAgents};
use crate::panel::{panel_phase, PanelEnv, PanelFlags, PanelOutcome, OUTPUT_CEILING_DEFAULT};
use crate::prompts::{lens_dispatch_prompt, DiffExposure, LensPromptInput};
use crate::review::{
    answer_of, bot_task, bot_vetting_prompt, check_lenses_returned, collect_candidates,
    dispatch_all, lens_labels, lens_task, Candidates, ReviewerSetup, ReviewerTask,
};
use crate::schemas;
use crate::synthesize::{
    anchored_payload, baseline_description, dispatch_failed, finish_report, graded,
    missing_identifiers, prior_check, prior_re_examined, ref_evidence, ref_evidence_targets,
    side_payloads, surviving_for_synthesis, synthesis_prompt, verdict, verified_anchors,
    verify_prior_prompt, with_banners, PriorCheck, Relay, Report, SynthesisInput, Verdict,
};
use crate::verify::{
    apply_dedup, apply_typecheck_result, dedup_prompt, grounded_gate, partition_results, Dedup,
    Grounded, GroundedGate, Verdicts, MAX_AGENTS_DEFAULT, MAX_PANELS_DEFAULT,
};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EvidenceStats {
    pub ran: bool,
    pub located: usize,
    pub not_found: usize,
    pub line_corrected: usize,
}

/// Applies the evidence resolver's answer to the candidates, as the workflow
/// does after its relay. `parsed` is the resolver's output; `relay_reason`
/// is why it is missing, when it is.
pub fn apply_evidence(
    candidates: &mut [Value],
    parsed: Option<&Value>,
    relay_reason: Option<&str>,
    logs: &mut Vec<String>,
) -> EvidenceStats {
    let mut s = EvidenceStats::default();
    let results = parsed
        .filter(|p| js::get(p, "ran") == Some(&Value::Bool(true)))
        .and_then(|p| js::get(p, "results"))
        .and_then(Value::as_array);
    let Some(results) = results else {
        logs.push(evidence_resolver_failure(
            parsed,
            candidates.len(),
            relay_reason,
        ));
        return s;
    };
    s.ran = true;
    for r in results {
        let Some(i) = js::get(r, "id")
            .and_then(Value::as_f64)
            .filter(|i| i.fract() == 0.0 && *i >= 0.0 && (*i as usize) < candidates.len())
        else {
            continue;
        };
        let mut resolution = r.as_object().cloned().unwrap_or_default();
        resolution.shift_remove("id");
        let resolution = Value::Object(resolution);
        let c = &mut candidates[i as usize];
        c["evidenceResolution"] = resolution.clone();
        if apply_line_correction(c, &resolution) {
            s.line_corrected += 1;
        }
        if js::get(&resolution, "evidenceLocated") == Some(&Value::Bool(true)) {
            s.located += 1;
        }
        if js::get(&resolution, "matchKind").and_then(Value::as_str) == Some("not-found") {
            s.not_found += 1;
        }
    }
    logs.push(format!(
        "Evidence resolver: {} located / {} not-found across {} candidate(s); {} patch line number(s) corrected to the file line",
        s.located,
        s.not_found,
        results.len(),
        s.line_corrected
    ));
    s
}

/// How the run's reviewers are dispatched, beyond what the profile decides.
#[derive(Debug, Clone)]
pub struct DispatchPolicy {
    pub lens_timeout: Duration,
    pub concurrency: usize,
    /// `--max-panels` and `--max-agents` after the workflow's defaulting.
    pub max_panels: f64,
    pub max_agents: f64,
    /// See [`ReviewerSetup::effort`].
    pub effort: Option<String>,
    /// The cost breaker's output-token ceiling.
    pub output_ceiling: f64,
}

impl Default for DispatchPolicy {
    fn default() -> Self {
        Self {
            lens_timeout: Duration::from_secs(30 * 60),
            concurrency: 8,
            max_panels: MAX_PANELS_DEFAULT,
            max_agents: MAX_AGENTS_DEFAULT,
            effort: None,
            output_ceiling: OUTPUT_CEILING_DEFAULT,
        }
    }
}

#[derive(Debug)]
pub struct ReviewOutcome {
    /// Wall-clock seconds of the context and lenses stages.
    pub context_seconds: f64,
    pub lenses_seconds: f64,
    pub prepared: Prepared,
    /// The bot-vetting pass's answer, when it ran.
    pub bot_vetting: Option<Value>,
    pub context: Box<ReviewContext>,
    pub lens_agent: String,
    pub labels: Vec<String>,
    pub dispatches: Vec<DispatchResult>,
    pub candidates: Candidates,
    /// The planned lenses whose dispatch came back empty.
    pub lenses_unreturned: Vec<Value>,
    pub evidence: EvidenceStats,
    pub exposure: DiffExposure,
}

#[derive(Debug)]
pub enum RunOutcome {
    Skipped { reason: String },
    Preflight { summary: Value },
    Reviewed(Box<ReviewOutcome>),
}

/// The run up to the end of the Review phase. `logs` receives every line the
/// workflow would `log`, in order.
///
/// # Errors
/// Where the workflow throws or stops the run.
pub async fn run_review<A: Agent>(
    agent: &A,
    rt: &Runtime,
    args: &RunArgs,
    policy: &DispatchPolicy,
    logs: &mut Vec<String>,
) -> Result<RunOutcome, String> {
    let context_started = std::time::Instant::now();
    let prepared = prepare(rt, args).await?;
    logs.extend(prepared.logs.iter().cloned());
    let rc = match context_phase(agent, rt, args, &prepared, logs).await? {
        ContextOutcome::Skipped { reason } => return Ok(RunOutcome::Skipped { reason }),
        ContextOutcome::Preflight { summary } => return Ok(RunOutcome::Preflight { summary }),
        ContextOutcome::Ready(rc) => rc,
    };
    let context_seconds = context_started.elapsed().as_secs_f64();
    let lenses_started = std::time::Instant::now();
    let profile = &prepared.profile;
    let lens_agent = lens_agent_for(profile)?.to_owned();
    let definition = agent_def::load(&rt.plugin_root, &lens_agent)?;
    let label_map = foci_label_for(profile)?;
    let foci: Vec<Value> = rc.plan.lens_plan.iter().map(|s| s.focus.clone()).collect();
    let labels = lens_labels(&foci, &label_map);

    let mut exposure = DiffExposure::default();
    let pr_number = args.pr.parse::<u64>().map_or(json!(args.pr), |n| json!(n));
    let high_risk = js::get(profile, "highRiskSurface");
    let mut tasks = Vec::new();
    for (i, slot) in rc.plan.lens_plan.iter().enumerate() {
        let prompt = lens_dispatch_prompt(
            &LensPromptInput {
                pr_number: &pr_number,
                repo: &prepared.repo,
                jira_ticket: js::get(&rc.pr_context, "jiraTicket"),
                context_block: &rc.decisions.context_block,
                dependents: &rc.dependents,
                high_risk_surface: high_risk,
                focus: &slot.focus,
                files: slot.files.as_deref(),
            },
            &rc.delivery,
            &prepared.conventions,
            &rc.methodology_line,
            &mut exposure,
        )?;
        tasks.push(lens_task(format!("lens:{}", i + 1), prompt));
    }
    let has_bot_comments = args.bot_comments
        && js::get(&rc.pr_context, "botComments")
            .and_then(Value::as_array)
            .is_some_and(|b| !b.is_empty());
    if has_bot_comments {
        let prompt = bot_vetting_prompt(
            &args.pr,
            &prepared.repo,
            &to_js_string(js::get(&rc.pr_context, "botCommentsPath")),
        );
        tasks.push(bot_task(
            prepared
                .conventions
                .reviewer_prompt(&rc.methodology_line, &prompt),
        ));
    }
    let setup = ReviewerSetup {
        agent: definition,
        model: args.review_model.clone(),
        cwd: reviewer_cwd(&rc, rt),
        timeout: policy.lens_timeout,
        concurrency: policy.concurrency,
        effort: policy.effort.clone(),
    };
    let dispatches = dispatch_all(agent, &setup, &tasks).await;
    let answers: Vec<Option<Value>> = dispatches.iter().map(answer_of).collect();
    let bot_vetting = if has_bot_comments {
        answers.get(rc.plan.lens_plan.len()).cloned().flatten()
    } else {
        None
    };

    let mut candidates = collect_candidates(
        &answers,
        &labels,
        has_bot_comments,
        js::get(&rc.pr_context, "botComments"),
        &rc.plan.files,
        rc.blast_radius.as_ref(),
        profile,
    )?;
    logs.extend(candidates.logs.iter().cloned());
    let plan_files: Vec<Option<Vec<String>>> =
        rc.plan.lens_plan.iter().map(|s| s.files.clone()).collect();
    let (lenses_unreturned, unreturned_log) =
        check_lenses_returned(&plan_files, &answers, &labels)?;
    logs.extend(unreturned_log);

    let mut evidence = EvidenceStats::default();
    if let (false, Some(diff_path)) = (
        candidates.all_candidates.is_empty(),
        rc.delivery.diff_path.as_deref(),
    ) {
        let wire: Vec<Value> = candidates
            .all_candidates
            .iter()
            .enumerate()
            .map(|(i, c)| json!({"id": i, "file": js::get(c, "file"), "line": js::get(c, "line"), "evidence": js::get(c, "evidence")}))
            .collect();
        let parsed = resolve_evidence_line::run(
            std::path::Path::new(diff_path),
            &Value::from(wire).to_string(),
        );
        let (parsed, reason) = match parsed {
            Ok(p) => (Some(p), None),
            Err(e) => (None, Some(e)),
        };
        evidence = apply_evidence(
            &mut candidates.all_candidates,
            parsed.as_ref(),
            reason.as_deref(),
            logs,
        );
    }
    Ok(RunOutcome::Reviewed(Box::new(ReviewOutcome {
        context_seconds,
        lenses_seconds: lenses_started.elapsed().as_secs_f64(),
        prepared,
        bot_vetting,
        context: rc,
        lens_agent,
        labels,
        dispatches,
        candidates,
        lenses_unreturned,
        evidence,
        exposure,
    })))
}

/// One dispatch outside the reviewer fan-out: a scribe or mechanic step.
#[allow(clippy::too_many_arguments)]
async fn dispatch_one<A: Agent>(
    agent: &A,
    def: &AgentDef,
    label: &str,
    prompt: String,
    model: &str,
    effort: Option<&String>,
    schema: Option<Value>,
    cwd: &std::path::Path,
) -> DispatchResult {
    let mut req = DispatchRequest::new(label, prompt, cwd);
    req.system_prompt = Some(def.body.clone());
    req.model = Some(model.to_owned());
    req.effort = effort.cloned();
    req.tools = def.tools.clone();
    req.schema = schema;
    req.timeout = Duration::from_secs(30 * 60);
    agent.run(&req).await
}

#[derive(Debug)]
pub struct VerifyOutcome {
    /// Wall-clock seconds of the dedup stage and of the panel stage, which
    /// excludes the completeness critic (`panel.critic_seconds`).
    pub dedup_seconds: f64,
    pub panel_seconds: f64,
    pub dedup: Dedup,
    pub typecheck: Option<TypecheckRun>,
    pub grounded: Grounded,
    pub gate: GroundedGate,
    pub panel: PanelOutcome,
    pub verdicts: Verdicts,
    pub dispatches: Vec<(String, DispatchResult)>,
    /// Where the external heads left their artifacts.
    pub vote_dir: Option<PathBuf>,
    /// A tree made for the heads alone, and whether it is a plain clone.
    pub external_tree: Option<(String, bool)>,
    /// `VOTE_DIR`: the probe's directory, else the shared fallback.
    pub vote_dir_path: PathBuf,
    /// `externalProbeAnswered`: null when no probe ran.
    pub probe_answered: Option<bool>,
    pub external_cwd: Option<String>,
    /// The refuters' share of the diff exposure.
    pub exposure: DiffExposure,
}

fn output_of(results: &[&DispatchResult]) -> f64 {
    results
        .iter()
        .flat_map(|d| d.model_usage().into_values())
        .map(|u| u.output_tokens as f64)
        .sum()
}

/// The Verify phase up to the "Verify done" line.
///
/// # Errors
/// Where the workflow throws.
#[allow(clippy::too_many_lines)]
pub async fn verify_phase<A: Agent>(
    agent: &A,
    rt: &Runtime,
    args: &RunArgs,
    policy: &DispatchPolicy,
    review: &ReviewOutcome,
    logs: &mut Vec<String>,
) -> Result<VerifyOutcome, String> {
    let all = &review.candidates.all_candidates;
    let mut dispatches = Vec::new();
    let dedup_started = std::time::Instant::now();
    let dedup = if all.len() >= 2 {
        let scribe = agent_def::load(&rt.plugin_root, "tribunal-scribe")?;
        let res = dispatch_one(
            agent,
            &scribe,
            "dedup-candidates",
            dedup_prompt(all),
            "haiku",
            policy.effort.as_ref(),
            Some(schemas::dedup()),
            &rt.operator.cwd,
        )
        .await;
        let answer = answer_of(&res);
        dispatches.push(("dedup-candidates".to_owned(), res));
        apply_dedup(all, answer.as_ref(), logs)?
    } else {
        Dedup {
            panel_candidates: all.clone(),
            ..Dedup::default()
        }
    };
    let dedup_seconds = dedup_started.elapsed().as_secs_f64();
    let rc = &review.context;
    let pr_files = &rc.plan.files;
    let eligible = dedup.panel_candidates.iter().any(|c| {
        js::truthy(js::get(c, "mechanicalClaim"))
            || js::get(c, "file")
                .and_then(Value::as_str)
                .is_some_and(|f| f.ends_with(".ts") || f.ends_with(".tsx"))
    });
    let head = js::get(&rc.pr_context, "headRefOid");
    let pin_ok = js::truthy(js::get(&rc.head_pin, "ok"));
    let pinned = js::get(&rc.head_pin, "path")
        .and_then(Value::as_str)
        .filter(|_| pin_ok);
    let mut grounded = Grounded::default();
    let mut typecheck = None;
    if eligible && js::truthy(head) {
        let run = grounded_typecheck::run(
            &rt.tools,
            &review.prepared.repo,
            &args.pr,
            &to_js_string(head),
            pr_files,
            pinned,
            &rt.operator.cwd,
            &rt.tmpdir,
        )
        .await;
        grounded = apply_typecheck_result(&run.text, pr_files, logs)?;
        typecheck = Some(run);
    }
    let gate = grounded_gate(&dedup.panel_candidates, &grounded, pr_files, logs)?;

    // The availability probe: which CLIs exist, and a tree for them to read.
    let flags = PanelFlags {
        force_codex: args.codex == Some(true),
        no_codex: args.codex == Some(false),
        force_gemini: args.gemini == Some(true),
        no_gemini: args.gemini == Some(false),
        force_batch: args.batch_panels == Some(true),
        no_batch: args.batch_panels == Some(false),
    };
    let any_candidate = !dedup.panel_candidates.is_empty();
    let probe = if any_candidate {
        !flags.no_codex || !flags.no_gemini
    } else {
        !flags.no_codex
    };
    let codex_installed = probe && heads::installed("codex");
    let gemini_installed = probe && heads::installed("gemini");
    let use_codex = codex_installed && (!flags.no_codex || flags.force_codex);
    let use_gemini = gemini_installed && flags.force_gemini;
    let mut external_tree = None;
    let mut external_cwd: Option<(String, bool)> = pinned.map(|p| {
        (
            p.to_owned(),
            js::get(&rc.head_pin, "kind") == Some(&json!("clone")),
        )
    });
    let wants_tree = js::truthy(head);
    if probe && wants_tree && external_cwd.is_none() && (use_codex || use_gemini) {
        let text = build_head_worktree(
            &rt.tools,
            &to_js_string(head),
            &review.prepared.repo,
            &args.pr,
            Some(&rt.operator.cwd),
            &rt.tmpdir,
        )
        .await;
        let pin = parse_head_worktree(Some(&json!(text.trim())))?;
        if let (true, Some(path)) = (pin.ok, pin.path) {
            let tree = (path, pin.kind == "clone");
            external_tree = Some(tree.clone());
            external_cwd = Some(tree);
        }
    }
    if use_codex {
        logs.push("Codex head enabled — default independent voter on every panel".into());
    }
    if use_gemini {
        logs.push(
            "Gemini head enabled — additive 3rd family on high-risk findings + Codex-slot fallback"
                .into(),
        );
    }
    if wants_tree && (use_codex || use_gemini) {
        logs.push(match &external_cwd {
            Some((_, clone)) => format!(
                "External refuters pinned to PR head {} ({})",
                if *clone { "clone (ambient cwd was not this repo)" } else { "worktree" },
                utf16_prefix_lossy(&to_js_string(head), 8)
            ),
            None => "Worktree AND fallback clone both failed — external refuters run in the orchestrator's ambient checkout (degraded, likely wrong repo)".into(),
        });
    }
    let vote_dir = if probe {
        let dir = mktemp_then_rmdir(&rt.tmpdir, &args.pr, Some(&rt.operator.cwd))
            .replace("peer-review-wt-", "peer-review-votes-");
        (!dir.is_empty() && std::fs::create_dir_all(&dir).is_ok()).then(|| PathBuf::from(dir))
    } else {
        None
    };
    let vote_dir_path = vote_dir.clone().unwrap_or_else(|| {
        let sha = js::get(&rc.pr_context, "headRefOid")
            .filter(|h| js::truthy(Some(h)))
            .map_or_else(|| "nosha".to_owned(), |h| to_js_string(Some(h)));
        PathBuf::from(format!(
            "{}/peer-review-votes-{}-{}",
            tribunal_context::proc::fallback_tmpdir(),
            args.pr,
            utf16_prefix_lossy(&sha, 8)
        ))
    });
    if vote_dir.is_none() {
        logs.push(format!("Artifact directory not resolved by the probe — falling back to {}. This path is shared by every run on this commit, so invocation counts read from it may include earlier runs.", vote_dir_path.display()));
    }

    let agents = PanelAgents {
        refuter: agent_def::load(&rt.plugin_root, "tribunal-refuter")?,
        strengthener: agent_def::load(&rt.plugin_root, "tribunal-strengthener")?,
        lens: agent_def::load(&rt.plugin_root, &review.lens_agent)?,
        scribe: agent_def::load(&rt.plugin_root, "tribunal-scribe")?,
    };
    let mut cross_repo = Vec::new();
    for a in js::get(&review.prepared.profile, "crossRepoAnchors")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        cross_repo.push("--cross-repo".to_owned());
        cross_repo.push(format!(
            "{}@{}",
            to_js_string(js::get(a, "repo")),
            to_js_string(js::get(a, "ref"))
        ));
    }
    let io = LiveIo {
        agent,
        agents,
        model: args.review_model.clone(),
        effort: policy.effort.clone(),
        reviewer_cwd: reviewer_cwd(rc, rt),
        operator_cwd: rt.operator.cwd.clone(),
        timeout: policy.lens_timeout,
        gate: std::sync::Arc::new(tokio::sync::Semaphore::new(policy.concurrency.max(1))),
        conventions: &review.prepared.conventions,
        methodology_line: &rc.methodology_line,
        heads: HeadSetup {
            vote_dir: vote_dir_path.clone(),
            external_cwd: external_cwd.as_ref().map(|(p, _)| PathBuf::from(p)),
            fallback_cwd: rt.operator.cwd.clone(),
            home: PathBuf::from(&rt.operator.home),
            codex_bin: "codex".into(),
            gemini_bin: "gemini".into(),
        },
        repo: review.prepared.repo.clone(),
        cross_repo,
        tools: rt.tools.clone(),
        dispatches: std::sync::Mutex::new(Vec::new()),
        output_tokens: std::sync::Mutex::new(0.0),
    };
    let mut exposure = DiffExposure::default();
    let diff_block = rc.delivery.refuter_diff_block(&mut exposure)?;
    let claims = &rc.decisions.retest_claims;
    let retest = |f: &Value| is_retest_target(claims, f);
    let spent = || io.spent();
    let before: Vec<&DispatchResult> = review
        .dispatches
        .iter()
        .chain(dispatches.iter().map(|(_, d)| d))
        .collect();
    let pr_number = args.pr.parse::<u64>().map_or(json!(args.pr), |n| json!(n));
    let env = PanelEnv {
        pr_number: &pr_number,
        repo: &review.prepared.repo,
        pr_context: &rc.pr_context,
        profile: &review.prepared.profile,
        combined_decisions: &rc.decisions.combined_decisions,
        retest: &retest,
        diff_block: &diff_block,
        section_index: rc.section_index.as_ref(),
        diff_path: rc.delivery.diff_path.as_deref(),
        blast_radius: rc.blast_radius.as_ref(),
        file_count: rc.plan.file_count,
        flags,
        codex_installed,
        gemini_installed,
        external_cwd: external_cwd.as_ref().map(|(p, _)| p.as_str()),
        max_panels: policy.max_panels,
        max_agents: policy.max_agents,
        output_ceiling: policy.output_ceiling,
        spent: &spent,
        spent_before: output_of(&before),
    };
    let panel_started = std::time::Instant::now();
    let panel = panel_phase(
        &io,
        &env,
        gate.panel_input.clone(),
        review.candidates.dropped_pre_panel.clone(),
        logs,
    )
    .await?;
    let panel_seconds = (panel_started.elapsed().as_secs_f64() - panel.critic_seconds).max(0.0);
    dispatches.extend(io.dispatches.into_inner().expect("lock"));
    let verdicts = partition_results(
        &panel.verified,
        &gate.confirmed,
        panel.dropped_pre_panel.len(),
        logs,
    );
    Ok(VerifyOutcome {
        dedup_seconds,
        panel_seconds,
        dedup,
        typecheck,
        grounded,
        gate,
        panel,
        verdicts,
        dispatches,
        vote_dir,
        external_tree,
        vote_dir_path,
        probe_answered: probe.then_some(true),
        external_cwd: external_cwd.map(|(p, _)| p),
        exposure,
    })
}

#[derive(Debug)]
pub struct SynthesisOutcome {
    pub synthesize_seconds: f64,
    pub prior: PriorCheck,
    pub prior_re_examined: Vec<Value>,
    pub surviving: Vec<Value>,
    pub verified_anchors: Value,
    pub ref_evidence: Value,
    pub verdict: Verdict,
    pub prompt: String,
    pub report: Report,
    /// The `riskPaths` files every lens that held them left unread for
    /// budget, which keep the verdict from APPROVE.
    pub unread_risk_files: Vec<String>,
    pub dispatches: Vec<(String, DispatchResult)>,
}

fn cross_repo_flags(profile: &Value) -> Vec<String> {
    js::get(profile, "crossRepoAnchors")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .flat_map(|a| {
            [
                "--cross-repo".to_owned(),
                format!(
                    "{}@{}",
                    to_js_string(js::get(a, "repo")),
                    to_js_string(js::get(a, "ref"))
                ),
            ]
        })
        .collect()
}

/// The Synthesize phase up to the report as posted.
///
/// # Errors
/// Where the workflow throws, and when the synthesis returned no report.
pub async fn synthesize_phase<A: Agent>(
    agent: &A,
    rt: &Runtime,
    args: &RunArgs,
    policy: &DispatchPolicy,
    review: &ReviewOutcome,
    verify: &VerifyOutcome,
    logs: &mut Vec<String>,
) -> Result<SynthesisOutcome, String> {
    let synthesize_started = std::time::Instant::now();
    let rc = &review.context;
    let plan = &rc.plan;
    let profile = &review.prepared.profile;
    let head = js::get(&rc.pr_context, "headRefOid");
    let v = &verify.verdicts;
    let mut dispatches = Vec::new();

    let current: Vec<Value> = v
        .surviving
        .iter()
        .chain(&v.placed_aside)
        .filter_map(|r| js::get(r, "finding").cloned())
        .collect();
    let prior = prior_check(
        plan.incremental_scope.as_ref(),
        plan.prior_review.as_ref(),
        &current,
        &plan.import_adjacency,
        logs,
    )?;
    let mut verified = Vec::new();
    if !prior.to_verify.is_empty() {
        let prior_review = plan.prior_review.clone().unwrap_or(Value::Null);
        let diff_body = rc.delivery.diff_body("");
        let tasks: Vec<ReviewerTask> = prior
            .to_verify
            .iter()
            .map(|p| ReviewerTask {
                label: format!(
                    "verify-prior:{}",
                    to_js_string(js::get(p, "file"))
                        .rsplit('/')
                        .next()
                        .unwrap_or("")
                ),
                prompt: verify_prior_prompt(
                    p,
                    &prior_review,
                    head,
                    &review.prepared.repo,
                    &diff_body,
                ),
                schema: schemas::prior_verification(),
            })
            .collect();
        let setup = ReviewerSetup {
            agent: agent_def::load(&rt.plugin_root, "finding-verifier")?,
            model: args.review_model.clone(),
            cwd: reviewer_cwd(rc, rt),
            timeout: policy.lens_timeout,
            concurrency: policy.concurrency,
            effort: policy.effort.clone(),
        };
        let results = dispatch_all(agent, &setup, &tasks).await;
        for ((p, t), res) in prior.to_verify.iter().zip(&tasks).zip(results) {
            verified.push(match (answer_of(&res), &res.exit) {
                (Some(a), _) => graded(p, Some(&a)),
                (None, Exit::SpawnFailed(_)) => dispatch_failed(p),
                (None, _) => graded(p, None),
            });
            dispatches.push((t.label.clone(), res));
        }
    }
    let re_examined = prior_re_examined(&verified, &prior, logs);

    let surviving = surviving_for_synthesis(&v.surviving);
    let (anchored, payload) = anchored_payload(&surviving);
    let relay = if anchored > 0 && js::truthy(head) {
        let head_s = to_js_string(head);
        let mut argv: Vec<String> = vec![
            "-".into(),
            head_s,
            "--repo".into(),
            review.prepared.repo.clone(),
            "--relay".into(),
        ];
        argv.extend(cross_repo_flags(profile));
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        Some(
            match resolve_verified_anchors::run(&argv, &payload, &rt.tools).await {
                Ok(data) => Relay {
                    ok: true,
                    data: Some(data),
                    reason: None,
                },
                Err(e) => Relay {
                    ok: false,
                    data: None,
                    reason: Some(e),
                },
            },
        )
    } else {
        None
    };
    let anchors = verified_anchors(anchored, head, &review.prepared.repo, relay.as_ref(), logs);

    let side = side_payloads(
        &v.placed_aside,
        &v.pre_decided,
        &v.tech_refuted,
        &v.surviving,
    );
    let baseline = baseline_description(plan.prior_review.as_ref());
    let pin_ok = js::truthy(js::get(&rc.head_pin, "ok"));
    let ref_ev = match js::get(&rc.head_pin, "path").and_then(Value::as_str) {
        Some(path) if pin_ok => {
            let (to_check, all) = ref_evidence_targets(&v.results);
            if all.is_empty() {
                json!({"ran": true, "checkedVotes": 0, "suspectVotes": 0, "suspects": []})
            } else {
                let missing = missing_identifiers(std::path::Path::new(path), &all);
                ref_evidence(&to_check, &missing, head, logs)
            }
        }
        _ => json!({"ran": false, "checkedVotes": 0, "suspectVotes": 0, "suspects": []}),
    };
    let mut verdict = verdict(
        &prior.outcome.carried,
        &re_examined,
        &surviving,
        profile,
        verify.panel.clean_sweep.as_ref(),
        review.lenses_unreturned.len(),
    )?;
    let scopes: Vec<Option<Vec<String>>> = plan.lens_plan.iter().map(|s| s.files.clone()).collect();
    let unread_risk = unread_risk_files(
        &review.candidates.lens_gaps,
        &scopes,
        &risk_path_rules(profile)?,
    );
    if !unread_risk.is_empty() {
        logs.push(format!(
            "No lens read {} (risk path) within its budget; the verdict cannot be APPROVE",
            unread_risk.join(", ")
        ));
    }
    verdict.guess = gated_by_coverage(verdict.guess, &unread_risk);
    let prior_reviews = Value::Array(
        rc.decisions
            .prior_reviews
            .iter()
            .map(|r| r.to_value())
            .collect(),
    );
    let grounded_refuted = &verify.gate.refuted;
    let pr_number = args.pr.parse::<u64>().map_or(json!(args.pr), |n| json!(n));
    let prompt = synthesis_prompt(&SynthesisInput {
        pr_number: &pr_number,
        pr_context: &rc.pr_context,
        repo: &review.prepared.repo,
        incremental_fallback: plan.incremental_fallback.as_ref(),
        incremental_scope: plan.incremental_scope.as_ref(),
        all_pr_files: plan.all_pr_files.len(),
        baseline_description: &baseline,
        prior_review: plan.prior_review.as_ref(),
        carried_forward: &prior.outcome.carried,
        moot: &prior.outcome.moot,
        still_open: &prior.outcome.still_open,
        prior_re_examined: &re_examined,
        surviving: &surviving,
        prior_reviews: &prior_reviews,
        pre_decided: &side.pre_decided,
        placed_aside: &side.placed_aside,
        tech_refuted: &side.tech_refuted,
        dropped_pre_panel: &verify.panel.dropped_pre_panel,
        grounded_refuted,
        debate_outcomes: &side.debate_outcomes,
        bot_vetting: review.bot_vetting.as_ref(),
        vet_bot_comments: args.bot_comments,
        critic_output: &verify.panel.critic_output,
        clean_sweep: verify.panel.clean_sweep.as_ref(),
        refines_skipped_for_budget: &verify.panel.refines_skipped_for_budget,
        cost_breaker_fired: verify.panel.cost_breaker_fired.as_ref(),
        cost_degraded_stages: verify.panel.cost_degraded_stages.len(),
        verdict: verdict.guess,
    });
    let scribe = agent_def::load(&rt.plugin_root, "tribunal-scribe")?;
    let res = dispatch_one(
        agent,
        &scribe,
        "synthesize",
        prompt.clone(),
        &args.review_model,
        policy.effort.as_ref(),
        None,
        &rt.operator.cwd,
    )
    .await;
    let text = res.text().map(str::to_owned);
    dispatches.push(("synthesize".to_owned(), res));
    let report = finish_report(
        text.as_deref(),
        verdict.guess,
        pin_ok,
        &ref_ev,
        head,
        &review.lenses_unreturned,
        review.labels.len(),
        logs,
    )?;
    let report = with_coverage(
        report,
        &coverage_banner(&review.candidates.lens_gaps, &unread_risk),
    );
    Ok(SynthesisOutcome {
        synthesize_seconds: synthesize_started.elapsed().as_secs_f64(),
        prior,
        prior_re_examined: re_examined,
        surviving,
        verified_anchors: anchors,
        ref_evidence: ref_ev,
        verdict,
        prompt,
        report,
        unread_risk_files: unread_risk,
        dispatches,
    })
}

/// APPROVE becomes COMMENT when a `riskPaths` file went unread for budget,
/// as it does when a lens returned nothing.
fn gated_by_coverage(guess: &'static str, unread_risk: &[String]) -> &'static str {
    if guess == "APPROVE" && !unread_risk.is_empty() {
        "COMMENT"
    } else {
        guess
    }
}

/// The report with the coverage lists above its draft comments, and the body
/// as posted recomputed from it.
fn with_coverage(report: Report, banner: &str) -> Report {
    if banner.is_empty() {
        return report;
    }
    let text = with_banners(&report.report, banner);
    Report {
        as_posted: strip_draft_comments_from_body(&text),
        report: text,
        corrected: report.corrected,
    }
}

/// A whole review, from the operator's arguments to the report.
#[derive(Debug)]
pub enum FullOutcome {
    Skipped {
        reason: String,
    },
    Preflight {
        summary: Value,
    },
    Reviewed {
        review: Box<ReviewOutcome>,
        verify: Box<VerifyOutcome>,
        synthesis: Box<SynthesisOutcome>,
        /// The metrics row and whether it landed; `None` when it could not be
        /// written at all.
        persisted: Option<crate::run_metrics::Persisted>,
        /// Step 4: `None` without `--post`.
        post: Option<crate::post::PostOutcome>,
    },
}

/// Removes the trees the run made: the pinned checkout and the typecheck's
/// own worktree. Best effort, as the workflow's teardown.
async fn teardown(rt: &Runtime, review: &ReviewOutcome, verify: Option<&VerifyOutcome>) {
    let pin = &review.context.head_pin;
    if js::truthy(js::get(pin, "ok")) {
        if let Some(path) = js::get(pin, "path").and_then(Value::as_str) {
            let clone = js::get(pin, "kind") == Some(&json!("clone"));
            remove_tree(&rt.tools, path, clone, &rt.operator.cwd).await;
        }
    }
    if let Some(wt) = verify
        .and_then(|v| v.typecheck.as_ref())
        .and_then(|t| t.created_worktree.as_deref())
    {
        remove_tree(&rt.tools, wt, false, &rt.operator.cwd).await;
    }
    if let Some((tree, clone)) = verify.and_then(|v| v.external_tree.as_ref()) {
        remove_tree(&rt.tools, tree, *clone, &rt.operator.cwd).await;
    }
}

/// # Errors
/// Where any phase fails. The run's trees are removed either way.
pub async fn run_full<A: Agent>(
    agent: &A,
    rt: &Runtime,
    args: &RunArgs,
    policy: &DispatchPolicy,
    logs: &mut Vec<String>,
) -> Result<FullOutcome, String> {
    let review = match run_review(agent, rt, args, policy, logs).await? {
        RunOutcome::Skipped { reason } => return Ok(FullOutcome::Skipped { reason }),
        RunOutcome::Preflight { summary } => return Ok(FullOutcome::Preflight { summary }),
        RunOutcome::Reviewed(r) => r,
    };
    let verify = match verify_phase(agent, rt, args, policy, &review, logs).await {
        Ok(v) => v,
        Err(e) => {
            teardown(rt, &review, None).await;
            return Err(e);
        }
    };
    let synthesis = synthesize_phase(agent, rt, args, policy, &review, &verify, logs).await;
    let persisted = match &synthesis {
        Ok(s) => {
            match crate::run_metrics::persist_phase(rt, args, policy, &review, &verify, s, logs) {
                Ok(p) => Some(p),
                Err(e) => {
                    logs.push(format!("METRICS ROW NOT WRITTEN — {e}"));
                    None
                }
            }
        }
        Err(_) => None,
    };
    let post = match (&synthesis, &persisted) {
        (Ok(s), Some(p)) if args.post => {
            let head = js::get(&review.context.pr_context, "headRefOid")
                .and_then(Value::as_str)
                .unwrap_or("");
            let cwd = reviewer_cwd(&review.context, rt);
            let input = crate::post::PostInput {
                metrics: &p.metrics,
                report: &s.report.report,
                verdict: s.verdict.guess,
                head_sha: head,
                repo: &review.prepared.repo,
                cwd: &cwd,
                effort: policy.effort.as_ref(),
                choice: crate::post::PostChoice::default(),
            };
            let outcome = crate::post::post_phase(agent, rt, args, &input, logs).await;
            logs.push(match &outcome {
                crate::post::PostOutcome::Posted { url, verdict } => {
                    format!("Posted {verdict}: {url}")
                }
                crate::post::PostOutcome::Held(why) => format!("NOT POSTED — {why}"),
            });
            Some(outcome)
        }
        (_, None) if args.post => Some(crate::post::PostOutcome::Held(
            "no metrics row, so no footer can be built and nothing is posted".into(),
        )),
        _ => None,
    };
    teardown(rt, &review, Some(&verify)).await;
    Ok(FullOutcome::Reviewed {
        review,
        verify: Box::new(verify),
        synthesis: Box::new(synthesis?),
        persisted,
        post,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unread_risk_file_turns_approve_into_comment_only() {
        let unread = ["src/auth/token.ts".to_owned()];
        assert_eq!(gated_by_coverage("APPROVE", &unread), "COMMENT");
        assert_eq!(
            gated_by_coverage("REQUEST_CHANGES", &unread),
            "REQUEST_CHANGES"
        );
        assert_eq!(gated_by_coverage("APPROVE", &[]), "APPROVE");
    }

    #[test]
    fn the_coverage_lists_go_above_the_draft_comments_and_are_posted() {
        let body = "## Verdict: COMMENT\n\nSummary.\n\n### Draft comments\n\n- src/a.ts:3 x";
        let r = Report {
            report: body.to_owned(),
            corrected: None,
            as_posted: strip_draft_comments_from_body(body),
        };
        let out = with_coverage(r.clone(), "\n\n---\n\n**Check by hand.** x");
        let at_list = out.report.find("Check by hand").unwrap();
        assert!(at_list < out.report.find("### Draft comments").unwrap());
        assert!(out.as_posted.contains("Check by hand"));
        assert!(!out.as_posted.contains("### Draft comments"));
        assert_eq!(with_coverage(r.clone(), ""), r);
    }
}
