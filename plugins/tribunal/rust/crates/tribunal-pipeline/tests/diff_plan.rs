//! `plan::build_plan` against the workflow's own glue, run by the span oracle
//! on the same generated PRs, probe answers and profiles.

mod common;

use common::{assert_same, sample, span, Span, LOGS, LOG_FN};
use proptest::prelude::*;
use serde_json::{json, Value};
use tribunal_pipeline::plan::{build_plan, PlanInput};

const PLAN_SPAN: Span = Span {
    start: "const startEpoch = Number.isFinite(contextBookkeeping?.startEpoch)",
    end: "fell outside every lens scope`);",
    deps: &["PROFILE-LOOKUP", "CARRY-FORWARD"],
    pre: &[],
    fns: &["log"],
    ret: "{prFiles: prContext.files, allPrFiles, incrementalScope, priorReview, importAdjacency, incrementalFallback, startEpoch, fileCount, churn, lensBudget, groupCount, lensesOverride, fileGroups, filePartitionMode, perGroupChurn, foci, lensCount, lensPlan, uncoveredFiles, logs: __LOGS__}",
};

const PATHS: &[&str] = &[
    "src/auth/login.ts",
    "prisma/schema.prisma",
    "prisma/migrations/001/migration.sql",
    "src/services/pay.ts",
    "src/controllers/v2/orders.ts",
    "src/jobs/queue.ts",
    "src/helpers/date.ts",
    "README.md",
    "src/role/permission.ts",
    "test/a.test.ts",
];

fn path() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => prop::sample::select(PATHS).prop_map(str::to_owned),
        1 => "[a-z]{1,6}/[a-z]{1,6}\\.(ts|js|md)",
    ]
}

fn profile() -> impl Strategy<Value = Value> {
    (any::<bool>(), any::<bool>(), 0usize..4).prop_map(|(migration, risk, order)| {
        let mut foci = json!({
            "correctness": "correctness sentence",
            "dataContract": "data contract sentence",
            "security": "security sentence",
            "performance": "performance sentence",
            "errorHandling": "error handling sentence",
        });
        if migration {
            foci["migration"] = json!("migration sentence");
        }
        let mut rules = vec![
            json!({"focus": "migration", "pattern": "prisma/(schema|migrations)|migration"}),
            json!({"focus": "security", "pattern": "auth|role|permission"}),
            json!({"focus": "dataContract", "pattern": "controllers/v2"}),
            json!({"focus": "money", "pattern": "pay"}),
        ];
        rules.rotate_left(order);
        let mut p = json!({
            "schemaVersion": 1, "project": "Widgets", "repo": "acme/widgets-api",
            "reviewer": "backend", "baseBranch": "main", "deployBranches": ["production"],
            "foci": foci, "highRiskSurface": "auth and payments",
        });
        if risk {
            p["riskPaths"] = Value::from(rules);
        }
        p
    })
}

fn number() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(json!(0)),
        (0u32..9000).prop_map(|n| json!(n)),
        Just(json!(199)),
        Just(json!(200)),
        Just(json!(4999)),
        Just(json!(5000)),
        Just(json!(1.5)),
        Just(Value::Null),
    ]
}

fn pr_context() -> impl Strategy<Value = Value> {
    (
        prop::collection::vec(path(), 0..32),
        prop::option::of(0u32..40),
        number(),
        number(),
        prop::collection::vec(
            (
                any::<prop::sample::Index>(),
                prop_oneof![
                    (0u32..800).prop_map(|n| json!(n)),
                    Just(json!("12")),
                    Just(Value::Null),
                ],
            ),
            0..12,
        ),
    )
        .prop_map(|(mut files, count, additions, deletions, churn)| {
            files.sort();
            files.dedup();
            let file_churn: Vec<Value> = churn
                .into_iter()
                .filter(|_| !files.is_empty())
                .map(|(i, c)| json!({"file": i.get(&files), "churn": c}))
                .collect();
            let mut pr = json!({
                "files": files,
                "fileCount": count.map_or(json!(files.len()), |c| json!(c)),
                "additions": additions,
                "deletions": deletions,
            });
            if !file_churn.is_empty() {
                pr["fileChurn"] = Value::from(file_churn);
            }
            pr
        })
}

