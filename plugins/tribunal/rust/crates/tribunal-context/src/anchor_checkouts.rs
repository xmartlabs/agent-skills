//! Local checkouts of a profile's `crossRepoAnchors`, made with the separate
//! conventions token. The token stays out of the reviewers' environment, so
//! they cannot reach those repos with `gh`; they read these checkouts instead.
//! Without the token this does nothing and the prompts do not change.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tribunal_core::js;

use crate::exec::{Output, Tools};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorCheckout {
    pub repo: String,
    pub ref_: String,
    pub sha: String,
    pub path: PathBuf,
}

#[derive(Debug, Default)]
pub struct Cloned {
    pub checkouts: Vec<AnchorCheckout>,
    pub warnings: Vec<String>,
}

/// Creates `dir` and its parents, readable only by this user where the
/// platform has that notion: what it holds comes from private repos.
///
/// # Errors
/// As [`std::fs::DirBuilder::create`].
pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// `owner/name`, each part a plain name, so neither becomes a flag of `gh` nor
/// a path that leaves the checkout directory.
fn split_repo(repo: &str) -> Option<&str> {
    let (owner, name) = repo.split_once('/')?;
    let plain = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && !s.starts_with('-')
            && s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    };
    (plain(owner) && plain(name)).then_some(name)
}

/// A ref git can take as a separate argument: not a flag, no space or control
/// character.
fn is_plain_ref(ref_: &str) -> bool {
    !ref_.is_empty()
        && !ref_.starts_with('-')
        && !ref_.chars().any(|c| c.is_whitespace() || c.is_control())
}

fn failure(what: &str, got: std::io::Result<Output>) -> Result<Output, String> {
    match got {
        Err(e) => Err(format!("cannot run {what}: {e}")),
        Ok(o) if o.ok() => Ok(o),
        Ok(o) => {
            let line = o
                .stderr
                .lines()
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned();
            Err(format!("{what} failed: {line}"))
        }
    }
}

async fn clone_one(tools: &Tools, repo: &str, ref_: &str, dest: &Path) -> Result<String, String> {
    let dir = dest.to_string_lossy();
    // A URL, not `owner/name`: `gh` would turn that into an SSH remote when its
    // `git_protocol` says so, and the operator's key would answer, not the token.
    let url = format!("https://github.com/{repo}");
    // `gh repo clone` keeps no credential, and a machine may have run no
    // `gh auth setup-git`: this fetch asks `gh`, which answers with `GH_TOKEN`.
    let credential_helper = format!("credential.helper=!\"{}\" auth git-credential", tools.gh);
    failure(
        "gh repo clone",
        tools
            .gh(
                &["repo", "clone", &url, &dir, "--", "--quiet", "--depth", "1"],
                None,
            )
            .await,
    )?;
    failure(
        "git fetch",
        tools
            .git(
                &[
                    "-c",
                    "credential.helper=",
                    "-c",
                    &credential_helper,
                    "-C",
                    &dir,
                    "fetch",
                    "--quiet",
                    "--depth",
                    "1",
                    "origin",
                    ref_,
                ],
                None,
            )
            .await,
    )?;
    failure(
        "git checkout",
        tools
            .git(
                &["-C", &dir, "checkout", "--quiet", "--detach", "FETCH_HEAD"],
                None,
            )
            .await,
    )?;
    let head = failure(
        "git rev-parse",
        tools.git(&["-C", &dir, "rev-parse", "HEAD"], None).await,
    )?;
    Ok(head.stdout.trim().to_owned())
}

/// Clones each anchor at its ref under `base`, as `base/<name>`, when the
/// tools carry a conventions token. An anchor that cannot be cloned is a
/// warning, and the review goes on without it. The reviewed repo is not an
/// anchor of itself.
pub async fn clone_anchors(tools: &Tools, profile: &Value, repo: &str, base: &Path) -> Cloned {
    let mut out = Cloned::default();
    if tools.conventions.is_none() {
        return out;
    }
    let anchors = js::get(profile, "crossRepoAnchors")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let docs_tools = tools.for_conventions();
    let mut seen = HashSet::new();
    for anchor in anchors {
        let text = |k: &str| {
            js::get(anchor, k)
                .and_then(Value::as_str)
                .unwrap_or_default()
        };
        let (anchor_repo, ref_) = (text("repo"), text("ref"));
        if anchor_repo.eq_ignore_ascii_case(repo) {
            continue;
        }
        let at = format!("crossRepoAnchors {anchor_repo}");
        let Some(name) = split_repo(anchor_repo).filter(|_| is_plain_ref(ref_)) else {
            out.warnings.push(format!(
                "{at}: not a plain owner/name and ref. Reviewing without a checkout."
            ));
            continue;
        };
        if !seen.insert(name.to_ascii_lowercase()) {
            continue;
        }
        let dest = base.join(name);
        let got = match private_dir(base) {
            Ok(()) => clone_one(&docs_tools, anchor_repo, ref_, &dest).await,
            Err(e) => Err(format!("cannot create {}: {e}", base.display())),
        };
        match got {
            Ok(sha) => out.checkouts.push(AnchorCheckout {
                repo: anchor_repo.to_owned(),
                ref_: ref_.to_owned(),
                sha,
                path: dest,
            }),
            Err(e) => out.warnings.push(format!(
                "{at}: {e}. It is read with the conventions token, and a 404 can also mean that token has no access to it. Reviewing without a checkout."
            )),
        }
    }
    out
}

/// The line each prompt carries about the checkouts; empty without any.
#[must_use]
pub fn note(checkouts: &[AnchorCheckout]) -> String {
    if checkouts.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        "CROSS-REPO CHECKOUTS: on this run `gh api` and `gh search` cannot read these repos. \
         Read and search them here instead, with the Read tool or `git -C <path> grep`, and \
         cite a file in one as `<repo name>/<path>`:"
            .to_owned(),
    ];
    lines.extend(checkouts.iter().map(|c| {
        format!(
            "- {}@{} ({}) at {}",
            c.repo,
            c.ref_,
            c.sha,
            c.path.display()
        )
    }));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repo_or_ref_that_could_be_a_flag_or_a_path_is_refused() {
        assert_eq!(split_repo("acme/api"), Some("api"));
        for bad in [
            "acme", "-x/api", "acme/-x", "acme/..", "a/b/c", "acme/a b", "",
        ] {
            assert_eq!(split_repo(bad), None, "{bad}");
        }
        assert!(is_plain_ref("release/1.2"));
        for bad in ["", "--upload-pack=x", "a b", "a\nb"] {
            assert!(!is_plain_ref(bad), "{bad}");
        }
    }

    #[test]
    fn no_checkout_means_no_note() {
        assert_eq!(note(&[]), "");
    }
}
