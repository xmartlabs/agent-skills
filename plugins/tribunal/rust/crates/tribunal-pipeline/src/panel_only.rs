//! `tribunal panel`: the refutation panel alone, over findings the operator
//! hands in, at a pinned commit of a real PR. It measures the refuters on
//! labelled claims, including false ones no lens would raise.
//!
//! It runs a review's own steps around the panel: SKILL.md's preparation, the
//! Context phase (the PR, the thread's decisions, the pinned checkout) and the
//! Verify phase through [`verify_phase`], with `RunArgs::panel_only` set. The
//! given findings stand in for the lenses' candidates and go through the
//! evidence resolver as lens findings do. They skip every gate that could keep
//! one from a panel: the out-of-diff gate (never applied, it lives in the
//! lens collection), the pre-panel dedup, the grounded typecheck, the severity
//! gate (each finding is a retest target) and the panel ceiling (raised to fit
//! them all). The completeness critic does not run. Nothing is synthesized,
//! no metrics row and no inflight marker is written, and nothing is posted.

use serde_json::{json, Map, Value};
use tribunal_agent::{Agent, DispatchResult};
use tribunal_core::js;
use tribunal_core::profile::high_risk_surface::risk_path_touched;
use tribunal_core::profile::lens_routing::lens_agent_for;
use tribunal_core::profile::profile_lookup::risk_path_rules;

use crate::context::{
    context_phase, prepare, ContextOutcome, Prepared, ReviewContext, RunArgs, Runtime,
};
use crate::metrics::{debate_row, vote_rows};
use crate::prompts::DiffExposure;
use crate::review::Candidates;
use crate::run::{
    resolve_evidence, teardown, verify_phase, DispatchPolicy, ReviewOutcome, VerifyOutcome,
};
use crate::verify::{panel_plan, AGENTS_FIXED_OVERHEAD, AGENTS_PER_PANEL, AGENTS_PER_REFINE};

/// The `lens` a given finding gets when it names none.
pub const DEFAULT_LENS: &str = "panel";
/// The confidence a given finding gets when it states none, as the critic's
/// and the sweep's findings do. The panel's gates read vote confidence, not
/// this.
pub const DEFAULT_CONFIDENCE: f64 = 0.6;
/// What a lens writes as evidence when it has nothing to quote.
pub const NO_EVIDENCE: &str = "No relevant quotes found";

const SEVERITIES: [&str; 3] = ["critical", "important", "nit"];

fn bad(i: usize, f: &Value, what: &str) -> String {
    match js::get(f, "title").and_then(Value::as_str) {
        Some(t) if !t.trim().is_empty() => format!("finding {i} ({t:?}) {what}"),
        _ => format!("finding {i} {what}"),
    }
}

/// Reads `--findings`: a JSON array of findings in the lens shape. `title`
/// and `file` are required; `claim` stands for `description`; the rest gets
/// the defaults a hand-written claim needs (see the README).
///
/// # Errors
/// When the text is not a non-empty JSON array of findings, or a field has the
/// wrong type.
pub fn parse_findings(text: &str) -> Result<Vec<Value>, String> {
    let parsed: Value =
        serde_json::from_str(text).map_err(|e| format!("--findings is not JSON: {e}"))?;
    let Value::Array(items) = parsed else {
        return Err("--findings must be a JSON array of findings".into());
    };
    if items.is_empty() {
        return Err("--findings is an empty array: nothing to put before the panel".into());
    }
    items
        .into_iter()
        .enumerate()
        .map(|(i, f)| normalize(i, f))
        .collect()
}