fn bookkeeping(files: &[String]) -> impl Strategy<Value = Value> {
    let files = files.to_vec();
    (
        prop_oneof![
            Just(Value::Null),
            (0u32..2_000_000_000).prop_map(|n| json!(n)),
            Just(json!("17"))
        ],
        prop::bool::weighted(0.75),
        prop::option::of(prop_oneof![Just("tribunal-row"), Just("human-review")]),
        prop::option::of("[a-z ]{0,12}"),
        prop::bool::weighted(0.75),
        prop::collection::vec(any::<prop::sample::Index>(), 0..10),
        prop::bool::weighted(0.15),
        prop::option::of(0u32..900),
        any::<bool>(),
    )
        .prop_map(
            move |(epoch, found, source, reason, usable, picks, stray, in_scope_churn, related)| {
                let in_scope: Vec<String> = if files.is_empty() {
                    Vec::new()
                } else {
                    let mut v: Vec<String> = picks.iter().map(|i| i.get(&files).clone()).collect();
                    v.dedup();
                    v
                };
                let mut in_scope = in_scope;
                if stray {
                    in_scope.push("not/in/the/pr.ts".to_owned());
                }
                let mut prior = json!({"found": found});
                if found {
                    prior["sha"] = json!("0123456789abcdef0123456789abcdef01234567");
                    if let Some(s) = source {
                        prior["source"] = json!(s);
                    }
                } else if let Some(r) = &reason {
                    prior["reason"] = json!(r);
                }
                let mut scope = json!({"usable": usable});
                if usable {
                    let half = in_scope.len() / 2;
                    scope["changed"] = Value::from(in_scope[..half].to_vec());
                    scope["added"] = Value::from(in_scope[half..].to_vec());
                    scope["unchanged"] = json!(["kept/one.ts"]);
                    scope["inScope"] = Value::from(in_scope);
                    if let Some(c) = in_scope_churn {
                        scope["inScopeChurn"] = json!(c);
                    }
                } else if let Some(r) = reason {
                    scope["reason"] = json!(r);
                }
                let ia = if related {
                    json!({"relatedFiles": {"src/a.ts": ["src/b.ts"]}, "scanned": 2})
                } else {
                    json!({"relatedFiles": {}, "scanned": 0})
                };
                json!({"startEpoch": epoch, "prior": prior, "scope": scope, "importAdjacency": ia})
            },
        )
}

fn case() -> impl Strategy<Value = Value> {
    (pr_context(), profile())
        .prop_flat_map(|(pr, profile)| {
            let files: Vec<String> = serde_json::from_value(pr["files"].clone()).unwrap();
            (
                Just(pr),
                Just(profile),
                prop::option::weighted(0.85, bookkeeping(&files)),
                prop::bool::weighted(0.8),
                any::<bool>(),
                prop::option::of(Just("89abcdef89abcdef89abcdef89abcdef89abcdef")),
                prop_oneof![
                    3 => Just(Value::Null),
                    1 => prop_oneof![Just(json!(0)), Just(json!(-1)), Just(json!(2.5)), Just(json!(3)), Just(json!(20)), Just(json!("3"))],
                ],
            )
        })
        .prop_map(|(pr, profile, bk, wants_probe, force, pinned, lenses)| {
            json!({
                "contextBookkeeping": bk.unwrap_or(Value::Null),
                "wantsProbe": wants_probe,
                "prContext": pr,
                "forceFullReview": force,
                "pinnedSha": pinned.unwrap_or(""),
                "parsedArgs": if lenses.is_null() { json!({}) } else { json!({"lenses": lenses}) },
                "PROFILE": profile,
                "log": LOG_FN,
            })
        })
}

fn rust_plan(p: &Value) -> Value {
    let bk = &p["contextBookkeeping"];
    let input = PlanInput {
        pr_context: &p["prContext"],
        bookkeeping: (!bk.is_null()).then_some(bk),
        wants_probe: p["wantsProbe"].as_bool().unwrap(),
        force_full: p["forceFullReview"].as_bool().unwrap(),
        pinned_sha: p["pinnedSha"].as_str().unwrap(),
        lenses: p["parsedArgs"].get("lenses"),
        profile: &p["PROFILE"],
    };
    build_plan(&input).expect("plan").to_value()
}

#[test]
fn plan_matches_the_workflow_glue() {
    let cases = sample(case());
    let ret = PLAN_SPAN.ret.replace("__LOGS__", LOGS);
    let js = span(
        &Span {
            ret: &ret,
            ..PLAN_SPAN
        },
        &cases,
    );
    let rust: Vec<Value> = cases.iter().map(rust_plan).collect();
    assert_same("lens plan", &cases, &rust, &js);
}

#[test]
fn the_generated_cases_reach_every_branch() {
    let cases = sample(case());
    let plans: Vec<Value> = cases.iter().map(rust_plan).collect();
    let count = |f: &dyn Fn(&Value) -> bool| plans.iter().filter(|p| f(p)).count();
    assert!(count(&|p| !p["incrementalScope"].is_null()) > 20);
    assert!(count(&|p| !p["incrementalFallback"].is_null()) > 20);
    assert!(count(&|p| p["groupCount"].as_f64().unwrap() > 1.0) > 20);
    assert!(count(&|p| !p["lensesOverride"].is_null()) > 20);
    assert!(
        count(
            &|p| p["filePartitionMode"] == "churn-aware" && p["groupCount"].as_f64().unwrap() > 1.0
        ) > 10
    );
    assert!(count(&|p| p["foci"][0] == "migration sentence") > 10);
}
