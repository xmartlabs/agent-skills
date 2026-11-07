//! The Review phase: one dispatch per planned lens, plus the bot-comment
//! vetting pass when asked, then the candidates they produced up to the
//! pre-panel out-of-diff gate. Ports the workflow from `phase("Review")` to
//! the out-of-diff log line.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use tokio::sync::Semaphore;
use tribunal_agent::{Agent, DispatchRequest, DispatchResult};
use tribunal_core::js;
use tribunal_core::lens::coverage_gaps::{collect_gaps, Gap};
use tribunal_core::lens::js_ext::{js_key_order, to_js_string};
use tribunal_core::lens::lens_budget::collect_not_reached;
use tribunal_core::lens::out_of_diff::reanchor_out_of_diff;
use tribunal_core::profile::high_risk_surface::risk_path_touched;
use tribunal_core::profile::profile_lookup::risk_path_rules;

use crate::agent_def::AgentDef;
use crate::schemas;

/// The bot-vetting dispatch's prompt, before the reviewer wrapper adds the
/// methodology and conventions lines.
#[must_use]
pub fn bot_vetting_prompt(pr_number: &str, repo: &str, bot_comments_path: &str) -> String {
    format!(
        r#"Vet these automated review comments on PR #{pr_number} in repo {repo}. Fetch the diff yourself per your own "PR number provided" scope rule (gh pr view, gh pr diff) to verify each claim.

Classify each as:
- VALID: real issue with evidence confirmed in the diff
- INVALID: hallucinated premise — bot claims about DB shape or field existence are hypotheses, not facts; trace to source before marking VALID
- POLICY: style/convention preference, not a bug

The bot comments are a JSON array in {bot_comments_path}: read that file. Return one entry per comment, with its position in the array as `index`.

For every VALID comment also return: `file` and `line` where the defect is (the comment's own path and line unless the defect sits elsewhere in this PR), `isBehaviorChanging` (true if, when real, it changes what a caller, user, stored row or job observes; false for style, docs, logging-only or test-only), and `verifiedBy`: at least one {{file, line, quote}} copying word for word the code the claim depends on. A VALID comment without `verifiedBy` cannot block the merge."#
    )
}

/// `FOCI_LABEL[focus] || lens<i+1>` for every planned lens.
#[must_use]
pub fn lens_labels(foci: &[Value], foci_label: &Map<String, Value>) -> Vec<String> {
    foci.iter()
        .enumerate()
        .map(
            |(i, focus)| match foci_label.get(&to_js_string(Some(focus))) {
                Some(l) if js::truthy(Some(l)) => to_js_string(Some(l)),
                _ => format!("lens{}", i + 1),
            },
        )
        .collect()
}

/// Ports `unreturnedLenses`: the planned lenses whose dispatch came back
/// empty, each with the files it was given (`None` for the whole diff).
#[must_use]
pub fn unreturned_lenses(
    plan_files: &[Option<Vec<String>>],
    results: &[Option<Value>],
    labels: &[String],
) -> Vec<Value> {
    plan_files
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            !results
                .get(*i)
                .and_then(Option::as_ref)
                .is_some_and(|r| js::truthy(Some(r)))
        })
        .map(|(i, files)| {
            json!({
                "lens": labels.get(i).map_or(Value::Null, |l| json!(l)),
                "files": files.as_ref().map_or(Value::Null, |f| json!(f)),
            })
        })
        .collect()
}