fn normalize(i: usize, f: Value) -> Result<Value, String> {
    let Value::Object(mut m) = f else {
        return Err(format!("finding {i} is not a JSON object"));
    };
    let shown = Value::Object(m.clone());
    let text = |m: &Map<String, Value>, k: &str| {
        m.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
    };
    let Some(title) = text(&m, "title") else {
        return Err(bad(i, &shown, "has no title"));
    };
    if text(&m, "file").is_none() {
        return Err(bad(i, &shown, "has no file"));
    }
    if m.get("line")
        .is_some_and(|l| !l.is_null() && !l.is_number())
    {
        return Err(bad(i, &shown, "has a line that is not a number"));
    }
    match m.get("severity") {
        None | Some(Value::Null) => {
            m.insert("severity".into(), json!("important"));
        }
        Some(Value::String(s)) if SEVERITIES.contains(&s.as_str()) => {}
        Some(other) => {
            return Err(bad(
                i,
                &shown,
                &format!("has severity {other}; use critical, important or nit"),
            ))
        }
    }
    for k in ["isBehaviorChanging", "touchesHighRiskSurface"] {
        if m.get(k).is_some_and(|b| !b.is_null() && !b.is_boolean()) {
            return Err(bad(
                i,
                &shown,
                &format!("has a {k} that is not true or false"),
            ));
        }
    }
    for k in ["description", "claim", "evidence", "lens"] {
        if m.get(k).is_some_and(|s| !s.is_null() && !s.is_string()) {
            return Err(bad(i, &shown, &format!("has a {k} that is not a string")));
        }
    }
    let description = text(&m, "description")
        .or_else(|| text(&m, "claim"))
        .unwrap_or(title);
    m.insert("description".into(), json!(description));
    if text(&m, "evidence").is_none() {
        m.insert("evidence".into(), json!(NO_EVIDENCE));
    }
    if text(&m, "lens").is_none() {
        m.insert("lens".into(), json!(DEFAULT_LENS));
    }
    if !m.get("isBehaviorChanging").is_some_and(Value::is_boolean) {
        m.insert("isBehaviorChanging".into(), json!(false));
    }
    if !m.get("confidence").is_some_and(Value::is_number) {
        m.insert("confidence".into(), json!(DEFAULT_CONFIDENCE));
    }
    m.insert("mergedFrom".into(), json!([]));
    Ok(Value::Object(m))
}

/// `touchesHighRiskSurface` from the profile's `riskPaths` when the finding
/// does not say, as the critic's findings get it.
fn with_risk_surface(mut f: Value, profile: &Value) -> Result<Value, String> {
    if !js::get(&f, "touchesHighRiskSurface").is_some_and(Value::is_boolean) {
        let rules = risk_path_rules(profile)?;
        let risk = risk_path_touched(js::get(&f, "file"), Some(&rules));
        f["touchesHighRiskSurface"] = json!(risk);
    }
    Ok(f)
}

/// The panel ceiling, raised when a review's would not seat every given
/// finding. Returns the policy to run with and, when it changed, the line
/// that says so.
#[must_use]
pub fn fit_budget(n: usize, policy: &DispatchPolicy) -> (DispatchPolicy, Option<String>) {
    let all = |_: &Value| true;
    let seated = |p: &DispatchPolicy| {
        let probe: Vec<Value> = (0..n)
            .map(|i| json!({"slotId": i, "severity": "important"}))
            .collect();
        panel_plan(&probe, &all, p.max_panels, p.max_agents)
            .bound
            .len()
    };
    if seated(policy) == n {
        return (policy.clone(), None);
    }
    let count = n as f64;
    let reserve = (count * 0.4).floor();
    let agents = (AGENTS_FIXED_OVERHEAD + reserve * AGENTS_PER_REFINE + count * AGENTS_PER_PANEL)
        .ceil()
        + 1.0;
    let mut raised = policy.clone();
    raised.max_panels = raised.max_panels.max(count);
    raised.max_agents = raised.max_agents.max(agents);
    let line = format!(
        "Panel ceiling raised to {} panels and {} agents so all {n} given findings open a panel (a review at {} panels and {} agents would have skipped some for budget).",
        js::number_to_string(raised.max_panels),
        js::number_to_string(raised.max_agents),
        js::number_to_string(policy.max_panels),
        js::number_to_string(policy.max_agents),
    );
    (raised, Some(line))
}

#[derive(Debug)]
pub struct PanelRun {
    pub review: Box<ReviewOutcome>,
    pub verify: Box<VerifyOutcome>,
    /// The ceilings the panel ran under.
    pub policy: DispatchPolicy,
}

