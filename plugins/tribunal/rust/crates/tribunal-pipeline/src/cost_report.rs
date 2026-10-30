//! `tribunal cost`: where the money of past reviews went, read from the rows
//! in `metrics.jsonl`. Dollars are the CLI's own list-price estimate per
//! dispatch (`agentCost.costUsd`, `lensCost`), so a row written before those
//! fields existed counts as a run but adds no cost.

use std::collections::BTreeMap;

use serde_json::{json, Value};

/// Which rows to read: one repo (`owner/name`) and rows whose `ts` is at or
/// after `since` (an ISO date or instant, compared as text).
#[derive(Debug, Clone, Default)]
pub struct CostFilter {
    pub repo: Option<String>,
    pub since: Option<String>,
}

#[derive(Default)]
struct LensTotals {
    lenses: u64,
    cost: f64,
    forced: u64,
    over_budget: u64,
    over_budget_cost: f64,
    candidates: f64,
    survived: f64,
}

#[derive(Default)]
struct RepoTotals {
    runs: u64,
    priced: u64,
    cost: f64,
    by_kind: BTreeMap<String, f64>,
    lenses: BTreeMap<String, LensTotals>,
}

fn num(v: Option<&Value>) -> f64 {
    v.and_then(Value::as_f64).unwrap_or(0.0)
}

fn usd(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn per(cost: f64, n: f64) -> Value {
    if n > 0.0 {
        json!(usd(cost / n))
    } else {
        Value::Null
    }
}

fn keep(row: &Value, f: &CostFilter) -> bool {
    let repo_ok = f
        .repo
        .as_deref()
        .is_none_or(|r| row.get("repo").and_then(Value::as_str) == Some(r));
    let since_ok = f.since.as_deref().is_none_or(|s| {
        row.get("ts")
            .and_then(Value::as_str)
            .is_some_and(|ts| ts >= s)
    });
    repo_ok && since_ok
}

fn add_row(t: &mut RepoTotals, row: &Value) {
    t.runs += 1;
    let Some(cost) = row.pointer("/agentCost/costUsd").and_then(Value::as_f64) else {
        return;
    };
    t.priced += 1;
    t.cost += cost;
    if let Some(kinds) = row.pointer("/agentCost/byKind").and_then(Value::as_object) {
        for (k, v) in kinds {
            *t.by_kind.entry(k.clone()).or_default() += num(v.get("costUsd"));
        }
    }
    for l in row
        .get("lensCost")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let key = l.get("lens").and_then(Value::as_str).unwrap_or("unknown");
        let e = t.lenses.entry(key.to_owned()).or_default();
        let c = num(l.get("costUsd"));
        e.lenses += 1;
        e.cost += c;
        if l.get("why").and_then(Value::as_str) == Some("forced") {
            e.forced += 1;
        }
        if l.get("overBudget").and_then(Value::as_bool) == Some(true) {
            e.over_budget += 1;
            e.over_budget_cost += c;
        }
    }
    for y in row
        .get("lensYield")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let key = y.get("lens").and_then(Value::as_str).unwrap_or("unknown");
        if let Some(e) = t.lenses.get_mut(key) {
            e.candidates += num(y.get("candidates"));
            e.survived += num(y.get("survivedToReport"));
        }
    }
}