/// The check after the out-of-diff gate: the lenses that came back empty and
/// the log line naming them.
///
/// # Errors
/// When no planned lens returned anything, where the workflow throws.
pub fn check_lenses_returned(
    plan_files: &[Option<Vec<String>>],
    results: &[Option<Value>],
    labels: &[String],
) -> Result<(Vec<Value>, Option<String>), String> {
    let unreturned = unreturned_lenses(plan_files, results, labels);
    let lens_count = plan_files.len();
    if lens_count > 0 && unreturned.len() == lens_count {
        return Err(format!(
            "No lens returned a result ({lens_count} of {lens_count} came back empty, as a session usage limit does), so nothing was reviewed and there is no verdict to give."
        ));
    }
    let log = (!unreturned.is_empty()).then(|| {
        let names: Vec<String> = unreturned
            .iter()
            .map(|l| to_js_string(js::get(l, "lens")))
            .collect();
        format!(
            "{} of {lens_count} lenses returned nothing ({}); the verdict cannot be APPROVE",
            unreturned.len(),
            names.join(", ")
        )
    });
    Ok((unreturned, log))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidates {
    pub lens_findings: Vec<Value>,
    pub lens_not_reached: Vec<String>,
    /// The same gaps with their kinds, for the coverage gate and banner.
    pub lens_gaps: Vec<Gap>,
    pub valid_bot_findings: Vec<Value>,
    pub all_candidates: Vec<Value>,
    pub dropped_pre_panel: Vec<Value>,
    pub reanchored: usize,
    pub logs: Vec<String>,
}

fn put(m: &mut Map<String, Value>, k: &str, v: Option<&Value>) {
    if let Some(v) = v {
        m.insert(k.to_owned(), v.clone());
    }
}

fn or<'a>(a: Option<&'a Value>, b: Option<&'a Value>) -> Option<&'a Value> {
    if js::truthy(a) {
        a
    } else {
        b
    }
}

/// What the lenses and the bot pass produced, through the out-of-diff gate.
/// `results` holds one answer per dispatch, lenses first and the bot pass
/// last; `None` is a dispatch that returned nothing.
///
/// # Errors
/// Where the workflow throws: a lens answer without a `findings` array, or
/// a profile whose risk paths do not compile.
pub fn collect_candidates(
    results: &[Option<Value>],
    labels: &[String],
    has_bot_comments: bool,
    bot_comments: Option<&Value>,
    pr_files: &[String],
    blast_radius: Option<&Value>,
    profile: &Value,
) -> Result<Candidates, String> {
    let mut logs = Vec::new();
    let lens_count = labels.len();
    let mut lens_findings = Vec::new();
    for (r, lens) in results.iter().take(lens_count).zip(labels) {
        let Some(r) = r.as_ref().filter(|r| js::truthy(Some(r))) else {
            continue;
        };
        let Some(Value::Array(findings)) = js::get(r, "findings") else {
            return Err("Cannot read properties of undefined (reading 'map')".to_owned());
        };
        for f in findings {
            let mut f = match f {
                Value::Object(m) => m.clone(),
                _ => Map::new(),
            };
            f.insert("lens".into(), json!(lens));
            f.insert("mergedFrom".into(), json!([]));
            lens_findings.push(Value::Object(f));
        }
    }
    let as_array = Value::from(
        results
            .iter()
            .map(|r| r.clone().unwrap_or(Value::Null))
            .collect::<Vec<_>>(),
    );
    let lens_not_reached = collect_not_reached(&as_array, lens_count);
    let lens_gaps = collect_gaps(results, lens_count);
    if !lens_not_reached.is_empty() {
        logs.push(format!(
            "Lenses reported {} surface(s) not reached within budget",
            lens_not_reached.len()
        ));
    }

    let bot_vetting = if has_bot_comments {
        results.get(lens_count).cloned().flatten()
    } else {
        None
    };
    let mut valid_bot_findings = Vec::new();
    if let Some(bv) = bot_vetting.as_ref().filter(|b| js::truthy(Some(b))) {
        let rules = risk_path_rules(profile)?;
        let comments = js::get(bv, "comments")
            .and_then(Value::as_array)
            .ok_or("Cannot read properties of undefined (reading 'filter')")?;
        for c in comments {
            if js::get(c, "classification").and_then(Value::as_str) != Some("VALID") {
                continue;
            }
            let src = js::get(c, "index")
                .and_then(Value::as_f64)
                .filter(|i| i.fract() == 0.0 && *i >= 0.0)
                .and_then(|i| bot_comments.and_then(|b| b.get(i as usize)))
                .filter(|s| !s.is_null())
                .cloned()
                .unwrap_or_else(|| json!({}));
            let file = match or(js::get(c, "file"), js::get(&src, "path")) {
                Some(f) if js::truthy(Some(f)) => f.clone(),
                _ => json!(""),
            };
            let line = match or(js::get(c, "line"), js::get(&src, "line")) {
                Some(l) if js::truthy(Some(l)) => l.clone(),
                _ => json!(0),
            };
            let mut f = Map::new();
            put(&mut f, "title", js::get(c, "summary"));
            f.insert("file".into(), file.clone());
            f.insert("line".into(), line);
            f.insert("severity".into(), json!("important"));
            put(&mut f, "description", js::get(c, "reason"));
            f.insert(
                "isBehaviorChanging".into(),
                json!(js::get(c, "isBehaviorChanging") == Some(&json!(true))),
            );
            f.insert(
                "touchesHighRiskSurface".into(),
                json!(risk_path_touched(Some(&file), Some(&rules))),
            );
            f.insert(
                "verifiedBy".into(),
                match js::get(c, "verifiedBy") {
                    Some(v @ Value::Array(_)) => v.clone(),
                    _ => json!([]),
                },
            );
            f.insert("lens".into(), json!("bot-comments"));
            f.insert("mergedFrom".into(), json!([]));
            valid_bot_findings.push(Value::Object(f));
        }
    }

    let mut all_candidates = Vec::new();
    let mut dropped_pre_panel = Vec::new();
    for c in lens_findings.iter().chain(&valid_bot_findings) {
        match reanchor_out_of_diff(c, pr_files, blast_radius)? {
            Some(kept) => all_candidates.push(kept),
            None => {
                let mut d = Map::new();
                put(&mut d, "title", js::get(c, "title"));
                put(&mut d, "file", js::get(c, "file"));
                put(&mut d, "severity", js::get(c, "severity"));
                d.insert("reason".into(), json!(OUT_OF_DIFF));
                dropped_pre_panel.push(Value::Object(d));
            }
        }
    }
    let reanchored = all_candidates
        .iter()
        .filter(|c| js::truthy(js::get(c, "reanchored")))
        .count();
    logs.push(format!(
        "{} candidates after pre-panel gate ({} dropped out-of-diff, {reanchored} re-anchored onto a changed file) across {lens_count} lenses",
        all_candidates.len(),
        dropped_pre_panel.len()
    ));
    Ok(Candidates {
        lens_findings,
        lens_not_reached,
        lens_gaps,
        valid_bot_findings,
        all_candidates,
        dropped_pre_panel,
        reanchored,
        logs,
    })
}

/// One reviewer dispatch, ready to run.
#[derive(Debug, Clone)]
pub struct ReviewerTask {
    pub label: String,
    /// The whole prompt, methodology and conventions lines included.
    pub prompt: String,
    pub schema: Value,
}

/// How every reviewer of the run is dispatched.
#[derive(Debug, Clone)]
pub struct ReviewerSetup {
    pub agent: AgentDef,
    pub model: String,
    /// The head worktree when there is one.
    pub cwd: PathBuf,
    pub timeout: Duration,
    /// At most this many dispatches run at once.
    pub concurrency: usize,
    /// The operator's effort, which a Workflow subagent inherits from its
    /// session. `None` leaves the CLI's default.
    pub effort: Option<String>,
}

pub fn lens_task(label: String, prompt: String) -> ReviewerTask {
    ReviewerTask {
        label,
        prompt,
        schema: schemas::findings(),
    }
}

pub fn bot_task(prompt: String) -> ReviewerTask {
    ReviewerTask {
        label: "bot-vetting".into(),
        prompt,
        schema: schemas::bot_vetting(),
    }
}

/// The schema'd answer of a dispatch, with object keys in JavaScript's order
/// so everything downstream sees what the workflow would.
#[must_use]
pub fn answer_of(result: &DispatchResult) -> Option<Value> {
    result.structured().map(js_key_order)
}

/// What the warm-up request asks for. It only has to be a valid, tiny turn.
pub const WARMUP_PROMPT: &str = "Cache warm-up. Return an empty findings list and stop.";

/// How long the warm-up may take before the lenses go ahead without it.
const WARMUP_TIMEOUT: Duration = Duration::from_secs(60);

/// The request a lens makes, minus its label and prompt. The model cache is
/// keyed on everything that comes before the user message (tools, schema,
/// system prompt, model), so a warm-up only warms a lens when it is built here.
fn lens_request(
    setup: &ReviewerSetup,
    label: String,
    prompt: String,
    schema: Value,
) -> DispatchRequest {
    let mut req = DispatchRequest::new(label, prompt, setup.cwd.clone());
    req.system_prompt = Some(setup.agent.body.clone());
    req.model = Some(setup.model.clone());
    req.effort = setup.effort.clone();
    req.tools = setup.agent.tools.clone();
    req.schema = Some(schema);
    req.timeout = setup.timeout;
    req
}

/// Writes the lenses' shared system prompt into the model cache with one small
/// request, so the lenses started after it read it instead of each creating it.
pub async fn warm_cache<A: Agent>(agent: &A, setup: &ReviewerSetup) -> DispatchResult {
    warm_cache_with(agent, setup, schemas::findings()).await
}

/// [`warm_cache`] with the schema the lenses use, which is part of the cached
/// prefix.
pub async fn warm_cache_with<A: Agent>(
    agent: &A,
    setup: &ReviewerSetup,
    schema: Value,
) -> DispatchResult {
    let mut req = lens_request(setup, "cache-warmup".into(), WARMUP_PROMPT.into(), schema);
    req.timeout = WARMUP_TIMEOUT;
    agent.run(&req).await
}

/// `dispatch_all`, after a `warm_cache` when `warm` is set and at least two
/// lenses will share the prompt. A warm-up that fails or times out costs
/// nothing but the cache saving: the lenses run either way. The warm-up's own
/// result comes back separately so the lens results keep their task order.
pub async fn dispatch_lenses<A: Agent>(
    agent: &A,
    setup: &ReviewerSetup,
    tasks: &[ReviewerTask],
    warm: bool,
) -> (Vec<DispatchResult>, Option<DispatchResult>) {
    let lenses = tasks
        .iter()
        .filter(|t| t.label.starts_with("lens:"))
        .count();
    let schema = tasks
        .iter()
        .find(|t| t.label.starts_with("lens:"))
        .map_or_else(schemas::findings, |t| t.schema.clone());
    let warmup = if warm && lenses >= 2 {
        Some(warm_cache_with(agent, setup, schema).await)
    } else {
        None
    };
    (dispatch_all(agent, setup, tasks).await, warmup)
}

/// Runs every task, at most `setup.concurrency` at a time, and returns the
/// results in task order.
pub async fn dispatch_all<A: Agent>(
    agent: &A,
    setup: &ReviewerSetup,
    tasks: &[ReviewerTask],
) -> Vec<DispatchResult> {
    let gate = Arc::new(Semaphore::new(setup.concurrency.max(1)));
    let runs = tasks.iter().map(|t| {
        let gate = Arc::clone(&gate);
        let req = lens_request(setup, t.label.clone(), t.prompt.clone(), t.schema.clone());
        async move {
            let _permit = gate
                .acquire_owned()
                .await
                .expect("semaphore is never closed");
            agent.run(&req).await
        }
    });
    futures_join_all(runs).await
}

async fn futures_join_all<F: std::future::Future>(futs: impl Iterator<Item = F>) -> Vec<F::Output> {
    let mut set = Vec::new();
    for f in futs {
        set.push(Box::pin(f));
    }
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

const OUT_OF_DIFF: &str =
    "out-of-diff (file not in this PR, and nothing ties it to a changed file)";

/// In an incremental run the gate's file list holds only what changed since
/// the baseline, so a finding on a PR file that did not change is dropped as
/// out-of-diff and the reason says the file is not in the PR. Rewrites that
/// reason to say what happened: the file is in the PR, unchanged since the
/// review at `prior_sha`, and so not re-reviewed.
pub fn relabel_unchanged_drops(
    dropped: &mut [Value],
    scoped: &[String],
    all_pr_files: &[String],
    prior_sha: Option<&str>,
) {
    let same =
        |a: &str, b: &str| a == b || a.ends_with(&format!("/{b}")) || b.ends_with(&format!("/{a}"));
    let since = prior_sha.filter(|s| !s.is_empty()).map_or_else(
        || "the earlier review".to_owned(),
        |s| format!("the review at {}", s.chars().take(8).collect::<String>()),
    );
    for d in dropped {
        let Some(obj) = d.as_object_mut() else {
            continue;
        };
        let Some(prefix) = obj
            .get("reason")
            .and_then(Value::as_str)
            .and_then(|r| r.strip_suffix(OUT_OF_DIFF))
            .map(str::to_owned)
        else {
            continue;
        };
        let file = obj.get("file").and_then(Value::as_str).unwrap_or("");
        if file.is_empty()
            || scoped.iter().any(|f| same(f, file))
            || !all_pr_files.iter().any(|f| same(f, file))
        {
            continue;
        }
        obj.insert(
            "reason".into(),
            json!(format!(
                "{prefix}out of scope (in this PR, unchanged since {since}, so this incremental run did not re-review it)"
            )),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drop(file: &str, reason: &str) -> Value {
        json!({"title": "t", "file": file, "severity": "nit", "reason": reason})
    }

    #[test]
    fn a_pr_file_unchanged_since_the_baseline_is_not_called_out_of_the_pr() {
        let critic = format!("completeness-critic gap {OUT_OF_DIFF}");
        let mut d = vec![
            drop("src/billing/statement.ts", OUT_OF_DIFF),
            drop("src/billing/statement.ts", &critic),
            drop("src/other.ts", OUT_OF_DIFF),
            drop("src/changed.ts", OUT_OF_DIFF),
            drop("src/billing/statement.ts", "pre-decided"),
        ];
        let scoped = vec!["src/changed.ts".to_owned()];
        let all = vec![
            "src/changed.ts".to_owned(),
            "src/billing/statement.ts".to_owned(),
        ];
        relabel_unchanged_drops(&mut d, &scoped, &all, Some("e8b4a9e1c0ffee"));
        let unchanged = "out of scope (in this PR, unchanged since the review at e8b4a9e1, so this incremental run did not re-review it)";
        assert_eq!(d[0]["reason"], unchanged);
        assert_eq!(
            d[1]["reason"],
            format!("completeness-critic gap {unchanged}")
        );
        // Not in the PR at all: the original reason is right.
        assert_eq!(d[2]["reason"], OUT_OF_DIFF);
        // In scope: the gate dropped it for another reason; leave it.
        assert_eq!(d[3]["reason"], OUT_OF_DIFF);
        assert_eq!(d[4]["reason"], "pre-decided");
    }

    #[test]
    fn a_full_run_keeps_every_reason() {
        let mut d = vec![drop("src/a.ts", OUT_OF_DIFF), drop("lib/b.ts", OUT_OF_DIFF)];
        let files = vec!["src/a.ts".to_owned()];
        relabel_unchanged_drops(&mut d, &files, &files, None);
        assert_eq!(d[0]["reason"], OUT_OF_DIFF);
        assert_eq!(d[1]["reason"], OUT_OF_DIFF);
    }
}
