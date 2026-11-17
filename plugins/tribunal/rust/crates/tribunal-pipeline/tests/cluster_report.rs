use serde_json::{json, Value};
use tribunal_core::profile::group_by;

#[test]
fn cluster_keys_are_added_only_when_valid_group_by_rules_exist() {
    let (rules, _) = group_by::rules(&json!({"groupBy": ["apps/*/"]}));
    let mut findings = vec![
        json!({"file": "apps/insights/a.ts"}),
        json!({"file": "docs/readme.md"}),
    ];
    tribunal_core::report::cluster_sections::add_cluster_keys(&mut findings, &rules);
    assert_eq!(findings[0]["cluster"], "apps/insights/");
    assert!(findings[1].get("cluster").is_none());

    let (no_rules, _) = group_by::rules(&json!({}));
    let mut plain: Vec<Value> = vec![json!({"file": "apps/insights/a.ts"})];
    tribunal_core::report::cluster_sections::add_cluster_keys(&mut plain, &no_rules);
    assert!(plain[0].get("cluster").is_none());
}
