//! The conventions token through `prepare`: a scripted `gh` and a local repo,
//! no model and no network.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use tribunal_context::exec::{ConventionsToken, Tools};
use tribunal_context::operator::OperatorEnv;
use tribunal_pipeline::context::{prepare, RunArgs, Runtime};

const SENTINEL: &str = "sentinel-docs-token";

fn test_gh() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent()
        .and_then(Path::parent)
        .unwrap()
        .join(format!("tribunal-test-gh{}", std::env::consts::EXE_SUFFIX))
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
}

fn runtime(name: &str, token: &str) -> (Runtime, RunArgs, PathBuf) {
    let root =
        std::env::temp_dir().join(format!("tribunal-conv-token-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("route.ts"), "route line\n").unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    git(&work, &["add", "."]);
    git(
        &work,
        &[
            "-c",
            "user.name=dev",
            "-c",
            "user.email=dev@example.com",
            "commit",
            "-qm",
            "x",
        ],
    );
    git(&root, &["clone", "-q", "--bare", "work", "bare.git"]);
    let profile = root.join("profile.json");
    std::fs::write(
        &profile,
        json!({
            "schemaVersion": 1, "project": "widgets", "repo": "acme/widgets",
            "reviewer": "backend", "baseBranch": "main", "deployBranches": [],
            "foci": {
                "correctness": "correctness and logic",
                "dataContract": "data contracts",
                "security": "security and authorization",
                "performance": "performance",
                "errorHandling": "error handling",
            },
            "highRiskSurface": "auth and payments",
            "crossRepoAnchors": [{"repo": "acme/api", "ref": "main"}],
            "riskPaths": [],
        })
        .to_string(),
    )
    .unwrap();
    let recordings = root.join("gh.json");
    std::fs::write(
        &recordings,
        json!({"calls": [{
            "args": ["pr", "view", "7", "--repo", "acme/widgets", "--json", "baseRefName", "--jq", ".baseRefName"],
            "stdout": "main\n", "stderr": "", "code": 0
        }]})
        .to_string(),
    )
    .unwrap();
    let tmp = root.join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let rt = Runtime {
        plugin_root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."),
        tmpdir: tmp.to_string_lossy().into_owned(),
        tools: Tools {
            gh: test_gh().to_string_lossy().into_owned(),
            env: vec![
                (OsString::from("TRIBUNAL_TEST_GH"), OsString::new()),
                (OsString::from("GH_RECORDINGS"), recordings.into_os_string()),
                (
                    OsString::from("GH_BARE"),
                    root.join("bare.git").into_os_string(),
                ),
            ],
            conventions: ConventionsToken::new(token),
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
        repo: "acme/widgets".into(),
        profile: profile.to_string_lossy().into_owned(),
        ..RunArgs::default()
    };
    (rt, args, tmp)
}

#[tokio::test]
async fn with_a_token_reviewers_are_pointed_at_a_checkout_and_never_see_the_token() {
    let (rt, args, tmp) = runtime("on", SENTINEL);
    let prepared = prepare(&rt, &args).await.unwrap();
    let checkout = tmp.join("tribunal-private-acme-widgets-7/anchors/api");
    assert_eq!(
        std::fs::read_to_string(checkout.join("route.ts")).unwrap(),
        "route line\n"
    );
    assert!(
        prepared
            .cross_repo_note
            .contains(&format!("at {}", checkout.display())),
        "{}",
        prepared.cross_repo_note
    );
    for prompt in [
        prepared.conventions.reviewer_prompt("M", "R"),
        prepared.conventions.with_conventions("R"),
    ] {
        assert!(prompt.contains("CROSS-REPO CHECKOUTS:"), "{prompt}");
    }
    assert!(!format!("{prepared:?}").contains(SENTINEL));
}

#[tokio::test]
async fn without_a_token_nothing_is_cloned_and_the_prompts_are_unchanged() {
    let (rt, args, tmp) = runtime("off", "");
    let prepared = prepare(&rt, &args).await.unwrap();
    assert_eq!(prepared.cross_repo_note, "");
    assert!(!tmp.join("tribunal-private-acme-widgets-7").exists());
    assert_eq!(prepared.conventions.reviewer_prompt("M", "R"), "M\n\nR");
    assert_eq!(prepared.conventions.with_conventions("R"), "R");
}
