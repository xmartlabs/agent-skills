//! The run's metrics row: what the workflow builds as `metrics` at the end of
//! the Synthesize phase, field for field and in its key order, from the state
//! every phase left behind. `references/metrics-schema.md` documents it.

use serde::Deserialize;
use serde_json::{json, Map, Value};
use tribunal_core::js;
use tribunal_core::lens::js_ext::{to_js_string, utf16_prefix_lossy};
use tribunal_core::lens::lens_yield::build_lens_yield;
use tribunal_core::report::merged_locations_check::merged_locations_missing_from_report_text;
use tribunal_core::report::report_fidelity_checks::{
    grounded_diagnostic_missing_from_report_text, kill_reason_missing_from_report_text,
    merged_from_annotation_missing_from_report_text, unexamined_wording_in_report_text,
};
use tribunal_core::verified_gate::demote_unanchored;
use tribunal_core::votes::js_ext::number_value;

use crate::decisions::is_retest_target;
use crate::panel::CLAUDE_BATCH_SIZE;
use crate::synthesize::T_KILL;
use crate::verify::{AGENTS_FIXED_OVERHEAD, AGENTS_PER_PANEL, AGENTS_PER_REFINE};

pub const LENS_TOOL_CALL_BUDGET: f64 = 40.0;

/// The state the row is read from, named as the workflow names it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MetricsScope {
    pub results: Vec<Value>,
    pub tokens_at_start: f64,
    pub tokens_at_context_end: f64,
    pub tokens_at_review_end: f64,
    pub tokens_at_verify_end: f64,
    pub tokens_at_end: f64,
    pub counter_resets: f64,
    pub pr_context: Value,
    pub diff_mode: Value,
    pub lens_receiver_count: f64,
    pub refuter_receiver_count: f64,
    pub sectioning_available: bool,
    pub section_count: f64,
    pub sectioned_receiver_count: f64,
    pub full_diff_receiver_count: f64,
    pub delivered_bytes: f64,
    pub unsectioned_files: Value,
    pub diff_unparsed_headers: Value,
    pub repo: String,
    pub pr_number: Value,
    pub platform: Value,
    pub ref_evidence: Value,
    pub head_pin: Value,
    pub pinned_sha: Value,
    pub file_count: f64,
    pub lens_plan: Value,
    pub lens_agent: String,
    pub profile_sha256: Value,
    pub profile_source: Value,
    pub project_rules_sha256: Value,
    pub project_rules_source: Value,
    pub conventions_manifest: Value,
    pub lens_labels: Value,
    pub review_results: Value,
    pub lens_findings: Value,
    pub lenses_override: Value,
    pub file_partition_mode: Value,
    pub per_group_churn: Value,
    pub use_codex_head: bool,
    pub external_probe_answered: Value,
    pub use_gemini_head: bool,
    pub external_cwd: Value,
    pub clean_sweep: Value,
    pub verified_anchors: Value,
    pub vote_quote_check: Value,
    pub batch_panels_active: bool,
    pub vet_bot_comments: bool,
    pub verdict_guess: String,
    pub report_verdict_corrected: Value,
    pub all_candidates: f64,
    pub placed_aside: f64,
    pub dropped_pre_panel: Vec<Value>,
    pub blast_relay: bool,
    pub blast_radius: Value,
    pub deduped_pre_panel: f64,
    pub vote_corrections: Vec<Value>,
    pub cross_file_clusters_rejected: f64,
    pub proximity_vetoed: f64,
    pub grounded_available: bool,
    pub grounded_confirmed: f64,
    pub grounded_refuted: f64,
    pub evidence_resolver_ran: bool,
    pub evidence_located_count: f64,
    pub evidence_line_corrected_count: f64,
    pub evidence_not_found_count: f64,
    pub critic_output: String,
    pub critic_gaps: f64,
    pub relays: Value,
    pub prior_reviews: Vec<Value>,
    pub prior_review_gate: Value,
    pub fidelity_critic: Value,
    pub retest_claims: Vec<String>,
    pub report_as_posted: String,
    pub group_count: f64,
    pub churn: f64,
    pub uncovered_files: Value,
    pub lens_not_reached: Vec<Value>,
    pub incremental_scope: Value,
    pub prior_review: Value,
    pub incremental_fallback: Value,
    pub all_pr_files: f64,
    pub carried_forward: f64,
    pub prior_outcome: Value,
    pub heuristic_escalated: f64,
    pub prior_re_examined: Vec<Value>,
    pub max_panels: f64,
    pub panel_skipped_for_budget: Value,
    pub tiebreak: Value,
    pub cost_breaker_fired: Value,
    pub output_ceiling: f64,
    pub budget_total: Value,
    pub cost_degraded_stages: Value,
    pub max_agents: f64,
    pub agents_projected: f64,
    pub agents_projected_unbatched: f64,
    pub round0_claude_batch_savings: f64,
    pub refine_claude_batch_savings: f64,
    pub max_refines: f64,
    pub refines_skipped_for_budget: Value,
    pub external_slots: f64,
    pub artifact_dir: String,
}

