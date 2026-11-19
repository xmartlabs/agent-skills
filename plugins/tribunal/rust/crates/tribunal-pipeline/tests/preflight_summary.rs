//! The partition fields the preflight summary carries.
use serde_json::json;
use tribunal_pipeline::context::preflight_partition_fields;
use tribunal_pipeline::plan::{build_plan, Plan, PlanInput};

fn plan(profile: serde_json::Value) -> Plan {
    let files: Vec<String> = (0..18)
        .map(|i| format!("apps/prototypes/p{}/f{i}.rs", i / 2))
        .collect();
    let churn: Vec<_> = files
        .iter()
        .map(|f| json!({"file": f, "churn": 10}))
        .collect();
    let pr = json!({"files": files, "fileCount": 18, "additions": 180, "deletions": 0, "fileChurn": churn});
    build_plan(&PlanInput {
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
    .unwrap()
}

#[test]
fn a_cohesion_preflight_shows_groups_mode_and_clusters() {
    let p = plan(json!({"groupBy": ["apps/prototypes/*/"]}));
    let fields = preflight_partition_fields(&p);
    assert_eq!(fields["fileGroups"], json!(p.file_groups));
    assert_eq!(fields["partitionMode"], json!("cohesion"));
    assert_eq!(fields["clusters"], p.to_value()["clusters"]);
    assert!(fields["clusters"]
        .as_object()
        .is_some_and(|c| !c.is_empty()));
}

#[test]
fn a_plain_preflight_shows_only_the_groups() {
    let p = plan(json!({}));
    let fields = preflight_partition_fields(&p);
    assert_eq!(fields["fileGroups"], json!(p.file_groups));
    assert!(fields.get("partitionMode").is_none());
    assert!(fields.get("clusters").is_none());
    assert_eq!(fields.len(), 1);
}
