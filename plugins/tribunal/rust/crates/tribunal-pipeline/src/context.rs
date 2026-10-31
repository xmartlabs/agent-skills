//! The steps before the first lens: SKILL.md's profile, methodology and
//! conventions loading, then the workflow's Context phase. Every step the
//! workflow hands a mechanic runs natively here; the one model call left is
//! the thread harvest, which writes the decisions no script can.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Map, Value};
use tribunal_agent::{Agent, DispatchRequest};
use tribunal_context::exec::Tools;
use tribunal_context::fetch_pr_context::{fetch_pr_context, FetchOptions};
use tribunal_context::head_worktree::{parse_head_worktree, prepare_head_worktree};
use tribunal_context::import_adjacency::compute_adjacency;
use tribunal_context::incremental_scope::scope_files;
use tribunal_context::inflight::{append_started, now_epoch as epoch_now};
use tribunal_context::operator::OperatorEnv;
use tribunal_context::{
    assemble_methodology, blast_radius, load_conventions, load_profile, resolve_baseline,
    section_diff,
};
use tribunal_core::evidence::fetch_pr_context::build_pr_context;
use tribunal_core::js;
use tribunal_core::lens::dependents_block::dependents_block;
use tribunal_core::lens::js_ext::{js_key_order, to_js_string};
use tribunal_core::profile::conventions_channel::{conventions_channel, ConventionsChannel};

use crate::agent_def;
use crate::decisions::{build_decisions, Decisions};
use crate::plan::{build_plan, Plan, PlanInput};
use crate::prompts::{conventions_path, methodology_line, methodology_path, DiffDelivery};
use crate::schemas;

/// What the operator asked for, as the workflow's `args` carry it.
#[derive(Debug, Clone, Default)]
pub struct RunArgs {
    pub pr: String,
    /// `--repo`; empty resolves it as load-profile does.
    pub repo: String,
    pub head_sha: String,
    pub thread_before: String,
    pub bot_comments: bool,
    pub full: bool,
    pub since: String,
    pub lenses: Option<Value>,
    pub context: Option<String>,
    pub retest: Option<Value>,
    pub review_model: String,
    pub preflight: bool,
    /// `--codex` / `--no-codex`, `--gemini` / `--no-gemini` and
    /// `--batch-panels` / `--no-batch-panels`: `None` when not passed.
    pub codex: Option<bool>,
    pub gemini: Option<bool>,
    pub batch_panels: Option<bool>,
    /// `--post`: post the review when the duplicate-review gate is clear.
    pub post: bool,
    /// `--profile` / `--rules`: a profile and review rules read from disk
    /// instead of the repo's base branch, to try them before they land.
    pub profile: String,
    pub rules: String,
}

/// Where the run executes.
#[derive(Debug, Clone)]
pub struct Runtime {
    pub plugin_root: PathBuf,
    pub tmpdir: String,
    pub tools: Tools,
    pub operator: OperatorEnv,
    /// Where `metrics.jsonl` and `inflight-runs.jsonl` live.
    pub data_dir: PathBuf,
}

/// SKILL.md's steps: the profile, the methodology the reviewers read and the
/// repo's conventions.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub repo: String,
    pub profile: Value,
    pub methodology_path: String,
    pub conventions: ConventionsChannel,
    /// `PROFILE_SHA256`, `PROFILE_SOURCE` and the project rules' pair, as
    /// the row records them: the string, else null.
    pub profile_sha256: Value,
    pub profile_source: Value,
    pub project_rules_sha256: Value,
    pub project_rules_source: Value,
    pub logs: Vec<String>,
}

fn tmp(rt: &Runtime, name: &str) -> String {
    format!("{}/{name}", rt.tmpdir.trim_end_matches('/'))
}