fn n(x: f64) -> Value {
    number_value(x)
}

fn get<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    js::get(v, k)
}

/// `x ?? null`.
fn or_null(v: Option<&Value>) -> Value {
    v.filter(|v| !v.is_null()).cloned().unwrap_or(Value::Null)
}

/// `x || fallback`.
fn or(v: Option<&Value>, fallback: Value) -> Value {
    if js::truthy(v) {
        v.cloned().unwrap_or(Value::Null)
    } else {
        fallback
    }
}

fn votes(v: &Value) -> &[Value] {
    get(v, "votes")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn claude_verdict(v: &Value) -> Option<&'static str> {
    let claude: Vec<&Value> = votes(v)
        .iter()
        .filter(|vote| get(vote, "source") == Some(&json!("claude")))
        .collect();
    if claude.is_empty() {
        return None;
    }
    let refuted = claude
        .iter()
        .filter(|vote| get(vote, "verdict") == Some(&json!("REFUTED")))
        .count();
    let kept = claude.len() - refuted;
    if refuted == kept {
        None
    } else if refuted > kept {
        Some("REFUTED")
    } else {
        Some("NOT_REFUTED")
    }
}

fn external_vote<'a>(v: &'a Value, family: &str) -> Option<&'a Value> {
    votes(v)
        .iter()
        .find(|vote| get(vote, "source") == Some(&json!(family)))
}

fn any_disagreement(paneled: &[&Value], f: impl Fn(&Value) -> Option<bool>) -> Value {
    let defined: Vec<bool> = paneled.iter().filter_map(|v| f(v)).collect();
    if defined.is_empty() {
        Value::Null
    } else {
        json!(defined.iter().any(|b| *b))
    }
}

