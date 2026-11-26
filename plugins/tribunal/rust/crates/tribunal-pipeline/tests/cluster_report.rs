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

fn findings() -> Vec<Value> {
    vec![
        json!({"file": "apps/insights/a.ts", "title": "t1", "severity": "critical", "line": 3}),
        json!({"file": "docs/readme.md", "title": "t2", "severity": "nit", "line": 1}),
    ]
}

#[test]
fn the_synthesizer_gets_the_findings_without_cluster_keys() {
    let (rules, _) = group_by::rules(&json!({"groupBy": ["apps/*/"]}));
    let (synthesis, keyed) =
        tribunal_core::report::cluster_sections::split_for_report(&findings(), &rules);
    assert_eq!(synthesis, findings());
    assert_eq!(keyed[0]["cluster"], "apps/insights/");
}

#[test]
fn cluster_keys_do_not_change_the_verdict_and_the_section_has_none() {
    let (rules, _) = group_by::rules(&json!({"groupBy": ["apps/*/"]}));
    let profile = json!({"riskPaths": []});
    let (plain, keyed) =
        tribunal_core::report::cluster_sections::split_for_report(&findings(), &rules);
    let a = tribunal_pipeline::synthesize::verdict(&[], &[], &plain, &profile, None, 0).unwrap();
    let b = tribunal_pipeline::synthesize::verdict(&[], &[], &keyed, &profile, None, 0).unwrap();
    assert_eq!((a.raw, a.guess), (b.raw, b.guess));
    let section = tribunal_core::report::cluster_sections::cluster_sections(
        &keyed,
        &["apps/insights/a.ts".into(), "docs/readme.md".into()],
        &rules,
    );
    for word in ["APPROVE", "REQUEST_CHANGES", "COMMENT", "Verdict"] {
        assert!(!section.contains(word), "{word} in {section}");
    }
}

#[test]
fn no_finding_areas_are_collapsed_into_one_path_ordered_line() {
    let (rules, _) = group_by::rules(&json!({"groupBy": ["apps/*/"]}));
    let section = tribunal_core::report::cluster_sections::cluster_sections(
        &[],
        &["apps/z/file.ts".into(), "apps/a/file.ts".into()],
        &rules,
    );
    assert!(section.contains("No findings in 2 area(s): `apps/a/`, `apps/z/`"));
    assert!(!section.contains("**`apps/a/`**"));
    assert!(!section.contains("**`apps/z/`**"));
}
