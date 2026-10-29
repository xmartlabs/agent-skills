//! SKILL.md's Step 4 with `--post`: the duplicate-review gate, the composed
//! GitHub-facing body (a model step: continuity, author refutations, the
//! template, the homosapiens pass), the report-fidelity gate, the footer,
//! `post-review` and the row patch. Nothing here posts unless the operator
//! passed `--post` and the gate reads `clear`; unattended, every other gate
//! value is "do not post".

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;
use tribunal_agent::{Agent, DispatchRequest};
use tribunal_context::check_report_fidelity::apply_fidelity_checks;
use tribunal_context::grounded_typecheck::remove_tree;
use tribunal_context::head_worktree::{build_head_worktree, parse_head_worktree};
use tribunal_context::ledger::LedgerEnv;
use tribunal_context::post_review::PostEnv;
use tribunal_context::{post_footer, post_review, review_record};
use tribunal_core::js;
use tribunal_core::lens::js_ext::to_js_string;
use tribunal_core::report::draft_comments_strip::strip_draft_comments_from_body;

use crate::context::{RunArgs, Runtime};

/// Rounds of the fidelity gate's patch step before the post is refused.
pub const FIDELITY_ROUNDS: usize = 3;

/// What the post step did.
#[derive(Debug, Clone, PartialEq)]
pub enum PostOutcome {
    /// Not posted, and why.
    Held(String),
    /// Posted: the PR URL and the review state submitted.
    Posted { url: String, verdict: String },
}

/// The `post-review` flag for a harness verdict.
#[must_use]
pub fn review_flag(verdict: &str) -> Option<&'static str> {
    match verdict {
        "APPROVE" => Some("--approve"),
        "REQUEST CHANGES" => Some("--request-changes"),
        "COMMENT" => Some("--comment"),
        _ => None,
    }
}

/// The harness verdict a `tribunal post --verdict` value names.
#[must_use]
pub fn verdict_from_flag(value: &str) -> Option<&'static str> {
    match value {
        "approve" => Some("APPROVE"),
        "request-changes" => Some("REQUEST CHANGES"),
        "comment" => Some("COMMENT"),
        _ => None,
    }
}

/// How a post puts the run on the PR.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PostMode {
    /// A new review carrying the whole report.
    #[default]
    Full,
    /// A COMMENT carrying only what is new since the operator's standing
    /// review (`--delta`).
    Delta,
    /// The operator's standing review with its body replaced; GitHub keeps its
    /// state (`--edit-existing`).
    Edit,
}

/// What the operator answered at SKILL.md's Step 4 for a saved run.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PostChoice {
    /// Post although their own earlier review stands (`--over-own-review`).
    pub over_own_review: bool,
    /// Post the full review whatever the gate says (`--over-standing-review`).
    pub over_standing_review: bool,
    /// The review state they picked (`--verdict`); `None` keeps the run's.
    pub verdict: Option<&'static str>,
    pub mode: PostMode,
}

/// The duplicate-review gate's answer to `choice`: `Ok` with the line to log
/// when the post may go ahead, `Err` with why not.
///
/// # Errors
/// The gate holds this post.
pub fn gate_decision(metrics: &Value, choice: &PostChoice) -> Result<Option<&'static str>, String> {
    let gate = js::get(metrics, "priorOpenReviews")
        .and_then(|p| js::get(p, "gate"))
        .and_then(Value::as_str);
    let own = matches!(gate, Some("own-open-review" | "own-prior-review"));
    match choice.mode {
        PostMode::Delta | PostMode::Edit if !own => Err(format!(
            "the duplicate-review gate is {}, so there is no standing review of yours to {}",
            gate.unwrap_or("missing"),
            if choice.mode == PostMode::Delta {
                "post a delta against"
            } else {
                "edit"
            }
        )),
        PostMode::Delta => Ok(Some(
            "Posting what is new since your standing review, as asked",
        )),
        PostMode::Edit => Ok(Some("Editing your standing review, as asked")),
        PostMode::Full => match gate_hold(metrics) {
            None => Ok(None),
            Some(_) if choice.over_standing_review => Ok(Some(
                "Posting the full review over the standing review, as asked",
            )),
            Some(_) if gate == Some("own-prior-review") && choice.over_own_review => {
                Ok(Some("Posting over your own standing review, as asked"))
            }
            Some(why) => Err(why),
        },
    }
}

