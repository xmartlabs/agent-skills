//! The Verify phase's glue: the pre-panel dedup, the grounded typecheck and
//! its gate, which findings a panel would judge and which it can afford, and
//! the results split the Synthesize phase reads. Ports the workflow from
//! `phase("Verify")` to the "Verify done" log line, except the panel itself.

use std::collections::HashSet;

use serde_json::{json, Map, Value};
use tribunal_core::evidence::tsc_oracle::{
    adjudicate_finding, classify_typecheck_text, filter_diagnostics_to_pr,
    group_diagnostics_by_file, parse_tsc_diagnostics, resolve_diag_file, DiagsByFile,
};
use tribunal_core::js;
use tribunal_core::lens::js_ext::to_js_string;
use tribunal_core::votes::dedup_veto::mergeable_within;
use tribunal_core::votes::js_ext::number_value;

/// The dedup dispatch's prompt.
#[must_use]
pub fn dedup_prompt(all: &[Value]) -> String {
    let shown: Vec<Value> = all
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut m = Map::new();
            m.insert("index".into(), json!(i));
            for k in ["file", "line", "title", "description", "evidence", "lens"] {
                if let Some(v) = js::get(c, k) {
                    m.insert(k.into(), v.clone());
                }
            }
            Value::Object(m)
        })
        .filter(|c| js::get(c, "lens") != Some(&json!("bot-comments")))
        .collect();
    format!(
        r#"You are given candidate code-review findings from DIFFERENT review lenses on the same PR. Group into clusters ONLY when two or more candidates describe the SAME underlying defect — never merely the same class of bug in a different function or call-site.

Negative example (do NOT merge cases like this): two candidates both about "duplicate orderIds causing false-negative rejection" in the same file quoteRequest.ts, one in the public save path (~line 100) and one in the private save path (~line 213) — these are TWO DISTINCT functions independently missing the same defensive check. They must stay separate.

Default to NOT merging when you are not sure — a wrong merge silently drops a real, distinct defect from the report, which is worse than the cost of an extra redundant panel.

Candidates (index, file, line, title, description, evidence, lens):
{}

Return only clusters of size 2 or more. An index not mentioned in any cluster is implicitly its own singleton — do not return single-index clusters."#,
        js::stringify(&Value::Array(shown), "  ")
    )
}

fn severity_rank(f: &Value) -> f64 {
    match js::get(f, "severity").and_then(Value::as_str) {
        Some("critical") => 3.0,
        Some("important") => 2.0,
        Some("nit") => 1.0,
        _ => 0.0,
    }
}

