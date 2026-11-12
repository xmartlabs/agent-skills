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

/// `visibility` answers `gh api repos/<repo> --jq .private` for each repo it
/// names; a repo it leaves out has no answer, so its visibility is unknown.
fn runtime(name: &str, token: &str, visibility: &[(&str, &str)]) -> (Runtime, RunArgs, PathBuf) {
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
    let mut calls = vec![json!({
        "args": ["pr", "view", "7", "--repo", "acme/widgets", "--json", "baseRefName", "--jq", ".baseRefName"],
        "stdout": "main\n", "stderr": "", "code": 0
    })];
    for (repo, private) in visibility {
        calls.push(json!({
            "args": ["api", format!("repos/{repo}"), "--jq", ".private"],
            "stdout": format!("{private}\n"), "stderr": "", "code": 0
        }));
    }
    std::fs::write(&recordings, json!({ "calls": calls }).to_string()).unwrap();
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
    let (rt, args, tmp) = runtime("on", SENTINEL, &[("acme/widgets", "true")]);
    let prepared = prepare(&rt, &args).await.unwrap();
    assert_eq!(prepared.repo_private, Some(true));
    assert!(prepared.foreign_repos_dropped.is_empty());
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
    let (rt, args, tmp) = runtime("off", "", &[("acme/widgets", "true")]);
    let prepared = prepare(&rt, &args).await.unwrap();
    assert_eq!(prepared.cross_repo_note, "");
    assert!(!tmp.join("tribunal-private-acme-widgets-7").exists());
    assert_eq!(prepared.conventions.reviewer_prompt("M", "R"), "M\n\nR");
    assert_eq!(prepared.conventions.with_conventions("R"), "R");
}

/// On a public repository (or one whose visibility gh does not answer), a
/// private or unknown sibling is neither cloned nor offered, the saved profile
/// no longer names it, and every prompt says to read no other repository.
async fn public_case(name: &str, visibility: &[(&str, &str)]) {
    let (rt, args, tmp) = runtime(name, SENTINEL, visibility);
    let prepared = prepare(&rt, &args).await.unwrap();
    assert_ne!(prepared.repo_private, Some(true));
    assert_eq!(prepared.foreign_repos_dropped, vec!["acme/api".to_owned()]);
    assert!(!tmp
        .join("tribunal-private-acme-widgets-7/anchors/api")
        .exists());
    assert!(
        prepared
            .cross_repo_note
            .starts_with("PUBLIC REPOSITORY: acme/widgets"),
        "{}",
        prepared.cross_repo_note
    );
    assert!(!prepared.cross_repo_note.contains("CROSS-REPO CHECKOUTS:"));
    assert!(prepared
        .profile
        .get("crossRepoAnchors")
        .and_then(|a| a.as_array())
        .is_some_and(Vec::is_empty));
    let saved = std::fs::read_to_string(tmp.join("review-profile-7.json")).unwrap();
    assert!(!saved.contains("acme/api"), "{saved}");
    for prompt in [
        prepared.conventions.reviewer_prompt("M", "R"),
        prepared.conventions.with_conventions("R"),
    ] {
        assert!(prompt.contains("PUBLIC REPOSITORY:"), "{prompt}");
    }
    assert!(
        prepared
            .logs
            .iter()
            .any(|l| l.contains("acme/api are not read")),
        "{:?}",
        prepared.logs
    );
}

#[tokio::test]
async fn a_public_repository_reads_no_private_sibling() {
    public_case("public", &[("acme/widgets", "false"), ("acme/api", "true")]).await;
}

#[tokio::test]
async fn unknown_visibility_counts_as_public_and_an_unknown_sibling_as_private() {
    public_case("unknown", &[]).await;
}

#[tokio::test]
async fn a_public_repository_still_reads_a_public_sibling() {
    let (rt, args, tmp) = runtime(
        "public-sibling",
        SENTINEL,
        &[("acme/widgets", "false"), ("acme/api", "false")],
    );
    let prepared = prepare(&rt, &args).await.unwrap();
    assert_eq!(prepared.repo_private, Some(false));
    assert!(prepared.foreign_repos_dropped.is_empty());
    assert!(tmp
        .join("tribunal-private-acme-widgets-7/anchors/api/route.ts")
        .exists());
    assert!(prepared.cross_repo_note.starts_with("PUBLIC REPOSITORY:"));
    assert!(prepared.cross_repo_note.contains("CROSS-REPO CHECKOUTS:"));
}
