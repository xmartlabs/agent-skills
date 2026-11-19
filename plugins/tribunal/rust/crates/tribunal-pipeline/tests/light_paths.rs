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
fn light_files_are_excluded_from_groups_and_tier_inputs() {
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

#[test]
fn a_single_group_plan_scopes_lenses_away_from_light_files() {
    // With one file group, lenses get no file list and read the whole diff,
    // so a light file must still be kept out by listing the others.
    let pr = json!({"files":["src/a.rs","src/b.rs","assets/logo.png"],"fileCount":3,"additions":85,"deletions":0,"fileChurn":[{"file":"src/a.rs","churn":40},{"file":"src/b.rs","churn":40},{"file":"assets/logo.png","churn":5}]});
    let profile = json!({"lightPaths":["^assets/"]});
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
    assert_eq!(plan.group_count, 1.0);
    assert!(!plan.lens_plan.is_empty());
    for slot in &plan.lens_plan {
        assert_eq!(
            slot.files.as_deref(),
            Some(&["src/a.rs".to_owned(), "src/b.rs".to_owned()][..])
        );
    }
}

#[test]
fn a_single_group_plan_without_light_files_keeps_whole_diff_lenses() {
    let pr = json!({"files":["src/a.rs","src/b.rs"],"fileCount":2,"additions":80,"deletions":0,"fileChurn":[{"file":"src/a.rs","churn":40},{"file":"src/b.rs","churn":40}]});
    let plan = build_plan(&PlanInput {
        pr_context: &pr,
        bookkeeping: None,
        wants_probe: false,
        force_full: false,
        pinned_sha: "",
        lenses: None,
        profile: &json!({}),
        light_no_logic: false,
    })
    .unwrap();
    assert!(plan.lens_plan.iter().all(|slot| slot.files.is_none()));
}

#[test]
fn all_light_files_are_kept_and_reviewed_without_light_filtering() {
    let pr = json!({"files":["assets/a.png","dist/b.js"],"fileCount":2,"additions":20,"deletions":0,"fileChurn":[{"file":"assets/a.png","churn":10},{"file":"dist/b.js","churn":10}]});
    let profile = json!({"lightPaths":["^assets/","^dist/"]});
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
    assert_eq!(plan.files, ["assets/a.png", "dist/b.js"]);
    assert!(plan.light_files.is_empty());
    assert!(plan.logs.iter().any(|line| line
        == "Every changed file matches lightPaths or --light-no-logic; reviewing them anyway"));
    assert!(plan.lens_plan.iter().all(|slot| slot.files.is_none()));
}

#[test]
fn malformed_risk_paths_error_with_and_without_light_paths() {
    let pr = json!({"files":["src/a.rs"],"fileCount":1,"additions":1});
    for light_paths in [None, Some(json!(["^assets/"]))] {
        let mut profile = json!({"riskPaths":[null]});
        if let Some(rules) = light_paths {
            profile["lightPaths"] = rules;
        }
        let result = build_plan(&PlanInput {
            pr_context: &pr,
            bookkeeping: None,
            wants_probe: false,
            force_full: false,
            pinned_sha: "",
            lenses: None,
            profile: &profile,
            light_no_logic: false,
        });
        assert!(result.unwrap_err().contains("focus"));
    }
}

#[test]
fn an_all_light_pr_is_reviewed_anyway_with_the_log_line() {
    let pr = json!({"files":["assets/a.png","assets/b.png"],"fileCount":2,"additions":10,"deletions":0,
        "fileChurn":[{"file":"assets/a.png","churn":5},{"file":"assets/b.png","churn":5}]});
    let profile = json!({"lightPaths":["^assets/"]});
    let p = build_plan(&PlanInput {
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
    assert_eq!(p.files, ["assets/a.png", "assets/b.png"]);
    assert!(p.light_files.is_empty() && p.to_value().get("lightFiles").is_none());
    assert_eq!((p.file_count, p.churn), (2.0, 10.0));
    assert!(p.lens_plan.iter().all(|s| s.files.is_none()));
    assert!(p
        .logs
        .iter()
        .any(|l| l
            == "Every changed file matches lightPaths or --light-no-logic; reviewing them anyway"));
    assert!(!p
        .logs
        .iter()
        .any(|l| l.starts_with("Not reviewed by lenses")));
}
