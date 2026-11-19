//! S12 end to end: `prepare` + `context_phase --preflight` through the
//! compiled `tribunal-test-gh`. A PR whose fetched file list is empty while
//! its diff and line counts are not stops with the retry error before
//! planning; a genuinely empty PR and a normal PR still reach the plan.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tribunal_agent::FakeAgent;
use tribunal_context::exec::Tools;
use tribunal_context::operator::OperatorEnv;
use tribunal_pipeline::context::{context_phase, prepare, ContextOutcome, RunArgs, Runtime};

const REPO: &str = "acme/widgets";
const VIEW: &str =
    "title,body,author,baseRefName,baseRefOid,headRefName,headRefOid,files,additions,deletions,reviews";

fn test_gh() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent()
        .and_then(Path::parent)
        .unwrap()
        .join(format!("tribunal-test-gh{}", std::env::consts::EXE_SUFFIX))
}

fn rec(args: &[&str], stdout: &str) -> Value {
    json!({"args": args, "stdout": stdout, "stderr": "", "code": 0})
}

/// Removes the scratch root even when the run under test returns early or panics.
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn preflight(
    name: &str,
    files: &[&str],
    additions: u64,
    diff: &str,
) -> Result<ContextOutcome, String> {
    preflight_on(name, files, additions, diff, &[]).await
}

/// `deploy_branches` lists the profile's deploy branches; the PR's base is `main`.
async fn preflight_on(
    name: &str,
    files: &[&str],
    additions: u64,
    diff: &str,
    deploy_branches: &[&str],
) -> Result<ContextOutcome, String> {
    let root = std::env::temp_dir().join(format!("tribunal-s12-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let _scratch = Scratch(root.clone());
    std::fs::create_dir_all(root.join("tmp")).unwrap();
    let profile = root.join("profile.json");
    std::fs::write(
        &profile,
        json!({
            "schemaVersion": 1, "project": "widgets", "repo": REPO,
            "reviewer": "backend", "baseBranch": "main", "deployBranches": deploy_branches,
            "foci": {
                "correctness": "correctness and logic", "dataContract": "data contracts",
                "security": "security and authorization", "performance": "performance",
                "errorHandling": "error handling",
            },
            "highRiskSurface": "auth and payments", "crossRepoAnchors": [], "riskPaths": [],
        })
        .to_string(),
    )
    .unwrap();
    let view = json!({
        "title": "t", "body": "", "author": {"login": "alice"}, "baseRefName": "main",
        "baseRefOid": "b0".repeat(20), "headRefName": "feat", "headRefOid": "ca".repeat(20),
        "files": files.iter().map(|f| json!({"path": f, "additions": 1, "deletions": 0})).collect::<Vec<_>>(),
        "additions": additions, "deletions": 0, "reviews": [],
    });
    let pulls = format!("repos/{REPO}/pulls/7/comments");
    let issues = format!("repos/{REPO}/issues/7/comments");
    let repo_api = format!("repos/{REPO}");
    let calls = vec![
        rec(&["pr", "view", "7", "--repo", REPO, "--json", "baseRefName", "--jq", ".baseRefName"], "main\n"),
        rec(&["api", &repo_api, "--jq", ".private"], "true\n"),
        rec(&["pr", "view", "7", "--repo", REPO, "--json", VIEW], &view.to_string()),
        rec(&["api", "--paginate", "--slurp", &pulls], "[[]]"),
        rec(&["api", "--paginate", "--slurp", &issues], "[[]]"),
        rec(&["api", "user", "--jq", ".login"], "bot\n"),
        rec(&["pr", "diff", "7", "--repo", REPO], diff),
        rec(
            &["pr", "view", "7", "--repo", REPO, "--json", "reviews", "--jq",
              "[.reviews[] | {author: .author.login, state, at: .submittedAt, commit: .commit.oid}]"],
            "[]",
        ),
    ];
    let recordings = root.join("gh.json");
    std::fs::write(&recordings, json!({ "calls": calls }).to_string()).unwrap();
    let rt = Runtime {
        plugin_root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."),
        tmpdir: root.join("tmp").to_string_lossy().into_owned(),
        tools: Tools {
            gh: test_gh().to_string_lossy().into_owned(),
            env: vec![
                (OsString::from("TRIBUNAL_TEST_GH"), OsString::new()),
                (OsString::from("GH_RECORDINGS"), recordings.into_os_string()),
            ],
            ..Tools::default()
        },
        operator: OperatorEnv {
            review_harness_config: None,
            home: root.to_string_lossy().into_owned(),
            cwd: root.clone(),
        },
        data_dir: root.join("data"),
    };
    let args = RunArgs {
        pr: "7".into(),
        repo: REPO.into(),
        profile: profile.to_string_lossy().into_owned(),
        preflight: true,
        ..RunArgs::default()
    };
    let prepared = prepare(&rt, &args).await?;
    let mut logs = Vec::new();
    context_phase(&FakeAgent::new(), &rt, &args, &prepared, &mut logs).await
}

const DIFF: &str =
    "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -0,0 +1 @@\n+x\n";

#[tokio::test]
async fn an_empty_file_list_with_changes_stops_before_planning() {
    let err = preflight("empty", &[], 120, DIFF)
        .await
        .expect_err("must stop");
    assert_eq!(
        err,
        "PR #7 has changes but its file list came back empty; retry the run (this may be a transient GitHub fetch failure)"
    );
}

#[tokio::test]
async fn a_genuinely_empty_pr_still_plans() {
    assert!(matches!(
        preflight("none", &[], 0, "").await,
        Ok(ContextOutcome::Preflight { .. })
    ));
}

#[tokio::test]
async fn a_normal_pr_still_plans() {
    let Ok(ContextOutcome::Preflight { summary }) =
        preflight("normal", &["src/a.rs"], 1, DIFF).await
    else {
        panic!("normal PR must reach the plan");
    };
    assert_eq!(summary["filesInScope"], 1);
}

#[tokio::test]
async fn a_deployment_pr_with_an_empty_file_list_is_still_skipped() {
    // A deployment PR is skipped before anything reads its files, so a
    // transient empty list must not turn that skip into an error.
    let out = preflight_on("deploy", &[], 120, DIFF, &["main"]).await;
    assert!(
        matches!(&out, Ok(ContextOutcome::Skipped { reason }) if reason == "deployment PR"),
        "{out:?}"
    );
}