/// Ports `mergeCandidates`: one candidate for a cluster, with the strongest
/// severity and flags any member carried.
#[must_use]
pub fn merge_candidates(members: &[&Value]) -> Value {
    let mut ranked: Vec<&&Value> = members.iter().collect();
    ranked.sort_by(|a, b| {
        severity_rank(b)
            .partial_cmp(&severity_rank(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let strongest = js::get(ranked[0], "severity").cloned();
    let mut out = members[0].as_object().cloned().unwrap_or_default();
    match strongest {
        Some(s) => {
            out.insert("severity".into(), s);
        }
        None => {
            out.shift_remove("severity");
        }
    }
    let any = |k: &str| members.iter().any(|m| js::truthy(js::get(m, k)));
    out.insert(
        "isBehaviorChanging".into(),
        json!(any("isBehaviorChanging")),
    );
    out.insert(
        "touchesHighRiskSurface".into(),
        json!(any("touchesHighRiskSurface")),
    );
    let confidence = members
        .iter()
        .map(|m| {
            js::get(m, "confidence")
                .and_then(Value::as_f64)
                .unwrap_or(0.5)
        })
        .fold(f64::NEG_INFINITY, f64::max);
    out.insert("confidence".into(), number_value(confidence));
    let lens = |m: &Value| match js::get(m, "lens") {
        l if js::truthy(l) => to_js_string(l),
        _ => "unknown".to_owned(),
    };
    out.insert(
        "evidence".into(),
        json!(members
            .iter()
            .map(|m| format!("[{}] {}", lens(m), to_js_string(js::get(m, "evidence"))))
            .collect::<Vec<_>>()
            .join("\n")),
    );
    out.insert(
        "mergedFrom".into(),
        json!(members.iter().map(|m| lens(m)).collect::<Vec<_>>()),
    );
    out.insert(
        "mergedLocations".into(),
        Value::Array(
            members
                .iter()
                .map(|m| {
                    let mut loc = Map::new();
                    for k in ["file", "line", "title"] {
                        if let Some(v) = js::get(m, k) {
                            loc.insert(k.into(), v.clone());
                        }
                    }
                    loc.insert("lens".into(), json!(lens(m)));
                    Value::Object(loc)
                })
                .collect(),
        ),
    );
    Value::Object(out)
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dedup {
    pub panel_candidates: Vec<Value>,
    pub deduped_pre_panel: usize,
    pub clusters: usize,
    pub cross_file_clusters_rejected: usize,
    pub proximity_vetoed: usize,
}

fn index_of(i: &Value, len: usize) -> Option<usize> {
    let n = i.as_f64()?;
    (n.fract() == 0.0 && n >= 0.0 && n < len as f64).then_some(n as usize)
}

/// What the workflow does with the dedup dispatch's answer: validate its
/// indices, keep only same-file clusters whose members are close to the
/// first, and fold each into one candidate.
///
/// # Errors
/// Where the workflow throws: `clusters` is truthy and not iterable.
pub fn apply_dedup(
    all: &[Value],
    answer: Option<&Value>,
    logs: &mut Vec<String>,
) -> Result<Dedup, String> {
    let raw = answer.and_then(|a| js::get(a, "clusters"));
    let raw: Vec<Value> = match raw {
        r if !js::truthy(r) => Vec::new(),
        Some(Value::Array(a)) => a.clone(),
        Some(Value::String(s)) => s.chars().map(|c| json!(c.to_string())).collect(),
        Some(other) => return Err(format!("rawClusters is not iterable: {other}")),
        None => Vec::new(),
    };
    let mut claimed: HashSet<usize> = HashSet::new();
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    let mut cross_file = 0;
    let mut proximity = 0;
    for idxs in &raw {
        let Some(idxs) = idxs.as_array() else {
            continue;
        };
        let mut seen = HashSet::new();
        let valid: Vec<usize> = idxs
            .iter()
            .filter_map(|i| {
                let i = index_of(i, all.len())?;
                if claimed.contains(&i)
                    || seen.contains(&i)
                    || js::get(&all[i], "lens") == Some(&json!("bot-comments"))
                {
                    return None;
                }
                seen.insert(i);
                Some(i)
            })
            .collect();
        let mut by_file: Vec<(Value, Vec<usize>)> = Vec::new();
        for &i in &valid {
            let key = match js::get(&all[i], "file") {
                f if js::truthy(f) => f.cloned().unwrap_or(Value::Null),
                _ => json!(""),
            };
            match by_file.iter_mut().find(|(k, _)| *k == key) {
                Some((_, g)) => g.push(i),
                None => by_file.push((key, vec![i])),
            }
        }
        let groups: Vec<&Vec<usize>> = by_file
            .iter()
            .map(|(_, g)| g)
            .filter(|g| g.len() >= 2)
            .collect();
        if by_file.len() > 1 {
            cross_file += valid.len() - groups.iter().map(|g| g.len()).sum::<usize>();
        }
        for group in groups {
            let anchor = &all[group[0]];
            let kept: Vec<usize> = group
                .iter()
                .enumerate()
                .filter(|&(pos, &i)| pos == 0 || mergeable_within(anchor, &all[i]))
                .map(|(_, &i)| i)
                .collect();
            proximity += group.len() - kept.len();
            if kept.len() < 2 {
                continue;
            }
            claimed.extend(kept.iter().copied());
            clusters.push(kept);
        }
    }
    let merged: Vec<Value> = clusters
        .iter()
        .map(|idxs| merge_candidates(&idxs.iter().map(|&i| &all[i]).collect::<Vec<_>>()))
        .collect();
    let mut panel_candidates: Vec<Value> = all
        .iter()
        .enumerate()
        .filter(|(i, _)| !claimed.contains(i))
        .map(|(_, c)| c.clone())
        .collect();
    panel_candidates.extend(merged);
    let deduped = all.len() - panel_candidates.len();
    if deduped > 0 {
        logs.push(format!(
            "{deduped} candidate(s) merged as duplicates across lenses ({} cluster(s))",
            clusters.len()
        ));
    }
    if cross_file > 0 {
        logs.push(format!(
            "{cross_file} candidate(s) kept separate — the dedup pass tried to merge across files"
        ));
    }
    if proximity > 0 {
        logs.push(format!(
            "{proximity} candidate(s) kept separate — same file, but too far apart or too dissimilar to be one defect"
        ));
    }
    Ok(Dedup {
        panel_candidates,
        deduped_pre_panel: deduped,
        clusters: clusters.len(),
        cross_file_clusters_rejected: cross_file,
        proximity_vetoed: proximity,
    })
}

/// What the grounded typecheck left for the gate: whether it can be trusted,
/// the PR's diagnostics by file, and the files its program compiled.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Grounded {
    pub available: bool,
    pub diags_by_file: DiagsByFile,
    pub covered: Vec<String>,
}

const SOURCE_EXT: [&str; 6] = [".ts", ".tsx", ".js", ".jsx", ".mts", ".cts"];

/// Ports `applyTypecheckResult` over the typecheck's text.
///
/// # Errors
/// Where the workflow throws grouping the diagnostics.
pub fn apply_typecheck_result(
    text: &str,
    pr_files: &[String],
    logs: &mut Vec<String>,
) -> Result<Grounded, String> {
    let mut g = Grounded::default();
    let kind = classify_typecheck_text(Some(text));
    if kind == "no-tsconfig" {
        logs.push("Grounded check: no TS typecheck in repo, skipped".into());
        return Ok(g);
    }
    if kind == "degraded" {
        logs.push(if text.contains("DEGRADED_SCHEMA_DRIFT") {
            "Grounded check skipped: the borrowed node_modules was generated from a different Prisma schema than the reviewed head — findings fall through to panel".into()
        } else {
            "Grounded check unavailable (degraded/unrecognized) — findings fall through to panel"
                .into()
        });
        return Ok(g);
    }
    let all = parse_tsc_diagnostics(Some(text));
    let ts2307: HashSet<&str> = all
        .iter()
        .filter(|d| d.code == "TS2307")
        .map(|d| d.file.as_str())
        .collect();
    if ts2307.len() >= 20 {
        logs.push(
            "Grounded check unavailable (node_modules resolution broken) — findings fall through to panel".into(),
        );
        return Ok(g);
    }
    const MARK: &str = "---COVERED---";
    let marker = text.find(MARK);
    let mut covered: Vec<String> = Vec::new();
    if let Some(at) = marker {
        for s in text[at + MARK.len()..].split('\n') {
            let s = js::trim(s);
            if !s.is_empty()
                && !s.starts_with('/')
                && SOURCE_EXT.iter().any(|e| s.ends_with(e))
                && !covered.iter().any(|c| c == s)
            {
                covered.push(s.to_owned());
            }
        }
    }
    let covered_count = marker.map_or(0, |at| {
        text[at + MARK.len()..]
            .split('\n')
            .map(js::trim)
            .filter(|s| {
                !s.is_empty() && !s.starts_with('/') && SOURCE_EXT.iter().any(|e| s.ends_with(e))
            })
            .count()
    });
    let pr_covered = pr_files.iter().filter(|f| covered.contains(f)).count();
    let sized = marker.and_then(|at| {
        let rest = text[at..].strip_prefix("---COVERED--- ")?;
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        (!digits.is_empty()).then(|| js::string_to_number(&digits))
    });
    let program_size = sized.unwrap_or(covered_count as f64);
    g.covered = covered;
    let grouped =
        || group_diagnostics_by_file(Some(&filter_diagnostics_to_pr(Some(&all), Some(pr_files))));
    if program_size == 0.0 {
        g.diags_by_file = grouped()?;
        g.available = true;
        logs.push(
            "Grounded typecheck ran but coverage is unknown — confirm-only, no pre-panel refutations".into(),
        );
    } else if pr_covered == 0 {
        logs.push(format!(
            "Grounded check covers 0 of {} PR file(s) (tsconfig program excludes them) — findings fall through to panel",
            pr_files.len()
        ));
    } else {
        g.diags_by_file = grouped()?;
        g.available = true;
        logs.push(format!(
            "Grounded typecheck ran — covers {pr_covered}/{} PR file(s), {} with diagnostics",
            pr_files.len(),
            g.diags_by_file.len()
        ));
    }
    Ok(g)
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GroundedGate {
    pub confirmed: Vec<Value>,
    pub refuted: Vec<Value>,
    pub panel_input: Vec<Value>,
}

/// The grounded gate over the deduped candidates, then the `slotId` stamp.
///
/// # Errors
/// Where `adjudicateFinding` throws.
pub fn grounded_gate(
    candidates: &[Value],
    grounded: &Grounded,
    pr_files: &[String],
    logs: &mut Vec<String>,
) -> Result<GroundedGate, String> {
    let mut gate = GroundedGate::default();
    let mut remaining = Vec::new();
    if grounded.available {
        let covered: HashSet<String> = grounded.covered.iter().cloned().collect();
        for finding in candidates {
            let mut probe = finding.clone();
            if let Some(Value::String(f)) = js::get(finding, "file") {
                if let Some(r) = resolve_diag_file(Some(f), Some(pr_files)) {
                    probe["file"] = json!(r);
                }
            }
            let a = adjudicate_finding(&probe, &grounded.diags_by_file, Some(&covered))?;
            let entry = json!({"finding": finding, "grounded": a.to_value()});
            match js::get(&entry["grounded"], "verdict").and_then(Value::as_str) {
                Some("confirmed") => gate.confirmed.push(entry),
                Some("refuted") => gate.refuted.push(entry),
                _ => remaining.push(finding.clone()),
            }
        }
        if !gate.confirmed.is_empty() || !gate.refuted.is_empty() {
            logs.push(format!(
                "Grounded gate: {} confirmed (panel skipped), {} refuted pre-panel",
                gate.confirmed.len(),
                gate.refuted.len()
            ));
        }
    } else {
        remaining = candidates.to_vec();
    }
    gate.panel_input = remaining
        .into_iter()
        .enumerate()
        .map(|(i, mut f)| {
            if let Some(m) = f.as_object_mut() {
                m.insert("slotId".into(), json!(i));
            }
            f
        })
        .collect();
    Ok(gate)
}

/// Ports `needsPanel`. `retest` is `isRetestTarget(finding)`.
#[must_use]
pub fn needs_panel(finding: &Value, retest: bool) -> bool {
    retest
        || js::get(finding, "lens") == Some(&json!("bot-comments"))
        || js::get(finding, "severity") != Some(&json!("nit"))
}

pub const MAX_PANELS_DEFAULT: f64 = 40.0;
pub const MAX_AGENTS_DEFAULT: f64 = 150.0;
pub const AGENTS_FIXED_OVERHEAD: f64 = 12.0;
pub const AGENTS_PER_PANEL: f64 = 2.4;
pub const AGENTS_PER_REFINE: f64 = 3.7;

/// Which findings open a panel and how many may refine.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PanelPlan {
    pub eligible: Vec<Value>,
    pub refine_reserve: f64,
    pub panel_budget: f64,
    pub max_panels: f64,
    pub max_refines: f64,
    pub bound: Vec<Value>,
    pub skipped_for_budget: Vec<Value>,
}

/// From `panelEligible` to `panelSkippedForBudget`. `retest` answers
/// `isRetestTarget` for a finding; `max_panels_arg` and `max_agents` are the
/// operator's ceilings after the workflow's defaulting.
#[must_use]
pub fn panel_plan(
    panel_input: &[Value],
    retest: &dyn Fn(&Value) -> bool,
    max_panels_arg: f64,
    max_agents: f64,
) -> PanelPlan {
    let eligible: Vec<Value> = panel_input
        .iter()
        .filter(|f| needs_panel(f, retest(f)))
        .cloned()
        .collect();
    let n = eligible.len() as f64;
    let refine_reserve = n.min((n * 0.4).floor());
    let panel_budget = 1f64.max(
        ((max_agents - AGENTS_FIXED_OVERHEAD - refine_reserve * AGENTS_PER_REFINE)
            / AGENTS_PER_PANEL)
            .floor(),
    );
    let max_panels = max_panels_arg.min(panel_budget);
    let max_refines = max_panels.min(
        0f64.max(
            ((max_agents - AGENTS_FIXED_OVERHEAD - max_panels * AGENTS_PER_PANEL)
                / AGENTS_PER_REFINE)
                .floor(),
        ),
    );
    let priority = |f: &Value| {
        let mut p = 0;
        if retest(f) {
            p += 8;
        }
        if js::get(f, "severity") == Some(&json!("critical")) {
            p += 4;
        }
        if js::truthy(js::get(f, "touchesHighRiskSurface")) {
            p += 2;
        }
        if js::truthy(js::get(f, "isBehaviorChanging"))
            || js::get(f, "lens") == Some(&json!("bot-comments"))
        {
            p += 1;
        }
        p
    };
    let mut ranked: Vec<(usize, &Value)> = eligible.iter().enumerate().collect();
    ranked.sort_by(|a, b| priority(b.1).cmp(&priority(a.1)).then(a.0.cmp(&b.0)));
    let take = if max_panels > 0.0 {
        max_panels as usize
    } else {
        0
    };
    let mut chosen: Vec<(usize, &Value)> = ranked.into_iter().take(take).collect();
    chosen.sort_by_key(|x| x.0);
    let bound: Vec<Value> = chosen.into_iter().map(|(_, f)| f.clone()).collect();
    let in_bound: Vec<Option<&Value>> = bound.iter().map(|f| js::get(f, "slotId")).collect();
    let skipped_for_budget = eligible
        .iter()
        .filter(|f| !in_bound.iter().any(|s| same_slot(*s, js::get(f, "slotId"))))
        .map(|f| {
            let mut m = Map::new();
            for k in ["title", "file", "severity"] {
                if let Some(v) = js::get(f, k) {
                    m.insert(k.into(), v.clone());
                }
            }
            Value::Object(m)
        })
        .collect();
    PanelPlan {
        eligible,
        refine_reserve,
        panel_budget,
        max_panels,
        max_refines,
        bound,
        skipped_for_budget,
    }
}

fn same_slot(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Value::Number(x)), Some(Value::Number(y))) => x.as_f64() == y.as_f64(),
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// `NO_PANEL_RESULT(finding)`.
#[must_use]
pub fn no_panel_result(finding: &Value) -> Value {
    json!({
        "finding": finding,
        "survived": true,
        "panelRan": false,
        "hadIndependentVoter": false,
        "hadExternalVoter": false,
        "failedSlots": [],
        "failedSlotsDetail": [],
        "discardedVotes": [],
        "unverified": false,
        "borderline": false,
        "refuteFraction": null,
        "votes": [],
        "allVotes": [],
        "preDecided": false,
        "debateFired": false,
        "debateOutcome": null,
        "debateRationale": null,
        "refined": false,
        "refineOutcome": null,
        "escalated": false,
    })
}

/// A typecheck-confirmed finding as a result.
#[must_use]
pub fn confirmed_result(entry: &Value) -> Value {
    json!({
        "finding": js::get(entry, "finding"),
        "grounded": js::get(entry, "grounded"),
        "survived": true,
        "panelRan": false,
        "hadIndependentVoter": false,
        "hadExternalVoter": false,
        "failedSlots": [],
        "failedSlotsDetail": [],
        "discardedVotes": [],
        "unverified": false,
        "borderline": false,
        "refuteFraction": null,
        "votes": [],
        "preDecided": false,
        "debateFired": false,
        "debateOutcome": null,
        "refined": false,
        "refineOutcome": null,
        "escalated": false,
    })
}

/// The results as Synthesize reads them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Verdicts {
    pub results: Vec<Value>,
    pub placed_aside: Vec<Value>,
    pub surviving: Vec<Value>,
    pub pre_decided: Vec<Value>,
    pub tech_refuted: Vec<Value>,
}

/// From `confirmedResults` to the "Verify done" line. `verified` is the
/// panel's per-finding results, `None` where one came back empty.
#[must_use]
pub fn partition_results(
    verified: &[Option<Value>],
    grounded_confirmed: &[Value],
    dropped_pre_panel: usize,
    logs: &mut Vec<String>,
) -> Verdicts {
    let results: Vec<Value> = verified
        .iter()
        .flatten()
        .filter(|v| js::truthy(Some(v)))
        .cloned()
        .chain(grounded_confirmed.iter().map(confirmed_result))
        .collect();
    let survived = |v: &Value| js::truthy(js::get(v, "survived"));
    let possibly = |v: &Value| js::truthy(js::get(v, "possiblyPreDecided"));
    let placed =
        |v: &Value| js::get(v, "placement").is_some_and(|p| js::truthy(js::get(p, "placement")));
    let contested: Vec<Value> = results
        .iter()
        .filter(|v| survived(v) && possibly(v))
        .cloned()
        .collect();
    let placed_aside: Vec<Value> = results
        .iter()
        .filter(|v| survived(v) && !possibly(v) && placed(v))
        .cloned()
        .collect();
    let surviving: Vec<Value> = results
        .iter()
        .filter(|v| survived(v) && !possibly(v) && !placed(v))
        .cloned()
        .collect();
    let refuted: Vec<&Value> = results.iter().filter(|v| !survived(v)).collect();
    let mut pre_decided: Vec<Value> = refuted
        .iter()
        .filter(|v| js::truthy(js::get(v, "preDecided")))
        .map(|v| (*v).clone())
        .collect();
    pre_decided.extend(contested);
    let tech_refuted: Vec<Value> = refuted
        .iter()
        .filter(|v| !js::truthy(js::get(v, "preDecided")))
        .map(|v| (*v).clone())
        .collect();
    logs.push(format!(
        "Verify done — {} survived / {} placed aside / {} pre-decided / {} refuted / {dropped_pre_panel} gated",
        surviving.len(),
        placed_aside.len(),
        pre_decided.len(),
        tech_refuted.len()
    ));
    Verdicts {
        results,
        placed_aside,
        surviving,
        pre_decided,
        tech_refuted,
    }
}
