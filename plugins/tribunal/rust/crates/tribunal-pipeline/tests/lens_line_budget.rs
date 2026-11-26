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

#[test]
fn a_cap_keeps_a_forced_focus_and_the_correctness_lens() {
    // A budget that would leave one lens still runs the focus the risk path
    // forced and the correctness lens beside it.
    let files: Vec<String> = (0..20).map(|i| format!("src/auth/f{i}.rs")).collect();
    let churn: Vec<_> = files
        .iter()
        .map(|f| json!({"file": f, "churn": 1}))
        .collect();
    let pr = json!({"files": files, "fileCount": 20, "additions": 20, "deletions": 0, "fileChurn": churn});
    let mut profile = json!({"foci": foci(), "lensLineBudget": 1000});
    profile["riskPaths"] = json!([{"focus": "security", "pattern": "auth/"}]);
    let capped = build_plan(&PlanInput {
        pr_context: &pr,
        bookkeeping: None,
        wants_probe: false,
        force_full: false,
        pinned_sha: "",
        lenses: None,
        profile: &profile,
        light_no_logic: false,
        partition_mode: None,
    })
    .unwrap();
    assert_eq!(capped.lens_count, 2.0);
    let foci: Vec<_> = capped.lens_plan.iter().map(|l| l.focus.clone()).collect();
    assert_eq!(foci, [json!("security"), json!("correctness")]);
}

#[test]
fn a_cap_keeps_every_forced_focus_and_correctness_when_two_risk_paths_match() {
    let mut files: Vec<String> = (0..10).map(|i| format!("src/auth/f{i}.rs")).collect();
    files.extend((0..10).map(|i| format!("db/migrations/m{i}.sql")));
    let churn: Vec<_> = files
        .iter()
        .map(|f| json!({"file": f, "churn": 1}))
        .collect();
    let pr = json!({"files": files, "fileCount": 20, "additions": 20, "deletions": 0, "fileChurn": churn});
    let mut foci = foci();
    foci["migration"] = json!("migrations");
    let mut profile = json!({"foci": foci, "lensLineBudget": 1000});
    profile["riskPaths"] = json!([
        {"focus": "security", "pattern": "auth/"},
        {"focus": "migration", "pattern": "migrations/"}
    ]);
    let capped = build_plan(&PlanInput {
        pr_context: &pr,
        bookkeeping: None,
        wants_probe: false,
        force_full: false,
        pinned_sha: "",
        lenses: None,
        profile: &profile,
        light_no_logic: false,
        partition_mode: None,
    })
    .unwrap();
    let mut got: Vec<String> = capped
        .lens_plan
        .iter()
        .map(|l| l.focus.as_str().unwrap().to_owned())
        .collect();
    got.sort();
    assert_eq!(got, ["correctness", "migrations", "security"]);
}