/// The panel over `findings` (already through [`parse_findings`]) on a
/// context the Context phase built. The run's trees are removed either way.
///
/// # Errors
/// Where the Verify phase fails, or the profile's `riskPaths` cannot be read.
#[allow(clippy::too_many_arguments)]
pub async fn panel_on_context<A: Agent>(
    agent: &A,
    rt: &Runtime,
    args: &RunArgs,
    policy: &DispatchPolicy,
    prepared: Prepared,
    rc: Box<ReviewContext>,
    findings: Vec<Value>,
    logs: &mut Vec<String>,
) -> Result<PanelRun, String> {
    let args = RunArgs {
        panel_only: true,
        ..args.clone()
    };
    let review = |prepared: Prepared, rc: Box<ReviewContext>, candidates, evidence, lens_agent| {
        Box::new(ReviewOutcome {
            context_seconds: 0.0,
            lenses_seconds: 0.0,
            prepared,
            bot_vetting: None,
            context: rc,
            lens_agent,
            labels: Vec::new(),
            dispatches: Vec::new(),
            warmup: None,
            candidates: Candidates {
                lens_findings: Vec::new(),
                lens_not_reached: Vec::new(),
                lens_gaps: Vec::new(),
                valid_bot_findings: Vec::new(),
                all_candidates: candidates,
                dropped_pre_panel: Vec::new(),
                reanchored: 0,
                logs: Vec::new(),
            },
            lenses_unreturned: Vec::new(),
            evidence,
            exposure: DiffExposure::default(),
            questions: None,
        })
    };
    let prepared_findings: Result<Vec<Value>, String> = findings
        .into_iter()
        .map(|f| with_risk_surface(f, &prepared.profile))
        .collect();
    let lens_agent = lens_agent_for(&prepared.profile).map(str::to_owned);
    let (mut candidates, lens_agent) = match (prepared_findings, lens_agent) {
        (Ok(c), Ok(l)) => (c, l),
        (Err(e), _) | (_, Err(e)) => {
            let empty = review(prepared, rc, Vec::new(), Default::default(), String::new());
            teardown(rt, &empty, None).await;
            return Err(e);
        }
    };
    let evidence = resolve_evidence(&mut candidates, rc.delivery.diff_path.as_deref(), logs);
    let (policy, raised) = fit_budget(candidates.len(), policy);
    logs.extend(raised);
    logs.push(format!(
        "Panel only: {} given finding(s) go to the panel as retest targets; no lens, dedup, grounded gate or completeness critic runs.",
        candidates.len()
    ));
    let review = review(prepared, rc, candidates, evidence, lens_agent);
    let verify = verify_phase(agent, rt, &args, &policy, &review, logs).await;
    teardown(rt, &review, verify.as_ref().ok()).await;
    Ok(PanelRun {
        review,
        verify: Box::new(verify?),
        policy,
    })
}

/// `tribunal panel` from the operator's arguments: the review's preparation
/// and Context phase, then [`panel_on_context`].
///
/// # Errors
/// When `--head-sha` is missing, a step before the panel stops the run, or
/// the panel fails.
pub async fn panel_only<A: Agent>(
    agent: &A,
    rt: &Runtime,
    args: &RunArgs,
    policy: &DispatchPolicy,
    findings: Vec<Value>,
    logs: &mut Vec<String>,
) -> Result<PanelRun, String> {
    if args.head_sha.is_empty() {
        return Err("tribunal panel needs --head-sha: the claims are judged at one commit".into());
    }
    let args = RunArgs {
        panel_only: true,
        preflight: false,
        post: false,
        ..args.clone()
    };
    let prepared = prepare(rt, &args).await?;
    logs.extend(prepared.logs.iter().cloned());
    let rc = match context_phase(agent, rt, &args, &prepared, logs).await? {
        ContextOutcome::Ready(rc) => rc,
        ContextOutcome::Skipped { reason } => {
            return Err(format!("the context phase stopped the run: {reason}"))
        }
        ContextOutcome::Preflight { .. } => {
            return Err("the context phase returned a preflight summary".into())
        }
    };
    panel_on_context(agent, rt, &args, policy, prepared, rc, findings, logs).await
}

