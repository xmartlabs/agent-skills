//! The reason behind a REQUEST CHANGES, written under the report's verdict
//! line. The verdict rule reads two flags that a lens sets and a profile's risk
//! paths can raise, and the ranking's mark at the blocking bar, so the report
//! alone does not say which finding caused it.
//! Rust only: the Workflow's report has no such line, and nothing here changes
//! the verdict.

use serde_json::Value;
use tribunal_core::js;
use tribunal_core::report::draft_comments_strip::strip_draft_comments_from_body;

use crate::synthesize::Report;

const MAX_TITLES: usize = 4;
const MAX_TITLE_CHARS: usize = 80;

fn flag(f: &Value, key: &str) -> bool {
    js::truthy(js::get(f, key))
}

/// Why each finding counts toward REQUEST CHANGES, under the same conditions
/// as `verdict_for`: a critical survivor, or an important one that changes
/// behavior, touches a high-risk surface or was ranked at the blocking bar
/// (`--block-min-worth`). `findings` must already carry the
/// profile's risk paths (`with_risk_path_surface`). `None` when no finding
/// qualifies, as when only a prior blocker caused the verdict.
#[must_use]
pub fn verdict_basis(findings: &[Value]) -> Option<String> {
    let mut causes: Vec<String> = Vec::new();
    for f in findings.iter().filter(|f| {
        js::truthy(Some(f))
            && !flag(f, "verifiedByGateNote")
            && js::get(f, "adjudicated") != Some(&Value::Bool(false))
            && !flag(f, crate::questions::RANKED_BELOW_BLOCKING)
            && !flag(f, crate::pre_existing::PRE_EXISTING_CODE)
    }) {
        let mut why: Vec<String> = Vec::new();
        match js::get(f, "severity").and_then(Value::as_str) {
            Some("critical") => why.push("critical".into()),
            Some("important") => {
                if flag(f, "isBehaviorChanging") {
                    why.push("changes behavior".into());
                }
                if flag(f, "touchesHighRiskSurface") {
                    why.push(if flag(f, "riskPathSurface") {
                        "its file is on a risk path of the profile".into()
                    } else {
                        "touches a high-risk surface".into()
                    });
                }
                if why.is_empty() && flag(f, crate::questions::RANKED_AT_BLOCKING) {
                    why.push(
                        js::get(f, crate::questions::RANK_WORTH)
                            .and_then(Value::as_u64)
                            .map_or_else(
                                || "ranked at the blocking bar".to_owned(),
                                |w| format!("ranked worth {w}, at the blocking bar"),
                            ),
                    );
                }
            }
            _ => {}
        }
        if why.is_empty() {
            continue;
        }
        let title = js::get(f, "title")
            .and_then(Value::as_str)
            .unwrap_or("untitled");
        let title: String = title.trim().chars().take(MAX_TITLE_CHARS).collect();
        causes.push(format!("\"{title}\" ({})", why.join(", ")));
    }
    if causes.is_empty() {
        return None;
    }
    let total = causes.len();
    let shown = causes
        .iter()
        .take(MAX_TITLES)
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    let more = if total > MAX_TITLES {
        format!("; and {} more", total - MAX_TITLES)
    } else {
        String::new()
    };
    Some(format!(
        "Why REQUEST CHANGES: {total} finding{} qualify under the verdict rule (a critical finding, or an important one that changes behavior, touches a high-risk surface or was ranked at the blocking bar): {shown}{more}.",
        if total == 1 { "" } else { "s" }
    ))
}

/// The report with `basis` on its own paragraph right under the verdict line,
/// and the body as posted recomputed from it. A report with no verdict line is
/// returned as it came.
#[must_use]
pub fn with_verdict_basis(report: Report, basis: &str) -> Report {
    let mut lines: Vec<&str> = report.report.split('\n').collect();
    let Some(at) = lines
        .iter()
        .position(|l| l.trim_start().starts_with("### Verdict suggestion:"))
    else {
        return report;
    };
    lines.insert(at + 1, "");
    lines.insert(at + 2, basis);
    let text = lines.join("\n");
    Report {
        as_posted: strip_draft_comments_from_body(&text),
        report: text,
        corrected: report.corrected,
    }
}
