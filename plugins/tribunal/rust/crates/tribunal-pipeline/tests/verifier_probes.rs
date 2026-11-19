//! Probes from the independent verification of the cohesion stack
//! (lightPaths, groupBy partition, report by area), kept as regression tests.
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use tribunal_core::profile::group_by;
use tribunal_core::report::cluster_sections::cluster_sections;
use tribunal_pipeline::plan::{build_plan, Plan, PlanInput};

fn plan_with(
    files: &[String],
    churn: &[(String, f64)],
    profile: &Value,
    mode: Option<&str>,
    lenses: Option<&Value>,
    light_no_logic: bool,
) -> Plan {
    let total: f64 = churn.iter().map(|(_, c)| c).sum();
    let pr = json!({"files": files, "fileCount": files.len(), "additions": total, "deletions": 0,
        "fileChurn": churn.iter().map(|(f, c)| json!({"file": f, "churn": c})).collect::<Vec<_>>()});
    build_plan(&PlanInput {
        pr_context: &pr,
        bookkeeping: None,
        wants_probe: false,
        force_full: false,
        pinned_sha: "",
        lenses,
        profile,
        light_no_logic,
        partition_mode: mode,
    })
    .unwrap()
}

fn foci() -> Value {
    json!({"correctness": "correctness and logic", "dataContract": "data contracts",
        "security": "security and authorization", "performance": "performance", "errorHandling": "error handling"})
}

/// xorshift64: deterministic inputs without a dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[test]
fn s1_opt_in_plan_has_no_new_keys() {
    let files: Vec<String> = (0..20).map(|i| format!("src/f{i}.rs")).collect();
    let churn: Vec<_> = files.iter().map(|f| (f.clone(), 30.0)).collect();
    let v = plan_with(&files, &churn, &json!({"foci": foci()}), None, None, false).to_value();
    for key in ["lightFiles", "partitionMode", "clusters"] {
        assert!(v.get(key).is_none(), "{key}");
    }
}

#[test]
fn s1_light_file_gets_no_lens_in_a_single_group_plan() {
    let files = vec![
        "src/a.rs".to_owned(),
        "src/b.rs".to_owned(),
        "assets/logo.png".to_owned(),
    ];
    let churn = vec![
        ("src/a.rs".to_owned(), 40.0),
        ("src/b.rs".to_owned(), 40.0),
        ("assets/logo.png".to_owned(), 5.0),
    ];
    let p = plan_with(
        &files,
        &churn,
        &json!({"foci": foci(), "lightPaths": ["^assets/"]}),
        None,
        None,
        false,
    );
    assert_eq!(p.light_files, ["assets/logo.png"]);
    assert!(
        p.lens_plan.iter().all(|s| s
            .files
            .as_ref()
            .is_some_and(|f| !f.contains(&"assets/logo.png".to_owned()))),
        "a lens with no file list reviews the whole diff: {:?}",
        p.lens_plan
    );
}

#[test]
fn s1_risk_wins_case_insensitive_and_warnings() {
    let files = vec![
        "src/Auth/Login.SNAP".to_owned(),
        "ASSETS/X.PNG".to_owned(),
        "src/a.rs".to_owned(),
    ];
    let churn: Vec<_> = files.iter().map(|f| (f.clone(), 10.0)).collect();
    let profile = json!({"foci": foci(), "riskPaths": [{"pattern": "auth/", "focus": "security"}], "lightPaths": ["\\.snap$", "^assets/", 7]});
    let p = plan_with(&files, &churn, &profile, None, None, false);
    assert_eq!(p.light_files, ["ASSETS/X.PNG"]);
    assert!(p.files.contains(&"src/Auth/Login.SNAP".to_owned()));
    assert!(p.logs.iter().any(|l| l.contains("lightPaths[2]")));
    let not_array = plan_with(
        &files,
        &churn,
        &json!({"lightPaths": "x"}),
        None,
        None,
        false,
    );
    assert!(
        not_array.light_files.is_empty() && not_array.logs.iter().any(|l| l.contains("lightPaths"))
    );
}

#[test]
fn s2_light_no_logic_is_opt_in() {
    let files = vec![
        "Cargo.lock".to_owned(),
        "src/a.rs".to_owned(),
        "docs/x.md".to_owned(),
    ];
    let churn: Vec<_> = files.iter().map(|f| (f.clone(), 10.0)).collect();
    let off = plan_with(&files, &churn, &json!({}), None, None, false);
    assert_eq!(off.files.len(), 3);
    assert!(off.light_files.is_empty());
    let on = plan_with(&files, &churn, &json!({}), None, None, true);
    assert_eq!(on.files, ["src/a.rs"]);
    assert_eq!(on.light_files, ["Cargo.lock", "docs/x.md"]);
    assert_eq!(on.file_count, 1.0);
    assert_eq!(on.churn, 10.0);
}

