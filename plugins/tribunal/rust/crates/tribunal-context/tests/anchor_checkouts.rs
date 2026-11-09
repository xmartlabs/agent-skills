//! Crossing to another repo with the conventions token: anchor checks, and the
//! checkouts the reviewers read those repos from.

mod common;
use common::*;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use tribunal_context::anchor_checkouts::{clone_anchors, note};
use tribunal_context::exec::{ConventionsToken, Tools};
use tribunal_context::resolve_verified_anchors;

fn tokens(log: &Path) -> Vec<Value> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn job_tools(name: &str, calls: Vec<Value>, conventions: &str) -> (Tools, PathBuf, PathBuf) {
    let dir = scratch(name);
    let mut t = tools(&recordings(&dir, calls));
    let log = dir.join("tokens.jsonl");
    t.env.push(("GH_TOKEN".into(), "main-token".into()));
    t.env.push(("GH_TOKEN_LOG".into(), log.clone().into()));
    t.conventions = ConventionsToken::new(conventions);
    (t, log, dir)
}

fn raw(endpoint: &str, text: &str) -> Value {
    call(
        &["api", "-H", "Accept: application/vnd.github.raw", endpoint],
        text,
        0,
    )
}

async fn resolve(t: &Tools, dir: &Path) -> Value {
    let payload = json!({"findings": [
        {"verifiedBy": [{"file": "src/own.ts", "line": 1, "quote": "own line"}]},
        {"verifiedBy": [{"file": "api/src/route.ts", "line": 1, "quote": "route line"}]},
    ]})
    .to_string();
    std::fs::write(dir.join("payload.json"), &payload).unwrap();
    resolve_verified_anchors::run(
        &[
            "-",
            "abc",
            "--repo",
            "acme/web-app",
            "--cross-repo",
            "acme/api@main",
        ],
        &payload,
        t,
    )
    .await
    .unwrap()
}

fn anchor_calls() -> Vec<Value> {
    vec![
        raw(
            "repos/acme/web-app/contents/src/own.ts?ref=abc",
            "own line\n",
        ),
        raw(
            "repos/acme/api/contents/src/route.ts?ref=main",
            "route line\n",
        ),
    ]
}

#[tokio::test]
async fn a_cross_repo_anchor_uses_the_conventions_token_and_a_local_one_does_not() {
    let (t, log, dir) = job_tools("anchor-routes", anchor_calls(), "docs-token");
    resolve(&t, &dir).await;
    let calls = tokens(&log);
    assert_eq!(calls.len(), 2, "{calls:?}");
    for c in &calls {
        let args = c["args"].to_string();
        let want = if args.contains("repos/acme/api/") {
            "docs-token"
        } else {
            "main-token"
        };
        assert_eq!(c["token"], want, "{args}");
    }
}

#[tokio::test]
async fn without_a_token_anchor_results_are_unchanged_and_use_the_main_token() {
    let (t, log, dir) = job_tools("anchor-none", anchor_calls(), "");
    let without = resolve(&t, &dir).await;
    assert!(tokens(&log).iter().all(|c| c["token"] == "main-token"));
    assert!(
        !without.to_string().contains("conventions token"),
        "{without}"
    );
}

/// A bare repo with one commit on `main`, what `gh repo clone` copies here.
fn bare_repo(root: &Path) -> (PathBuf, String) {
    let git = |dir: &Path, args: &[&str]| {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    let work = root.join("work");
    std::fs::create_dir_all(work.join("src")).unwrap();
    git(&work, &["init", "--quiet", "-b", "main"]);
    std::fs::write(work.join("src/route.ts"), "route line\n").unwrap();
    git(&work, &["add", "."]);
    git(
        &work,
        &[
            "-c",
            "user.name=dev",
            "-c",
            "user.email=dev@example.com",
            "commit",
            "--quiet",
            "-m",
            "x",
        ],
    );
    let sha = git(&work, &["rev-parse", "HEAD"]).trim().to_owned();
    let bare = root.join("bare.git");
    git(root, &["clone", "--quiet", "--bare", "work", "bare.git"]);
    (bare, sha)
}

#[tokio::test]
async fn an_anchor_is_cloned_at_its_ref_with_the_conventions_token_only() {
    let (mut t, log, dir) = job_tools("anchor-clone", vec![], "sentinel-docs-token");
    let (bare, sha) = bare_repo(&dir);
    t.env.push(("GH_BARE".into(), bare.into()));
    let base = dir.join("private/anchors");
    let profile = json!({"crossRepoAnchors": [
        {"repo": "acme/api", "ref": "main"},
        {"repo": "ACME/web-app", "ref": "main"},
    ]});
    let got = clone_anchors(&t, &profile, "acme/web-app", &base).await;
    assert!(got.warnings.is_empty(), "{:?}", got.warnings);
    assert_eq!(got.checkouts.len(), 1, "the reviewed repo is not cloned");
    let c = &got.checkouts[0];
    assert_eq!(
        (c.repo.as_str(), c.sha.as_str()),
        ("acme/api", sha.as_str())
    );
    assert_eq!(
        std::fs::read_to_string(base.join("api/src/route.ts")).unwrap(),
        "route line\n"
    );
    let calls = tokens(&log);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0]["token"], "sentinel-docs-token");
    assert_eq!(
        calls[0]["args"][2], "https://github.com/acme/api",
        "HTTPS, so `git_protocol: ssh` cannot swap the token for the operator's key"
    );
    let text = note(&got.checkouts);
    assert!(text.starts_with("CROSS-REPO CHECKOUTS:"), "{text}");
    assert!(
        text.contains(&format!(
            "acme/api@main ({sha}) at {}",
            base.join("api").display()
        )),
        "{text}"
    );
    assert!(!text.contains("sentinel-docs-token"));
}

#[tokio::test]
async fn an_anchor_that_cannot_be_cloned_warns_and_the_rest_go_on() {
    let (t, _, dir) = job_tools("anchor-fail", vec![], "sentinel-docs-token");
    let profile = json!({"crossRepoAnchors": [
        {"repo": "acme/api", "ref": "main"},
        {"repo": "--flag/x", "ref": "main"},
        {"repo": "acme/ok", "ref": "--upload-pack=x"},
    ]});
    let got = clone_anchors(&t, &profile, "acme/web-app", &dir.join("anchors")).await;
    assert!(got.checkouts.is_empty());
    assert_eq!(got.warnings.len(), 3, "{:?}", got.warnings);
    assert!(
        got.warnings[0].contains("conventions token"),
        "{:?}",
        got.warnings
    );
    assert!(
        got.warnings[1].contains("not a plain owner/name"),
        "{:?}",
        got.warnings
    );
    assert!(!format!("{:?}", got.warnings).contains("sentinel-docs-token"));
}

#[tokio::test]
async fn without_a_token_nothing_is_cloned() {
    let (t, log, dir) = job_tools("anchor-off", vec![], "");
    let base = dir.join("anchors");
    let profile = json!({"crossRepoAnchors": [{"repo": "acme/api", "ref": "main"}]});
    let got = clone_anchors(&t, &profile, "acme/web-app", &base).await;
    assert!(got.checkouts.is_empty() && got.warnings.is_empty());
    assert!(!base.exists());
    assert!(tokens(&log).is_empty());
    assert_eq!(note(&got.checkouts), "");
}
