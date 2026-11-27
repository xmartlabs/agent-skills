//! Whether a person should review the PR, computed from facts the run already
//! recorded. The engine never changes its verdict for this flag; the signals
//! and floors come from the profile (`NeedsHumanConfig`).

use serde_json::{json, Value};
use tribunal_core::js;
use tribunal_core::panel::panel_gates::T_KILL;
use tribunal_core::profile::high_risk_surface::with_risk_path_surface;
use tribunal_core::profile::needs_human::{NeedsHumanConfig, SeverityFloor, Signal};
use tribunal_core::profile::profile_lookup::RiskRule;
use tribunal_core::report::draft_comments_strip::strip_draft_comments_from_body;

use crate::synthesize::Report;

/// What the run knew when it finished. Every field is read-only.
pub struct NeedsHumanFacts<'a> {
    /// `review.lenses_unreturned`, items `{lens, files}`.
    pub lenses_unreturned: &'a [Value],
    /// `plan.lens_plan.len()`.
    pub lens_count: usize,
    pub unread_risk_files: &'a [String],
    /// Files of the PR that match the profile's risk path rules.
    pub touched_risk_files: &'a [String],
    pub typecheck_applies: bool,
    pub typecheck_text: Option<&'a str>,
    pub panel_skipped_for_budget: &'a [Value],
    pub cost_breaker_fired: Option<&'a Value>,
    /// `verify.verdicts.results`, after the risk-path surface is applied.
    pub results: &'a [Value],
    pub critic_not_checked: &'a [String],
    pub incremental_fallback: Option<&'a Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reason {
    pub signal: Signal,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NeedsHuman {
    pub raised: bool,
    pub reasons: Vec<Reason>,
}

impl NeedsHuman {
    #[must_use]
    pub fn to_value(&self) -> Value {
        json!({
            "raised": self.raised,
            "reasons": self.reasons.iter().map(|r| json!({
                "signal": r.signal.as_str(),
                "detail": r.detail,
            })).collect::<Vec<_>>(),
        })
    }

