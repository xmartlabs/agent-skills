use serde_json::json;
use tribunal_pipeline::plan::{build_plan, PlanInput};

fn plan(
    files: Vec<String>,
    churn: Vec<serde_json::Value>,
    profile: serde_json::Value,
    mode: Option<&str>,
    lenses: Option<serde_json::Value>,
) -> tribunal_pipeline::plan::Plan {
    let pr = json!({"files":files,"fileCount":files.len(),"additions":churn.iter().map(|x| x["churn"].as_f64().unwrap_or(0.0)).sum::<f64>(),"deletions":0,"fileChurn":churn});
    build_plan(&PlanInput {
        pr_context: &pr,
        bookkeeping: None,
        wants_probe: false,
        force_full: false,
        pinned_sha: "",
        lenses: lenses.as_ref(),
        profile: &profile,
        light_no_logic: false,
        partition_mode: mode,
    })
    .unwrap()
}

fn many_files() -> Vec<String> {
    (0..18)
        .map(|i| format!("apps/prototypes/p{i}/src.ts"))
        .collect()
}

#[test]
fn group_by_extracts_cluster_keys_and_warns_for_invalid_entries() {
    let files = vec!["apps/prototypes/a/src.ts".into(), "solo.rs".into()];
    let plan = plan(
        files,
        vec![],
        json!({"groupBy":["apps/prototypes/*/", "missing", "two/*/parts/*/", 7, "bad*segment/"]}),
        None,
        None,
    );
    assert_eq!(plan.to_value()["partitionMode"], "cohesion");
    assert_eq!(
        plan.to_value()["clusters"],
        json!({"0":["apps/prototypes/a/"]})
    );
    assert!(plan.logs.iter().any(|line| line.contains("groupBy")));
}

#[test]
fn group_by_uses_the_first_matching_pattern() {
    let files = vec!["apps/prototypes/a/src.ts".into()];
    let p = plan(
        files,
        vec![],
        json!({"groupBy":["apps/*/", "apps/prototypes/*/"]}),
        None,
        None,
    );
    assert_eq!(p.to_value()["clusters"]["0"], json!(["apps/prototypes/"]));
}

#[test]
fn cohesion_places_clusters_whole_and_keeps_lone_files_as_units() {
    let files = (0..18)
        .map(|i| format!("apps/prototypes/p{}/f{i}.rs", i / 2))
        .collect::<Vec<_>>();
    let churn: Vec<_> = files.iter().map(|f| json!({"file":f,"churn":10})).collect();
    let p = plan(
        files,
        churn,
        json!({"groupBy":["apps/prototypes/*/"]}),
        None,
        None,
    );
    assert_eq!(p.to_value()["partitionMode"], "cohesion");
    for key in (0..9).map(|i| format!("apps/prototypes/p{i}/")) {
        let containing = p
            .file_groups
            .iter()
            .filter(|g| g.iter().any(|f| f.starts_with(&key)))
            .count();
        assert_eq!(containing, 1, "cluster {key} split across groups");
    }
}

#[test]
fn cohesion_keeps_churn_lens_count_and_foci_per_slot() {
    // Two files per cluster, so churn's per-file LPT splits clusters and
    // cohesion keeps them whole: only the groups' files may differ.
    let files: Vec<String> = (0..18)
        .map(|i| format!("apps/prototypes/p{}/f{i}.rs", i / 2))
        .collect();
    let churn: Vec<_> = files.iter().map(|f| json!({"file":f,"churn":10})).collect();
    let profile = json!({"groupBy":["apps/prototypes/*/"]});
    let cohesion = plan(files.clone(), churn.clone(), profile, None, None);
    let churn_plan = plan(files, churn, json!({}), Some("churn"), None);
    assert_eq!(cohesion.lens_count, churn_plan.lens_count);
    assert_eq!(cohesion.lens_plan.len(), churn_plan.lens_plan.len());
    for (cohesion_slot, churn_slot) in cohesion.lens_plan.iter().zip(&churn_plan.lens_plan) {
        assert_eq!(cohesion_slot.focus, churn_slot.focus);
    }
    assert_ne!(cohesion.file_groups, churn_plan.file_groups);
}

#[test]
fn churn_override_and_cohesion_without_profile_preserve_churn_partition() {
    let files = many_files();
    let churn: Vec<_> = files.iter().map(|f| json!({"file":f,"churn":10})).collect();
    let base = plan(files.clone(), churn.clone(), json!({}), None, None);
    let override_plan = plan(
        files.clone(),
        churn.clone(),
        json!({"groupBy":["apps/prototypes/*/"]}),
        Some("churn"),
        None,
    );
    let no_profile = plan(files, churn, json!({}), Some("cohesion"), None);
    assert_eq!(override_plan.file_groups, base.file_groups);
    assert_eq!(override_plan.lens_plan, base.lens_plan);
    assert_eq!(no_profile.file_groups, base.file_groups);
    assert!(no_profile
        .logs
        .iter()
        .any(|line| line.contains("cohesion") && line.contains("churn")));
}

#[test]
fn oversized_clusters_split_to_individual_files_and_single_group_ignores_cohesion() {
    let mut files = many_files();
    files.push("apps/prototypes/huge/a.rs".into());
    files.push("apps/prototypes/huge/b.rs".into());
    let churn: Vec<_> = files
        .iter()
        .map(|f| json!({"file":f,"churn":if f.contains("huge") {1000} else {1}}))
        .collect();
    let p = plan(
        files,
        churn,
        json!({"groupBy":["apps/prototypes/*/"]}),
        None,
        Some(json!(2)),
    );
    assert_eq!(p.group_count, 2.0);
    assert!(
        p.file_groups
            .iter()
            .filter(|g| g.iter().any(|f| f.contains("huge")))
            .count()
            > 1
    );
    assert_eq!(
        p.lens_count, 2.0,
        "--lenses retains exact ablation lens count"
    );
}

#[test]
fn one_group_keeps_churn_lens_plan_shape_and_lone_files_are_units() {
    let only = vec!["apps/prototypes/a/src.rs".into()];
    let clustered = plan(
        only,
        vec![],
        json!({"groupBy":["apps/prototypes/*/"]}),
        None,
        None,
    );
    let churn = plan(
        vec!["apps/prototypes/a/src.rs".into()],
        vec![],
        json!({}),
        None,
        None,
    );
    assert_eq!(clustered.group_count, 1.0);
    assert_eq!(clustered.lens_count, churn.lens_count);
    assert_eq!(clustered.lens_plan, churn.lens_plan);

    let files = (0..18)
        .map(|i| format!("src/file{i}.rs"))
        .collect::<Vec<_>>();
    let churn: Vec<_> = files.iter().map(|f| json!({"file":f,"churn":10})).collect();
    let p = plan(
        files.clone(),
        churn,
        json!({"groupBy":["apps/prototypes/*/"]}),
        None,
        None,
    );
    assert_eq!(
        p.file_groups
            .iter()
            .flatten()
            .cloned()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        files.len()
    );
}
