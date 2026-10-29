//! The Synthesize phase's glue: the previous review's findings carried or
//! re-checked, the payload the report is written from, the verifiedBy
//! anchors, the wrong-ref audit of refutations, the verdict, the report
//! prompt, and the report as posted. Ports the workflow from
//! `phase("Synthesize")` to `reportAsPosted`. The prompt texts under
//! `synthesis/` are cut from the workflow's own template literals, with each
//! interpolation named `{{name}}`.

use std::collections::HashSet;
use std::path::Path;

use serde_json::{json, Map, Value};
use tribunal_core::evidence::ref_evidence_check::{cited_identifiers, ref_evidence_verdict};
use tribunal_core::js;
use tribunal_core::lens::js_ext::{to_js_string, utf16_prefix_lossy};
use tribunal_core::profile::high_risk_surface::with_risk_path_surface;
use tribunal_core::profile::profile_lookup::risk_path_rules;
use tribunal_core::report::carry_forward::{
    classify_prior_findings, grade_prior_verification, prior_grade, unresolved_prior_blockers_from,
    verdict_for, PriorClassification,
};
use tribunal_core::report::draft_comments_strip::{
    is_draft_heading, strip_draft_comments_from_body,
};
use tribunal_core::report_verdict::{pin_report_verdict, Correction};
use tribunal_core::verified_gate::demote_unanchored;

const REPORT: &str = include_str!("synthesis/report.txt");
const FALLBACK_BLOCK: &str = include_str!("synthesis/fallbackBlock.txt");
const SCOPE_BLOCK: &str = include_str!("synthesis/scopeBlock.txt");
const CLEAN_SWEEP_BLOCK: &str = include_str!("synthesis/cleanSweepBlock.txt");
const REFINE_BLOCK: &str = include_str!("synthesis/refineBlock.txt");
const REFINE_RULE: &str = include_str!("synthesis/refineRule.txt");
const COST_RULE: &str = include_str!("synthesis/costRule.txt");
const SWEEP_RULE: &str = include_str!("synthesis/sweepRule.txt");
const SCOPED_RULES: &str = include_str!("synthesis/scopedRules.txt");
const SCOPE_LINE: &str = include_str!("synthesis/scopeLine.txt");
const CARRIED_FORMAT: &str = include_str!("synthesis/carriedFormat.txt");
const VERIFY_PRIOR: &str = include_str!("synthesis/verify-prior.txt");

pub const PRIOR_VERIFY_CEILING: usize = 6;
pub const IMPORT_ESCALATION_CEILING: usize = 3;
pub const T_KILL: f64 = 0.45;