    /// The inverse of `to_value`. `None` when the shape is wrong or a signal
    /// id is unknown.
    #[must_use]
    pub fn from_value(v: &Value) -> Option<NeedsHuman> {
        let raised = v.get("raised")?.as_bool()?;
        let reasons = v
            .get("reasons")?
            .as_array()?
            .iter()
            .map(|r| {
                Some(Reason {
                    signal: Signal::parse(r.get("signal")?.as_str()?)?,
                    detail: r.get("detail")?.as_str()?.to_owned(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(NeedsHuman { raised, reasons })
    }
}

/// The PR files that match a risk path rule, in PR order.
#[must_use]
pub fn touched_risk_files(files: &[String], rules: &[RiskRule]) -> Vec<String> {
    files
        .iter()
        .filter(|f| rules.iter().any(|r| r.re.test(f)))
        .cloned()
        .collect()
}

/// `results` with each result's `finding` carrying `touchesHighRiskSurface`
/// when its file is on a risk path, so killed and unverified findings are
/// judged by the same floor as the ones that survived.
#[must_use]
pub fn results_with_risk_surface(results: &[Value], rules: &[RiskRule]) -> Vec<Value> {
    results
        .iter()
        .map(|r| {
            let Some(finding) = js::get(r, "finding") else {
                return r.clone();
            };
            let raised = with_risk_path_surface(Some(std::slice::from_ref(finding)), Some(rules));
            let mut out = r.clone();
            if let (Some(obj), Some(f)) = (out.as_object_mut(), raised.into_iter().next()) {
                obj.insert("finding".to_owned(), f);
            }
            out
        })
        .collect()
}

/// The line a person reads under the verdict; `None` when nothing is raised.
#[must_use]
pub fn needs_human_line(nh: &NeedsHuman) -> Option<String> {
    if !nh.raised || nh.reasons.is_empty() {
        return None;
    }
    let reasons: Vec<String> = nh
        .reasons
        .iter()
        .map(|r| format!("{} ({})", r.detail, r.signal.as_str()))
        .collect();
    Some(format!(
        "**A person should review this PR:** {}.",
        reasons.join("; ")
    ))
}

/// The report with the needs-human line on its own paragraph right under the
/// verdict line, and the body as posted recomputed from it. Unchanged when
/// nothing is raised or the report has no verdict line.
#[must_use]
pub fn with_needs_human_line(report: Report, nh: &NeedsHuman) -> Report {
    let Some(line) = needs_human_line(nh) else {
        return report;
    };
    let mut lines: Vec<&str> = report.report.split('\n').collect();
    let Some(at) = lines
        .iter()
        .position(|l| l.trim_start().starts_with("### Verdict suggestion:"))
    else {
        return report;
    };
    lines.insert(at + 1, "");
    lines.insert(at + 2, &line);
    let text = lines.join("\n");
    Report {
        as_posted: strip_draft_comments_from_body(&text),
        report: text,
        corrected: report.corrected,
    }
}

fn flag(finding: &Value, key: &str) -> bool {
    js::truthy(js::get(finding, key))
}

/// Whether `finding` is at least as severe as `floor`. `Blocking` is the
/// verdict rule: critical, or important with a behavior, risk or ranking flag.
#[must_use]
pub fn meets_floor(finding: &Value, floor: SeverityFloor) -> bool {
    let severity = js::get(finding, "severity").and_then(Value::as_str);
    let critical = severity == Some("critical");
    let important = severity == Some("important");
    match floor {
        SeverityFloor::Nit => true,
        SeverityFloor::Important => critical || important,
        SeverityFloor::Critical => critical,
        SeverityFloor::Blocking => {
            critical
                || (important
                    && (flag(finding, "isBehaviorChanging")
                        || flag(finding, "touchesHighRiskSurface")
                        || flag(finding, "rankedAtBlocking")))
        }
    }
}

fn len_of(result: &Value, key: &str) -> usize {
    js::get(result, key)
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

fn is_true(result: &Value, key: &str) -> bool {
    js::get(result, key) == Some(&Value::Bool(true))
}

/// The first `DEGRADED_*` code in the typecheck text.
fn degraded_code(text: &str) -> Option<String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("DEGRADED_"))?;
    Some(
        line.chars()
            .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
            .collect(),
    )
}

fn unverified_detail(facts: &NeedsHumanFacts, config: &NeedsHumanConfig) -> Option<String> {
    let (mut none, mut few) = (0usize, 0usize);
    for r in facts.results {
        if !is_true(r, "survived")
            || !is_true(r, "panelRan")
            || !meets_floor(&r["finding"], config.unverified_min)
        {
            continue;
        }
        let counted = if is_true(r, "unverified") {
            0
        } else {
            len_of(r, "votes").saturating_sub(len_of(r, "discardedVotes"))
        };
        if counted == 0 {
            none += 1;
        } else if counted < config.unverified_min_votes && len_of(r, "failedSlots") > 0 {
            few += 1;
        }
    }
    let mut parts = Vec::new();
    if none > 0 {
        parts.push(format!("{none} finding(s) survived without a counted vote"));
    }
    if few > 0 {
        parts.push(format!(
            "{few} finding(s) kept with fewer than {} counted vote(s) after refuters failed",
            config.unverified_min_votes
        ));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

fn borderline_detail(facts: &NeedsHumanFacts, config: &NeedsHumanConfig) -> Option<String> {
    let (mut close, mut killed) = (0usize, 0usize);
    for r in facts.results {
        if is_true(r, "unverified") || !meets_floor(&r["finding"], config.borderline_min) {
            continue;
        }
        let Some(fraction) = js::get(r, "refuteFraction").and_then(Value::as_f64) else {
            continue;
        };
        if (fraction - T_KILL).abs() <= config.borderline_band {
            close += 1;
            if !is_true(r, "survived") {
                killed += 1;
            }
        }
    }
    (close > 0).then(|| format!("{close} finding(s) decided by a close vote ({killed} killed)"))
}

fn panel_cut_detail(facts: &NeedsHumanFacts) -> Option<String> {
    let mut parts = Vec::new();
    if !facts.panel_skipped_for_budget.is_empty() {
        parts.push(format!(
            "{} finding(s) skipped the panel for budget",
            facts.panel_skipped_for_budget.len()
        ));
    }
    if facts
        .cost_breaker_fired
        .is_some_and(|v| js::truthy(Some(v)))
    {
        parts.push("cost breaker fired".to_owned());
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// The most file names a detail lists before it counts the rest.
const MAX_NAMED_FILES: usize = 5;

/// `files` joined with commas, the first `MAX_NAMED_FILES` named and the rest
/// counted: "a, b, c, d, e and 3 more".
fn capped_files(files: &[String]) -> String {
    let named = files[..files.len().min(MAX_NAMED_FILES)].join(", ");
    match files.len().saturating_sub(MAX_NAMED_FILES) {
        0 => named,
        more => format!("{named} and {more} more"),
    }
}

fn detail_for(
    signal: Signal,
    facts: &NeedsHumanFacts,
    config: &NeedsHumanConfig,
) -> Option<String> {
    match signal {
        Signal::LensEmpty => (!facts.lenses_unreturned.is_empty()).then(|| {
            format!(
                "{} of {} lens(es) returned nothing",
                facts.lenses_unreturned.len(),
                facts.lens_count
            )
        }),
        Signal::RiskPathUnread => (!facts.unread_risk_files.is_empty())
            .then(|| format!("no lens read {}", capped_files(facts.unread_risk_files))),
        Signal::TypecheckNotRun => {
            if facts.typecheck_applies {
                facts.typecheck_text.and_then(degraded_code)
            } else {
                None
            }
        }
        Signal::PanelCut => panel_cut_detail(facts),
        Signal::RiskPathTouched => (!facts.touched_risk_files.is_empty())
            .then(|| format!("changes {}", capped_files(facts.touched_risk_files))),
        Signal::FindingUnverified => unverified_detail(facts, config),
        Signal::FindingBorderline => borderline_detail(facts, config),
        Signal::CriticNotChecked => (!facts.critic_not_checked.is_empty()).then(|| {
            format!(
                "the critic did not check {} item(s)",
                facts.critic_not_checked.len()
            )
        }),
        Signal::IncrementalFallback => facts
            .incremental_fallback
            .filter(|v| js::truthy(Some(v)))
            .map(|v| {
                js::get(v, "reason")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("incremental review fell back")
                    .to_owned()
            }),
    }
}

/// The reasons a person should review this PR, in `Signal` order.
#[must_use]
pub fn needs_human(facts: &NeedsHumanFacts, config: &NeedsHumanConfig) -> NeedsHuman {
    let reasons: Vec<Reason> = Signal::ALL
        .into_iter()
        .filter(|s| config.signals.contains(s))
        .filter_map(|signal| {
            detail_for(signal, facts, config).map(|detail| Reason { signal, detail })
        })
        .collect();
    NeedsHuman {
        raised: !reasons.is_empty(),
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tribunal_core::profile::profile_lookup::risk_path_rules;

    fn rules() -> Vec<RiskRule> {
        risk_path_rules(&json!({"riskPaths": [{"focus": "security", "pattern": "^db/"}]})).unwrap()
    }

    #[test]
    fn risk_files_are_the_pr_files_a_rule_matches() {
        let files = [
            "src/widgets.ts".to_owned(),
            "db/migrations/001.sql".to_owned(),
        ];
        assert_eq!(
            touched_risk_files(&files, &rules()),
            ["db/migrations/001.sql"]
        );
    }

    #[test]
    fn a_result_finding_on_a_risk_path_is_flagged() {
        let results = [
            json!({"finding": {"file": "db/a.sql", "severity": "important"}, "survived": false}),
            json!({"finding": {"file": "src/widgets.ts", "severity": "important"}}),
            json!({"survived": true}),
        ];
        let out = results_with_risk_surface(&results, &rules());
        assert_eq!(out[0]["finding"]["touchesHighRiskSurface"], json!(true));
        assert_eq!(out[0]["survived"], json!(false));
        assert!(out[1]["finding"].get("touchesHighRiskSurface").is_none());
        assert_eq!(out[2], results[2]);
    }
}
