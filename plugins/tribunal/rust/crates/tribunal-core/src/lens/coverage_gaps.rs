//! What each lens says it did not check, by kind, and what the run does with
//! it. Rust only: the Workflow keeps `notReached` as plain strings.
//!
//! A lens files every gap as `{surface, file, kind}`. Only a `budget` gap is a
//! hole in coverage: the lens meant to read the code and ran out of room. The
//! other kinds are left to another lens's focus, cannot be settled by reading
//! code, or sit outside the PR.

use serde::Serialize;
use serde_json::{json, Value};

use crate::profile::high_risk_surface::risk_path_touched;
use crate::profile::profile_lookup::RiskRule;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GapKind {
    Budget,
    OutOfFocus,
    NeedsExecution,
    OutsidePr,
    /// A plain string, as a lens wrote before kinds existed.
    Unstated,
}

impl GapKind {
    fn parse(s: &str) -> Self {
        match s {
            "budget" => Self::Budget,
            "out-of-focus" => Self::OutOfFocus,
            "needs-execution" => Self::NeedsExecution,
            "outside-pr" => Self::OutsidePr,
            _ => Self::Unstated,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Gap {
    /// The lens's index in the plan.
    pub lens: usize,
    pub surface: String,
    pub file: Option<String>,
    pub kind: GapKind,
}

fn non_empty(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Every gap the first `lens_count` results declared, in order. A crashed
/// lens (`None`) declares nothing; it is reported as unreturned instead.
#[must_use]
pub fn collect_gaps(results: &[Option<Value>], lens_count: usize) -> Vec<Gap> {
    let mut out = Vec::new();
    for (lens, r) in results.iter().take(lens_count).enumerate() {
        let Some(Value::Array(items)) = r.as_ref().and_then(|r| r.get("notReached")) else {
            continue;
        };
        for item in items {
            let gap = match item {
                Value::String(_) => non_empty(Some(item)).map(|surface| Gap {
                    lens,
                    surface,
                    file: None,
                    kind: GapKind::Unstated,
                }),
                Value::Object(m) => non_empty(m.get("surface")).map(|surface| Gap {
                    lens,
                    surface,
                    file: non_empty(m.get("file")),
                    kind: m
                        .get("kind")
                        .and_then(Value::as_str)
                        .map_or(GapKind::Unstated, GapKind::parse),
                }),
                _ => None,
            };
            out.extend(gap);
        }
    }
    out
}

/// How many gaps of each kind: the row's `lensNotReachedKinds`.
#[must_use]
pub fn kind_counts(gaps: &[Gap]) -> Value {
    let n = |k: GapKind| gaps.iter().filter(|g| g.kind == k).count();
    json!({
        "budget": n(GapKind::Budget),
        "outOfFocus": n(GapKind::OutOfFocus),
        "needsExecution": n(GapKind::NeedsExecution),
        "outsidePr": n(GapKind::OutsidePr),
        "unstated": n(GapKind::Unstated),
    })
}

/// The `riskPaths` files that no lens read: every lens whose scope held the
/// file filed a `budget` gap on it. `scopes[i]` is lens `i`'s file list,
/// `None` for the whole diff. Sorted, without repeats.
#[must_use]
pub fn unread_risk_files(
    gaps: &[Gap],
    scopes: &[Option<Vec<String>>],
    rules: &[RiskRule],
) -> Vec<String> {
    let mut files: Vec<String> = gaps
        .iter()
        .filter(|g| g.kind == GapKind::Budget)
        .filter_map(|g| g.file.clone())
        .filter(|f| risk_path_touched(Some(&json!(f)), Some(rules)))
        .collect();
    files.sort();
    files.dedup();
    files.retain(|f| {
        let owners: Vec<usize> = scopes
            .iter()
            .enumerate()
            .filter(|(_, s)| s.as_ref().is_none_or(|s| s.contains(f)))
            .map(|(i, _)| i)
            .collect();
        !owners.is_empty()
            && owners.iter().all(|&i| {
                gaps.iter().any(|g| {
                    g.lens == i && g.kind == GapKind::Budget && g.file.as_deref() == Some(f)
                })
            })
    });
    files
}

fn line(g: &Gap) -> String {
    match &g.file {
        Some(f) if !g.surface.contains(f.as_str()) => format!("  - `{f}`: {}", g.surface),
        _ => format!("  - {}", g.surface),
    }
}

fn unique_lines<'a>(gaps: impl Iterator<Item = &'a Gap>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in gaps.map(line) {
        if !out.contains(&l) {
            out.push(l);
        }
    }
    out
}

/// The report's two coverage lists, written by the harness rather than left
/// to the report's writer. Empty when no lens filed a `budget` or a
/// `needs-execution` gap.
#[must_use]
pub fn coverage_banner(gaps: &[Gap], unread_risk: &[String]) -> String {
    let budget = unique_lines(gaps.iter().filter(|g| g.kind == GapKind::Budget));
    let by_hand = unique_lines(gaps.iter().filter(|g| g.kind == GapKind::NeedsExecution));
    let mut out = String::new();
    if !budget.is_empty() {
        out.push_str("\n\n---\n\n**Not reviewed within the lens budget.** A lens meant to read these and ran out of room:\n\n");
        out.push_str(&budget.join("\n"));
        if !unread_risk.is_empty() {
            let files: Vec<String> = unread_risk.iter().map(|f| format!("`{f}`")).collect();
            out.push_str(&format!(
                "\n\nNo lens read {}, which the profile lists as a risk path, so this review cannot approve the PR.",
                files.join(", ")
            ));
        }
    }
    if !by_hand.is_empty() {
        out.push_str("\n\n---\n\n**Check by hand.** Reading the code cannot settle these:\n\n");
        out.push_str(&by_hand.join("\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::profile_lookup::risk_rules_of;

    fn gap(file: &str, kind: &str) -> Value {
        json!({"surface": format!("callers of x in {file}"), "file": file, "kind": kind})
    }

    fn results(per_lens: Vec<Vec<Value>>) -> Vec<Option<Value>> {
        per_lens
            .into_iter()
            .map(|g| Some(json!({"findings": [], "notReached": g})))
            .collect()
    }

    fn rules() -> Vec<RiskRule> {
        risk_rules_of(Some(
            &json!([{"focus": "security", "pattern": "src/auth/"}]),
        ))
        .unwrap()
    }

    #[test]
    fn strings_and_objects_are_read_and_blanks_dropped() {
        let r = vec![
            Some(
                json!({"notReached": ["  src/a.ts ", "", 7, {"surface": "the job", "kind": "budget", "file": "src/b.ts"}, {"kind": "budget"}]}),
            ),
            None,
        ];
        let g = collect_gaps(&r, 2);
        assert_eq!(g.len(), 2);
        assert_eq!(
            (g[0].surface.as_str(), g[0].kind),
            ("src/a.ts", GapKind::Unstated)
        );
        assert_eq!(
            (g[1].file.as_deref(), g[1].kind),
            (Some("src/b.ts"), GapKind::Budget)
        );
        assert_eq!(kind_counts(&g)["budget"], 1);
        assert_eq!(kind_counts(&g)["unstated"], 1);
    }

    #[test]
    fn a_risk_file_is_unread_only_when_every_lens_that_held_it_skipped_it() {
        let f = "src/auth/token.ts";
        let both = collect_gaps(
            &results(vec![vec![gap(f, "budget")], vec![gap(f, "budget")]]),
            2,
        );
        assert_eq!(unread_risk_files(&both, &[None, None], &rules()), [f]);

        let one = collect_gaps(&results(vec![vec![gap(f, "budget")], vec![]]), 2);
        assert!(unread_risk_files(&one, &[None, None], &rules()).is_empty());

        let scoped = vec![
            Some(vec![f.to_owned()]),
            Some(vec!["src/other.ts".to_owned()]),
        ];
        assert_eq!(unread_risk_files(&one, &scoped, &rules()), [f]);

        let not_risk = collect_gaps(&results(vec![vec![gap("src/ui.ts", "budget")]]), 1);
        assert!(unread_risk_files(&not_risk, &[None], &rules()).is_empty());

        let other_kind = collect_gaps(&results(vec![vec![gap(f, "out-of-focus")]]), 1);
        assert!(unread_risk_files(&other_kind, &[None], &rules()).is_empty());
    }

    #[test]
    fn the_banner_lists_budget_and_by_hand_gaps_only() {
        let g = collect_gaps(
            &results(vec![vec![
                gap("src/a.ts", "budget"),
                gap("src/a.ts", "budget"),
                json!({"surface": "run the migration against a copy", "kind": "needs-execution"}),
                gap("src/c.ts", "outside-pr"),
            ]]),
            1,
        );
        let b = coverage_banner(&g, &["src/auth/token.ts".to_owned()]);
        assert_eq!(b.matches("callers of x in src/a.ts").count(), 1);
        assert!(b.contains("run the migration against a copy"));
        assert!(!b.contains("src/c.ts"));
        assert!(b.contains("cannot approve"));
        assert!(coverage_banner(&[], &[]).is_empty());
    }
}