/// The duplicate-review gate: `None` when posting may go ahead.
#[must_use]
pub fn gate_hold(metrics: &Value) -> Option<String> {
    let gate = js::get(metrics, "priorOpenReviews").and_then(|p| js::get(p, "gate"));
    match gate.and_then(Value::as_str) {
        Some("clear") => None,
        Some(g) => Some(format!(
            "duplicate-review gate is {g}: a review by you already stands on this PR (or ownership is unknown), so nothing is posted unattended. Post by hand after comparing it with this report"
        )),
        None => Some("the metrics row carries no duplicate-review gate, so nothing is posted".into()),
    }
}

/// The compose step's prompt.
#[must_use]
pub fn compose_prompt(pr: &str, repo: &str, head: &str, report: &Path, out: &Path) -> String {
    format!(
        "You are writing the GitHub-facing body of a code review of PR #{pr} in {repo}, reviewed at commit {head}. The harness report is the file {report}. Work through these steps in order and skip none.

1. Continuity. Run `gh pr view {pr} --repo {repo} --json reviews` and `gh api --paginate repos/{repo}/issues/{pr}/comments`. If either holds content, open the body with a short status section naming each earlier review or comment (author and date) and, for each of its Blocking or should-fix items, whether it is fixed, still open or disputed. Decide that from the code at {head} in this checkout, not from GitHub's review state and not from a commit message.
2. Author refutations. For every reply by the PR author that dismisses or explains away a standing finding, check the claim against the code and say whether it is confirmed, refuted or narrower than claimed. Skip a claim an earlier review by the same reviewer already answered.
3. Template. Put what this review contributes under `### 🔴 Blocking`, `### 🟡 Should fix, not blocking` and `### 🔵 Nits` (say when a nit did not go through the panel). Each item: a short bold title, the exact `file:line`, then the explanation. Move out of Blocking anything whose own verified text calls it non-blocking.
4. Mentions. Use `@handle` only to address someone; a passing reference uses the bare handle.
5. Keep, exactly as in the report: every `(found by: ...)` annotation, every `also at:` location, every kill-reason category, every typecheck diagnostic, the `Verdict suggestion:` line and the whole `### Draft comments` section.
6. Run the homosapiens skill over the body in embedded mode (`homosapiens:homosapiens` when that plugin is installed, else `homosapiens`). If neither skill exists, write nothing and answer exactly NO-HOMOSAPIENS.

Write the final body to {out} with the Write tool, then answer with the single word DONE.",
        report = report.display(),
        out = out.display(),
    )
}

/// The compose prompt for [`PostMode::Edit`]: the full body, written to
/// replace the operator's own latest review, which step 1 must not report on.
#[must_use]
pub fn edit_prompt(pr: &str, repo: &str, head: &str, report: &Path, out: &Path) -> String {
    compose_prompt(pr, repo, head, report, out).replacen(
        "1. Continuity.",
        "0. This body replaces the operator's own most recent review on the PR (their login is what `gh api user --jq .login` prints). Leave that review out of step 1: it is the one being rewritten.\n1. Continuity.",
        1,
    )
}

/// The compose prompt for [`PostMode::Delta`]: a comment carrying only what
/// this run adds to the operator's standing review.
#[must_use]
pub fn delta_prompt(pr: &str, repo: &str, head: &str, report: &Path, out: &Path) -> String {
    format!(
        "You are writing a GitHub comment on PR #{pr} in {repo}, reviewed at commit {head}, that adds to a review the operator already has standing on it. The harness report is the file {report}. Work through these steps in order and skip none.

1. The standing review. Run `gh api user --jq .login` for the operator's login, then `gh pr view {pr} --repo {repo} --json reviews`, and take that login's most recent review. Open the comment with one line naming it, its state and its date, and saying this comment covers what is new since.
2. What is new. Compare the report's findings with that review, reading the code at {head} in this checkout. Keep a finding only when the review does not carry it, or carries it in a form this run changes: a new location, another severity, or now fixed. Say which of those applies to each finding you keep. When nothing is new, write the opening line and one sentence saying so.
3. Author refutations. For every reply by the PR author since that review that dismisses or explains away one of its findings, check the claim against the code and say whether it is confirmed, refuted or narrower than claimed.
4. Template. Put the findings you keep under `### 🔴 Blocking`, `### 🟡 Should fix, not blocking` and `### 🔵 Nits` (say when a nit did not go through the panel). Each item: its bold title exactly as the report words it, the exact `file:line`, then the explanation.
5. Mentions. Use `@handle` only to address someone; a passing reference uses the bare handle.
6. Keep, exactly as in the report, for every finding you keep: its `(found by: ...)` annotation, its `also at:` locations, its kill-reason category and its typecheck diagnostic. Keep the whole `### Draft comments` section.
7. Run the homosapiens skill over the body in embedded mode (`homosapiens:homosapiens` when that plugin is installed, else `homosapiens`). If neither skill exists, write nothing and answer exactly NO-HOMOSAPIENS.

Write the final body to {out} with the Write tool, then answer with the single word DONE.",
        report = report.display(),
        out = out.display(),
    )
}

