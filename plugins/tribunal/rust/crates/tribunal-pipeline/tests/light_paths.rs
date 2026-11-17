use serde_json::json;
use tribunal_pipeline::plan::{build_plan, PlanInput};

#[test]
fn light_paths_are_removed_before_plan_accounting_and_recorded() {
    let pr = json!({"files":["src/a.rs","assets/logo.png"],"fileCount":2,"additions":45,"deletions":0,"fileChurn":[{"file":"src/a.rs","churn":40},{"file":"assets/logo.png","churn":5}]});
    let profile = json!({"lightPaths":["assets/.*\\.png"]});
    let plan = build_plan(&PlanInput {
        pr_context: &pr,
        bookkeeping: None,
        wants_probe: false,
        force_full: false,
        pinned_sha: "",
        lenses: None,
        profile: &profile,
        light_no_logic: false,
    })
    .unwrap();
    assert_eq!(plan.files, ["src/a.rs"]);
    assert_eq!(plan.file_count, 1.0);
    assert_eq!(plan.churn, 40.0);
    assert_eq!(plan.to_value()["lightFiles"], json!(["assets/logo.png"]));
    assert!(plan.uncovered_files.is_empty());
    assert!(plan
        .logs
        .iter()
        .any(|line| line == "Not reviewed by lenses (light): assets/logo.png"));
}

#[test]
fn invalid_light_path_entries_warn_and_are_ignored() {
    let pr = json!({"files":["src/a.rs"],"fileCount":1,"additions":1});
    let profile = json!({"lightPaths":["[", 7]});
    let plan = build_plan(&PlanInput {
        pr_context: &pr,
        bookkeeping: None,
        wants_probe: false,
        force_full: false,
        pinned_sha: "",
        lenses: None,
        profile: &profile,
        light_no_logic: false,
    })
    .unwrap();
    assert_eq!(plan.files, ["src/a.rs"]);
    assert!(plan.logs.iter().any(|s| s.contains("lightPaths")));
}

#[test]
fn light_paths_do_not_affect_lpt_groups_or_file_and_churn_tiers() {
    let files: Vec<String> = (0..13)
        .map(|i| format!("src/{i}.rs"))
        .chain(["assets/a.png".into()])
        .collect();
    let churn: Vec<_> = files
        .iter()
        .map(|file| json!({"file":file,"churn":10}))
        .collect();
    let pr = json!({"files":files,"fileCount":14,"additions":140,"deletions":0,"fileChurn":churn});
    let profile = json!({"lightPaths":["\\.png$"]});
    let plan = build_plan(&PlanInput {
        pr_context: &pr,
        bookkeeping: None,
        wants_probe: false,
        force_full: false,
        pinned_sha: "",
        lenses: None,
        profile: &profile,
        light_no_logic: false,
    })
    .unwrap();
    assert_eq!(plan.file_count, 13.0);
    assert_eq!(plan.churn, 130.0);
    assert_eq!(plan.group_count, 3.0);
    assert!(plan
        .file_groups
        .iter()
        .flatten()
        .all(|file| !file.ends_with(".png")));
    assert_eq!(plan.file_groups.iter().map(Vec::len).sum::<usize>(), 13);
    assert!(!plan.uncovered_files.iter().any(|f| f.ends_with(".png")));
}

#[test]
fn builtin_light_filter_is_opt_in_and_applies_inside_incremental_scope() {
    let pr = json!({"files":["src/a.rs","Cargo.lock","src/api.generated.rs"],"fileCount":3,"additions":30,"deletions":0,"fileChurn":[{"file":"src/a.rs","churn":10},{"file":"Cargo.lock","churn":10},{"file":"src/api.generated.rs","churn":10}]});
    let profile = json!({});
    let bk = json!({"prior":{"found":true,"sha":"123456789"},"scope":{"usable":true,"inScope":["src/a.rs","Cargo.lock","src/api.generated.rs"],"changed":["src/a.rs","Cargo.lock","src/api.generated.rs"],"added":[],"unchanged":[],"inScopeChurn":30}});
    let plan = build_plan(&PlanInput {
        pr_context: &pr,
        bookkeeping: Some(&bk),
        wants_probe: true,
        force_full: false,
        pinned_sha: "",
        lenses: None,
        profile: &profile,
        light_no_logic: true,
    })
    .unwrap();
    assert!(!plan.files.contains(&"Cargo.lock".to_owned()));
    assert_eq!(plan.light_files, ["Cargo.lock", "src/api.generated.rs"]);
    assert_eq!(plan.churn, 10.0);

    let flag_off = build_plan(&PlanInput {
        pr_context: &pr,
        bookkeeping: Some(&bk),
        wants_probe: true,
        force_full: false,
        pinned_sha: "",
        lenses: None,
        profile: &profile,
        light_no_logic: false,
    })
    .unwrap();
    assert!(flag_off.files.contains(&"Cargo.lock".to_owned()));
}

#[test]
fn a_risk_path_is_never_light() {
    let pr = json!({"files":["src/a.rs","src/auth/keys.lock"],"fileCount":2,"additions":20,"deletions":0,"fileChurn":[{"file":"src/a.rs","churn":10},{"file":"src/auth/keys.lock","churn":10}]});
    let profile = json!({
        "lightPaths": ["\\.lock$"],
        "riskPaths": [{"focus": "security", "pattern": "src/auth/"}]
    });
    let plan = build_plan(&PlanInput {
        pr_context: &pr,
        bookkeeping: None,
        wants_probe: false,
        force_full: false,
        pinned_sha: "",
        lenses: None,
        profile: &profile,
        light_no_logic: true,
    })
    .unwrap();
    assert_eq!(plan.files, ["src/a.rs", "src/auth/keys.lock"]);
    assert!(plan.light_files.is_empty());
    assert!(plan.to_value().get("lightFiles").is_none());
}