#[test]
fn s3_s10_s14_random_property() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let prof = |with: bool| {
        let mut p = json!({"foci": foci()});
        if with {
            p["groupBy"] = json!(["apps/proto/*/"]);
        }
        p
    };
    for case in 0..400 {
        let n_files = 13 + rng.below(60) as usize;
        let n_clusters = 1 + rng.below(8);
        let (mut files, mut churn) = (Vec::new(), Vec::new());
        for i in 0..n_files {
            let f = if rng.below(4) == 0 {
                format!("lone/f{i}.rs")
            } else {
                format!("apps/proto/c{}/f{i}.tsx", rng.below(n_clusters))
            };
            let c = if rng.below(10) == 0 {
                0.0
            } else {
                (1 + rng.below(400)) as f64
            };
            churn.push((f.clone(), c));
            files.push(f);
        }
        let lenses = (rng.below(3) == 0).then(|| json!(1 + rng.below(6)));
        let coh = plan_with(&files, &churn, &prof(true), None, lenses.as_ref(), false);
        let forced = plan_with(
            &files,
            &churn,
            &prof(true),
            Some("churn"),
            lenses.as_ref(),
            false,
        );
        let plain = plan_with(&files, &churn, &prof(false), None, lenses.as_ref(), false);
        assert_eq!(forced.to_value(), plain.to_value(), "case {case}");
        assert_eq!(
            (coh.lens_count, coh.group_count, coh.lens_plan.len()),
            (plain.lens_count, plain.group_count, plain.lens_plan.len()),
            "case {case}"
        );
        let g = coh.file_groups.len();
        for (i, (a, b)) in coh.lens_plan.iter().zip(&plain.lens_plan).enumerate() {
            assert_eq!(a.focus, b.focus, "case {case} slot {i}");
            if coh.group_count > 1.0 {
                assert_eq!(a.files.as_ref(), Some(&coh.file_groups[i % g]));
            } else {
                assert!(a.files.is_none());
            }
        }
        let mut placed: Vec<&String> = coh.file_groups.iter().flatten().collect();
        placed.sort();
        let mut expected: Vec<&String> = files.iter().collect();
        expected.sort();
        assert_eq!(placed, expected, "case {case}");
        if g > 1 {
            assert!(
                coh.file_groups.iter().all(|grp| !grp.is_empty()),
                "case {case}: empty group"
            );
            let avg = churn.iter().map(|(_, c)| c).sum::<f64>() / g as f64;
            let mut by: BTreeMap<String, (f64, BTreeSet<usize>)> = BTreeMap::new();
            for (gi, grp) in coh.file_groups.iter().enumerate() {
                for f in grp {
                    if let Some(rest) = f.strip_prefix("apps/proto/") {
                        let e = by
                            .entry(rest.split('/').next().unwrap().to_owned())
                            .or_default();
                        e.0 += churn.iter().find(|(x, _)| x == f).unwrap().1;
                        e.1.insert(gi);
                    }
                }
            }
            for (key, (sum, groups)) in by {
                if sum <= avg {
                    assert_eq!(groups.len(), 1, "case {case}: {key} split");
                } else {
                    assert!(
                        groups.len() <= ((sum / avg).ceil() as usize).min(g),
                        "case {case}: {key}"
                    );
                }
            }
        }
    }
}

#[test]
fn s3_cohesion_without_group_by_falls_back_with_a_log() {
    let files: Vec<String> = (0..20).map(|i| format!("src/f{i}.rs")).collect();
    let churn: Vec<_> = files.iter().map(|f| (f.clone(), 30.0)).collect();
    let p = plan_with(
        &files,
        &churn,
        &json!({"groupBy": ["no-star/"]}),
        Some("cohesion"),
        None,
        false,
    );
    assert_eq!(
        p.file_groups,
        plan_with(&files, &churn, &json!({}), None, None, false).file_groups
    );
    assert!(
        p.logs.iter().any(|l| l.contains("Invalid groupBy entry"))
            && p.logs.iter().any(|l| l.contains("using churn"))
    );
}

#[test]
fn s5_every_finding_is_listed_once() {
    let (rules, _) = group_by::rules(&json!({"groupBy": ["apps/*/"]}));
    let findings = vec![
        json!({"file": "apps/a/x.ts", "title": "InA", "line": 1, "severity": "important"}),
        json!({"title": "NoFile", "severity": "nit"}),
        json!({"file": "apps/zz/y.ts", "title": "ClusterNotInPr", "line": 2, "severity": "nit"}),
    ];
    let text = cluster_sections(
        &findings,
        &["apps/a/x.ts".into(), "apps/b/q.ts".into()],
        &rules,
    );
    assert!(text.contains("**`apps/b/`**\n- No findings."));
    for t in ["InA", "NoFile", "ClusterNotInPr"] {
        assert_eq!(text.matches(t).count(), 1, "{t}:\n{text}");
    }
}

#[test]
fn s3_s5_a_file_beside_the_star_directory_has_no_cluster() {
    let (rules, _) = group_by::rules(&json!({"groupBy": ["src/features/*/"]}));
    assert_eq!(
        group_by::cluster_key("src/features/billing/a.ts", &rules).as_deref(),
        Some("src/features/billing/")
    );
    assert_eq!(group_by::cluster_key("src/features/index.ts", &rules), None);
    let text = cluster_sections(
        &[
            json!({"file": "src/features/index.ts", "title": "Barrel", "line": 1, "severity": "nit"}),
        ],
        &[
            "src/features/index.ts".into(),
            "src/features/billing/a.ts".into(),
        ],
        &rules,
    );
    assert!(
        !text.contains("index.ts/") && text.contains("**Outside every area**\n- nit Barrel"),
        "{text}"
    );
}

#[test]
fn s5_a_finding_without_a_file_has_no_empty_location() {
    let (rules, _) = group_by::rules(&json!({"groupBy": ["apps/*/"]}));
    let text = cluster_sections(
        &[json!({"title": "NoFile", "severity": "nit"})],
        &["apps/a/x.ts".into()],
        &rules,
    );
    assert!(text.contains("- nit NoFile"), "{text}");
    assert!(!text.contains("(``") && !text.contains(":?)"), "{text}");
}