/// The findings a delta body carries, by their title. A delta leaves out what
/// the standing review already says, so only these must keep their
/// annotations; [`delta_prompt`] asks for the report's titles verbatim.
#[must_use]
pub fn findings_in_body(findings: &[Value], body: &str) -> Vec<Value> {
    let text = strip_draft_comments_from_body(body);
    findings
        .iter()
        .filter(|f| {
            js::get(f, "title")
                .and_then(Value::as_str)
                .is_some_and(|t| !t.is_empty() && text.contains(t))
        })
        .cloned()
        .collect()
}

/// The fidelity gate's patch prompt for one round.
#[must_use]
pub fn patch_prompt(body: &Path, missing: &[String]) -> String {
    format!(
        "The review body in {} must carry these items and does not:\n{}\n\nEdit the file so each one appears, in the finding it belongs to. Change nothing else, keep the `### Draft comments` section, and do not add a verdict banner or a footer. Answer with the single word DONE.",
        body.display(),
        missing.iter().map(|m| format!("- {m}")).collect::<Vec<_>>().join("\n")
    )
}

/// The items the posted body is missing, one line each, from the four
/// `*MissingFromReport` arrays the fidelity checks write.
///
/// # Errors
/// The checks' own TypeError on a malformed finding.
pub fn missing_items(findings: &[Value], body: &str) -> Result<Vec<String>, String> {
    let mut list = Value::Array(findings.to_vec());
    apply_fidelity_checks(&mut list, &strip_draft_comments_from_body(body))?;
    let mut out = Vec::new();
    for f in list.as_array().into_iter().flatten() {
        let title = js::get(f, "title")
            .and_then(Value::as_str)
            .unwrap_or("(untitled)");
        for (key, what) in [
            ("mergedLocationsMissingFromReport", "also at"),
            ("killReasonMissingFromReport", "kill reason"),
            ("mergedFromAnnotationMissingFromReport", "found by"),
            (
                "groundedDiagnosticMissingFromReport",
                "typecheck diagnostic",
            ),
        ] {
            for m in js::get(f, key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                out.push(format!("{title}: {what} {}", to_js_string(Some(m))));
            }
        }
    }
    Ok(out)
}

async fn ask<A: Agent>(
    agent: &A,
    label: &str,
    prompt: String,
    model: &str,
    effort: Option<&String>,
    cwd: &Path,
) -> Option<String> {
    let mut req = DispatchRequest::new(label, prompt, cwd);
    req.model = Some(model.to_owned());
    req.effort = effort.cloned();
    req.tools = ["Bash", "Read", "Write", "Edit", "Grep", "Glob", "Skill"]
        .map(str::to_owned)
        .to_vec();
    req.timeout = Duration::from_secs(30 * 60);
    req.inherit_settings = true;
    agent.run(&req).await.text().map(|t| js::trim(t).to_owned())
}

/// Everything the post step needs from the run.
#[derive(Debug, Clone)]
pub struct PostInput<'a> {
    pub metrics: &'a Value,
    pub report: &'a str,
    pub verdict: &'a str,
    pub head_sha: &'a str,
    pub repo: &'a str,
    /// Where the compose step reads code: the pinned checkout when there is one.
    pub cwd: &'a Path,
    pub effort: Option<&'a String>,
    /// SKILL.md's Step 4 answer: how to post, and whether the operator chose
    /// to post past the duplicate-review gate. The default is a full review
    /// that only a `clear` gate lets through.
    pub choice: PostChoice,
}

fn post_env(rt: &Runtime) -> PostEnv {
    PostEnv {
        cwd: rt.operator.cwd.clone(),
        tmp_dir: PathBuf::from(&rt.tmpdir),
    }
}

