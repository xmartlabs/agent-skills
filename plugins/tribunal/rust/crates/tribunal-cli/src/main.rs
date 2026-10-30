//! `tribunal`: the review harness as a program. This version runs a review
//! from the context through the refutation panel to the report, and writes
//! both; it posts nothing.
//!
//!   tribunal review --pr <n> [--repo <owner/name>] [--head-sha <sha>]
//!     [--thread-before <iso>] [--full] [--since <sha>] [--lenses <n>]
//!     [--context <text>] [--bot-comments] [--model <id>] [--preflight]
//!     [--max-panels <n>] [--max-agents <n>] [--concurrency <n>]
//!     [--effort <level>] [--codex|--no-codex] [--gemini|--no-gemini] [--batch-panels|--no-batch-panels] [--max-output-tokens <n>] [--post|--no-post] [--out <file>]

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::{json, Value};
use tribunal_agent::{Agent, ClaudeAgent, DispatchRequest, DispatchResult};
use tribunal_context::exec::Tools;
use tribunal_context::fetch_pr_context::default_tmpdir;
use tribunal_context::operator::OperatorEnv;
use tribunal_pipeline::context::{RunArgs, Runtime};
use tribunal_pipeline::post::PostMode;
use tribunal_pipeline::run::{run_full, DispatchPolicy, FullOutcome};

const DEFAULT_MODEL: &str = "claude-sonnet-5-5";

fn usage() -> ExitCode {
    eprintln!("usage: tribunal --version");
    eprintln!("usage: tribunal cost [--repo <owner/name>] [--since <iso date>] [--json]");
    eprintln!("usage: tribunal override --ts <row ts> --title <finding title prefix> (--direction reinstated|rejected|withdrawn | --severity nit|important) --reason <how it was checked> [--by <login>] [--verified-at <sha>] [--dry-run]");
    eprintln!("usage: tribunal selfcheck [--apply] [--no-battery] [--model <id>] [--effort <level>] [--out <file>]");
    eprintln!("usage: tribunal post --from <run.json> [--verdict approve|request-changes|comment] [--delta|--edit-existing] [--over-own-review] [--over-standing-review] [--model <id>] [--effort <level>]");
    eprintln!("usage: tribunal review --pr <n> [--repo <owner/name>] [--profile <path> [--rules <path>]] [--head-sha <sha>] [--thread-before <iso>] [--full] [--since <sha>] [--lenses <n>] [--context <text>] [--bot-comments] [--model <id>] [--preflight] [--max-panels <n>] [--max-agents <n>] [--concurrency <n>] [--effort <level>] [--codex|--no-codex] [--gemini|--no-gemini] [--batch-panels|--no-batch-panels] [--max-output-tokens <n>] [--post|--no-post] [--out <file>]");
    ExitCode::from(2)
}

struct Cli {
    args: RunArgs,
    policy: DispatchPolicy,
    out: Option<String>,
}

