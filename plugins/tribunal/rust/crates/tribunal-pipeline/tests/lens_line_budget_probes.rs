//! Probes from the independent verification of the compact area line and
//! the opt-in lens line budget, kept as regression tests.
use serde_json::{json, Value};
use tribunal_core::profile::group_by;
use tribunal_core::report::cluster_sections::cluster_sections;
use tribunal_pipeline::plan::{build_plan, Plan, PlanInput};

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

fn foci() -> Value {
    json!({"correctness":"correctness and logic","dataContract":"data contracts",
        "security":"security and authorization","performance":"performance","errorHandling":"error handling"})
}

fn plan(pr: &Value, profile: &Value, lenses: Option<&Value>, bk: Option<&Value>) -> Plan {
    build_plan(&PlanInput {
        pr_context: pr,
        bookkeeping: bk,
        wants_probe: bk.is_some(),
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
fn line_budget_caps_never_raises_and_keeps_every_file_covered() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    for case in 0..1500 {
        let n = 1 + rng.below(80) as usize;
        let (mut files, mut churn, mut light_churn) = (Vec::new(), Vec::new(), 0.0);
        for i in 0..n {
            let f = match rng.below(5) {
                0 => format!("assets/a{i}.png"),
                1 => format!("pkg/m{}/f{i}.rs", rng.below(4)),
                _ => format!("src/f{i}.rs"),
            };
            let c = rng.below(400) as f64;
            if f.starts_with("assets/") {
                light_churn += c;
            }
            churn.push(json!({"file": f, "churn": c}));
            files.push(f);
        }
        let total: f64 = churn.iter().map(|c| c["churn"].as_f64().unwrap()).sum();
        let pr = json!({"files": files, "fileCount": n, "additions": total, "deletions": 0, "fileChurn": churn});
        let mut base = json!({"foci": foci(), "lightPaths": ["^assets/"]});
        if rng.below(2) == 0 {
            base["groupBy"] = json!(["pkg/*/"]);
        }
        let budget = 1 + rng.below(1500);
        let mut with = base.clone();
        with["lensLineBudget"] = json!(budget);
        let today = plan(&pr, &base, None, None);
        let capped = plan(&pr, &with, None, None);
        let relevant = if files.iter().all(|f| f.starts_with("assets/")) {
            total
        } else {
            total - light_churn
        };
        let cap = (relevant / budget as f64).ceil().max(1.0);
        assert_eq!(capped.lens_count, today.lens_count.min(cap), "case {case}");
        assert!(
            capped.group_count <= capped.lens_count.max(1.0),
            "case {case}"
        );
        assert!(capped.uncovered_files.is_empty(), "case {case}");
        let mut placed: Vec<&String> = capped.file_groups.iter().flatten().collect();
        placed.sort();
        let mut want: Vec<&String> = capped.files.iter().collect();
        want.sort();
        assert_eq!(placed, want, "case {case}");
        assert_eq!(capped.to_value()["lensLineBudget"], json!(budget));
        let forced = json!(1 + rng.below(6));
        assert_eq!(
            plan(&pr, &with, Some(&forced), None).lens_count,
            plan(&pr, &base, Some(&forced), None).lens_count
        );
    }
}

#[test]
fn invalid_line_budgets_warn_and_leave_the_plan_unchanged() {
    let files: Vec<String> = (0..30).map(|i| format!("src/f{i}.rs")).collect();
    let churn: Vec<_> = files
        .iter()
        .map(|f| json!({"file": f, "churn": 5}))
        .collect();
    let pr = json!({"files": files, "fileCount": 30, "additions": 150, "deletions": 0, "fileChurn": churn});
    let base = plan(&pr, &json!({"foci": foci()}), None, None);
    for bad in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!("300"),
        Value::Null,
        json!(true),
        json!([300]),
    ] {
        let p = plan(
            &pr,
            &json!({"foci": foci(), "lensLineBudget": bad}),
            None,
            None,
        );
        assert_eq!(
            (p.lens_count, &p.file_groups),
            (base.lens_count, &base.file_groups),
            "{bad}"
        );
        assert!(p.to_value().get("lensLineBudget").is_none(), "{bad}");
        assert!(p.logs.iter().any(|l| l.contains("lensLineBudget")), "{bad}");
    }
}

#[test]
fn an_incremental_budget_uses_the_in_scope_churn() {
    let files: Vec<String> = (0..30).map(|i| format!("src/f{i}.rs")).collect();
    let scoped: Vec<String> = files[..20].to_vec();
    let churn: Vec<_> = files
        .iter()
        .map(|f| json!({"file": f, "churn": 100}))
        .collect();
    let pr = json!({"files": files, "fileCount": 30, "additions": 3000, "deletions": 0, "fileChurn": churn});
    let bk = json!({"prior":{"found":true,"sha":"123456789"},
        "scope":{"usable":true,"inScope":scoped,"changed":scoped,"added":[],"unchanged":files[20..].to_vec(),"inScopeChurn":250}});
    let p = plan(
        &pr,
        &json!({"foci": foci(), "lensLineBudget": 100}),
        None,
        Some(&bk),
    );
    assert_eq!(p.files.len(), 20);
    assert_eq!(p.lens_count, 3.0);
}

fn finding(file: &str, title: &str) -> Value {
    json!({"file": file, "title": title, "line": 3, "severity": "nit"})
}

#[test]
fn quiet_areas_collapse_into_one_line_in_path_order() {
    let (rules, _) = group_by::rules(&json!({"groupBy": ["apps/*/"]}));
    let pr: Vec<String> = [
        "apps/z/a.ts",
        "apps/c/a.ts",
        "apps/m/a.ts",
        "apps/b/a.ts",
        "x.rs",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let text = cluster_sections(
        &[finding("apps/m/a.ts", "InM"), finding("x.rs", "Loose")],
        &pr,
        &rules,
    );
    assert!(text.contains("**`apps/m/`**\n- nit InM"), "{text}");
    assert!(
        text.contains("No findings in 3 area(s): `apps/b/`, `apps/c/`, `apps/z/`"),
        "{text}"
    );
    assert_eq!(text.matches("No findings in").count(), 1);
    for k in ["apps/b/", "apps/c/", "apps/z/"] {
        assert!(!text.contains(&format!("**`{k}`**")));
    }
    assert!(
        text.contains("**Outside every area**\n- nit Loose"),
        "{text}"
    );
    for t in ["InM", "Loose"] {
        assert_eq!(text.matches(t).count(), 1);
    }
}

#[test]
fn no_findings_gives_only_the_line_and_no_area_gives_no_section() {
    let (rules, _) = group_by::rules(&json!({"groupBy": ["apps/*/"]}));
    assert_eq!(
        cluster_sections(&[], &["apps/a/x.ts".into()], &rules),
        "\n\n---\n\n**Findings by area.**\n\nNo findings in 1 area(s): `apps/a/`"
    );
    assert_eq!(cluster_sections(&[], &["src/x.rs".into()], &rules), "");
    assert_eq!(
        cluster_sections(&[finding("apps/a/x.ts", "A")], &["apps/a/x.ts".into()], &[]),
        ""
    );
}