/// The operator's newest review on the PR, with the harness verdict its state
/// names, for [`PostMode::Edit`]: `post-review --overwrite` keeps the state,
/// so the recorded verdict is that one.
async fn standing_review(
    rt: &Runtime,
    pr: &str,
    repo: &str,
) -> Result<(String, &'static str), String> {
    let login = match rt.tools.gh(&["api", "user", "--jq", ".login"], None).await {
        Ok(out) if out.ok() && !out.stdout.trim().is_empty() => out.stdout.trim().to_owned(),
        Ok(out) => {
            return Err(format!(
                "cannot read your GitHub login: {}",
                out.stderr.trim()
            ))
        }
        Err(e) => return Err(format!("cannot run gh: {e}")),
    };
    let found = post_review::run(
        &["--pr", pr, "--repo", repo, "--find-review-id", &login].map(str::to_owned),
        &rt.tools,
        &post_env(rt),
    )
    .await;
    if found.code != 0 {
        return Err(format!(
            "no review of yours to edit: {}",
            found.stderr.trim()
        ));
    }
    let id = found.stdout.trim().to_owned();
    let endpoint = format!("repos/{repo}/pulls/{pr}/reviews/{id}");
    let state = match rt.tools.gh(&["api", &endpoint], None).await {
        Ok(out) if out.ok() => serde_json::from_str::<Value>(&out.stdout)
            .ok()
            .and_then(|v| {
                js::get(&v, "state")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }),
        Ok(out) => return Err(format!("cannot read review {id}: {}", out.stderr.trim())),
        Err(e) => return Err(format!("cannot run gh: {e}")),
    };
    let verdict = match state.as_deref() {
        Some("APPROVED") => "APPROVE",
        Some("CHANGES_REQUESTED") => "REQUEST CHANGES",
        Some("COMMENTED") => "COMMENT",
        s => {
            return Err(format!(
                "review {id} has state {s:?}, which no edit can keep"
            ))
        }
    };
    Ok((id, verdict))
}