fn json_line(stdout: &str, what: &str) -> Result<Map<String, Value>, String> {
    let line = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    match serde_json::from_str::<Value>(line) {
        Ok(Value::Object(m)) if m.get("ok") == Some(&Value::Bool(true)) => Ok(m),
        Ok(Value::Object(m)) => Err(format!("{what}: {}", to_js_string(m.get("error")))),
        _ => Err(format!("{what} printed no JSON line: {}", stdout.trim())),
    }
}

async fn gh_text(rt: &Runtime, args: &[&str]) -> Result<String, String> {
    let out = rt
        .tools
        .gh(args, None)
        .await
        .map_err(|e| format!("gh {}: {e}", args.join(" ")))?;
    if !out.ok() {
        return Err(format!(
            "gh {} failed: {}",
            args.join(" "),
            out.stderr.trim()
        ));
    }
    Ok(out.stdout)
}

/// The PR's diff at the head under review: the three-dot compare for a pin,
/// `gh pr diff` otherwise.
async fn fetch_patch(rt: &Runtime, repo: &str, pr: &str, pin: &str) -> Result<String, String> {
    if pin.is_empty() {
        return gh_text(rt, &["pr", "diff", pr, "--repo", repo]).await;
    }
    let base = gh_text(
        rt,
        &[
            "pr",
            "view",
            pr,
            "--repo",
            repo,
            "--json",
            "baseRefOid",
            "--jq",
            ".baseRefOid",
        ],
    )
    .await?;
    gh_text(
        rt,
        &[
            "api",
            &format!("repos/{repo}/compare/{}...{pin}", base.trim()),
            "-H",
            "Accept: application/vnd.github.v3.diff",
        ],
    )
    .await
}