/// Fills every `{{name}}` in one pass, so a value that itself holds
/// `{{…}}` is left as it is. A name with no value stays as written.
#[must_use]
pub fn render(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(at) = rest.find("{{") {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 2..];
        match tail.find("}}") {
            Some(end) => {
                let name = &tail[..end];
                match vars.iter().find(|(k, _)| *k == name) {
                    Some((_, v)) => out.push_str(v),
                    None => out.push_str(&rest[at..at + 4 + end]),
                }
                rest = &tail[end + 2..];
            }
            None => {
                out.push_str(&rest[at..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn str_of(v: Option<&Value>) -> String {
    to_js_string(v)
}

fn or_str(v: Option<&Value>, fallback: &str) -> String {
    if js::truthy(v) {
        to_js_string(v)
    } else {
        fallback.to_owned()
    }
}

fn slice8(v: Option<&Value>) -> String {
    utf16_prefix_lossy(&to_js_string(v), 8)
}

/// `Array.prototype.join`: `null` and `undefined` elements print as nothing.
fn join(v: Option<&Value>, sep: &str) -> String {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .map(|e| match e {
                Value::Null => String::new(),
                Value::Array(_) => join(Some(e), ","),
                other => to_js_string(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(sep),
        _ => String::new(),
    }
}

fn len_of(v: Option<&Value>) -> usize {
    v.and_then(Value::as_array).map_or(0, Vec::len)
}

fn spread_with(base: &Value, extra: &[(&str, Value)]) -> Value {
    let mut m = base.as_object().cloned().unwrap_or_default();
    for (k, v) in extra {
        m.insert((*k).to_owned(), v.clone());
    }
    Value::Object(m)
}

// ------------------------------------------------------------ prior findings

/// The previous review's findings, sorted into what this run carries, drops
/// as moot, saw again, and must re-check.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PriorCheck {
    pub outcome: PriorClassification,
    pub genuine: Vec<Value>,
    pub heuristic: Vec<Value>,
    pub to_verify: Vec<Value>,
}

/// From `classifyPriorFindings` to the two ceiling logs.
///
/// # Errors
/// Where `classifyPriorFindings` throws.
pub fn prior_check(
    scope: Option<&Value>,
    prior_review: Option<&Value>,
    current: &[Value],
    import_adjacency: &Value,
    logs: &mut Vec<String>,
) -> Result<PriorCheck, String> {
    let empty = json!([]);
    let findings = prior_review
        .filter(|p| js::truthy(Some(p)))
        .and_then(|p| js::get(p, "findings"))
        .filter(|f| js::truthy(Some(f)))
        .unwrap_or(&empty);
    let outcome =
        classify_prior_findings(scope, Some(findings), current, import_adjacency.as_object())?;
    let escalated = |p: &&Value| js::truthy(js::get(p, "escalatedByImportHeuristic"));
    let genuine: Vec<Value> = outcome
        .not_resurfaced
        .iter()
        .filter(|p| !escalated(p))
        .cloned()
        .collect();
    let heuristic: Vec<Value> = outcome
        .not_resurfaced
        .iter()
        .filter(escalated)
        .cloned()
        .collect();
    let to_verify: Vec<Value> = genuine
        .iter()
        .take(PRIOR_VERIFY_CEILING)
        .chain(heuristic.iter().take(IMPORT_ESCALATION_CEILING))
        .cloned()
        .collect();
    if genuine.len() > PRIOR_VERIFY_CEILING {
        logs.push(format!(
            "Prior-finding verification capped at {PRIOR_VERIFY_CEILING} of {} — the rest are reported as unverified, not as fixed.",
            genuine.len()
        ));
    }
    if heuristic.len() > IMPORT_ESCALATION_CEILING {
        logs.push(format!(
            "Import-heuristic escalation capped at {IMPORT_ESCALATION_CEILING} of {} — the rest are reported as unverified, not as fixed.",
            heuristic.len()
        ));
    }
    Ok(PriorCheck {
        outcome,
        genuine,
        heuristic,
        to_verify,
    })
}

/// The `verify-prior` dispatch's prompt for one prior finding.
#[must_use]
pub fn verify_prior_prompt(
    p: &Value,
    prior_review: &Value,
    head: Option<&Value>,
    repo: &str,
    diff_body: &str,
) -> String {
    let why = if js::truthy(js::get(p, "escalatedByImportHeuristic")) {
        let related = js::get(p, "relatedFiles").filter(|r| js::truthy(Some(r)));
        let files = match join(related, ", ") {
            f if f.is_empty() => "a related file".to_owned(),
            f => f,
        };
        format!("The file it lives in has NOT changed since — but this PR DID change {files}, which this file imports (or is imported by), so a fix may have landed there instead of in the finding's own file.")
    } else {
        "The file it lives in HAS CHANGED since, this run re-read it, and this run did NOT raise the finding again.".to_owned()
    };
    render(
        VERIFY_PRIOR,
        &[
            ("why", &why),
            ("head", &str_of(head)),
            ("prior8", &slice8(js::get(prior_review, "sha"))),
            ("severity", &or_str(js::get(p, "severity"), "unknown")),
            ("title", &str_of(js::get(p, "title"))),
            ("file", &str_of(js::get(p, "file"))),
            ("diffBody", diff_body),
            ("repo", repo),
        ],
    )
}

fn cannot_tell(reason: &str) -> Value {
    json!({"verdict": "CANNOT_TELL", "quote": "", "reason": reason})
}

/// A verified prior finding, from the dispatch's answer (`None` when it
/// returned nothing).
#[must_use]
pub fn graded(p: &Value, answer: Option<&Value>) -> Value {
    spread_with(p, &[("verification", grade_prior_verification(answer))])
}

/// A prior finding whose verification dispatch failed outright.
#[must_use]
pub fn dispatch_failed(p: &Value) -> Value {
    spread_with(
        p,
        &[(
            "verification",
            cannot_tell("the verification dispatch failed"),
        )],
    )
}

/// `priorReExamined` and the "re-checked" log line.
#[must_use]
pub fn prior_re_examined(
    verified: &[Value],
    check: &PriorCheck,
    logs: &mut Vec<String>,
) -> Vec<Value> {
    let mut out: Vec<Value> = verified.to_vec();
    out.extend(check.genuine.iter().skip(PRIOR_VERIFY_CEILING).map(|p| {
        spread_with(p, &[("verification", cannot_tell(&format!("not verified — this run's prior-finding verification ceiling ({PRIOR_VERIFY_CEILING}) was reached")))])
    }));
    out.extend(check.heuristic.iter().skip(IMPORT_ESCALATION_CEILING).map(|p| {
        spread_with(p, &[("verification", cannot_tell(&format!("not verified — this run's import-heuristic escalation ceiling ({IMPORT_ESCALATION_CEILING}) was reached")))])
    }));
    if !verified.is_empty() {
        logs.push(format!(
            "Prior findings re-checked: {}",
            verified
                .iter()
                .map(|p| {
                    let file = or_str(js::get(p, "file"), "");
                    format!(
                        "{}={}",
                        file.rsplit('/').next().unwrap_or(""),
                        str_of(js::get(p, "verification").and_then(|v| js::get(v, "verdict")))
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    out
}

// ------------------------------------------------------------- the payload

fn put(m: &mut Map<String, Value>, k: &str, v: Option<&Value>) {
    if let Some(v) = v {
        m.insert(k.to_owned(), v.clone());
    }
}

/// A spread's `k: v`: an `undefined` value hides whatever the base held.
fn set(m: &mut Map<String, Value>, k: &str, v: Option<&Value>) {
    match v {
        Some(v) => {
            m.insert(k.to_owned(), v.clone());
        }
        None => {
            m.shift_remove(k);
        }
    }
}

fn votes_of(v: &Value) -> &[Value] {
    js::get(v, "votes")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// `survivingFindingsForSynthesis`: each survivor's finding with what its
/// panel recorded, through the VERIFIED-GATE.
#[must_use]
pub fn surviving_for_synthesis(surviving: &[Value]) -> Vec<Value> {
    let rows: Vec<Value> = surviving
        .iter()
        .map(|v| {
            let mut m = js::get(v, "finding")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            set(&mut m, "refuteFraction", js::get(v, "refuteFraction"));
            set(&mut m, "refined", js::get(v, "refined"));
            m.insert(
                "refineOutcome".into(),
                js::get(v, "refineOutcome").cloned().unwrap_or(Value::Null),
            );
            set(&mut m, "escalated", js::get(v, "escalated"));
            set(&mut m, "debateFired", js::get(v, "debateFired"));
            set(&mut m, "debateOutcome", js::get(v, "debateOutcome"));
            let used = |s: &str| {
                votes_of(v)
                    .iter()
                    .any(|vote| js::get(vote, "source") == Some(&json!(s)))
            };
            m.insert("codexSlotUsed".into(), json!(used("codex")));
            m.insert("geminiSlotUsed".into(), json!(used("gemini")));
            let proposed = js::get(v, "placement").and_then(|p| js::get(p, "proposedSeverity"));
            m.insert(
                "panelProposedSeverity".into(),
                if js::truthy(proposed) {
                    proposed.cloned().unwrap_or(Value::Null)
                } else {
                    Value::Null
                },
            );
            for k in ["failedSlots", "failedSlotsDetail", "discardedVotes"] {
                let x = js::get(v, k);
                m.insert(
                    k.into(),
                    if js::truthy(x) {
                        x.cloned().unwrap_or(Value::Null)
                    } else {
                        json!([])
                    },
                );
            }
            m.insert(
                "unverified".into(),
                json!(js::truthy(js::get(v, "unverified"))),
            );
            let grounded = js::get(v, "grounded");
            m.insert(
                "grounded".into(),
                if js::truthy(grounded) {
                    grounded.cloned().unwrap_or(Value::Null)
                } else {
                    json!({"verdict": null, "diagnostic": null})
                },
            );
            let confirmed = js::truthy(grounded)
                && grounded.and_then(|g| js::get(g, "verdict")) == Some(&json!("confirmed"));
            m.insert(
                "adjudicated".into(),
                json!(
                    (js::truthy(js::get(v, "panelRan")) && !js::truthy(js::get(v, "unverified")))
                        || confirmed
                ),
            );
            m.insert(
                "votes".into(),
                Value::Array(
                    votes_of(v)
                        .iter()
                        .map(|vote| {
                            let mut o = Map::new();
                            for k in ["verdict", "confidence", "reason"] {
                                put(&mut o, k, js::get(vote, k));
                            }
                            Value::Object(o)
                        })
                        .collect(),
                ),
            );
            Value::Object(m)
        })
        .collect();
    demote_unanchored(&rows)
}

/// The anchor resolver's relay as the workflow receives it.
#[derive(Debug, Clone, PartialEq)]
pub struct Relay {
    pub ok: bool,
    pub data: Option<Value>,
    pub reason: Option<String>,
}

/// The findings whose verifiedBy the resolver checks, and its payload.
#[must_use]
pub fn anchored_payload(findings: &[Value]) -> (usize, String) {
    let anchored: Vec<Value> = findings
        .iter()
        .filter(|f| len_of(js::get(f, "verifiedBy")) > 0)
        .map(|f| {
            let mut m = Map::new();
            put(&mut m, "title", js::get(f, "title"));
            put(&mut m, "verifiedBy", js::get(f, "verifiedBy"));
            Value::Object(m)
        })
        .collect();
    (anchored.len(), js::stringify(&Value::Array(anchored), ""))
}

fn status_of(r: &Value) -> Option<&str> {
    js::get(r, "status").and_then(Value::as_str)
}

/// `verifiedAnchors` from the resolver's relay. `relay` is `None` when the
/// resolver did not run because nothing was anchored or there is no ref.
#[must_use]
pub fn verified_anchors(
    anchored: usize,
    head: Option<&Value>,
    repo: &str,
    relay: Option<&Relay>,
    logs: &mut Vec<String>,
) -> Value {
    if anchored == 0 {
        return json!({"ran": false, "reason": "no-anchored-findings"});
    }
    if !js::truthy(head) {
        logs.push(format!(
            "verifiedBy anchors: {anchored} anchored finding(s) went unresolved because this run has no head ref to read at. NOT a finding that their quotes check out."
        ));
        return json!({"ran": false, "reason": "no-head-ref"});
    }
    let ref8 = slice8(head);
    let out = relay.filter(|r| r.ok).and_then(|r| r.data.as_ref());
    let results = out
        .filter(|o| js::get(o, "ran") == Some(&json!(true)))
        .and_then(|o| js::get(o, "results"))
        .and_then(Value::as_array);
    let (Some(out), Some(results)) = (out, results) else {
        let reason = match out {
            None if relay.is_some_and(|r| r.reason.as_deref().is_some_and(|s| !s.is_empty())) => {
                "relay-not-verified".to_owned()
            }
            None => "dispatch-returned-nothing".to_owned(),
            Some(o) => or_str(js::get(o, "reason"), "no-results-array"),
        };
        logs.push(format!(
            "verifiedBy anchor resolution did not run ({reason}) — {anchored} anchored finding(s) went unchecked. This is NOT a finding that their quotes check out."
        ));
        return json!({"ran": false, "reason": reason});
    };
    let lookup_failed = |r: &Value| matches!(status_of(r), Some("ref-not-found" | "lookup-failed"));
    let broken = js::get(out, "suspectLookup") == Some(&json!(true))
        || results.iter().any(lookup_failed)
        || (results.len() > 1
            && results
                .iter()
                .all(|r| status_of(r) == Some("file-not-at-ref")));
    let nullish = |v: Option<&Value>| v.is_none_or(Value::is_null);
    let mut by_status = Map::new();
    for r in results {
        let key = str_of(js::get(r, "status"));
        let n = by_status.get(&key).and_then(Value::as_u64).unwrap_or(0);
        by_status.insert(key, json!(n + 1));
    }
    let by_status = Value::Object(by_status);
    let unresolved_detail: Vec<Value> = results
        .iter()
        .filter(|r| !matches!(status_of(r), Some("found" | "found-shifted")))
        .map(|r| {
            let mut m = Map::new();
            put(&mut m, "file", js::get(r, "file"));
            put(&mut m, "claimedLine", js::get(r, "claimedLine"));
            put(&mut m, "status", js::get(r, "status"));
            Value::Object(m)
        })
        .collect();
    let anchors = if nullish(js::get(out, "anchors")) {
        json!(results.len())
    } else {
        js::get(out, "anchors").cloned().unwrap_or(Value::Null)
    };
    let pick = |k: &str| {
        js::get(out, k)
            .filter(|v| !v.is_null())
            .cloned()
            .unwrap_or(Value::Null)
    };
    let va = json!({
        "ran": true,
        "ref": head,
        "repo": repo,
        "anchors": anchors,
        "resolved": pick("resolved"),
        "unresolved": pick("unresolved"),
        "suspectLookup": broken,
        "byStatus": by_status,
        "unresolvedDetail": unresolved_detail,
    });
    let unresolved = js::truthy(js::get(&va, "unresolved"));
    let stat = js::stringify(&va["byStatus"], "");
    logs.push(if unresolved && broken {
        format!(
            "verifiedBy anchors: {} of {} did not resolve, and the pattern says the LOOKUP failed, not the anchors ({stat}). Read this as a broken check, not as findings citing missing code — that misreading is what this run's own predecessor produced on PR #2083.",
            str_of(js::get(&va, "unresolved")),
            str_of(js::get(&va, "anchors"))
        )
    } else if unresolved {
        format!(
            "verifiedBy anchors: {} of {} could NOT be found in the code at {ref8} ({stat}). Shadow mode — nothing was demoted, but a blocking finding resting on an anchor nobody can locate is the PR #2042 failure one level up.",
            str_of(js::get(&va, "unresolved")),
            str_of(js::get(&va, "anchors"))
        )
    } else {
        format!(
            "verifiedBy anchors: all {} located in the code at {ref8}.",
            str_of(js::get(&va, "anchors"))
        )
    });
    va
}

/// The four lists the report prompt carries besides the survivors.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SidePayloads {
    pub placed_aside: Vec<Value>,
    pub pre_decided: Vec<Value>,
    pub tech_refuted: Vec<Value>,
    pub debate_outcomes: Vec<Value>,
}

fn finding_field<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    js::get(v, "finding").and_then(|f| js::get(f, k))
}

fn vote_with<'a>(v: &'a Value, key: &str, value: &str) -> Option<&'a Value> {
    votes_of(v)
        .iter()
        .find(|vote| js::get(vote, key) == Some(&json!(value)))
}

#[must_use]
pub fn side_payloads(
    placed_aside: &[Value],
    pre_decided: &[Value],
    tech_refuted: &[Value],
    surviving: &[Value],
) -> SidePayloads {
    let obj = |pairs: Vec<(&str, Option<Value>)>| {
        let mut m = Map::new();
        for (k, v) in pairs {
            if let Some(v) = v {
                m.insert(k.to_owned(), v);
            }
        }
        Value::Object(m)
    };
    let get = |v: Option<&Value>| v.cloned();
    let placed = placed_aside
        .iter()
        .map(|v| {
            let p = js::get(v, "placement");
            obj(vec![
                ("title", get(finding_field(v, "title"))),
                ("file", get(finding_field(v, "file"))),
                (
                    "line",
                    Some(finding_field(v, "line").cloned().unwrap_or(Value::Null)),
                ),
                ("severity", get(finding_field(v, "severity"))),
                ("placement", get(p.and_then(|p| js::get(p, "placement")))),
                ("intendedBy", get(p.and_then(|p| js::get(p, "intendedBy")))),
                ("refuteFraction", get(js::get(v, "refuteFraction"))),
            ])
        })
        .collect();
    let decided = pre_decided
        .iter()
        .map(|v| {
            let q = vote_with(v, "refutationType", "PRE_DECIDED")
                .and_then(|vote| js::get(vote, "decisionQuestioned"));
            let mut o = obj(vec![
                ("title", get(finding_field(v, "title"))),
                (
                    "decisionQuestioned",
                    Some(if js::truthy(q) {
                        q.cloned().unwrap_or(Value::Null)
                    } else {
                        json!("")
                    }),
                ),
            ]);
            let possibly = js::get(v, "possiblyPreDecided");
            if js::truthy(possibly) {
                let p = possibly.unwrap();
                o["possiblyPreDecided"] = obj(vec![
                    ("file", get(finding_field(v, "file"))),
                    ("decisionFiles", get(js::get(p, "files"))),
                    ("decisionSource", get(js::get(p, "decisionSource"))),
                ]);
            }
            o
        })
        .collect();
    let refuted = tech_refuted
        .iter()
        .map(|v| {
            let r = vote_with(v, "verdict", "REFUTED");
            let reason = r.and_then(|r| js::get(r, "reason"));
            let kill = r.and_then(|r| js::get(r, "killReason"));
            obj(vec![
                ("title", get(finding_field(v, "title"))),
                ("refuteFraction", get(js::get(v, "refuteFraction"))),
                (
                    "reason",
                    Some(if js::truthy(reason) {
                        reason.cloned().unwrap_or(Value::Null)
                    } else {
                        json!("")
                    }),
                ),
                (
                    "killReason",
                    Some(if js::truthy(kill) {
                        kill.cloned().unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    }),
                ),
            ])
        })
        .collect();
    let debates = surviving
        .iter()
        .chain(tech_refuted)
        .filter(|v| js::truthy(js::get(v, "debateFired")))
        .map(|v| {
            let r = js::get(v, "debateRationale");
            obj(vec![
                ("title", get(finding_field(v, "title"))),
                ("outcome", get(js::get(v, "debateOutcome"))),
                (
                    "rationale",
                    Some(if js::truthy(r) {
                        r.cloned().unwrap_or(Value::Null)
                    } else {
                        json!("")
                    }),
                ),
            ])
        })
        .collect();
    SidePayloads {
        placed_aside: placed,
        pre_decided: decided,
        tech_refuted: refuted,
        debate_outcomes: debates,
    }
}

/// `baselineDescription`.
#[must_use]
pub fn baseline_description(prior_review: Option<&Value>) -> String {
    let Some(p) = prior_review.filter(|p| js::truthy(Some(p))) else {
        return String::new();
    };
    if js::get(p, "source") == Some(&json!("human-review")) {
        let kind = if js::get(p, "reviewState") == Some(&json!("CHANGES_REQUESTED")) {
            "a CHANGES_REQUESTED"
        } else {
            "an APPROVED"
        };
        return format!(
            "{kind} review by {} at commit {}",
            or_str(js::get(p, "reviewer"), "a reviewer"),
            slice8(js::get(p, "sha"))
        );
    }
    let ts = js::get(p, "ts");
    let on = if js::truthy(ts) {
        format!(" on {}", utf16_prefix_lossy(&str_of(ts), 10))
    } else {
        String::new()
    };
    format!(
        "the review posted at commit {}{on}",
        slice8(js::get(p, "sha"))
    )
}

// ------------------------------------------------------- wrong-ref evidence

/// What the wrong-ref audit greps for: each refutation's cited identifiers.
#[must_use]
pub fn ref_evidence_targets(results: &[Value]) -> (Vec<Value>, Vec<String>) {
    let mut to_check = Vec::new();
    for v in results {
        for vote in votes_of(v) {
            let reason = js::get(vote, "reason");
            if js::get(vote, "verdict") == Some(&json!("REFUTED")) && js::truthy(reason) {
                let cited: Vec<String> = cited_identifiers(reason)
                    .into_iter()
                    .filter(|i| {
                        !i.is_empty()
                            && i.chars()
                                .all(|c| c.is_ascii_alphanumeric() || "_./-".contains(c))
                    })
                    .collect();
                if !cited.is_empty() {
                    let mut t = Map::new();
                    put(&mut t, "file", js::get(v, "file"));
                    t.insert(
                        "source".into(),
                        json!(or_str(js::get(vote, "source"), "claude")),
                    );
                    t.insert("cited".into(), json!(cited));
                    to_check.push(Value::Object(t));
                }
            }
        }
    }
    let mut all: Vec<String> = Vec::new();
    for t in &to_check {
        for c in t["cited"].as_array().unwrap() {
            let c = c.as_str().unwrap_or_default().to_owned();
            if !all.contains(&c) {
                all.push(c);
            }
        }
    }
    (to_check, all)
}

/// `grep -rqF <id> <dir>` for every id: the ones no file under `dir` holds.
/// Symbolic links below `dir` are not followed, as `grep -r` does not.
#[must_use]
pub fn missing_identifiers(dir: &Path, ids: &[String]) -> HashSet<String> {
    let mut missing: Vec<&String> = ids.iter().collect();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if missing.is_empty() {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                stack.push(e.path());
            } else if t.is_file() {
                if let Ok(bytes) = std::fs::read(e.path()) {
                    missing.retain(|id| {
                        let needle = id.as_bytes();
                        needle.is_empty() || !bytes.windows(needle.len()).any(|w| w == needle)
                    });
                }
            }
        }
    }
    missing.into_iter().cloned().collect()
}

/// `refEvidence` once the identifiers were looked up.
#[must_use]
pub fn ref_evidence(
    to_check: &[Value],
    missing: &HashSet<String>,
    head: Option<&Value>,
    logs: &mut Vec<String>,
) -> Value {
    let suspects: Vec<Value> = to_check
        .iter()
        .filter_map(|t| {
            let cited = t["cited"].as_array().unwrap();
            let gone: Vec<Value> = cited
                .iter()
                .filter(|c| missing.contains(c.as_str().unwrap_or_default()))
                .cloned()
                .collect();
            let v = ref_evidence_verdict(true, Some(cited), Some(&gone));
            (v.suspect == Some(true)).then(|| {
                let mut m = Map::new();
                put(&mut m, "file", js::get(t, "file"));
                put(&mut m, "source", js::get(t, "source"));
                m.insert("missing".into(), json!(v.missing));
                Value::Object(m)
            })
        })
        .collect();
    if !suspects.is_empty() {
        logs.push(format!(
            "REF-EVIDENCE: {} of {} refutation(s) cite code absent at {} — possible wrong-ref kill, see the metrics row",
            suspects.len(),
            to_check.len(),
            utf16_prefix_lossy(&or_str(head, ""), 8)
        ));
    }
    json!({"ran": true, "checkedVotes": to_check.len(), "suspectVotes": suspects.len(), "suspects": suspects})
}

// --------------------------------------------------------------- the verdict

#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub unresolved_prior_blockers: Vec<Value>,
    pub raw: &'static str,
    pub sweep_unconfirmed: bool,
    pub guess: &'static str,
}

/// # Errors
/// Where reading the profile's risk paths throws.
pub fn verdict(
    carried: &[Value],
    re_examined: &[Value],
    surviving_for_synthesis: &[Value],
    profile: &Value,
    clean_sweep: Option<&Value>,
    lenses_unreturned: usize,
) -> Result<Verdict, String> {
    let unresolved = unresolved_prior_blockers_from(carried, re_examined);
    let rules = risk_path_rules(profile)?;
    let raw = verdict_for(
        Some(&with_risk_path_surface(
            Some(surviving_for_synthesis),
            Some(&rules),
        )),
        Some(&unresolved.iter().map(prior_grade).collect::<Vec<_>>()),
    );
    let sweep_unconfirmed = clean_sweep.is_some_and(|s| {
        js::truthy(Some(s))
            && (js::truthy(js::get(s, "unparseable"))
                || js::get(s, "skippedReason") == Some(&json!("no-pinned-head")))
    });
    let guess = if raw == "APPROVE" && (sweep_unconfirmed || lenses_unreturned > 0) {
        "COMMENT"
    } else {
        raw
    };
    Ok(Verdict {
        unresolved_prior_blockers: unresolved,
        raw,
        sweep_unconfirmed,
        guess,
    })
}

// ------------------------------------------------------------ the report

/// Everything the report prompt interpolates.
pub struct SynthesisInput<'a> {
    pub pr_number: &'a Value,
    pub pr_context: &'a Value,
    pub repo: &'a str,
    pub incremental_fallback: Option<&'a Value>,
    pub incremental_scope: Option<&'a Value>,
    pub all_pr_files: usize,
    pub baseline_description: &'a str,
    pub prior_review: Option<&'a Value>,
    pub carried_forward: &'a [Value],
    pub moot: &'a [Value],
    pub still_open: &'a [Value],
    pub prior_re_examined: &'a [Value],
    pub surviving: &'a [Value],
    pub prior_reviews: &'a Value,
    pub pre_decided: &'a [Value],
    pub placed_aside: &'a [Value],
    pub tech_refuted: &'a [Value],
    pub dropped_pre_panel: &'a [Value],
    pub grounded_refuted: &'a [Value],
    pub debate_outcomes: &'a [Value],
    pub bot_vetting: Option<&'a Value>,
    pub vet_bot_comments: bool,
    pub critic_output: &'a str,
    pub clean_sweep: Option<&'a Value>,
    pub refines_skipped_for_budget: &'a [Value],
    pub cost_breaker_fired: Option<&'a Value>,
    pub cost_degraded_stages: usize,
    pub verdict: &'a str,
}

fn pretty(v: &[Value], indent: &str) -> String {
    js::stringify(&Value::Array(v.to_vec()), indent)
}

/// The `synthesize` dispatch's prompt.
#[must_use]
pub fn synthesis_prompt(s: &SynthesisInput) -> String {
    let prior = s.prior_review.unwrap_or(&Value::Null);
    let human = js::get(prior, "source") == Some(&json!("human-review"));
    let scope = s.incremental_scope.filter(|x| js::truthy(Some(x)));
    let in_scope = scope.and_then(|x| js::get(x, "inScope"));
    let in_scope_count = len_of(in_scope).to_string();
    let unchanged_count = len_of(scope.and_then(|x| js::get(x, "unchanged"))).to_string();
    let all_count = s.all_pr_files.to_string();

    let fallback = match s.incremental_fallback.filter(|f| js::truthy(Some(f))) {
        Some(f) => render(
            FALLBACK_BLOCK,
            &[
                (
                    "existsPhrase",
                    if js::get(f, "source") == Some(&json!("human-review")) {
                        "A human review exists"
                    } else {
                        "A posted review exists"
                    },
                ),
                ("baseline8", &slice8(js::get(f, "baseline"))),
                ("reason", &str_of(js::get(f, "reason"))),
            ],
        ),
        None => String::new(),
    };
    let scope_block = if scope.is_some() {
        let moot = if s.moot.is_empty() {
            String::new()
        } else {
            format!(
                "\nMOOT — prior findings whose file has left this PR's diff entirely:\n{}",
                pretty(s.moot, " ")
            )
        };
        let still_open = if s.still_open.is_empty() {
            String::new()
        } else {
            format!("\nSTILL OPEN — prior findings on files this run DID re-read, and this run raised the same defect again:\n{}", pretty(s.still_open, " "))
        };
        let re_checked = if s.prior_re_examined.is_empty() {
            String::new()
        } else {
            format!("\nRE-CHECKED — prior findings on files this run DID re-read and did not raise again, each put to a dedicated verifier that read the code at HEAD and answered ONE question: is it fixed? A FIXED or STILL_OPEN verdict is only recorded when it came with a word-for-word quote; anything else was downgraded to CANNOT_TELL:\n{}\nReport each by its verification.verdict, and quote the verification.quote. FIXED means a verifier read the code and pointed at the fix — say so, and say what closed it. CANNOT_TELL is NOT a fix: report it as unresolved with the verifier's reason.", pretty(s.prior_re_examined, " "))
        };
        render(
            SCOPE_BLOCK,
            &[
                ("inScopeCount", &in_scope_count),
                ("allCount", &all_count),
                ("baselineDescription", s.baseline_description),
                ("unchangedCount", &unchanged_count),
                (
                    "humanNote",
                    if human {
                        " This baseline is a human review, not an earlier tribunal run: there are no prior findings to carry forward or re-verify, only a narrower file list."
                    } else {
                        ""
                    },
                ),
                (
                    "inScopeList",
                    &or_str(Some(&json!(join(in_scope, ", "))), "(none)"),
                ),
                (
                    "carried",
                    &if s.carried_forward.is_empty() {
                        "(none)".to_owned()
                    } else {
                        pretty(s.carried_forward, " ")
                    },
                ),
                ("moot", &moot),
                ("stillOpen", &still_open),
                ("reChecked", &re_checked),
            ],
        )
    } else {
        String::new()
    };
    let clean_sweep = s.clean_sweep.filter(|c| js::truthy(Some(c)));
    let clean_sweep_block = clean_sweep.map_or(String::new(), |c| {
        render(
            CLEAN_SWEEP_BLOCK,
            &[("cleanSweep", &js::stringify(c, "  "))],
        )
    });
    let refine_block = if s.refines_skipped_for_budget.is_empty() {
        String::new()
    } else {
        render(
            REFINE_BLOCK,
            &[(
                "refinesSkipped",
                &pretty(s.refines_skipped_for_budget, "  "),
            )],
        )
    };
    let refine_rule = if s.refines_skipped_for_budget.is_empty() {
        ""
    } else {
        REFINE_RULE
    };
    let cost_rule = match s.cost_breaker_fired.filter(|c| js::truthy(Some(c))) {
        Some(c) => render(
            COST_RULE,
            &[
                ("reason", &str_of(js::get(c, "reason"))),
                ("stages", &s.cost_degraded_stages.to_string()),
                ("spent", &str_of(js::get(c, "spentOutput"))),
            ],
        ),
        None => String::new(),
    };
    let sweep_rule = if clean_sweep.is_some() {
        SWEEP_RULE
    } else {
        ""
    };
    let scoped_rules = if scope.is_some() { SCOPED_RULES } else { "" };
    let scope_line = if scope.is_some() {
        render(
            SCOPE_LINE,
            &[
                ("inScopeCount", &in_scope_count),
                ("allCount", &all_count),
                ("baselineDescription", s.baseline_description),
                ("unchangedCount", &unchanged_count),
                (
                    "carriedPhrase",
                    &if human {
                        "this baseline is a human review, so there are no prior findings to carry forward".to_owned()
                    } else {
                        format!(
                            "{} finding(s) from that review are carried forward below",
                            s.carried_forward.len()
                        )
                    },
                ),
            ],
        )
    } else {
        String::new()
    };
    let carried_format = if scope.is_some() {
        render(CARRIED_FORMAT, &[("sha8", &slice8(js::get(prior, "sha")))])
    } else {
        String::new()
    };
    let grounded_refuted: Vec<Value> = s
        .grounded_refuted
        .iter()
        .map(|c| {
            let file = js::get(c, "finding").and_then(|f| js::get(f, "file"));
            let mut m = Map::new();
            put(
                &mut m,
                "title",
                js::get(c, "finding").and_then(|f| js::get(f, "title")),
            );
            put(&mut m, "file", file);
            m.insert(
                "reason".into(),
                json!(format!(
                    "typecheck clean at {} — compile-level claim not reproduced",
                    str_of(file)
                )),
            );
            Value::Object(m)
        })
        .collect();
    let bot_vetted = match s.bot_vetting.filter(|b| js::truthy(Some(b))) {
        Some(b) => {
            js::get(b, "comments").map_or("undefined".to_owned(), |c| js::stringify(c, "  "))
        }
        None if s.vet_bot_comments => "(none)".to_owned(),
        None => "(turned off with --no-bot-comments)".to_owned(),
    };
    render(
        REPORT,
        &[
            ("prNumber", &str_of(Some(s.pr_number))),
            ("title", &str_of(js::get(s.pr_context, "title"))),
            ("author", &str_of(js::get(s.pr_context, "author"))),
            ("jira", &or_str(js::get(s.pr_context, "jiraTicket"), "none")),
            ("repo", s.repo),
            ("fallbackBlock", &fallback),
            ("scopeBlock", &scope_block),
            ("surviving", &pretty(s.surviving, "  ")),
            ("priorReviews", &js::stringify(s.prior_reviews, "  ")),
            ("preDecided", &pretty(s.pre_decided, "  ")),
            ("placedAside", &pretty(s.placed_aside, "  ")),
            ("techRefuted", &pretty(s.tech_refuted, "  ")),
            ("droppedPrePanel", &pretty(s.dropped_pre_panel, "  ")),
            ("groundedRefuted", &pretty(&grounded_refuted, "  ")),
            ("debateOutcomes", &pretty(s.debate_outcomes, "  ")),
            ("botVetted", &bot_vetted),
            (
                "critic",
                if s.critic_output.is_empty() {
                    "(not run — small PR)"
                } else {
                    s.critic_output
                },
            ),
            ("cleanSweepBlock", &clean_sweep_block),
            ("refineBlock", &refine_block),
            ("refineRule", refine_rule),
            ("costRule", &cost_rule),
            ("sweepRule", sweep_rule),
            ("scopedRules", scoped_rules),
            ("verdict", s.verdict),
            ("scopeLine", &scope_line),
            ("tKill", &js::number_to_string(T_KILL)),
            ("carriedFormat", &carried_format),
        ],
    )
}

const HEAD_PIN_DEGRADED_BANNER: &str = "\n\n---\n\n⚠️ **No pinned checkout on this run.** Source reads were pinned by prompt instruction only, not by a checkout at the reviewed commit. Measured unpinned-read rates under that arrangement: 3% for lenses, 5% for refuters. A finding killed on a wrong-ref read leaves no trace here, so treat refutations in this report as less settled than usual.";

fn ref_evidence_banner(ref_evidence: &Value, head: Option<&Value>) -> String {
    if !js::truthy(js::get(ref_evidence, "ran"))
        || js::get(ref_evidence, "suspectVotes").and_then(Value::as_f64) == Some(0.0)
    {
        return String::new();
    }
    let lines: Vec<String> = js::get(ref_evidence, "suspects")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .map(|s| {
            let missing: Vec<String> = js::get(s, "missing")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .map(|m| format!("`{}`", str_of(Some(m))))
                .collect();
            format!(
                "  - `{}` ({}) cites: {}",
                str_of(js::get(s, "file")),
                str_of(js::get(s, "source")),
                missing.join(", ")
            )
        })
        .collect();
    format!(
        "\n\n---\n\n⚠️ **{} of {} refutation(s) cite code that is not in the reviewed commit.** Each one below refuted a finding while naming an identifier absent at `{}` — the signature of a reviewer reading a different checkout. Re-check these before trusting the kill:\n\n{}\n\nAn audit flag, not a verdict: an identifier can also be missing because it names a library type this repo never mentions.",
        str_of(js::get(ref_evidence, "suspectVotes")),
        str_of(js::get(ref_evidence, "checkedVotes")),
        utf16_prefix_lossy(&or_str(head, ""), 8),
        lines.join("\n")
    )
}

/// Ports `lensesUnreturnedBanner`: the lenses whose dispatch came back empty
/// and what they were given.
#[must_use]
pub fn lenses_unreturned_banner(unreturned: &[Value], lens_count: usize) -> String {
    if unreturned.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = unreturned
        .iter()
        .map(|l| {
            let files = match js::get(l, "files") {
                Some(Value::Array(files)) => files
                    .iter()
                    .map(|f| format!("`{}`", to_js_string(Some(f))))
                    .collect::<Vec<_>>()
                    .join(", "),
                _ => "the whole diff".to_owned(),
            };
            format!("  - {}: {files}", to_js_string(js::get(l, "lens")))
        })
        .collect();
    format!(
        "\n\n---\n\n⚠️ **{} of {lens_count} lens(es) returned nothing** (a crash, a timeout or a usage limit), so nobody reviewed what they were given. This review cannot approve the PR for that reason:\n\n{}",
        unreturned.len(),
        lines.join("\n")
    )
}

/// Ports `withBanners`: the banners go above the `### Draft comments`
/// section, which the posting strip removes up to the next heading.
#[must_use]
pub fn with_banners(report: &str, banners: &str) -> String {
    if banners.is_empty() {
        return report.to_owned();
    }
    let lines: Vec<&str> = report.split('\n').collect();
    let Some(start) = lines.iter().position(|l| is_draft_heading(l)) else {
        return format!("{report}{banners}");
    };
    let head = lines[..start].join("\n");
    format!(
        "{}{banners}\n\n{}",
        head.trim_end_matches(js::is_js_whitespace),
        lines[start..].join("\n")
    )
}

/// The report with its verdict line pinned, and the body as posted.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub report: String,
    pub corrected: Option<Correction>,
    pub as_posted: String,
}

/// From `reportVerdictPin` to `reportAsPosted`.
///
/// # Errors
/// When the synthesis returned no text, where the workflow throws.
#[allow(clippy::too_many_arguments)]
pub fn finish_report(
    report: Option<&str>,
    verdict: &str,
    head_pin_ok: bool,
    ref_evidence: &Value,
    head: Option<&Value>,
    lenses_unreturned: &[Value],
    lens_count: usize,
    logs: &mut Vec<String>,
) -> Result<Report, String> {
    let Some(text) = report else {
        return Err("the synthesis returned no report".into());
    };
    let (text, corrected) = if js::trim(text).is_empty() {
        (text.to_owned(), None)
    } else {
        let p = pin_report_verdict(Some(text), verdict);
        (p.text, p.corrected)
    };
    if let Some(c) = &corrected {
        logs.push(format!(
            "Report verdict line rewritten to the harness verdict {verdict} (the report said {}, {})",
            c.from.as_deref().filter(|f| !f.is_empty()).unwrap_or("nothing readable"),
            c.reason
        ));
    }
    let banners = format!(
        "{}{}{}",
        if head_pin_ok {
            ""
        } else {
            HEAD_PIN_DEGRADED_BANNER
        },
        ref_evidence_banner(ref_evidence, head),
        lenses_unreturned_banner(lenses_unreturned, lens_count)
    );
    let text = with_banners(&text, &banners);
    let as_posted = strip_draft_comments_from_body(&text);
    Ok(Report {
        report: text,
        corrected,
        as_posted,
    })
}
