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
fn oversized_clusters_split_and_single_group_preserves_the_lens_plan() {
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
fn oversized_cluster_uses_only_the_fewest_groups_that_fit() {
    let mut files = (0..4)
        .map(|i| format!("apps/prototypes/large/f{i}.rs"))
        .collect::<Vec<_>>();
    files.extend((0..10).map(|i| format!("src/solo{i}.rs")));
    let churn: Vec<_> = files
        .iter()
        .map(|f| json!({"file":f,"churn":if f.contains("large") {30} else {10}}))
        .collect();
    let p = plan(
        files.clone(),
        churn,
        json!({"groupBy":["apps/prototypes/*/"]}),
        None,
        None,
    );
    assert_eq!(p.group_count, 3.0);
    let containing = p
        .file_groups
        .iter()
        .filter(|g| g.iter().any(|f| f.contains("large")))
        .count();
    assert_eq!(
        containing, 2,
        "cluster churn 120 / average load 73.3 requires two groups"
    );
    let placed = p
        .file_groups
        .iter()
        .flatten()
        .filter(|f| f.contains("large"))
        .count();
    assert_eq!(
        placed, 4,
        "every large-cluster file must be placed exactly once"
    );
    assert!(p.file_groups.iter().enumerate().all(|(i, g)| {
        !g.iter().any(|f| f.contains("large"))
            || p.to_value()["clusters"][i.to_string()]
                .as_array()
                .is_some_and(|ks| ks.contains(&json!("apps/prototypes/large/")))
    }));
    let all = p.file_groups.iter().flatten().cloned().collect::<Vec<_>>();
    assert_eq!(all.len(), files.len());
    assert_eq!(
        all.iter().collect::<std::collections::HashSet<_>>().len(),
        files.len()
    );
}

#[test]
fn unknown_churn_lpt_keeps_every_group_nonempty_when_files_cover_groups() {
    let files = (0..18)
        .map(|i| format!("apps/prototypes/p{}/f{i}.rs", i % 2))
        .collect::<Vec<_>>();
    let p = plan(
        files.clone(),
        vec![],
        json!({"groupBy":["apps/prototypes/*/"]}),
        None,
        Some(json!(3)),
    );

    assert!(p.file_groups.iter().all(|group| !group.is_empty()));
    let placed = p.file_groups.iter().flatten().cloned().collect::<Vec<_>>();
    assert_eq!(placed.len(), files.len());
    assert_eq!(
        placed
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        files.len()
    );
}

#[test]
fn cluster_with_at_least_eighty_percent_churn_uses_every_group() {
    let mut files = (0..4)
        .map(|i| format!("apps/prototypes/dominant/f{i}.rs"))
        .collect::<Vec<_>>();
    files.extend((0..10).map(|i| format!("src/solo{i}.rs")));
    let churn: Vec<_> = files
        .iter()
        .map(|f| json!({"file":f,"churn":if f.contains("dominant") {100} else {5}}))
        .collect();
    let p = plan(
        files,
        churn,
        json!({"groupBy":["apps/prototypes/*/"]}),
        None,
        None,
    );
    let containing = p
        .file_groups
        .iter()
        .filter(|g| g.iter().any(|f| f.contains("dominant")))
        .count();
    assert_eq!(p.group_count, 3.0);
    assert_eq!(
        containing, 3,
        "a cluster holding over 80% of churn needs all three groups"
    );
}

#[test]
fn oversized_cluster_keeps_churn_lens_count_and_foci_and_conserves_files() {
    let mut files = (0..4)
        .map(|i| format!("apps/prototypes/large/f{i}.rs"))
        .collect::<Vec<_>>();
    files.extend((0..10).map(|i| format!("src/solo{i}.rs")));
    let churn: Vec<_> = files
        .iter()
        .map(|f| json!({"file":f,"churn":if f.contains("large") {30} else {10}}))
        .collect();
    let cohesion = plan(
        files.clone(),
        churn.clone(),
        json!({"groupBy":["apps/prototypes/*/"]}),
        None,
        None,
    );
    let churn_plan = plan(files.clone(), churn, json!({}), Some("churn"), None);
    assert_eq!(cohesion.lens_count, churn_plan.lens_count);
    assert_eq!(cohesion.lens_plan.len(), churn_plan.lens_plan.len());
    for (cohesion_slot, churn_slot) in cohesion.lens_plan.iter().zip(&churn_plan.lens_plan) {
        assert_eq!(cohesion_slot.focus, churn_slot.focus);
    }
    let placed = cohesion
        .file_groups
        .iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(placed.len(), files.len());
    assert_eq!(
        placed
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        files.len()
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

#[test]
fn a_file_beside_the_star_directory_has_no_cluster() {
    use tribunal_core::profile::group_by;
    let (rules, _) = group_by::rules(&json!({"groupBy": ["src/features/*/"]}));
    assert_eq!(
        group_by::cluster_key("src/features/billing/a.ts", &rules).as_deref(),
        Some("src/features/billing/")
    );
    assert_eq!(group_by::cluster_key("src/features/index.ts", &rules), None);
    assert_eq!(group_by::cluster_key("src/features", &rules), None);
}