/// # Errors
/// Where SKILL.md stops the run: a profile, methodology or conventions file
/// that does not load.
pub async fn prepare(rt: &Runtime, args: &RunArgs) -> Result<Prepared, String> {
    let mut logs = Vec::new();
    let profile_file = tmp(rt, &format!("review-profile-{}.json", args.pr));
    let rules_file = tmp(rt, &format!("review-rules-{}.md", args.pr));
    let mut argv = vec![
        "--pr".to_owned(),
        args.pr.clone(),
        "--save".to_owned(),
        profile_file.clone(),
        "--save-rules".to_owned(),
        rules_file.clone(),
    ];
    if !args.repo.is_empty() {
        argv.extend(["--repo".to_owned(), args.repo.clone()]);
    }
    if !args.profile.is_empty() {
        argv.extend(["--profile".to_owned(), args.profile.clone()]);
    }
    if !args.rules.is_empty() {
        argv.extend(["--rules".to_owned(), args.rules.clone()]);
    }
    let loaded = load_profile::main(&rt.tools, &rt.operator, &argv).await;
    let loaded = json_line(&loaded.stdout, "load-profile")?;
    for w in loaded
        .get("warnings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        logs.push(format!("profile warning: {}", to_js_string(Some(w))));
    }
    let repo = to_js_string(loaded.get("repo"));
    let profile = loaded.get("profile").cloned().unwrap_or(Value::Null);
    let reviewer = to_js_string(js::get(&profile, "reviewer"));

    let methodology = methodology_path(&repo, &args.pr);
    let mut argv = vec!["--kind".to_owned(), reviewer];
    if let Some(rules) = loaded.get("projectRules").filter(|r| r.is_object()) {
        argv.extend(["--rules".to_owned(), rules_file.clone()]);
        if let Some(sha) = js::get(rules, "sha256").and_then(Value::as_str) {
            argv.extend(["--rules-sha256".to_owned(), sha.to_owned()]);
        }
    }
    argv.extend(["--out".to_owned(), methodology.clone()]);
    let assembled = assemble_methodology::main(&rt.plugin_root.to_string_lossy(), &argv);
    let assembled = json_line(&assembled.stdout, "assemble-methodology")?;
    if let Some(unknown) = assembled
        .get("unknown")
        .and_then(Value::as_array)
        .filter(|u| !u.is_empty())
    {
        logs.push(format!(
            "the rules file has tribunal blocks that fill no slot, so no reviewer reads them: {}",
            unknown
                .iter()
                .map(|u| to_js_string(Some(u)))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let lists = |k: &str| matches!(js::get(&profile, k), Some(Value::Array(a)) if !a.is_empty());
    let lists_conventions = lists("conventionFiles") || lists("conventionSources");
    let conventions_args = if lists_conventions {
        let patch = fetch_patch(rt, &repo, &args.pr, &args.head_sha).await?;
        let patch_file = tmp(rt, &format!("peer-review-diff-{}.patch", args.pr));
        std::fs::write(&patch_file, patch)
            .map_err(|e| format!("cannot write {patch_file}: {e}"))?;
        let out = conventions_path(&repo, &args.pr);
        // A profile read from disk has no ref; its conventions are read at
        // the PR's base, where the profile will live.
        let conventions_ref = match loaded.get("profileRef") {
            Some(Value::String(r)) => r.clone(),
            _ => to_js_string(loaded.get("baseRef")),
        };
        let mut argv: Vec<String> = [
            "--profile",
            &profile_file,
            "--ref",
            &conventions_ref,
            "--diff",
            &patch_file,
            "--out",
            &out,
            "--pr",
            &args.pr,
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        // A `submodule:` source reads its pointer at the commit under review.
        if !args.head_sha.is_empty() {
            argv.extend(["--head-sha".to_owned(), args.head_sha.clone()]);
        }
        let got = load_conventions::main(&rt.tools, &argv).await;
        let got = json_line(&got.stdout, "load-conventions")?;
        for w in got
            .get("warnings")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            logs.push(format!("conventions warning: {}", to_js_string(Some(w))));
        }
        if let Some(missing) = got
            .get("missing")
            .and_then(Value::as_array)
            .filter(|m| !m.is_empty())
        {
            logs.push(format!(
                "the profile names convention files the repo does not have: {}",
                missing
                    .iter()
                    .map(|m| to_js_string(Some(m)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        match got.get("out").filter(|o| o.is_string()) {
            Some(path) => json!({
                "conventionsPath": path,
                "conventionsSha256": got.get("sha256"),
                "conventions": Value::Object(got.clone()),
            }),
            None => json!({}),
        }
    } else {
        json!({})
    };
    let conventions = conventions_channel(&conventions_args, &profile)?;
    let text = |v: Option<&Value>| v.filter(|x| x.is_string()).cloned().unwrap_or(Value::Null);
    let rules = loaded.get("projectRules");
    Ok(Prepared {
        repo,
        profile,
        methodology_path: methodology,
        conventions,
        profile_sha256: text(loaded.get("sha256")),
        profile_source: text(loaded.get("source")),
        project_rules_sha256: text(rules.and_then(|r| js::get(r, "sha256"))),
        project_rules_source: text(rules.and_then(|r| js::get(r, "source"))),
        logs,
    })
}

/// `process.platform` for this machine.
fn platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

/// fetch-pr-context's `relayContext`: writes the human text and the bot
/// comments beside the context and returns the context the workflow reads.
///
/// # Errors
/// When the directory or a file cannot be written.
pub fn relay_context(fetched: &Value, dir: &str) -> Result<Value, String> {
    let context = js::get(fetched, "context").cloned().unwrap_or(Value::Null);
    let raw = js::get(fetched, "raw").cloned().unwrap_or(Value::Null);
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {dir}: {e}"))?;
    let raw_text = serde_json::to_string(&raw).expect("JSON");
    let raw_path = format!("{dir}/raw.json");
    let bot_path = format!("{dir}/bot-comments.json");
    let bot = js::get(&context, "botComments")
        .cloned()
        .unwrap_or(json!([]));
    std::fs::write(&raw_path, &raw_text).map_err(|e| format!("cannot write {raw_path}: {e}"))?;
    std::fs::write(&bot_path, serde_json::to_string(&bot).expect("JSON"))
        .map_err(|e| format!("cannot write {bot_path}: {e}"))?;
    let mut out = match context {
        Value::Object(m) => m,
        _ => return Err("fetch-pr returned no context".into()),
    };
    out.insert("platform".into(), json!(platform()));
    let summary: Vec<Value> = bot
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, c)| {
            json!({
                "i": i,
                "user": js::get(c, "user"),
                "path": js::get(c, "path").filter(|p| js::truthy(Some(p))).cloned().unwrap_or(json!("")),
                "line": js::get(c, "line").cloned().unwrap_or(Value::Null),
                "bytes": js::get(c, "body").and_then(Value::as_str).map_or(0, str::len),
            })
        })
        .collect();
    out.insert("botComments".into(), Value::from(summary));
    out.insert("botCommentsPath".into(), json!(bot_path));
    out.insert("rawPath".into(), json!(raw_path));
    out.insert("rawBytes".into(), json!(raw_text.len()));
    let has_human = raw.as_object().is_some_and(|m| {
        m.values()
            .any(|l| l.as_array().is_some_and(|a| !a.is_empty()))
    });
    out.insert("rawHasHumanText".into(), json!(has_human));
    out.insert(
        "errors".into(),
        js::get(fetched, "errors").cloned().unwrap_or(json!([])),
    );
    Ok(Value::Object(out))
}

/// The fetch-pr dispatch's second step, which no script can do: the
/// decisions already made in the PR's conversation and the summaries of the
/// reviews requesting changes. The human text goes in the prompt rather than
/// through a `cat`, so the child needs no tool.
#[must_use]
pub fn harvest_prompt(pr: &str, repo: &str, raw: &str) -> String {
    format!(
        "Read the human text of PR #{pr} in repo {repo} below and return structured data.\n\n<raw>\n{raw}\n</raw>\n\n{}",
        HARVEST_RULES
    )
}

const HARVEST_RULES: &str = include_str!("harvest-rules.txt");

/// # Errors
/// When neither the haiku answer nor the sonnet retry returned an object.
pub async fn harvest<A: Agent>(
    agent: &A,
    rt: &Runtime,
    pr: &str,
    repo: &str,
    raw_path: &str,
) -> Result<Value, String> {
    let raw =
        std::fs::read_to_string(raw_path).map_err(|e| format!("cannot read {raw_path}: {e}"))?;
    let mechanic = agent_def::load(&rt.plugin_root, "tribunal-mechanic")?;
    let mut last = String::new();
    for (model, label) in [("haiku", "fetch-pr"), ("sonnet", "fetch-pr-retry")] {
        let mut req = DispatchRequest::new(
            label,
            harvest_prompt(pr, repo, &raw),
            rt.operator.cwd.clone(),
        );
        req.system_prompt = Some(mechanic.body.clone());
        req.model = Some(model.to_owned());
        req.effort = Some("low".to_owned());
        req.schema = Some(schemas::thread_harvest());
        req.timeout = Duration::from_secs(10 * 60);
        let res = agent.run(&req).await;
        if let Some(v) = res.structured() {
            return Ok(js_key_order(v));
        }
        last = format!("{label}: {:?} {}", res.exit, res.stderr_tail.trim());
    }
    Err(format!("the thread harvest returned no answer ({last})"))
}

/// The incremental probe: the baseline review, what changed since it, and
/// which carried files import a changed one. Every failure degrades to a
/// full review, as the bookkeeping bash does.
pub async fn probe(rt: &Runtime, args: &RunArgs, repo: &str, head_sha: &str) -> Value {
    let pr_n: f64 = args.pr.parse().unwrap_or(f64::NAN);
    let reviews = gh_text(
        rt,
        &[
            "pr",
            "view",
            &args.pr,
            "--repo",
            repo,
            "--json",
            "reviews",
            "--jq",
            "[.reviews[] | {author: .author.login, state, at: .submittedAt, commit: .commit.oid}]",
        ],
    )
    .await
    .ok()
    .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    .filter(Value::is_array)
    .unwrap_or_else(|| json!([]));
    let input = json!({"pr": pr_n, "repo": repo, "headSha": head_sha, "since": args.since, "reviews": reviews});
    let ran = resolve_baseline::run(&input.to_string(), &resolve_baseline::metrics_path());
    let prior = resolve_baseline::parse_json(ran.stdout.trim())
        .unwrap_or_else(|_| json!({"found": false, "reason": format!("resolve-baseline failed: {}", ran.stderr.trim())}));
    let empty_ia = json!({"relatedFiles": {}, "scanned": 0, "aliasResolutionAttempted": false});
    if !js::truthy(js::get(&prior, "found")) {
        return json!({"startEpoch": null, "prior": prior, "scope": {"usable": false, "reason": "no baseline"}, "importAdjacency": empty_ia});
    }
    let sha = to_js_string(js::get(&prior, "sha"));
    let base = gh_text(
        rt,
        &[
            "pr",
            "view",
            &args.pr,
            "--repo",
            repo,
            "--json",
            "baseRefOid",
            "--jq",
            ".baseRefOid",
        ],
    )
    .await
    .unwrap_or_default();
    let baseline = tmp(rt, &format!("peer-review-baseline-{}-{sha}.patch", args.pr));
    let compare = format!("repos/{repo}/compare/{}...{sha}", base.trim());
    let got = gh_text(
        rt,
        &[
            "api",
            &compare,
            "-H",
            "Accept: application/vnd.github.v3.diff",
        ],
    )
    .await
    .unwrap_or_default();
    let _ = std::fs::write(&baseline, got);
    let current = tmp(rt, &format!("peer-review-probe-diff-{}.patch", args.pr));
    let cur = gh_text(rt, &["pr", "diff", &args.pr, "--repo", repo])
        .await
        .unwrap_or_default();
    let _ = std::fs::write(&current, &cur);
    let non_empty = |p: &str| std::fs::metadata(p).is_ok_and(|m| m.len() > 0);
    let scope = if non_empty(&current) && non_empty(&baseline) {
        serde_json::from_str(&scope_files(Some(&baseline), Some(&current)).to_json())
            .unwrap_or(json!({"usable": false}))
    } else {
        json!({"usable": false, "reason": format!("patch not found or empty (current: {}, baseline: {baseline})", if non_empty(&current) { current.as_str() } else { "none" })})
    };
    let ia = import_adjacency(rt, repo, head_sha, &prior, &scope)
        .await
        .unwrap_or(empty_ia);
    json!({"startEpoch": null, "prior": prior, "scope": scope, "importAdjacency": ia})
}

const ADJACENCY_CAP: usize = 30;

async fn import_adjacency(
    rt: &Runtime,
    repo: &str,
    sha: &str,
    prior: &Value,
    scope: &Value,
) -> Option<Value> {
    if !js::truthy(js::get(scope, "usable")) {
        return None;
    }
    let strs = |k: &str| -> Vec<String> {
        js::get(scope, k)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    };
    let unchanged = strs("unchanged");
    let mut changed = strs("changed");
    for a in strs("added") {
        if !changed.contains(&a) {
            changed.push(a);
        }
    }
    let mut carried: Vec<String> = js::get(prior, "findings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|f| js::get(f, "file").and_then(Value::as_str))
        .filter(|f| unchanged.iter().any(|u| u == f))
        .map(str::to_owned)
        .collect();
    carried.sort();
    carried.dedup();
    if carried.is_empty() || changed.is_empty() {
        return None;
    }
    let mut candidates: Vec<String> = carried.iter().chain(&changed).cloned().collect();
    candidates.sort();
    candidates.dedup();
    if candidates.len() > ADJACENCY_CAP {
        return None;
    }
    let fetch = |path: String| async move {
        let api = format!("repos/{repo}/contents/{path}?ref={sha}");
        gh_text(
            rt,
            &["api", &api, "-H", "Accept: application/vnd.github.raw"],
        )
        .await
        .ok()
    };
    let mut files = Map::new();
    for path in &candidates {
        if let Some(text) = fetch(path.clone()).await {
            files.insert(path.clone(), json!(text));
        }
    }
    let tsconfig = fetch("tsconfig.json".to_owned()).await;
    let attempted = tsconfig.is_some();
    let aliases = tsconfig
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|t| {
            js::get(&t, "compilerOptions")
                .and_then(|c| js::get(c, "paths"))
                .cloned()
        })
        .unwrap_or_else(|| json!({}));
    let manifest =
        json!({"files": files, "carried": carried, "changed": changed, "aliases": aliases});
    let mut out = compute_adjacency(&manifest).ok()?;
    if let Value::Object(m) = &mut out {
        m.entry("aliasResolutionAttempted")
            .or_insert(json!(attempted));
    }
    Some(out)
}

/// Everything the Review phase needs, or why the run stopped early.
#[derive(Debug, Clone)]
pub enum ContextOutcome {
    Skipped { reason: String },
    Preflight { summary: Value },
    Ready(Box<ReviewContext>),
}

#[derive(Debug, Clone)]
pub struct ReviewContext {
    pub pr_context: Value,
    pub plan: Plan,
    pub decisions: Decisions,
    pub delivery: DiffDelivery,
    pub head_pin: Value,
    pub blast_radius: Option<Value>,
    pub dependents: String,
    pub section_index: Option<Value>,
    pub methodology_line: String,
}

pub const SECTIONS_ABOVE_FILES: usize = 12;

/// The workflow's Context phase after SKILL.md's steps.
///
/// # Errors
/// Where the workflow throws or a native step cannot write its files.
pub async fn context_phase<A: Agent>(
    agent: &A,
    rt: &Runtime,
    args: &RunArgs,
    prepared: &Prepared,
    logs: &mut Vec<String>,
) -> Result<ContextOutcome, String> {
    let repo = prepared.repo.as_str();
    let deploy: Vec<String> = js::get(&prepared.profile, "deployBranches")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|b| to_js_string(Some(b)))
        .collect();
    let fetched = fetch_pr_context(
        &rt.tools,
        &FetchOptions {
            pr_number: args.pr.clone(),
            repo: repo.to_owned(),
            head_sha: args.head_sha.clone(),
            thread_before: args.thread_before.clone(),
            with_bot_comments: args.bot_comments,
            with_diff_text: false,
            deploy_branches: deploy,
            tmpdir: rt.tmpdir.clone(),
        },
    )
    .await?;
    if js::get(&fetched, "context").is_none_or(Value::is_null) {
        return Ok(ContextOutcome::Skipped {
            reason: format!(
                "fetch-pr failed: {}",
                js::get(&fetched, "errors").map_or(String::new(), Value::to_string)
            ),
        });
    }
    let context_dir = tmp(rt, &format!("tribunal-context-{}", args.pr));
    let data = relay_context(&fetched, &context_dir)?;
    let answer = if js::truthy(js::get(&data, "rawHasHumanText")) {
        let raw_path = to_js_string(js::get(&data, "rawPath"));
        match harvest(agent, rt, &args.pr, repo, &raw_path).await {
            Ok(a) => {
                let n = |k: &str| js::get(&a, k).and_then(Value::as_array).map_or(0, Vec::len);
                logs.push(format!(
                    "Thread harvest: {} decision(s), {} changes-requested summary(ies)",
                    n("inThreadDecisions"),
                    n("priorReviewSummaries")
                ));
                a
            }
            Err(e) => {
                logs.push(format!("fetch-pr: {e} — cannot classify or review this PR"));
                return Ok(ContextOutcome::Skipped {
                    reason: "fetch-pr failed".into(),
                });
            }
        }
    } else {
        json!({"inThreadDecisions": [], "priorReviewSummaries": []})
    };
    let mut pr_context = build_pr_context(&data, &answer)?;
    if js::truthy(js::get(&pr_context, "isDeploymentPR")) {
        logs.push("Deployment PR — skipping review".into());
        return Ok(ContextOutcome::Skipped {
            reason: "deployment PR".into(),
        });
    }
    if !args.head_sha.is_empty() {
        pr_context["headRefOid"] = json!(args.head_sha);
    }
    logs.push(format!(
        "PR #{}: \"{}\" — {} files, +{}/-{}",
        args.pr,
        to_js_string(js::get(&pr_context, "title")),
        to_js_string(js::get(&pr_context, "fileCount")),
        to_js_string(js::get(&pr_context, "additions")),
        to_js_string(js::get(&pr_context, "deletions")),
    ));
    if !args.head_sha.is_empty() {
        logs.push(format!(
            "PINNED RUN — reviewing commit {}, not the PR's current head. This is a measurement re-run: the report describes that tree and must not be posted as a review of the PR as it stands.",
            args.head_sha.chars().take(8).collect::<String>()
        ));
    }

    let wants_probe = !args.full && args.head_sha.is_empty();
    let head_sha = to_js_string(js::get(&pr_context, "headRefOid"));
    let started = if args.preflight {
        Value::Null
    } else {
        let deploy: Vec<String> = js::get(&prepared.profile, "deployBranches")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|b| to_js_string(Some(b)))
            .collect();
        let pr_number = args.pr.parse::<u64>().unwrap_or(0);
        append_started(
            &rt.data_dir,
            pr_number,
            repo,
            Some(head_sha.as_str()).filter(|h| !h.is_empty() && *h != "undefined"),
            args.batch_panels == Some(true),
            &to_js_string(js::get(&pr_context, "baseRefName")),
            &deploy,
            epoch_now(),
        )
        .map_or(Value::Null, |e| json!(e))
    };
    let bookkeeping = if wants_probe {
        let mut p = probe(rt, args, repo, &head_sha).await;
        if !args.preflight {
            p["startEpoch"] = started;
        }
        Some(p)
    } else if !args.preflight {
        Some(
            json!({"startEpoch": started, "prior": {"found": false, "reason": "incremental probe not run (--full or --head-sha)"}, "scope": {"usable": false}, "importAdjacency": {"relatedFiles": {}, "scanned": 0, "aliasResolutionAttempted": false}}),
        )
    } else {
        None
    };
    let plan = build_plan(&PlanInput {
        pr_context: &pr_context,
        bookkeeping: bookkeeping.as_ref(),
        wants_probe,
        force_full: args.full,
        pinned_sha: &args.head_sha,
        lenses: args.lenses.as_ref(),
        profile: &prepared.profile,
    })?;
    logs.extend(plan.logs.iter().cloned());
    pr_context["files"] = Value::from(plan.files.clone());

    let decisions = build_decisions(args.context.as_deref(), &pr_context, args.retest.as_ref());
    logs.extend(decisions.logs.iter().cloned());

    let diff_path = to_js_string(
        js::get(&pr_context, "diffPath")
            .filter(|p| js::truthy(Some(p)))
            .or(Some(&json!(""))),
    );
    let diff_path = js::trim(&diff_path).to_owned();
    logs.push(if diff_path.is_empty() {
        "No diff path returned — agents will fetch it themselves (degraded)".to_owned()
    } else {
        format!("Diff read from {diff_path}")
    });

    if args.preflight {
        let scope = plan.incremental_scope.as_ref();
        let summary = json!({
            "pr": args.pr.parse::<u64>().map_or(json!(args.pr), |n| json!(n)),
            "headSha": js::get(&pr_context, "headRefOid"),
            "diffBytes": js::get(&pr_context, "diffBytes").cloned().unwrap_or(Value::Null),
            "diffMode": if diff_path.is_empty() { "self-fetch" } else { "path" },
            "reviewMode": if scope.is_some() { "incremental" } else { "full" },
            "baseline": plan.prior_review.as_ref().and_then(|p| js::get(p, "sha")).cloned().unwrap_or(Value::Null),
            "baselineSource": plan.prior_review.as_ref().and_then(|p| js::get(p, "source")).cloned().unwrap_or(Value::Null),
            "baselineFindings": plan.prior_review.as_ref().and_then(|p| js::get(p, "findings")).and_then(Value::as_array).map_or(0, Vec::len),
            "filesInPr": plan.all_pr_files.len(),
            "filesInScope": plan.files.len(),
            "inScope": plan.files,
            "filesCarriedForward": scope.and_then(|s| js::get(s, "unchanged")).and_then(Value::as_array).map_or(0, Vec::len),
            "lensCount": plan.lens_count,
            "lensGroups": plan.group_count,
            "incrementalFallback": plan.incremental_fallback,
        });
        return Ok(ContextOutcome::Preflight { summary });
    }

    let worktree = prepare_head_worktree(
        &rt.tools,
        &pr_context,
        repo,
        &args.pr,
        Some(&rt.operator.cwd),
        &rt.tmpdir,
    )
    .await?;
    let head_pin = match &worktree {
        Some(text) => parse_head_worktree(Some(&json!(text)))?.to_value(),
        None => json!({"path": null, "kind": "none", "ok": false}),
    };
    if worktree.is_some() {
        logs.push(if js::truthy(js::get(&head_pin, "ok")) {
            format!(
                "Pinned checkout ready at {} ({}) — lenses and refuters read source there, not in the ambient clone",
                to_js_string(js::get(&head_pin, "path")),
                to_js_string(js::get(&head_pin, "kind"))
            )
        } else {
            "NO PINNED CHECKOUT — lenses and refuters fall back to prompt-level pinning, which measured 3% (lenses) and 5% (refuters) unpinned reads across 8 runs".to_owned()
        });
    }

    let section_index = if !diff_path.is_empty() && plan.all_pr_files.len() > SECTIONS_ABOVE_FILES {
        let out = format!(
            "{}/tribunal-sections-{}-{}",
            rt.tmpdir.trim_end_matches('/'),
            args.pr,
            std::process::id()
        );
        let num = |k: &str| {
            js::get(&pr_context, k)
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
        };
        section_diff::run(&diff_path, &out, num("additions"), num("deletions")).ok()
    } else {
        None
    };

    let pinned = js::truthy(js::get(&head_pin, "ok"));
    let blast = if pinned && !diff_path.is_empty() {
        let dir = to_js_string(js::get(&head_pin, "path"));
        blast_radius::run(&diff_path, &dir, None, &rt.tools)
            .await
            .ok()
    } else {
        None
    };
    let dependents = dependents_block(blast.as_ref())?;

    let delivery = DiffDelivery {
        diff_path: (!diff_path.is_empty()).then(|| diff_path.clone()),
        diff_bytes: js::get(&pr_context, "diffBytes").and_then(Value::as_f64),
        head_pin: head_pin.clone(),
        head_sha: js::get(&pr_context, "headRefOid").cloned(),
    };
    Ok(ContextOutcome::Ready(Box::new(ReviewContext {
        pr_context,
        plan,
        decisions,
        delivery,
        head_pin,
        blast_radius: blast,
        dependents,
        section_index,
        methodology_line: methodology_line(&prepared.methodology_path),
    })))
}

/// The directory reviewers run in: the pinned checkout, else where the
/// operator ran the binary.
#[must_use]
pub fn reviewer_cwd(rc: &ReviewContext, rt: &Runtime) -> PathBuf {
    match js::get(&rc.head_pin, "path").and_then(Value::as_str) {
        Some(p) if js::truthy(js::get(&rc.head_pin, "ok")) && Path::new(p).is_dir() => {
            PathBuf::from(p)
        }
        _ => rt.operator.cwd.clone(),
    }
}