/// The report over `rows`, one entry per repo, most expensive first. A lens's
/// `costPerSurvived` divides what that focus cost by the findings it raised
/// that reached the report; it is null when none did.
#[must_use]
pub fn cost_report(rows: &[Value], f: &CostFilter) -> Value {
    let mut repos: BTreeMap<String, RepoTotals> = BTreeMap::new();
    for row in rows.iter().filter(|r| keep(r, f)) {
        let repo = row.get("repo").and_then(Value::as_str).unwrap_or("unknown");
        add_row(repos.entry(repo.to_owned()).or_default(), row);
    }
    let mut out: Vec<Value> = repos
        .into_iter()
        .map(|(repo, t)| {
            let mut lenses: Vec<(String, LensTotals)> = t.lenses.into_iter().collect();
            lenses.sort_by(|a, b| b.1.cost.total_cmp(&a.1.cost));
            json!({
                "repo": repo,
                "runs": t.runs,
                "pricedRuns": t.priced,
                "costUsd": usd(t.cost),
                "perRun": per(t.cost, t.priced as f64),
                "byKind": t.by_kind.iter().map(|(k, c)| (k.clone(), json!(usd(*c)))).collect::<serde_json::Map<_, _>>(),
                "lenses": lenses.iter().map(|(k, l)| json!({
                    "lens": k,
                    "lenses": l.lenses,
                    "costUsd": usd(l.cost),
                    "perLens": per(l.cost, l.lenses as f64),
                    "forced": l.forced,
                    "overBudget": l.over_budget,
                    "overBudgetCostUsd": usd(l.over_budget_cost),
                    "candidates": l.candidates,
                    "survived": l.survived,
                    "costPerSurvived": per(l.cost, l.survived),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    out.sort_by(|a, b| num(b.get("costUsd")).total_cmp(&num(a.get("costUsd"))));
    json!({ "repos": out })
}

fn money(v: Option<&Value>) -> String {
    v.and_then(Value::as_f64)
        .map_or_else(|| "-".to_owned(), |x| format!("${x:.2}"))
}

/// The report as text for a terminal.
#[must_use]
pub fn render(report: &Value) -> String {
    let mut s = String::new();
    let repos = report
        .get("repos")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if repos.is_empty() {
        return "No runs match.\n".to_owned();
    }
    for r in &repos {
        s += &format!(
            "{}: {} runs ({} priced), {} total, {} per run\n",
            r["repo"].as_str().unwrap_or("?"),
            r["runs"],
            r["pricedRuns"],
            money(r.get("costUsd")),
            money(r.get("perRun")),
        );
        if let Some(kinds) = r["byKind"].as_object().filter(|k| !k.is_empty()) {
            let mut kinds: Vec<_> = kinds.iter().collect();
            kinds.sort_by(|a, b| num(Some(b.1)).total_cmp(&num(Some(a.1))));
            let parts: Vec<String> = kinds
                .iter()
                .map(|(k, v)| format!("{k} {}", money(Some(v))))
                .collect();
            s += &format!("  by kind: {}\n", parts.join(", "));
        }
        let lenses = r["lenses"].as_array().cloned().unwrap_or_default();
        if !lenses.is_empty() {
            // `Value`'s Display ignores width, so every cell is a String.
            let line = |c: [String; 8]| {
                format!(
                    "  {:<15} {:>6} {:>8} {:>8} {:>6} {:>11} {:>16} {:>12}\n",
                    c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]
                )
            };
            s += &line(
                [
                    "lens",
                    "lenses",
                    "cost",
                    "per lens",
                    "forced",
                    "over budget",
                    "over budget cost",
                    "per survived",
                ]
                .map(str::to_owned),
            );
            for l in &lenses {
                s += &line([
                    l["lens"].as_str().unwrap_or("?").to_owned(),
                    l["lenses"].to_string(),
                    money(l.get("costUsd")),
                    money(l.get("perLens")),
                    l["forced"].to_string(),
                    l["overBudget"].to_string(),
                    money(l.get("overBudgetCostUsd")),
                    money(l.get("costPerSurvived")),
                ]);
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(repo: &str, ts: &str, cost: Option<f64>) -> Value {
        let mut r = json!({
            "repo": repo,
            "ts": ts,
            "lensYield": [
                {"lens": "correctness", "candidates": 3, "survivedToReport": 2},
                {"lens": "migration", "candidates": 1, "survivedToReport": 0},
            ],
        });
        if let Some(c) = cost {
            r["agentCost"] = json!({"costUsd": c, "byKind": {
                "lens": {"costUsd": 0.75}, "critic": {"costUsd": c - 0.75},
            }});
            r["lensCost"] = json!([
                {"lens": "migration", "why": "forced", "overBudget": false, "costUsd": 0.25},
                {"lens": "correctness", "why": "default", "overBudget": true, "costUsd": 0.5},
            ]);
        }
        r
    }

    #[test]
    fn sums_cost_per_repo_kind_and_lens() {
        let rows = [
            row("a/e2e", "2026-10-30T10:00:00Z", Some(1.0)),
            row("a/e2e", "2026-10-30T11:00:00Z", Some(2.0)),
            row("a/be", "2026-10-30T11:00:00Z", Some(0.9)),
        ];
        let r = cost_report(&rows, &CostFilter::default());
        let e2e = &r["repos"][0];
        assert_eq!(e2e["repo"], json!("a/e2e"));
        assert_eq!(e2e["costUsd"], json!(3.0));
        assert_eq!(e2e["perRun"], json!(1.5));
        assert_eq!(e2e["byKind"]["lens"], json!(1.5));
        assert_eq!(e2e["byKind"]["critic"], json!(1.5));
        let correctness = &e2e["lenses"][0];
        assert_eq!(correctness["lens"], json!("correctness"));
        assert_eq!(correctness["costUsd"], json!(1.0));
        assert_eq!(correctness["overBudget"], json!(2));
        assert_eq!(correctness["overBudgetCostUsd"], json!(1.0));
        assert_eq!(correctness["survived"], json!(4.0));
        assert_eq!(correctness["costPerSurvived"], json!(0.25));
        let migration = &e2e["lenses"][1];
        assert_eq!(migration["forced"], json!(2));
        assert_eq!(migration["costPerSurvived"], Value::Null);
        assert_eq!(r["repos"][1]["repo"], json!("a/be"));
    }

    #[test]
    fn a_row_without_cost_counts_as_a_run_but_adds_nothing() {
        let rows = [
            row("a/e2e", "2026-10-30", Some(1.0)),
            row("a/e2e", "2026-10-29", None),
        ];
        let r = cost_report(&rows, &CostFilter::default());
        assert_eq!(r["repos"][0]["runs"], json!(2));
        assert_eq!(r["repos"][0]["pricedRuns"], json!(1));
        assert_eq!(r["repos"][0]["perRun"], json!(1.0));
    }

    #[test]
    fn filters_by_repo_and_date() {
        let rows = [
            row("a/e2e", "2026-10-01T00:00:00Z", Some(1.0)),
            row("a/e2e", "2026-10-30T00:00:00Z", Some(2.0)),
            row("a/be", "2026-10-30T00:00:00Z", Some(5.0)),
        ];
        let f = CostFilter {
            repo: Some("a/e2e".into()),
            since: Some("2026-10-15".into()),
        };
        let r = cost_report(&rows, &f);
        assert_eq!(r["repos"].as_array().unwrap().len(), 1);
        assert_eq!(r["repos"][0]["costUsd"], json!(2.0));
    }

    #[test]
    fn renders_one_block_per_repo() {
        let r = cost_report(
            &[row("a/e2e", "2026-10-30", Some(1.0))],
            &CostFilter::default(),
        );
        let text = render(&r);
        assert!(
            text.starts_with("a/e2e: 1 runs (1 priced), $1.00 total, $1.00 per run\n"),
            "{text}"
        );
        assert!(text.contains("correctness"), "{text}");
        assert_eq!(render(&json!({"repos": []})), "No runs match.\n");
    }
}
