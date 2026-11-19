use serde_json::json;
use tribunal_pipeline::context::empty_file_list_error;

#[test]
fn empty_file_list_with_diff_names_pr_and_requests_retry() {
    let err = empty_file_list_error(
        &json!({
            "files": [], "diffBytes": 1, "additions": 0, "deletions": 0
        }),
        "42",
    )
    .unwrap();
    assert!(err.contains("file list came back empty"));
    assert!(err.contains("PR #42"));
    assert!(err.contains("retry"));
}

#[test]
fn empty_file_list_on_genuinely_empty_pr_is_allowed() {
    assert_eq!(
        empty_file_list_error(
            &json!({
                "files": [], "diffBytes": 0, "additions": 0, "deletions": 0
            }),
            "42"
        ),
        None
    );
}

#[test]
fn a_normal_pr_with_files_is_allowed_to_plan() {
    assert_eq!(
        empty_file_list_error(
            &json!({
                "files": ["src/main.rs"], "diffBytes": 12, "additions": 1, "deletions": 0
            }),
            "42"
        ),
        None
    );
}