/// Step 4 for a run the operator asked to post.
#[allow(clippy::too_many_lines)]
pub async fn post_phase<A: Agent>(
    agent: &A,
    rt: &Runtime,
    args: &RunArgs,
    input: &PostInput<'_>,
    logs: &mut Vec<String>,
) -> PostOutcome {
    match gate_decision(input.metrics, &input.choice) {
        Ok(Some(line)) => logs.push(line.into()),
        Ok(None) => {}
        Err(why) => return PostOutcome::Held(why),
    }
    let mode = input.choice.mode;
    let (verdict, post_args): (&str, Vec<String>) = match mode {
        PostMode::Edit => match standing_review(rt, &args.pr, input.repo).await {
            Ok((id, verdict)) => (verdict, vec!["--overwrite".into(), id]),
            Err(why) => return PostOutcome::Held(why),
        },
        PostMode::Delta => ("COMMENT", vec!["--comment".into()]),
        PostMode::Full => match review_flag(input.verdict) {
            Some(flag) => (input.verdict, vec![flag.into()]),
            None => {
                return PostOutcome::Held(format!(
                    "no review state for verdict {:?}",
                    input.verdict
                ))
            }
        },
    };
    let dir = PathBuf::from(format!(
        "{}/tribunal-post-{}-{}",
        rt.tmpdir.trim_end_matches('/'),
        args.pr,
        std::process::id()
    ));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return PostOutcome::Held(format!("cannot create {}: {e}", dir.display()));
    }
    let report_file = dir.join("report.md");
    let body_file = dir.join("body.md");
    let footer_file = dir.join("footer.txt");
    if let Err(e) = std::fs::write(&report_file, input.report) {
        return PostOutcome::Held(format!("cannot write the report: {e}"));
    }

    let prompt = match mode {
        PostMode::Full => compose_prompt,
        PostMode::Delta => delta_prompt,
        PostMode::Edit => edit_prompt,
    };
    let answer = ask(
        agent,
        "compose-post",
        prompt(
            &args.pr,
            input.repo,
            input.head_sha,
            &report_file,
            &body_file,
        ),
        &args.review_model,
        input.effort,
        input.cwd,
    )
    .await;
    if answer.as_deref() == Some("NO-HOMOSAPIENS") {
        return PostOutcome::Held(
            "the homosapiens skill is not installed; install the homosapiens plugin before posting"
                .into(),
        );
    }
    let Ok(mut body) = std::fs::read_to_string(&body_file) else {
        return PostOutcome::Held(format!(
            "the compose step wrote no body (answer: {answer:?})"
        ));
    };

    let findings = js::get(input.metrics, "findings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut round = 0;
    loop {
        let checked = if mode == PostMode::Delta {
            findings_in_body(&findings, &body)
        } else {
            findings.clone()
        };
        let missing = match missing_items(&checked, &body) {
            Ok(m) => m,
            Err(e) => return PostOutcome::Held(format!("the fidelity gate failed: {e}")),
        };
        if missing.is_empty() {
            break;
        }
        if round == FIDELITY_ROUNDS {
            return PostOutcome::Held(format!(
                "the body still misses {} item(s) after {FIDELITY_ROUNDS} patch rounds: {}",
                missing.len(),
                missing.join("; ")
            ));
        }
        round += 1;
        logs.push(format!(
            "Fidelity gate round {round}: patching {} missing item(s)",
            missing.len()
        ));
        ask(
            agent,
            &format!("patch-post:{round}"),
            patch_prompt(&body_file, &missing),
            &args.review_model,
            input.effort,
            input.cwd,
        )
        .await;
        body = std::fs::read_to_string(&body_file).unwrap_or_default();
    }

    let ledger = LedgerEnv {
        home: rt.operator.home.clone(),
        cwd: rt.operator.cwd.to_string_lossy().into_owned(),
        data_dir: Some(rt.data_dir.to_string_lossy().into_owned()),
        project_dir: None,
        lock_wait: tribunal_context::ledger::lock_wait(),
    };
    let agent_count = js::get(input.metrics, "agentsActual")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let footer = post_footer::run(
        &[
            "--mode",
            "workflow",
            "--pr",
            args.pr.as_str(),
            "--head-sha",
            input.head_sha,
            "--agent-count",
            js::number_to_string(agent_count).as_str(),
        ]
        .map(str::to_owned),
        &ledger,
        SystemTime::now(),
        &rt.plugin_root.to_string_lossy(),
    );
    if footer.code != 0 {
        return PostOutcome::Held(format!("the footer was refused: {}", footer.stderr.trim()));
    }
    if let Err(e) = std::fs::write(&footer_file, &footer.stdout) {
        return PostOutcome::Held(format!("cannot write the footer: {e}"));
    }

    let mut post_argv = vec![
        "--pr".to_owned(),
        args.pr.clone(),
        "--repo".to_owned(),
        input.repo.to_owned(),
        "--report-file".to_owned(),
        body_file.to_string_lossy().into_owned(),
        "--footer-file".to_owned(),
        footer_file.to_string_lossy().into_owned(),
    ];
    post_argv.extend(post_args);
    let posted = post_review::run(&post_argv, &rt.tools, &post_env(rt)).await;
    if posted.code != 0 {
        return PostOutcome::Held(format!(
            "post-review failed (exit {}): {}",
            posted.code,
            posted.stderr.trim()
        ));
    }
    let url = posted.stdout.trim().to_owned();

    let record = review_record::run(
        &[
            "--pr",
            args.pr.as_str(),
            "--repo",
            input.repo,
            "--head-sha",
            input.head_sha,
            "--posted",
            "true",
            "--verdict",
            verdict,
            "--report-file",
            body_file.to_string_lossy().as_ref(),
        ]
        .map(str::to_owned),
        &ledger,
    );
    if record.code != 0 {
        logs.push(format!(
            "Posted, but the row patch failed: {}",
            record.stderr.trim()
        ));
    }
    let _ = std::fs::remove_dir_all(&dir);
    PostOutcome::Posted {
        url,
        verdict: verdict.to_owned(),
    }
}