/// `mergeNotChecked`: every entry as a string, falsy ones dropped, first
/// occurrence kept.
fn merge_not_checked(lists: &[&Value]) -> Vec<Value> {
    let mut out: Vec<String> = Vec::new();
    for list in lists {
        for e in list.as_array().map_or(&[][..], Vec::as_slice) {
            let s = to_js_string(Some(e));
            if !s.is_empty() && !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out.into_iter().map(Value::String).collect()
}

fn adjudicated(v: &Value) -> bool {
    let grounded = get(v, "grounded");
    (js::truthy(get(v, "panelRan")) && !js::truthy(get(v, "unverified")))
        || (js::truthy(grounded)
            && grounded.and_then(|g| get(g, "verdict")) == Some(&json!("confirmed")))
}

/// `v.survived && severity !== "nit" && ((v.panelRan && !v.unverified) ||
/// (v.grounded && v.grounded.verdict === "confirmed"))`, which returns an
/// operand: `None` is `undefined`, and the key is then left out.
fn blocking(v: &Value, f: &Value) -> Option<Value> {
    let survived = get(v, "survived");
    if !js::truthy(survived) {
        return survived.cloned();
    }
    if get(f, "severity") == Some(&json!("nit")) {
        return Some(json!(false));
    }
    let ran = get(v, "panelRan");
    let first = if js::truthy(ran) {
        Some(json!(!js::truthy(get(v, "unverified"))))
    } else {
        ran.cloned()
    };
    if js::truthy(first.as_ref()) {
        return first;
    }
    let grounded = get(v, "grounded");
    if !js::truthy(grounded) {
        return grounded.cloned();
    }
    Some(json!(
        grounded.and_then(|g| get(g, "verdict")) == Some(&json!("confirmed"))
    ))
}

/// `verifiedGateNoteFor`: the note the evidence gate puts on a surviving
/// finding it demotes, or `None`. The report reads the demoted copy; the row
/// is built from the verdicts, which the gate never touched (issue #46).
fn verified_gate_note(v: &Value) -> Option<Value> {
    if !js::truthy(get(v, "survived")) {
        return None;
    }
    let mut f = get(v, "finding")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    f.insert("adjudicated".into(), json!(adjudicated(v)));
    demote_unanchored(&[Value::Object(f)])
        .into_iter()
        .next()
        .and_then(|d| get(&d, "verifiedByGateNote").cloned())
}

fn finding_row(v: &Value, s: &MetricsScope) -> Result<Value, String> {
    let f = get(v, "finding").cloned().unwrap_or(Value::Null);
    let fg = |k: &str| get(&f, k);
    let grounded = or(
        get(v, "grounded"),
        json!({"verdict": null, "diagnostic": null}),
    );
    let report = s.report_as_posted.as_str();
    let merged_locations = or(fg("mergedLocations"), json!([]));
    let merged_from = or(fg("mergedFrom"), json!([]));
    let refuted_vote = votes(v)
        .iter()
        .find(|vote| get(vote, "verdict") == Some(&json!("REFUTED")));
    let line_corrected_from = fg("lineCorrectedFrom").filter(|x| js::is_integer(Some(x)));
    let resolution = fg("evidenceResolution").filter(|r| js::truthy(Some(r)));
    let refined = js::truthy(get(v, "refined"));
    let mut m = Map::new();
    let mut put = |k: &str, x: Value| {
        m.insert(k.to_owned(), x);
    };
    put("title", fg("title").cloned().unwrap_or(Value::Null));
    put("file", fg("file").cloned().unwrap_or(Value::Null));
    put("severity", fg("severity").cloned().unwrap_or(Value::Null));
    put(
        "severityAtIntake",
        get(v, "severityAtIntake")
            .filter(|x| !x.is_null())
            .or(fg("severity"))
            .cloned()
            .unwrap_or(Value::Null),
    );
    put(
        "refineProposedSeverity",
        or_null(fg("refineProposedSeverity")),
    );
    put(
        "touchesHighRiskSurface",
        json!(js::truthy(fg("touchesHighRiskSurface"))),
    );
    put(
        "isBehaviorChanging",
        json!(js::truthy(fg("isBehaviorChanging"))),
    );
    put("lens", or(fg("lens"), json!("unknown")));
    put("reanchored", or_null(fg("reanchored")));
    put("evidence", or(fg("evidence"), Value::Null));
    put("line", or_null(fg("line")));
    let gate_note = verified_gate_note(v);
    let unexamined_wording = unexamined_wording_in_report_text(
        &json!({
            "file": fg("file").cloned().unwrap_or(Value::Null),
            "line": or_null(fg("line")),
            "votes": get(v, "votes").cloned().unwrap_or(Value::Null),
            "verifiedByGateNote": gate_note.clone().unwrap_or(Value::Null),
        }),
        report,
    );
    if let Some(b) = blocking(v, &f) {
        // A demoted finding is not blocking: the report lists it as an open
        // question, and the row says what was posted.
        let b = if js::truthy(Some(&b)) {
            json!(gate_note.is_none())
        } else {
            b
        };
        put("blocking", b);
    }
    put("adjudicated", json!(adjudicated(v)));
    put("verifiedByGateNote", gate_note.unwrap_or(Value::Null));
    put("posted", json!(false));
    put("operatorOverride", Value::Null);
    put(
        "panelRan",
        get(v, "panelRan").cloned().unwrap_or(Value::Null),
    );
    put("forcedPanel", json!(is_retest_target(&s.retest_claims, &f)));
    put(
        "refuteFraction",
        get(v, "refuteFraction").cloned().unwrap_or(Value::Null),
    );
    put("threshold", n(T_KILL));
    put("borderline", json!(js::truthy(get(v, "borderline"))));
    put(
        "survived",
        get(v, "survived").cloned().unwrap_or(Value::Null),
    );
    put("refined", get(v, "refined").cloned().unwrap_or(Value::Null));
    put("refineOutcome", or_null(get(v, "refineOutcome")));
    put("escalated", json!(js::truthy(get(v, "escalated"))));
    put("mergedFrom", merged_from.clone());
    put("mergedLocations", merged_locations.clone());
    put(
        "mergedLocationsMissingFromReport",
        json!(merged_locations_missing_from_report_text(
            Some(&merged_locations),
            report
        )?),
    );
    put(
        "killReasonMissingFromReport",
        json!(
            refuted_vote.map_or_else(Vec::new, |vote| kill_reason_missing_from_report_text(
                get(vote, "killReason"),
                report
            ))
        ),
    );
    put(
        "mergedFromAnnotationMissingFromReport",
        json!(merged_from_annotation_missing_from_report_text(
            Some(&merged_from),
            report
        )),
    );
    put("grounded", grounded.clone());
    put(
        "groundedDiagnosticMissingFromReport",
        json!(grounded_diagnostic_missing_from_report_text(
            Some(&grounded),
            report
        )),
    );
    put("unexaminedWordingInReport", json!(unexamined_wording));
    put(
        "evidenceLocated",
        resolution.map_or(Value::Null, |r| {
            get(r, "evidenceLocated").cloned().unwrap_or(Value::Null)
        }),
    );
    put(
        "lineCorrected",
        if resolution.is_some() {
            json!(line_corrected_from.is_some())
        } else {
            Value::Null
        },
    );
    if let Some(from) = line_corrected_from {
        put("lineCorrectedFrom", from.clone());
    }
    put(
        "evidenceResolution",
        resolution.cloned().unwrap_or(Value::Null),
    );
    put("unverified", json!(js::truthy(get(v, "unverified"))));
    put(
        "hadIndependentVoter",
        get(v, "hadIndependentVoter")
            .cloned()
            .unwrap_or(Value::Null),
    );
    put(
        "hadExternalVoter",
        get(v, "hadExternalVoter").cloned().unwrap_or(Value::Null),
    );
    put("failedSlots", or(get(v, "failedSlots"), json!([])));
    put(
        "failedSlotsDetail",
        or(get(v, "failedSlotsDetail"), json!([])),
    );
    put(
        "possiblyPreDecided",
        or(get(v, "possiblyPreDecided"), Value::Null),
    );
    put("placement", or(get(v, "placement"), Value::Null));
    put("discardedVotes", or(get(v, "discardedVotes"), json!([])));
    put(
        "votes",
        Value::Array(
            votes(v)
                .iter()
                .map(|vote| {
                    let g = |k: &str| get(vote, k);
                    let pre = g("refutationType") == Some(&json!("PRE_DECIDED"));
                    let mut o = Map::new();
                    o.insert("source".into(), or(g("source"), json!("claude")));
                    o.insert(
                        "verdict".into(),
                        g("verdict").cloned().unwrap_or(Value::Null),
                    );
                    o.insert(
                        "confidence".into(),
                        g("confidence").cloned().unwrap_or(Value::Null),
                    );
                    o.insert(
                        "confidenceDeclared".into(),
                        or_null(g("confidenceDeclared")),
                    );
                    o.insert("coreStands".into(), json!(js::truthy(g("coreStands"))));
                    o.insert("reason".into(), or(g("reason"), json!("")));
                    o.insert(
                        "refutationType".into(),
                        or(g("refutationType"), Value::Null),
                    );
                    o.insert("round".into(), json!(if refined { 1 } else { 0 }));
                    o.insert(
                        "fromEscalation".into(),
                        json!(js::truthy(g("fromEscalation"))),
                    );
                    o.insert("killReason".into(), or(g("killReason"), Value::Null));
                    o.insert(
                        "introducedByPr".into(),
                        or(g("introducedByPr"), Value::Null),
                    );
                    o.insert("contradiction".into(), or(g("contradiction"), Value::Null));
                    o.insert(
                        "quoteFound".into(),
                        match g("quoteFound") {
                            Some(Value::Bool(b)) => json!(b),
                            _ => Value::Null,
                        },
                    );
                    o.insert(
                        "proposedSeverity".into(),
                        or(g("proposedSeverity"), Value::Null),
                    );
                    o.insert("intendedBy".into(), or(g("intendedBy"), Value::Null));
                    o.insert(
                        "decisionQuestioned".into(),
                        if pre {
                            or(g("decisionQuestioned"), json!(""))
                        } else {
                            Value::Null
                        },
                    );
                    Value::Object(o)
                })
                .collect(),
        ),
    );
    put(
        "debate",
        json!({
            "fired": get(v, "debateFired"),
            "outcome": get(v, "debateOutcome"),
            "rationale": or(get(v, "debateRationale"), Value::Null),
        }),
    );
    Ok(Value::Object(m))
}

/// The `metrics` object.
///
/// # Errors
/// Where the workflow throws building it.
#[allow(clippy::too_many_lines)]
pub fn metrics_row(s: &MetricsScope) -> Result<Value, String> {
    let paneled: Vec<&Value> = s
        .results
        .iter()
        .filter(|v| js::truthy(get(v, "panelRan")))
        .collect();
    let diff_bytes = match get(&s.pr_context, "diffBytes") {
        Some(Value::Number(x)) if x.as_f64().is_some_and(|b| b > 0.0) => Value::Number(x.clone()),
        _ => Value::Null,
    };
    let receivers = s.lens_receiver_count + s.refuter_receiver_count;
    let diff_exposure = json!({
        "diffBytes": diff_bytes,
        "diffMode": s.diff_mode,
        "lensReceivers": n(s.lens_receiver_count),
        "refuterReceivers": n(s.refuter_receiver_count),
        "totalReceivers": n(receivers),
        "proxyBytes": diff_bytes.as_f64().map_or(Value::Null, |b| n(b * receivers)),
        "sectioningAvailable": s.sectioning_available,
        "sectionCount": n(s.section_count),
        "sectionedReceivers": n(s.sectioned_receiver_count),
        "fullDiffReceivers": n(s.full_diff_receiver_count),
        "deliveredBytes": n(s.delivered_bytes),
        "unsectionedFiles": s.unsectioned_files,
        "unparsedDiffHeaders": s.diff_unparsed_headers,
    });
    let plan: Vec<Value> = s
        .lens_plan
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut m = Map::new();
            m.insert(
                "lens".into(),
                s.lens_labels.get(i).cloned().unwrap_or(Value::Null),
            );
            if let Some(files) = get(p, "files") {
                m.insert("files".into(), files.clone());
            }
            Value::Object(m)
        })
        .collect();
    let report_findings: Vec<Value> = s
        .results
        .iter()
        .map(|v| {
            let f = get(v, "finding");
            let mut m = Map::new();
            if let Some(l) = f.and_then(|f| get(f, "lens")) {
                m.insert("lens".into(), l.clone());
            }
            if let Some(mf) = f.and_then(|f| get(f, "mergedFrom")) {
                m.insert("mergedFrom".into(), mf.clone());
            }
            if let Some(sv) = get(v, "survived") {
                m.insert("survived".into(), sv.clone());
            }
            Value::Object(m)
        })
        .collect();
    let lens_yield = build_lens_yield(
        &Value::Array(plan),
        &s.review_results,
        &s.lens_findings,
        &Value::Array(report_findings),
    );
    let scope = s
        .incremental_scope
        .as_object()
        .map(|_| &s.incremental_scope);
    let in_scope_len = |k: &str| {
        scope
            .and_then(|sc| get(sc, k))
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    };
    let prior_sha = get(&s.prior_review, "sha");
    let unchanged_notes: Vec<Value> = match scope {
        Some(sc) => get(sc, "unchanged")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|f| {
                json!(format!(
                    "{} (unchanged since {} — not re-read)",
                    to_js_string(Some(f)),
                    utf16_prefix_lossy(&to_js_string(prior_sha), 8)
                ))
            })
            .collect(),
        None => Vec::new(),
    };
    let lens_not_reached = Value::Array(s.lens_not_reached.clone());
    let not_checked = merge_not_checked(&[
        &s.uncovered_files,
        &lens_not_reached,
        &Value::Array(unchanged_notes),
    ]);
    let still_open = get(&s.prior_outcome, "stillOpen")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let not_resurfaced = get(&s.prior_outcome, "notResurfaced")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let findings: Vec<Value> = s
        .results
        .iter()
        .map(|v| finding_row(v, s))
        .collect::<Result<_, _>>()?;
    let in_thread: Vec<Value> = get(&s.pr_context, "inThreadDecisions")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .map(|d| {
            let mut m = Map::new();
            for k in ["category", "summary", "source"] {
                if let Some(x) = get(d, k) {
                    m.insert(k.into(), x.clone());
                }
            }
            m.insert(
                "files".into(),
                match get(d, "files") {
                    Some(Value::Array(a)) => Value::Array(a.clone()),
                    _ => json!([]),
                },
            );
            Value::Object(m)
        })
        .collect();
    let dependents = if s.blast_relay {
        let br = &s.blast_radius;
        let present = js::truthy(Some(br));
        json!({
            "verified": present,
            "listed": if present { get(br, "callers").and_then(Value::as_array).map_or(Value::Null, |c| json!(c.len())) } else { json!(0) },
            "total": if present { get(br, "total").cloned().unwrap_or(Value::Null) } else { json!(0) },
            "truncated": if present { get(br, "truncated").cloned().unwrap_or(Value::Null) } else { json!(false) },
        })
    } else {
        Value::Null
    };
    let viewer = js::trim(&to_js_string(Some(&or(
        get(&s.pr_context, "viewer"),
        json!(""),
    ))))
    .to_owned();
    let platform = if js::truthy(Some(&s.platform)) {
        s.platform.clone()
    } else {
        or(get(&s.pr_context, "platform"), Value::Null)
    };
    let mut m = Map::new();
    let mut put = |k: &str, x: Value| {
        m.insert(k.to_owned(), x);
    };
    put("schemaVersion", json!(3));
    put("ts", Value::Null);
    put("repo", json!(s.repo));
    put("pr", s.pr_number.clone());
    put("platform", platform);
    put("headSha", or(get(&s.pr_context, "headRefOid"), Value::Null));
    put("refEvidence", s.ref_evidence.clone());
    put(
        "headPin",
        json!({"pinned": get(&s.head_pin, "ok"), "kind": get(&s.head_pin, "kind")}),
    );
    put("pinned", json!(js::truthy(Some(&s.pinned_sha))));
    put("mode", json!("workflow"));
    put("fileCount", n(s.file_count));
    put("additions", or_null(get(&s.pr_context, "additions")));
    put("deletions", or_null(get(&s.pr_context, "deletions")));
    put(
        "lensCount",
        json!(s.lens_plan.as_array().map_or(0, Vec::len)),
    );
    put("lensAgent", json!(s.lens_agent));
    put("profileSha256", s.profile_sha256.clone());
    put("profileSource", s.profile_source.clone());
    put("projectRulesSha256", s.project_rules_sha256.clone());
    put("projectRulesSource", s.project_rules_source.clone());
    put("conventions", s.conventions_manifest.clone());
    put(
        "lensYield",
        serde_json::to_value(lens_yield).map_err(|e| e.to_string())?,
    );
    put("lensCountOverride", s.lenses_override.clone());
    put("filePartitionMode", s.file_partition_mode.clone());
    put("perGroupChurn", s.per_group_churn.clone());
    put("codexEnabled", json!(s.use_codex_head));
    put("externalProbeAnswered", s.external_probe_answered.clone());
    put(
        "externalHeadPinned",
        if s.use_codex_head || s.use_gemini_head {
            json!(js::truthy(Some(&s.external_cwd)))
        } else {
            Value::Null
        },
    );
    put("geminiEnabled", json!(s.use_gemini_head));
    put("cleanSweep", s.clean_sweep.clone());
    put("verifiedAnchors", s.verified_anchors.clone());
    put("voteQuoteCheck", s.vote_quote_check.clone());
    put("batchPanels", json!(s.batch_panels_active));
    put("botComments", json!(s.vet_bot_comments));
    put("verdict", json!(s.verdict_guess));
    put("reportVerdictCorrected", s.report_verdict_corrected.clone());
    put("posted", json!(false));
    put("candidates", n(s.all_candidates));
    put("paneled", json!(paneled.len()));
    put(
        "killed",
        json!(s
            .results
            .iter()
            .filter(|v| !js::truthy(get(v, "survived")))
            .count()),
    );
    put("placedAside", n(s.placed_aside));
    put("droppedPrePanel", json!(s.dropped_pre_panel.len()));
    put(
        "droppedPrePanelItems",
        Value::Array(
            s.dropped_pre_panel
                .iter()
                .map(|d| {
                    let mut o = Map::new();
                    if let Some(x) = get(d, "title") {
                        o.insert("title".into(), x.clone());
                    }
                    if let Some(x) = get(d, "file") {
                        o.insert("file".into(), x.clone());
                    }
                    o.insert("severity".into(), or_null(get(d, "severity")));
                    Value::Object(o)
                })
                .collect(),
        ),
    );
    put("inThreadDecisions", Value::Array(in_thread));
    put("dependents", dependents);
    put("dedupedPrePanel", n(s.deduped_pre_panel));
    put("voteCorrections", json!(s.vote_corrections.len()));
    put(
        "voteCorrectionsDetail",
        Value::Array(s.vote_corrections.clone()),
    );
    put(
        "crossFileClustersRejected",
        n(s.cross_file_clusters_rejected),
    );
    put("proximityVetoed", n(s.proximity_vetoed));
    put("groundedChecked", json!(s.grounded_available));
    put("groundedConfirmed", n(s.grounded_confirmed));
    put("groundedRefuted", n(s.grounded_refuted));
    put("evidenceResolverRan", json!(s.evidence_resolver_ran));
    put("evidenceLocatedCount", n(s.evidence_located_count));
    put(
        "evidenceLineCorrectedCount",
        n(s.evidence_line_corrected_count),
    );
    put("evidenceNotFoundCount", n(s.evidence_not_found_count));
    put("completenessCritic", json!(!s.critic_output.is_empty()));
    put("completenessCriticGaps", n(s.critic_gaps));
    put(
        "claudeOutputTokens",
        json!({
            "context": n(s.tokens_at_context_end - s.tokens_at_start),
            "review": n(s.tokens_at_review_end - s.tokens_at_context_end),
            "verify": n(s.tokens_at_verify_end - s.tokens_at_review_end),
            "synthesize": n(s.tokens_at_end - s.tokens_at_verify_end),
            "total": n(s.tokens_at_end - s.tokens_at_start),
            "counterResets": n(s.counter_resets),
        }),
    );
    put("wallClockSeconds", Value::Null);
    put("wallClockNote", Value::Null);
    put("wallClockSource", Value::Null);
    put("agentCost", Value::Null);
    put("diffExposure", diff_exposure);
    put("relays", s.relays.clone());
    put(
        "priorOpenReviews",
        json!({
            "count": s.prior_reviews.len(),
            "reviewers": s.prior_reviews.iter().map(|r| {
                let mut o = Map::new();
                if let Some(x) = get(r, "author") { o.insert("author".into(), x.clone()); }
                if let Some(x) = get(r, "at") { o.insert("at".into(), x.clone()); }
                Value::Object(o)
            }).collect::<Vec<_>>(),
            "gate": get(&s.prior_review_gate, "status"),
            "ownOpenReview": get(&s.prior_review_gate, "ownOpenReview"),
            "ownStandingReview": get(&s.prior_review_gate, "ownStandingReview"),
            "viewer": if viewer.is_empty() { Value::Null } else { json!(viewer) },
        }),
    );
    put("fidelityCritic", s.fidelity_critic.clone());
    put("findings", Value::Array(findings));
    put(
        "crossModelDisagreement",
        json!({
            "codexVsClaudeMajority": any_disagreement(&paneled, |v| {
                let ext = external_vote(v, "codex")?;
                let claude = claude_verdict(v)?;
                Some(get(ext, "verdict") != Some(&json!(claude)))
            }),
            "geminiVsClaudeMajority": any_disagreement(&paneled, |v| {
                let ext = external_vote(v, "gemini")?;
                let claude = claude_verdict(v)?;
                Some(get(ext, "verdict") != Some(&json!(claude)))
            }),
            "codexVsGemini": any_disagreement(&paneled, |v| {
                let c = external_vote(v, "codex")?;
                let g = external_vote(v, "gemini")?;
                Some(get(c, "verdict") != get(g, "verdict"))
            }),
        }),
    );
    put("diffMode", s.diff_mode.clone());
    put("lensGroups", n(s.group_count));
    put("churn", n(s.churn));
    put("notChecked", Value::Array(not_checked));
    put(
        "reviewMode",
        json!(if scope.is_some() {
            "incremental"
        } else {
            "full"
        }),
    );
    put(
        "incrementalBase",
        if scope.is_some() {
            prior_sha.cloned().unwrap_or(Value::Null)
        } else {
            Value::Null
        },
    );
    put("incrementalFallback", s.incremental_fallback.clone());
    put(
        "filesInScope",
        if scope.is_some() {
            json!(in_scope_len("inScope"))
        } else {
            n(s.all_pr_files)
        },
    );
    put(
        "filesCarriedForward",
        json!(if scope.is_some() {
            in_scope_len("unchanged")
        } else {
            0
        }),
    );
    put(
        "findingsCarriedForward",
        if scope.is_some() {
            n(s.carried_forward)
        } else {
            json!(0)
        },
    );
    put(
        "findingsReExamined",
        json!(if scope.is_some() {
            still_open + not_resurfaced
        } else {
            0
        }),
    );
    put(
        "findingsNotResurfaced",
        json!(if scope.is_some() { not_resurfaced } else { 0 }),
    );
    put(
        "findingsEscalatedByImportHeuristic",
        if scope.is_some() {
            n(s.heuristic_escalated)
        } else {
            json!(0)
        },
    );
    put(
        "priorVerification",
        if scope.is_some() {
            Value::Array(
                s.prior_re_examined
                    .iter()
                    .map(|p| {
                        let ver = get(p, "verification").cloned().unwrap_or(Value::Null);
                        json!({
                            "file": or(get(p, "file"), Value::Null),
                            "title": or(get(p, "title"), Value::Null),
                            "severity": or(get(p, "severity"), Value::Null),
                            "blocking": js::truthy(get(p, "blocking")),
                            "verdict": get(&ver, "verdict"),
                            "quote": or(get(&ver, "quote"), Value::Null),
                            "reason": or(get(&ver, "reason"), Value::Null),
                        })
                    })
                    .collect(),
            )
        } else {
            json!([])
        },
    );
    put("lensBudgetSkips", json!(s.lens_not_reached.len()));
    put("lensToolCallBudget", n(LENS_TOOL_CALL_BUDGET));
    put("panelCeiling", n(s.max_panels));
    put("panelSkippedForBudget", s.panel_skipped_for_budget.clone());
    put("panelCeilingTiebreak", s.tiebreak.clone());
    put(
        "costBreaker",
        json!({
            "fired": s.cost_breaker_fired,
            "outputCeiling": n(s.output_ceiling),
            "budgetTotal": s.budget_total,
            "stages": s.cost_degraded_stages,
        }),
    );
    put("agentCeiling", n(s.max_agents));
    put("agentsProjected", n(s.agents_projected));
    put(
        "agentCostModel",
        json!({
            "fixed": n(AGENTS_FIXED_OVERHEAD),
            "perPanel": n(AGENTS_PER_PANEL),
            "perRefine": n(AGENTS_PER_REFINE),
            "claudeBatchSize": CLAUDE_BATCH_SIZE,
            "agentsProjectedUnbatched": n(s.agents_projected_unbatched),
            "round0ClaudeBatchSavings": n(s.round0_claude_batch_savings),
            "refineClaudeBatchSavings": n(s.refine_claude_batch_savings),
        }),
    );
    put("refineCeiling", n(s.max_refines));
    put(
        "refinesSkippedForBudget",
        s.refines_skipped_for_budget.clone(),
    );
    put("voteReconciliation", Value::Null);
    put("externalSlots", n(s.external_slots));
    put("agentsActual", Value::Null);
    put("harnessVersion", Value::Null);
    put("codexModelUsed", Value::Null);
    put("artifactDir", json!(s.artifact_dir));
    put(
        "selfAudit",
        json!({"passed": null, "checked": 0, "violations": []}),
    );
    Ok(Value::Object(m))
}
