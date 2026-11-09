//! The decisions check that runs when the refutation panel is off.
//!
//! A refuter was the only step that could move a candidate repeating a
//! DEFERRED, BY-DESIGN or SPLIT-OUT decision from the PR's thread into
//! "Pre-decided": it answered `PRE_DECIDED` and quoted the decision. With the
//! panel off by default every candidate survived instead, so an author's
//! answer harvested from the thread no longer kept its finding out of the
//! verdict (BE #2288: a BY-DESIGN reply became the only blocker of a REQUEST
//! CHANGES). This asks one model call the refuter's question for all the
//! candidates at once, and holds each answer to the refuter's own citation
//! gate, `pre_decided_covers_file`.

use std::time::Duration;

use serde_json::{json, Value};
use tribunal_agent::{Agent, DispatchRequest, DispatchResult};
use tribunal_core::js;
use tribunal_core::panel::panel_gates::pre_decided_covers_file;

use crate::agent_def::AgentDef;
use crate::decisions::{serialize_thread_decisions, PRECEDENCE_RULE};
use crate::review::answer_of;
use crate::schemas;

pub const LABEL: &str = "pre-decided-check";

/// A RESOLVED item is left out: it only stands if the fix holds at HEAD, which
/// takes reading the code, and the panel's refuters were the ones that did.
const CITABLE: [&str; 3] = ["DEFERRED", "BY-DESIGN", "SPLIT-OUT"];

/// The thread decisions a candidate may be pre-decided by.
#[must_use]
pub fn citable_decisions(in_thread: Option<&Value>) -> Value {
    let Some(Value::Array(items)) = in_thread else {
        return json!([]);
    };
    Value::Array(
        items
            .iter()
            .filter(|d| {
                js::get(d, "category")
                    .and_then(Value::as_str)
                    .is_some_and(|c| CITABLE.contains(&c.trim().to_uppercase().as_str()))
            })
            .cloned()
            .collect(),
    )
}

fn has_items(v: &Value) -> bool {
    v.as_array().is_some_and(|a| !a.is_empty())
}

/// The prompt, or `None` when there is nothing to check.
#[must_use]
pub fn prompt(candidates: &[Value], citable: &Value, operator_context: &str) -> Option<String> {
    if candidates.is_empty() || !has_items(citable) {
        return None;
    }
    let thread = serialize_thread_decisions(Some(citable));
    let operator = operator_context.trim();
    let operator_block = if operator.is_empty() {
        String::new()
    } else {
        format!("[OPERATOR — explicit --context for this run, highest authority]\n{operator}\n\n")
    };
    let listed: Vec<Value> = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| {
            json!({
                "index": i,
                "file": js::get(c, "file"),
                "line": js::get(c, "line"),
                "title": js::get(c, "title"),
                "description": js::get(c, "description"),
            })
        })
        .collect();
    let findings = serde_json::to_string_pretty(&listed).unwrap_or_default();
    Some(format!(
        "Findings were raised by reviewers of a pull request. The PR's conversation already holds decisions that the author or a reviewer made. For each finding, decide whether it merely repeats one of those decisions rather than raising a defect the decision does not cover.\n\n\
Rules:\n\
- preDecided is true only when the finding's defect IS the subject of a decision below: same file and same mechanism. A finding about the same topic that names a different defect, a different file, or a case the decision does not address is NOT pre-decided.\n\
- When a decision carries a \"[files: ...]\" tag it applies ONLY to those files. A decision with no tag is broad and applies wherever it is genuinely on-topic.\n\
- A finding that only asks the author to confirm a fact the decision already states (for example that production data has a given shape) repeats the decision.\n\
- When preDecided is true, copy the decision's text word for word into decisionQuestioned (the part after the category and before the source). Otherwise leave decisionQuestioned empty.\n\
- When unsure, answer false. A wrong true hides a real defect from the verdict.\n\
- Answer once per finding, by its index, with a one-line reason.\n\n\
{PRECEDENCE_RULE}\n\n\
Decisions:\n{operator_block}[THREAD — harvested from this PR's review conversation]\n{thread}\n\n\
Findings:\n{findings}\n"
    ))
}