/// The kill reason of the first counted `REFUTED` vote, for a finding the
/// panel killed.
fn kill_reason(v: &Value) -> Value {
    if js::truthy(js::get(v, "survived")) {
        return Value::Null;
    }
    js::get(v, "votes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|vote| js::get(vote, "verdict") == Some(&json!("REFUTED")))
        .and_then(|vote| js::get(vote, "killReason"))
        .cloned()
        .unwrap_or(Value::Null)
}

fn field(v: Option<&Value>, k: &str) -> Value {
    v.and_then(|v| js::get(v, k))
        .cloned()
        .unwrap_or(Value::Null)
}

/// What the panel decided for each given finding, in the given order, with
/// the metrics row's field names.
#[must_use]
pub fn finding_results(run: &PanelRun) -> Vec<Value> {
    let given = &run.review.candidates.all_candidates;
    let panel = &run.verify.panel;
    given
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let k = panel
                .panel_input
                .iter()
                .position(|f| js::get(f, "slotId").and_then(Value::as_u64) == Some(i as u64));
            let v = k
                .and_then(|k| panel.verified.get(k))
                .and_then(Option::as_ref);
            let mut m = Map::new();
            for key in ["title", "file", "line", "severity"] {
                m.insert(key.into(), field(Some(g), key));
            }
            if let Some(from) = js::get(g, "lineCorrectedFrom") {
                m.insert("lineCorrectedFrom".into(), from.clone());
            }
            m.insert(
                "panelRan".into(),
                json!(v.is_some_and(|v| js::truthy(js::get(v, "panelRan")))),
            );
            for key in [
                "survived",
                "unverified",
                "borderline",
                "refuteFraction",
                "escalated",
                "refined",
                "refineOutcome",
                "placement",
            ] {
                m.insert(key.into(), field(v, key));
            }
            m.insert("killReason".into(), v.map_or(Value::Null, kill_reason));
            m.insert("votes".into(), v.map_or(json!([]), vote_rows));
            m.insert(
                "failedSlots".into(),
                v.and_then(|v| js::get(v, "failedSlots"))
                    .cloned()
                    .unwrap_or(json!([])),
            );
            m.insert("debate".into(), v.map_or(Value::Null, debate_row));
            Value::Object(m)
        })
        .collect()
}

/// `{given, paneled, killed, unverified}` over [`finding_results`].
#[must_use]
pub fn tally(findings: &[Value]) -> Value {
    let count = |p: &dyn Fn(&Value) -> bool| findings.iter().filter(|f| p(f)).count();
    json!({
        "given": findings.len(),
        "paneled": count(&|f| f["panelRan"] == true),
        "killed": count(&|f| f["panelRan"] == true && f["survived"] == false),
        "unverified": count(&|f| f["unverified"] == true),
    })
}

