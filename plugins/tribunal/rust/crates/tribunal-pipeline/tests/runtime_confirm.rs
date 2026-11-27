//! The pipeline side of `--runtime-confirm`: the schema a lens gets and the
//! two metrics fields. The runner itself is tested in tribunal-context.

use serde_json::{json, Value};
use tribunal_pipeline::run_metrics::runtime_metrics;
use tribunal_pipeline::schemas;

#[test]
fn the_runtime_schema_is_the_findings_schema_plus_one_optional_property() {
    let mut with = schemas::findings_with_runtime_claim();
    let item = &with["properties"]["findings"]["items"];
    let claim = &item["properties"]["runtimeClaim"];
    assert_eq!(claim["required"], json!(["function", "args", "expect"]));
    assert_eq!(claim["type"], json!(["object", "null"]));
    assert!(!item["required"]
        .as_array()
        .unwrap()
        .contains(&json!("runtimeClaim")));
    with["properties"]["findings"]["items"]["properties"]
        .as_object_mut()
        .unwrap()
        .remove("runtimeClaim");
    assert_eq!(with, schemas::findings());
}

#[test]
fn the_row_says_whether_it_ran_and_what_it_found() {
    let off: Vec<(&str, Value)> = runtime_metrics(false, None).into();
    assert_eq!(
        off,
        vec![
            ("runtimeConfirm", json!(false)),
            ("runtimeChecks", Value::Null)
        ]
    );
    let record = json!({"runner": "vitest", "confirmed": 1, "cases": []});
    let on: Vec<(&str, Value)> = runtime_metrics(true, Some(&record)).into();
    assert_eq!(
        on,
        vec![("runtimeConfirm", json!(true)), ("runtimeChecks", record)]
    );
}

#[test]
fn a_lens_is_asked_for_runtime_claims_only_over_files_a_case_can_run_for() {
    use tribunal_pipeline::run::can_run_a_case;
    let files = |f: &[&str]| f.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    assert!(can_run_a_case(&files(&["src/widgets.ts"])));
    assert!(can_run_a_case(&files(&["pkg/widgets.py", "web/a.tsx"])));
    assert!(!can_run_a_case(&files(&[
        "pkg/widgets.py",
        "pkg/test_widgets.py"
    ])));
    assert!(!can_run_a_case(&files(&["README.md", "go.mod"])));
    assert!(!can_run_a_case(&[]));
}