/// The result row of a candidate the check found pre-decided, in the shape
/// `partition_results` sorts into `pre_decided`: not survived, `preDecided`,
/// and the citation as a `PRE_DECIDED` vote, where the report reads it.
#[must_use]
pub fn decided_result(candidate: &Value, decision_questioned: &str, reason: &str) -> Value {
    json!({
        "finding": candidate,
        "grounded": {"verdict": null, "diagnostic": null},
        "survived": false,
        "panelRan": false,
        "hadIndependentVoter": false,
        "hadExternalVoter": false,
        "failedSlots": [],
        "failedSlotsDetail": [],
        "discardedVotes": [],
        "unverified": false,
        "borderline": false,
        "refuteFraction": null,
        "votes": [{
            "source": LABEL,
            "verdict": "REFUTED",
            "refutationType": "PRE_DECIDED",
            "decisionQuestioned": decision_questioned,
            "reason": reason,
        }],
        "preDecided": true,
        "debateFired": false,
        "debateOutcome": null,
        "refined": false,
        "refineOutcome": null,
        "escalated": false,
    })
}

/// Splits `candidates` by the answer: the ones that still go on, and the
/// result rows of the ones whose citation holds. An answer that is missing,
/// malformed, or cites a decision the gate rejects keeps the candidate.
#[must_use]
pub fn apply(
    candidates: Vec<Value>,
    answer: Option<&Value>,
    citable: &Value,
    logs: &mut Vec<String>,
) -> (Vec<Value>, Vec<Value>) {
    let items: &[Value] = answer
        .and_then(|a| js::get(a, "items"))
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    let mut kept = Vec::new();
    let mut decided = Vec::new();
    let mut rejected = 0usize;
    for (i, c) in candidates.into_iter().enumerate() {
        let item = items
            .iter()
            .find(|it| js::get(it, "index").and_then(Value::as_f64) == Some(i as f64));
        let says_decided = item
            .and_then(|it| js::get(it, "preDecided"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !says_decided {
            kept.push(c);
            continue;
        }
        let dq = item.and_then(|it| js::get(it, "decisionQuestioned"));
        let quoted = dq.and_then(Value::as_str).map(str::trim).unwrap_or("");
        let file = js::get(&c, "file").and_then(Value::as_str);
        if quoted.is_empty() || !pre_decided_covers_file(dq, file, citable) {
            rejected += 1;
            kept.push(c);
            continue;
        }
        let reason = item
            .and_then(|it| js::get(it, "reason"))
            .and_then(Value::as_str)
            .unwrap_or("");
        decided.push(decided_result(&c, quoted, reason));
    }
    if answer.is_none() {
        logs.push("Pre-decided check: no answer, every candidate goes on".into());
    } else {
        logs.push(format!(
            "Pre-decided check (refutation panel off): {} pre-decided, {} kept{}",
            decided.len(),
            kept.len(),
            if rejected > 0 {
                format!(", {rejected} citation(s) rejected by the file and quote gate")
            } else {
                String::new()
            }
        ));
    }
    (kept, decided)
}

/// Runs the check when there is something to check. Returns the candidates
/// that go on, the pre-decided result rows and the dispatch, when one ran.
#[allow(clippy::too_many_arguments)]
pub async fn check<A: Agent>(
    agent: &A,
    scribe: &AgentDef,
    candidates: Vec<Value>,
    in_thread: Option<&Value>,
    operator_context: &str,
    model: &str,
    effort: Option<&String>,
    cwd: &std::path::Path,
    logs: &mut Vec<String>,
) -> (Vec<Value>, Vec<Value>, Option<DispatchResult>) {
    let citable = citable_decisions(in_thread);
    let Some(text) = prompt(&candidates, &citable, operator_context) else {
        return (candidates, Vec::new(), None);
    };
    let mut req = DispatchRequest::new(LABEL, text, cwd);
    req.system_prompt = Some(scribe.body.clone());
    req.model = Some(model.to_owned());
    req.effort = effort.cloned();
    req.tools = scribe.tools.clone();
    req.schema = Some(schemas::pre_decided());
    req.timeout = Duration::from_secs(10 * 60);
    let res = agent.run(&req).await;
    let answer = answer_of(&res);
    let (kept, decided) = apply(candidates, answer.as_ref(), &citable, logs);
    (kept, decided, Some(res))
}