fn parse(argv: &[String]) -> Result<Cli, String> {
    let mut args = RunArgs {
        review_model: DEFAULT_MODEL.to_owned(),
        ..RunArgs::default()
    };
    let mut policy = DispatchPolicy::default();
    let mut out = None;
    let mut it = argv.iter();
    let value = |it: &mut std::slice::Iter<String>, flag: &str| {
        it.next()
            .filter(|v| !v.starts_with("--"))
            .cloned()
            .ok_or_else(|| format!("{flag} needs a value"))
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--pr" => args.pr = value(&mut it, a)?,
            "--repo" => args.repo = value(&mut it, a)?,
            "--profile" => args.profile = value(&mut it, a)?,
            "--rules" => args.rules = value(&mut it, a)?,
            "--head-sha" => args.head_sha = value(&mut it, a)?,
            "--thread-before" => args.thread_before = value(&mut it, a)?,
            "--since" => args.since = value(&mut it, a)?,
            "--lenses" => {
                let n: f64 = value(&mut it, a)?
                    .parse()
                    .map_err(|_| "--lenses takes a number")?;
                args.lenses = Some(json!(n));
            }
            "--context" => args.context = Some(value(&mut it, a)?),
            "--model" => args.review_model = value(&mut it, a)?,
            "--concurrency" => {
                policy.concurrency = value(&mut it, a)?
                    .parse()
                    .map_err(|_| "--concurrency takes a number")?;
            }
            "--max-panels" | "--max-agents" => {
                let n: f64 = value(&mut it, a)?
                    .parse()
                    .map_err(|_| format!("{a} takes a number"))?;
                if n.is_finite() && n > 0.0 {
                    if a == "--max-panels" {
                        policy.max_panels = n.floor();
                    } else {
                        policy.max_agents = n.floor();
                    }
                }
            }
            "--post" => args.post = true,
            "--no-post" => args.post = false,
            "--codex" => args.codex = Some(true),
            "--no-codex" => args.codex = Some(false),
            "--gemini" => args.gemini = Some(true),
            "--no-gemini" => args.gemini = Some(false),
            "--batch-panels" => args.batch_panels = Some(true),
            "--no-batch-panels" => args.batch_panels = Some(false),
            "--max-output-tokens" => {
                let n: f64 = value(&mut it, a)?
                    .parse()
                    .map_err(|_| "--max-output-tokens takes a number")?;
                if n.is_finite() && n > 0.0 {
                    policy.output_ceiling = n.floor();
                }
            }
            "--effort" => policy.effort = Some(value(&mut it, a)?),
            "--out" => out = Some(value(&mut it, a)?),
            "--full" => args.full = true,
            "--bot-comments" => args.bot_comments = true,
            "--preflight" => args.preflight = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if args.pr.is_empty() || !args.pr.bytes().all(|b| b.is_ascii_digit()) {
        return Err("--pr <n> is required".into());
    }
    if !args.thread_before.is_empty() && !is_utc_instant(&args.thread_before) {
        return Err(format!(
            "--thread-before must be an ISO 8601 UTC instant, got {:?}",
            args.thread_before
        ));
    }
    if policy.effort.is_none() {
        policy.effort = operator_effort();
    }
    Ok(Cli { args, policy, out })
}

/// `/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?Z$/`.
fn is_utc_instant(s: &str) -> bool {
    let b = s.as_bytes();
    let digits =
        |r: std::ops::Range<usize>| b.get(r).is_some_and(|d| d.iter().all(u8::is_ascii_digit));
    if b.len() < 20
        || !digits(0..4)
        || b[4] != b'-'
        || !digits(5..7)
        || b[7] != b'-'
        || !digits(8..10)
        || b[10] != b'T'
        || !digits(11..13)
        || b[13] != b':'
        || !digits(14..16)
        || b[16] != b':'
        || !digits(17..19)
    {
        return false;
    }
    match &b[19..] {
        [b'Z'] => true,
        [b'.', rest @ .., b'Z'] => !rest.is_empty() && rest.iter().all(u8::is_ascii_digit),
        _ => false,
    }
}

/// The effort a Workflow subagent inherits: the operator's `effortLevel` in
/// `~/.claude/settings.json`, which an isolated child does not read itself.
fn operator_effort() -> Option<String> {
    let home = tribunal_context::proc::home_dir()?;
    let text = std::fs::read_to_string(PathBuf::from(home).join(".claude/settings.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("effortLevel")?.as_str().map(str::to_owned)
}

fn plugin_root() -> PathBuf {
    std::env::var_os("TRIBUNAL_PLUGIN_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."))
}

/// Prints one line to stderr when each dispatch starts and one when it ends,
/// so a run can be followed while it is in flight.
struct Progress<A> {
    inner: A,
    started: std::time::Instant,
}

impl<A> Progress<A> {
    fn stamp(&self) -> String {
        let s = self.started.elapsed().as_secs();
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

impl<A: Agent> Agent for Progress<A> {
    async fn run(&self, req: &DispatchRequest) -> DispatchResult {
        eprintln!("[{}] ▶ {}", self.stamp(), req.label);
        let res = self.inner.run(req).await;
        let o = res.output.as_ref();
        let out: u64 = res.model_usage().values().map(|u| u.output_tokens).sum();
        eprintln!(
            "[{}] {} {} ({}s, {} turns, {} tool calls, {out} out, ${:.2})",
            self.stamp(),
            if res.text().is_some() || res.structured().is_some() {
                "✓"
            } else {
                "✗"
            },
            req.label,
            o.map_or(0, |o| o.duration_ms / 1000),
            o.map_or(0, |o| o.num_turns),
            res.tool_calls.len(),
            o.map_or(0.0, |o| o.total_cost_usd),
        );
        res
    }
}

fn dispatch_row(label: &str, d: &tribunal_agent::DispatchResult) -> Value {
    let o = d.output.as_ref();
    json!({
        "label": label,
        "exit": format!("{:?}", d.exit),
        "answered": d.structured().is_some() || d.text().is_some(),
        "turns": o.map(|o| o.num_turns),
        "durationMs": o.map(|o| o.duration_ms),
        "costUsd": o.map(|o| o.total_cost_usd),
        "modelUsage": serde_json::to_value(d.model_usage()).unwrap_or(Value::Null),
        "toolCalls": d.tool_calls.len(),
        "tools": d.tool_calls.iter().fold(serde_json::Map::new(), |mut m, t| {
            let n = m.get(t).and_then(Value::as_u64).unwrap_or(0);
            m.insert(t.clone(), json!(n + 1));
            m
        }),
        "transcript": d.transcript,
        "stderrTail": d.stderr_tail,
    })
}

fn report(outcome: &FullOutcome, logs: &[String]) -> Value {
    match outcome {
        FullOutcome::Skipped { reason } => json!({"skipped": true, "reason": reason, "logs": logs}),
        FullOutcome::Preflight { summary } => {
            json!({"preflight": true, "summary": summary, "logs": logs})
        }
        FullOutcome::Reviewed {
            review: r,
            verify: v,
            synthesis: s,
            persisted,
            post,
        } => {
            let mut dispatches: Vec<Value> = r
                .dispatches
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    let label = r
                        .labels
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| "bot-vetting".into());
                    dispatch_row(&label, d)
                })
                .collect();
            dispatches.extend(v.dispatches.iter().map(|(l, d)| dispatch_row(l, d)));
            dispatches.extend(s.dispatches.iter().map(|(l, d)| dispatch_row(l, d)));
            let cost: f64 = dispatches
                .iter()
                .filter_map(|d| d["costUsd"].as_f64())
                .sum();
            json!({
                "repo": r.prepared.repo,
                "lensAgent": r.lens_agent,
                "headPin": r.context.head_pin,
                "lensCount": r.context.plan.lens_count,
                "lensPlan": r.context.plan.to_value()["lensPlan"],
                "dispatches": dispatches,
                "costUsd": cost,
                "lensFindings": r.candidates.lens_findings.len(),
                "candidates": r.candidates.all_candidates,
                "droppedPrePanel": r.candidates.dropped_pre_panel,
                "lensNotReached": r.candidates.lens_not_reached,
                "evidence": {
                    "ran": r.evidence.ran, "located": r.evidence.located,
                    "notFound": r.evidence.not_found, "lineCorrected": r.evidence.line_corrected,
                },
                "dedupedPrePanel": v.dedup.deduped_pre_panel,
                "groundedTypecheck": v.typecheck.as_ref().map(|t| &t.text),
                "groundedConfirmed": v.gate.confirmed,
                "groundedRefuted": v.gate.refuted,
                "panelSkippedForBudget": v.panel.panel_skipped_for_budget,
                "cleanSweep": v.panel.clean_sweep,
                "criticOutput": v.panel.critic_output,
                "costBreakerFired": v.panel.cost_breaker_fired,
                "voteQuoteCheck": v.panel.vote_quote_check,
                "voteDir": v.vote_dir,
                "results": v.verdicts.results,
                "surviving": s.surviving,
                "priorReExamined": s.prior_re_examined,
                "verifiedAnchors": s.verified_anchors,
                "refEvidence": s.ref_evidence,
                "verdict": s.verdict.guess,
                "reportVerdictCorrected": s.report.corrected,
                "report": s.report.report,
                "metrics": persisted.as_ref().map(|p| &p.metrics),
                "metricsRowVerified": persisted.as_ref().map(|p| p.verified),
                "metricsRowDetail": persisted.as_ref().and_then(|p| p.detail.as_ref()),
                "post": post.as_ref().map(|p| match p {
                    tribunal_pipeline::post::PostOutcome::Posted { url, verdict } => json!({"posted": true, "url": url, "verdict": verdict}),
                    tribunal_pipeline::post::PostOutcome::Held(why) => json!({"posted": false, "reason": why}),
                }),
                "logs": logs,
            })
        }
    }
}

struct PostCli {
    from: String,
    choice: tribunal_pipeline::post::PostChoice,
    model: String,
    effort: Option<String>,
}

fn parse_post(argv: &[String]) -> Result<PostCli, String> {
    let mut cli = PostCli {
        from: String::new(),
        choice: tribunal_pipeline::post::PostChoice::default(),
        model: DEFAULT_MODEL.to_owned(),
        effort: None,
    };
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        let mut value = || {
            it.next()
                .filter(|v| !v.starts_with("--"))
                .cloned()
                .ok_or_else(|| format!("{a} needs a value"))
        };
        match a.as_str() {
            "--from" => cli.from = value()?,
            "--model" => cli.model = value()?,
            "--effort" => cli.effort = Some(value()?),
            "--verdict" => {
                let v = value()?;
                cli.choice.verdict = Some(
                    tribunal_pipeline::post::verdict_from_flag(&v).ok_or_else(|| {
                        format!("--verdict is approve, request-changes or comment, not {v}")
                    })?,
                );
            }
            "--over-own-review" => cli.choice.over_own_review = true,
            "--over-standing-review" => cli.choice.over_standing_review = true,
            "--delta" | "--edit-existing" => {
                if cli.choice.mode != PostMode::Full {
                    return Err("--delta and --edit-existing exclude each other".into());
                }
                cli.choice.mode = if a.as_str() == "--delta" {
                    PostMode::Delta
                } else {
                    PostMode::Edit
                };
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if cli.from.is_empty() {
        return Err("--from <run.json> is required".into());
    }
    match (cli.choice.mode, cli.choice.verdict) {
        (PostMode::Edit, Some(_)) => {
            return Err("--edit-existing keeps the review's state; drop --verdict".into())
        }
        (PostMode::Delta, Some(v)) if v != "COMMENT" => {
            return Err("--delta posts a comment; --verdict can only be comment".into())
        }
        _ => {}
    }
    if cli.effort.is_none() {
        cli.effort = operator_effort();
    }
    Ok(cli)
}

fn runtime() -> Runtime {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let home = tribunal_context::proc::home_dir().unwrap_or_default();
    Runtime {
        data_dir: tribunal_context::inflight::data_dir(
            std::env::var("TRIBUNAL_DATA_DIR").ok().as_deref(),
            &home,
        ),
        plugin_root: plugin_root(),
        tmpdir: default_tmpdir(),
        tools: Tools::default(),
        operator: OperatorEnv {
            review_harness_config: std::env::var("REVIEW_HARNESS_CONFIG").ok(),
            home,
            cwd,
        },
    }
}

/// `tribunal post`: posts a finished run's report, as `--post` would have.
async fn post_main(argv: &[String]) -> ExitCode {
    let cli = match parse_post(argv) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("tribunal: {e}");
            return usage();
        }
    };
    let run: serde_json::Value = match std::fs::read_to_string(&cli.from)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
    {
        Ok(v) => v,
        Err(e) => {
            eprintln!("tribunal: cannot read {}: {e}", cli.from);
            return ExitCode::FAILURE;
        }
    };
    let rt = runtime();
    let agent = Progress {
        inner: ClaudeAgent::from_env(|k| std::env::var(k).ok()),
        started: std::time::Instant::now(),
    };
    let mut logs = Vec::new();
    let outcome = tribunal_pipeline::post::post_saved_run(
        &agent,
        &rt,
        &run,
        cli.choice,
        &cli.model,
        cli.effort.as_ref(),
        &mut logs,
    )
    .await;
    for l in &logs {
        eprintln!("{l}");
    }
    match outcome {
        tribunal_pipeline::post::PostOutcome::Posted { url, verdict } => {
            println!("Posted {verdict}: {url}");
            ExitCode::SUCCESS
        }
        tribunal_pipeline::post::PostOutcome::Held(why) => {
            eprintln!("NOT POSTED: {why}");
            ExitCode::FAILURE
        }
    }
}

/// `tribunal selfcheck`: audits the last run, diagnoses and verifies each
/// violation, and proposes a patch. Prints the result as one JSON object.
async fn selfcheck_main(argv: &[String]) -> ExitCode {
    let mut args = tribunal_pipeline::selfcheck::SelfcheckArgs {
        model: DEFAULT_MODEL.to_owned(),
        ..Default::default()
    };
    let mut out = None;
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        let mut value = || {
            it.next()
                .filter(|v| !v.starts_with("--"))
                .cloned()
                .ok_or_else(|| format!("{a} needs a value"))
        };
        let parsed = match a.as_str() {
            "--apply" => {
                args.apply = true;
                Ok(())
            }
            "--no-battery" => {
                args.no_battery = true;
                Ok(())
            }
            "--model" => value().map(|v| args.model = v),
            "--effort" => value().map(|v| args.effort = Some(v)),
            "--out" => value().map(|v| out = Some(v)),
            other => Err(format!("unknown argument {other}")),
        };
        if let Err(e) = parsed {
            eprintln!("tribunal: {e}");
            return usage();
        }
    }
    if args.effort.is_none() {
        args.effort = operator_effort();
    }
    let rt = runtime();
    let agent = Progress {
        inner: ClaudeAgent::from_env(|k| std::env::var(k).ok()),
        started: std::time::Instant::now(),
    };
    let mut logs = Vec::new();
    let result = tribunal_pipeline::selfcheck::selfcheck(&agent, &rt, &args, &mut logs).await;
    for l in &logs {
        eprintln!("{l}");
    }
    let text = format!("{result}\n");
    match out {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, text) {
                eprintln!("tribunal: cannot write {path}: {e}");
                return ExitCode::FAILURE;
            }
        }
        None => print!("{text}"),
    }
    ExitCode::SUCCESS
}

