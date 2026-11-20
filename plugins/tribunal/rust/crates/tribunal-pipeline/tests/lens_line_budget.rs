use serde_json::{json, Value};
use tribunal_pipeline::plan::{build_plan, Plan, PlanInput};

fn foci() -> Value {
    json!({"correctness": "correctness", "dataContract": "contracts", "security": "security", "performance": "performance", "errorHandling": "errors"})
}

fn files(count: usize) -> Vec<String> {
    (0..count)
        .map(|i| format!("src/feature{i}/file.rs"))
        .collect()
}

fn plan(
    files: &[String],
    churn_each: f64,
    profile: &Value,
    lenses: Option<&Value>,
    bookkeeping: Option<&Value>,
) -> Plan {
    let churn = files
        .iter()
        .map(|f| json!({"file": f, "churn": churn_each}))
        .collect::<Vec<_>>();
    let total = churn_each * files.len() as f64;
    let pr = json!({"files": files, "fileCount": files.len(), "additions": total, "deletions": 0,
        "fileChurn": churn});
    build_plan(&PlanInput {
        pr_context: &pr,
        bookkeeping,
        wants_probe: bookkeeping.is_some(),
        force_full: false,
        pinned_sha: "",
        lenses,
        profile,
        light_no_logic: false,
        partition_mode: None,
    })
    .unwrap()
}

#[test]
fn small_churn_caps_thirty_five_files_to_one_lens_and_group() {
    let files = files(35);
    let p = plan(
        &files,
        2.0,
        &json!({"foci": foci(), "lensLineBudget": 300}),
        None,
        None,
    );
    assert_eq!(p.lens_count, 1.0);
    assert_eq!(p.group_count, 1.0);
    assert!(p.uncovered_files.is_empty());
    assert!(p.to_value().get("lensLineBudget").is_some());
}

#[test]
fn large_churn_budget_never_increases_todays_lens_count() {
    let files = files(35);
    let baseline = plan(&files, 200.0, &json!({"foci": foci()}), None, None);
    let capped = plan(
        &files,
        200.0,
        &json!({"foci": foci(), "lensLineBudget": 1}),
        None,
        None,
    );
    assert!(capped.lens_count <= baseline.lens_count);
}

#[test]
fn absent_budget_preserves_plan_json_with_unrelated_profile_fields() {
    let files = files(15);
    let a = plan(&files, 20.0, &json!({"foci": foci()}), None, None).to_value();
    let b = plan(
        &files,
        20.0,
        &json!({"foci": foci(), "unrelated": true}),
        None,
        None,
    )
    .to_value();
    assert_eq!(a, b);
    assert!(a.get("lensLineBudget").is_none());
}

#[test]
fn explicit_lenses_override_ignores_the_profile_budget() {
    let files = files(35);
    let p = plan(
        &files,
        2.0,
        &json!({"foci": foci(), "lensLineBudget": 300}),
        Some(&json!(3)),
        None,
    );
    assert_eq!(p.lens_count, 3.0);
}

#[test]
fn invalid_line_budgets_warn_and_are_ignored() {
    let files = files(35);
    for value in [json!(0), json!(-2), json!(1.5), json!("300"), Value::Null] {
        let p = plan(
            &files,
            2.0,
            &json!({"foci": foci(), "lensLineBudget": value}),
            None,
            None,
        );
        assert!(
            p.logs.iter().any(|log| log.contains("lensLineBudget")),
            "{:?}",
            p.logs
        );
        assert_eq!(
            p.lens_count,
            plan(&files, 2.0, &json!({"foci": foci()}), None, None).lens_count
        );
        assert!(p.to_value().get("lensLineBudget").is_none());
    }
}

#[test]
fn incremental_scope_uses_its_in_scope_churn_for_the_budget() {
    let files = files(35);
    let bookkeeping = json!({"prior":{"found":true,"sha":"123456789"},
        "scope":{"usable":true,"inScope":files,"changed":files,"added":[],"unchanged":[],"inScopeChurn":100}});
    let p = plan(
        &files,
        500.0,
        &json!({"foci": foci(), "lensLineBudget": 300}),
        None,
        Some(&bookkeeping),
    );
    assert_eq!(p.churn, 100.0);
    assert_eq!(p.lens_count, 1.0);
}

#[test]
fn grouped_files_are_placed_once_under_a_line_budget() {
    let files = files(35);
    let p = plan(
        &files,
        2.0,
        &json!({"foci": foci(), "groupBy": ["src/*/"], "lensLineBudget": 300}),
        None,
        None,
    );
    assert_eq!(p.lens_count, 1.0);
    assert_eq!(p.group_count, 1.0);
    assert!(p.uncovered_files.is_empty());
    let mut placed = p.file_groups.iter().flatten().cloned().collect::<Vec<_>>();
    placed.sort();
    let mut expected = files;
    expected.sort();
    assert_eq!(placed, expected);
}
