use serde_json::json;
use tribunal_pipeline::context::{empty_file_list_error, failed_file_list_error};

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

#[test]
fn additions_or_deletions_alone_count_as_changes() {
    for key in ["additions", "deletions"] {
        let mut ctx = json!({"files": [], "diffBytes": 0, "additions": 0, "deletions": 0});
        ctx[key] = json!(3);
        assert!(empty_file_list_error(&ctx, "42").is_some(), "{key}");
    }
}

#[test]
fn a_context_without_a_files_key_is_left_alone() {
    assert_eq!(empty_file_list_error(&json!({"diffBytes": 10}), "42"), None);
}

#[test]
fn a_failed_compare_with_everything_empty_is_rejected() {
    // A pinned run sums additions and deletions from the compare rows, so when
    // both compare calls fail every count is 0 and diffBytes is null.
    let ctx = json!({"files": [], "diffBytes": null, "additions": 0, "deletions": 0});
    assert_eq!(empty_file_list_error(&ctx, "42"), None);
    let errors = json!([
        {"step": "compare-files", "detail": "HTTP 502"},
        {"step": "compare-diff", "detail": "HTTP 502"}
    ]);
    let err = failed_file_list_error(&ctx, Some(&errors), "42").unwrap();
    assert!(err.contains("PR #42"));
    assert!(err.contains("compare-files, compare-diff"));
    assert!(err.contains("retry"));
}

#[test]
fn an_unrelated_fetch_error_does_not_reject_an_empty_pr() {
    let ctx = json!({"files": [], "diffBytes": 0, "additions": 0, "deletions": 0});
    let errors = json!([{"step": "viewer", "detail": "HTTP 502"}]);
    assert_eq!(failed_file_list_error(&ctx, Some(&errors), "42"), None);
    assert_eq!(failed_file_list_error(&ctx, None, "42"), None);
}

#[test]
fn a_failed_step_with_files_present_is_allowed() {
    let ctx = json!({"files": ["src/main.rs"], "diffBytes": null, "additions": 1, "deletions": 0});
    let errors = json!([{"step": "compare-diff", "detail": "HTTP 502"}]);
    assert_eq!(failed_file_list_error(&ctx, Some(&errors), "42"), None);
}