/// `tribunal post --from <run.json>`: Step 4 for a run that finished without
/// posting. The PR must still be at the commit the run reviewed; the compose
/// step reads the code in a checkout pinned there, removed afterwards.
pub async fn post_saved_run<A: Agent>(
    agent: &A,
    rt: &Runtime,
    run: &Value,
    choice: PostChoice,
    review_model: &str,
    effort: Option<&String>,
    logs: &mut Vec<String>,
) -> PostOutcome {
    let Some(metrics) = js::get(run, "metrics").filter(|m| m.is_object()) else {
        return PostOutcome::Held("the run has no metrics row, so no footer can be built".into());
    };
    let text = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or("").to_owned();
    let report = text(js::get(run, "report"));
    let verdict = choice
        .verdict
        .map_or_else(|| text(js::get(run, "verdict")), str::to_owned);
    let repo = text(js::get(run, "repo"));
    let head = text(js::get(metrics, "headSha"));
    let pr = js::get(metrics, "pr")
        .map(|p| to_js_string(Some(p)))
        .unwrap_or_default();
    if report.is_empty() || repo.is_empty() || head.is_empty() || pr.is_empty() {
        return PostOutcome::Held(
            "the run lacks its report, repo, PR number or head commit".into(),
        );
    }
    let live = rt
        .tools
        .gh(
            &[
                "pr",
                "view",
                &pr,
                "--repo",
                &repo,
                "--json",
                "headRefOid",
                "--jq",
                ".headRefOid",
            ],
            None,
        )
        .await;
    match live {
        Ok(out) if out.ok() && out.stdout.trim() == head => {}
        Ok(out) if out.ok() => {
            return PostOutcome::Held(format!(
            "the PR head is {} now, and the run reviewed {head}: review it again before posting",
            out.stdout.trim()
        ))
        }
        Ok(out) => {
            return PostOutcome::Held(format!("cannot read the PR head: {}", out.stderr.trim()))
        }
        Err(e) => return PostOutcome::Held(format!("cannot run gh: {e}")),
    }
    let built = build_head_worktree(
        &rt.tools,
        &head,
        &repo,
        &pr,
        Some(&rt.operator.cwd),
        &rt.tmpdir,
    )
    .await;
    let pin = parse_head_worktree(Some(&Value::String(built.trim().to_owned()))).ok();
    let pinned = pin.as_ref().and_then(|p| p.path.clone());
    let cwd = pinned
        .as_ref()
        .map_or_else(|| rt.operator.cwd.clone(), PathBuf::from);
    if pinned.is_none() {
        logs.push(format!(
            "No checkout pinned at {head}; the compose step reads {}",
            cwd.display()
        ));
    }
    let args = RunArgs {
        pr,
        repo: repo.clone(),
        review_model: review_model.to_owned(),
        post: true,
        ..RunArgs::default()
    };
    let input = PostInput {
        metrics,
        report: &report,
        verdict: &verdict,
        head_sha: &head,
        repo: &repo,
        cwd: &cwd,
        effort,
        choice,
    };
    let outcome = post_phase(agent, rt, &args, &input, logs).await;
    if let (Some(path), Some(p)) = (pinned.as_deref(), pin.as_ref()) {
        remove_tree(&rt.tools, path, p.kind == "clone", &rt.operator.cwd).await;
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_clear_gate_posts() {
        assert_eq!(
            gate_hold(&json!({"priorOpenReviews": {"gate": "clear"}})),
            None
        );
        for g in ["own-open-review", "own-prior-review", "unknown-viewer"] {
            assert!(gate_hold(&json!({"priorOpenReviews": {"gate": g}})).is_some());
        }
        assert!(gate_hold(&json!({})).is_some());
    }

    #[test]
    fn verdicts_map_to_review_flags() {
        assert_eq!(review_flag("REQUEST CHANGES"), Some("--request-changes"));
        assert_eq!(review_flag("APPROVE"), Some("--approve"));
        assert_eq!(review_flag("COMMENT"), Some("--comment"));
        assert_eq!(review_flag("Request changes"), None);
    }

    #[test]
    fn verdict_flags_name_the_harness_verdicts() {
        for v in ["approve", "request-changes", "comment"] {
            assert!(review_flag(verdict_from_flag(v).unwrap()).is_some(), "{v}");
        }
        assert_eq!(verdict_from_flag("REQUEST CHANGES"), None);
        assert_eq!(verdict_from_flag("reject"), None);
    }

    #[test]
    fn a_delta_body_carries_the_findings_whose_title_it_quotes() {
        let findings = [
            json!({"title": "Lost write"}),
            json!({"title": "Stale cache"}),
            json!({"title": ""}),
            json!({}),
        ];
        let body = "**Lost write** `a.ts:1`\n\n### Draft comments\n- Stale cache\n";
        assert_eq!(
            findings_in_body(&findings, body),
            [json!({"title": "Lost write"})]
        );
    }

    #[test]
    fn the_edit_prompt_leaves_the_review_it_replaces_out_of_continuity() {
        let p = edit_prompt("7", "o/r", "abc", Path::new("r.md"), Path::new("b.md"));
        let step0 = p.find("0. This body replaces").unwrap();
        assert!(step0 < p.find("1. Continuity.").unwrap());
        assert!(p.contains("Write the final body to b.md with the Write tool"));
    }
}