/// The command's output: one JSON object.
#[must_use]
pub fn report(run: &PanelRun, args: &RunArgs, logs: &[String]) -> Value {
    let findings = finding_results(run);
    let labelled: Vec<(&str, &DispatchResult)> = run
        .verify
        .dispatches
        .iter()
        .map(|(l, d)| (l.as_str(), d))
        .collect();
    let panel = &run.verify.panel;
    json!({
        "pr": args.pr.parse::<u64>().map_or(json!(args.pr), |n| json!(n)),
        "repo": run.review.prepared.repo,
        "headSha": js::get(&run.review.context.pr_context, "headRefOid"),
        "summary": tally(&findings),
        "findings": findings,
        "headPin": run.review.context.head_pin,
        "useCodex": panel.use_codex,
        "useGemini": panel.use_gemini,
        "batchPanels": panel.batch_panels_active,
        "maxPanels": run.policy.max_panels,
        "maxAgents": run.policy.max_agents,
        "panelSkippedForBudget": panel.panel_skipped_for_budget,
        "refinesSkippedForBudget": panel.refines_skipped_for_budget,
        "costBreakerFired": panel.cost_breaker_fired,
        "voteQuoteCheck": panel.vote_quote_check,
        "agentCost": crate::run_metrics::agent_cost(&labelled),
        "logs": logs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ceiling_seats_every_given_finding() {
        let policy = DispatchPolicy::default();
        for n in 1..=300 {
            let (p, _) = fit_budget(n, &policy);
            let probe: Vec<Value> = (0..n)
                .map(|i| json!({"slotId": i, "severity": "nit"}))
                .collect();
            let plan = panel_plan(&probe, &|_| true, p.max_panels, p.max_agents);
            assert_eq!(plan.bound.len(), n, "n = {n}");
        }
        assert_eq!(fit_budget(5, &policy).1, None);
        assert!(fit_budget(60, &policy).1.is_some());
    }

    #[test]
    fn a_hand_written_claim_gets_the_lens_defaults() {
        let got = parse_findings(r#"[{"title": "t", "file": "src/a.ts", "claim": "c"}]"#).unwrap();
        assert_eq!(
            got[0],
            json!({
                "title": "t", "file": "src/a.ts", "claim": "c", "description": "c",
                "severity": "important", "evidence": NO_EVIDENCE, "lens": DEFAULT_LENS,
                "isBehaviorChanging": false, "confidence": DEFAULT_CONFIDENCE, "mergedFrom": [],
            })
        );
        let kept = parse_findings(
            r#"[{"title": "t", "file": "f", "line": 4, "severity": "nit", "description": "d", "evidence": "e", "lens": "security", "isBehaviorChanging": true, "confidence": 0.9}]"#,
        )
        .unwrap();
        for (k, want) in [
            ("line", json!(4)),
            ("severity", json!("nit")),
            ("description", json!("d")),
            ("evidence", json!("e")),
            ("lens", json!("security")),
            ("isBehaviorChanging", json!(true)),
            ("confidence", json!(0.9)),
        ] {
            assert_eq!(kept[0][k], want, "{k}");
        }
        let untitled =
            parse_findings(r#"[{"title": "t", "file": "f", "description": ""}]"#).unwrap();
        assert_eq!(untitled[0]["description"], json!("t"));
    }

    #[test]
    fn malformed_findings_are_refused_with_the_reason() {
        let err = |t: &str| parse_findings(t).unwrap_err();
        assert!(err("{").contains("not JSON"));
        assert!(err(r#"{"title": "t"}"#).contains("JSON array"));
        assert!(err("[]").contains("empty"));
        assert!(err("[1]").contains("finding 0 is not a JSON object"));
        assert!(err(r#"[{"file": "f"}]"#).contains("finding 0 has no title"));
        assert!(err(r#"[{"title": " ", "file": "f"}]"#).contains("has no title"));
        assert!(err(r#"[{"title": "t"}]"#).contains("\"t\") has no file"));
        assert!(err(r#"[{"title": "t", "file": "f", "severity": "major"}]"#).contains("severity"));
        assert!(err(r#"[{"title": "t", "file": "f", "line": "3"}]"#).contains("line"));
        assert!(
            err(r#"[{"title": "t", "file": "f", "isBehaviorChanging": "yes"}]"#)
                .contains("isBehaviorChanging")
        );
    }

    #[test]
    fn the_profile_decides_the_risk_surface_only_when_the_finding_does_not() {
        let profile = json!({"riskPaths": [{"focus": "security", "pattern": "auth/"}]});
        let f =
            |v: Value| with_risk_surface(v, &profile).unwrap()["touchesHighRiskSurface"].clone();
        assert_eq!(f(json!({"file": "src/auth/token.ts"})), json!(true));
        assert_eq!(f(json!({"file": "src/ui/a.ts"})), json!(false));
        assert_eq!(
            f(json!({"file": "src/auth/token.ts", "touchesHighRiskSurface": false})),
            json!(false)
        );
    }
}