/// `tribunal cost`: where the money of past reviews went, from the rows in
/// `metrics.jsonl`. Reads only; dispatches nothing.
fn cost_main(argv: &[String]) -> ExitCode {
    let mut filter = tribunal_pipeline::cost_report::CostFilter::default();
    let mut as_json = false;
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        let mut value = || {
            it.next()
                .filter(|v| !v.starts_with("--"))
                .cloned()
                .ok_or_else(|| format!("{a} needs a value"))
        };
        let parsed = match a.as_str() {
            "--repo" => value().map(|v| filter.repo = Some(v)),
            "--since" => value().map(|v| filter.since = Some(v)),
            "--json" => {
                as_json = true;
                Ok(())
            }
            other => Err(format!("unknown argument {other}")),
        };
        if let Err(e) = parsed {
            eprintln!("tribunal: {e}");
            return usage();
        }
    }
    let path = runtime().data_dir.join("metrics.jsonl");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("tribunal: cannot read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let rows: Vec<Value> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let report = tribunal_pipeline::cost_report::cost_report(&rows, &filter);
    if as_json {
        println!("{report}");
    } else {
        print!("{}", tribunal_pipeline::cost_report::render(&report));
    }
    ExitCode::SUCCESS
}

/// `tribunal override`: writes a person's verdict on one finding into its
/// ledger row, under the ledger lock a run takes to append.
fn override_main(argv: &[String]) -> ExitCode {
    use tribunal_pipeline::operator_override::{apply, Change, Request};
    let mut req = Request {
        ts: String::new(),
        title_prefix: String::new(),
        change: Change::Severity(String::new()),
        reason: String::new(),
        by: None,
        verified_at: None,
        at: tribunal_context::post_footer::iso_timestamp(std::time::SystemTime::now()),
    };
    let mut change = None;
    let mut dry_run = false;
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        let mut value = || {
            it.next()
                .filter(|v| !v.starts_with("--"))
                .cloned()
                .ok_or_else(|| format!("{a} needs a value"))
        };
        let parsed = match a.as_str() {
            "--ts" => value().map(|v| req.ts = v),
            "--title" => value().map(|v| req.title_prefix = v),
            "--reason" => value().map(|v| req.reason = v),
            "--by" => value().map(|v| req.by = Some(v)),
            "--verified-at" => value().map(|v| req.verified_at = Some(v)),
            "--direction" => value().map(|v| change = Some(Change::Direction(v))),
            "--severity" => value().map(|v| change = Some(Change::Severity(v))),
            "--dry-run" => {
                dry_run = true;
                Ok(())
            }
            other => Err(format!("unknown argument {other}")),
        };
        if let Err(e) = parsed {
            eprintln!("tribunal: {e}");
            return usage();
        }
    }
    let Some(change) = change.filter(|_| !req.ts.is_empty() && !req.title_prefix.is_empty()) else {
        eprintln!("tribunal: override needs --ts, --title and one of --direction or --severity");
        return usage();
    };
    req.change = change;
    let env = tribunal_context::ledger::LedgerEnv::from_process();
    let path = env.metrics_path();
    let written = tribunal_context::ledger::with_ledger_lock(&path, env.lock_wait, || {
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
        let applied = apply(&text, &req)?;
        if !dry_run {
            let tmp = format!("{path}.override.tmp");
            std::fs::write(&tmp, &applied.ledger)
                .and_then(|()| std::fs::rename(&tmp, &path))
                .map_err(|e| format!("cannot write {path}: {e}"))?;
        }
        Ok::<_, String>(applied)
    });
    match written.and_then(|r| r) {
        Ok(a) => {
            for w in &a.warnings {
                eprintln!("tribunal: warning: {w}");
            }
            println!(
                "{} {} on \"{}\"{}",
                if dry_run { "would set" } else { "set" },
                a.field,
                a.title,
                if dry_run {
                    " (dry run, nothing written)"
                } else {
                    ""
                }
            );
            println!("{}", a.value);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("tribunal: {e}");
            ExitCode::FAILURE
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("review") => {}
        Some("post") => return post_main(&argv[1..]).await,
        Some("selfcheck") => return selfcheck_main(&argv[1..]).await,
        Some("cost") => return cost_main(&argv[1..]),
        Some("override") => return override_main(&argv[1..]),
        Some("--version" | "version") => {
            println!(
                "tribunal {} ({})",
                env!("CARGO_PKG_VERSION"),
                tribunal_pipeline::run_metrics::ENGINE_VERSION
            );
            return ExitCode::SUCCESS;
        }
        _ => return usage(),
    }
    let cli = match parse(&argv[1..]) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("tribunal: {e}");
            return usage();
        }
    };
    let rt = runtime();
    let mut agent = ClaudeAgent::from_env(|k| std::env::var(k).ok());
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    agent.transcripts = Some(PathBuf::from(format!(
        "{}/tribunal-transcripts-{}-{stamp}",
        rt.tmpdir.trim_end_matches('/'),
        cli.args.pr
    )));
    let agent = Progress {
        inner: agent,
        started: std::time::Instant::now(),
    };
    let mut logs = Vec::new();
    let outcome = run_full(&agent, &rt, &cli.args, &cli.policy, &mut logs).await;
    for l in &logs {
        eprintln!("{l}");
    }
    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => {
            eprintln!("tribunal: {e}");
            return ExitCode::FAILURE;
        }
    };
    let text = serde_json::to_string_pretty(&report(&outcome, &logs)).expect("JSON");
    let path = cli.out.unwrap_or_else(|| {
        format!(
            "{}/tribunal-run-{}.json",
            rt.tmpdir.trim_end_matches('/'),
            cli.args.pr
        )
    });
    if let Err(e) = std::fs::write(&path, &text) {
        eprintln!("tribunal: cannot write {path}: {e}");
        return ExitCode::FAILURE;
    }
    eprintln!("wrote {path}");
    if let FullOutcome::Reviewed { synthesis, .. } = &outcome {
        let md = format!(
            "{}/tribunal-report-{}.md",
            rt.tmpdir.trim_end_matches('/'),
            cli.args.pr
        );
        if let Err(e) = std::fs::write(&md, &synthesis.report.as_posted) {
            eprintln!("tribunal: cannot write {md}: {e}");
            return ExitCode::FAILURE;
        }
        eprintln!("wrote {md}. Nothing was posted.");
        println!("{}", synthesis.report.as_posted);
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_instants() {
        assert!(is_utc_instant("2026-10-14T18:02:11Z"));
        assert!(is_utc_instant("2026-10-14T18:02:11.123Z"));
        assert!(!is_utc_instant("2026-10-14T18:02:11.Z"));
        assert!(!is_utc_instant("2026-10-14 18:02:11Z"));
        assert!(!is_utc_instant("2026-10-14T18:02:11+00:00"));
    }

    #[test]
    fn flags() {
        let a: Vec<String> = ["--pr", "12", "--head-sha", "abc", "--lenses", "2", "--full"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let c = parse(&a).unwrap();
        assert_eq!(
            (c.args.pr.as_str(), c.args.head_sha.as_str(), c.args.full),
            ("12", "abc", true)
        );
        assert_eq!(c.args.lenses, Some(json!(2.0)));
        assert!(parse(&["--pr".to_string()]).is_err());
        assert!(parse(&["--pr".to_string(), "x".to_string()]).is_err());
    }

    #[test]
    fn a_profile_and_rules_from_disk() {
        let a: Vec<String> = ["--pr", "7", "--profile", "p.json", "--rules", "r.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let c = parse(&a).unwrap();
        assert_eq!(
            (c.args.profile.as_str(), c.args.rules.as_str()),
            ("p.json", "r.md")
        );
        let missing: Vec<String> = ["--pr", "7", "--profile"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(parse(&missing).is_err());
    }
}
