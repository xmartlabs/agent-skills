//! Where the reviewed repo is checked out on this machine, when the working
//! directory belongs to another repo.
//!
//! The grounded typecheck needs a clone of the reviewed repo with its
//! dependencies installed, and it takes the working directory for that clone.
//! Run from a different repo and it used to print `DEGRADED_WRONG_REPO` and
//! stop, with nothing in the report to say a check had been skipped. This
//! module looks for the right clone first, so the check can run; the caller
//! reports what it found either way.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::exec::Tools;
use crate::operator::cwd_repo;

/// A directory that holds a checkout of the reviewed repo, tried before the
/// siblings of the working directory.
pub const ENV_REPO_DIR: &str = "TRIBUNAL_REPO_DIR";

/// Most directories looked at when searching siblings.
const MAX_CANDIDATES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoCwd {
    /// The working directory is a checkout of the reviewed repo.
    Ambient,
    /// Another local checkout of the reviewed repo. `branch` and `head` say
    /// what it has checked out, since a clone left on an old branch gives the
    /// typecheck its dependencies and its tsconfig.
    Resolved {
        dir: PathBuf,
        how: String,
        branch: Option<String>,
        head: Option<String>,
    },
    /// No local checkout was found.
    Unresolved {
        cwd_origin: Option<String>,
        searched: usize,
    },
}

impl RepoCwd {
    /// The directory the typecheck should run in.
    #[must_use]
    pub fn dir<'a>(&'a self, cwd: &'a Path) -> &'a Path {
        match self {
            Self::Resolved { dir, .. } => dir,
            _ => cwd,
        }
    }

    #[must_use]
    pub fn state(&self) -> &'static str {
        match self {
            Self::Ambient => "ambient",
            Self::Resolved { .. } => "resolved",
            Self::Unresolved { .. } => "unresolved",
        }
    }

    /// The `repoResolution` object of the metrics row.
    #[must_use]
    pub fn to_json(&self, repo: &str, cwd: &Path) -> Value {
        let mut v = json!({
            "state": self.state(),
            "repo": repo,
            "cwd": cwd.to_string_lossy(),
        });
        match self {
            Self::Ambient => {}
            Self::Resolved {
                dir,
                how,
                branch,
                head,
            } => {
                v["dir"] = json!(dir.to_string_lossy());
                v["how"] = json!(how);
                v["branch"] = json!(branch);
                v["head"] = json!(head);
            }
            Self::Unresolved {
                cwd_origin,
                searched,
            } => {
                v["cwdOrigin"] = json!(cwd_origin);
                v["searched"] = json!(searched);
            }
        }
        v
    }
}

fn same_repo(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

async fn checks_out(tools: &Tools, dir: &Path, repo: &str) -> bool {
    dir.join(".git").exists()
        && cwd_repo(tools, dir)
            .await
            .is_some_and(|o| same_repo(&o, repo))
}

/// The git directories one and two levels above `cwd`, each listed one level
/// down: a workspace that keeps its repos side by side is found from any of
/// them. A directory named like the repo comes first.
fn candidate_dirs(cwd: &Path, repo: &str) -> Vec<PathBuf> {
    let name = repo.rsplit('/').next().unwrap_or(repo);
    let mut roots: Vec<PathBuf> = Vec::new();
    for up in cwd.ancestors().skip(1).take(2) {
        roots.push(up.to_path_buf());
    }
    let mut found: Vec<PathBuf> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p != cwd && p.is_dir() && p.join(".git").exists() && !found.contains(&p) {
                found.push(p);
            }
        }
    }
    found.sort_by_key(|p| {
        let n = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        (!n.eq_ignore_ascii_case(name), n)
    });
    found.truncate(MAX_CANDIDATES);
    found
}

async fn git_line(tools: &Tools, dir: &Path, args: &[&str]) -> Option<String> {
    let d = dir.to_string_lossy();
    let mut full = vec!["-C", d.as_ref()];
    full.extend_from_slice(args);
    let out = tools.git(&full, None).await.ok()?;
    if !out.ok() {
        return None;
    }
    let line = out.stdout.trim();
    (!line.is_empty()).then(|| line.to_owned())
}

async fn found(tools: &Tools, dir: PathBuf, how: &str) -> RepoCwd {
    RepoCwd::Resolved {
        branch: git_line(tools, &dir, &["rev-parse", "--abbrev-ref", "HEAD"]).await,
        head: git_line(tools, &dir, &["rev-parse", "--short", "HEAD"]).await,
        dir,
        how: how.to_owned(),
    }
}

/// Settles which directory stands for `repo`. Reads `origin`, the branch and
/// the commit; changes nothing.
pub async fn resolve(tools: &Tools, repo: &str, cwd: &Path, env_dir: Option<&str>) -> RepoCwd {
    let cwd_origin = cwd_repo(tools, cwd).await;
    if cwd_origin.as_deref().is_some_and(|o| same_repo(o, repo)) {
        return RepoCwd::Ambient;
    }
    if let Some(d) = env_dir.filter(|d| !d.is_empty()) {
        let dir = PathBuf::from(d);
        if checks_out(tools, &dir, repo).await {
            return found(tools, dir, ENV_REPO_DIR).await;
        }
    }
    let mut searched = 0;
    for dir in candidate_dirs(cwd, repo) {
        searched += 1;
        if checks_out(tools, &dir, repo).await {
            return found(tools, dir, "sibling").await;
        }
    }
    RepoCwd::Unresolved {
        cwd_origin,
        searched,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn dir_how(got: &RepoCwd) -> Option<(PathBuf, String)> {
        match got {
            RepoCwd::Resolved { dir, how, .. } => Some((dir.clone(), how.clone())),
            _ => None,
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p =
            std::env::temp_dir().join(format!("tribunal-locator-{}-{n}-{tag}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn repo_at(dir: &Path, origin: &str) {
        std::fs::create_dir_all(dir).unwrap();
        for args in [vec!["init", "-q"], vec!["remote", "add", "origin", origin]] {
            let out = Command::new("git")
                .args(&args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
        }
    }

    #[tokio::test]
    async fn the_working_directory_is_used_when_it_is_the_repo() {
        let root = scratch("ambient");
        repo_at(&root.join("widgets"), "https://github.com/acme/widgets.git");
        let got = resolve(
            &Tools::default(),
            "acme/widgets",
            &root.join("widgets"),
            None,
        )
        .await;
        assert_eq!(got, RepoCwd::Ambient);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_sibling_checkout_is_found_from_another_repo() {
        let root = scratch("sibling");
        repo_at(&root.join("other"), "git@github.com:acme/other.git");
        repo_at(&root.join("widgets"), "git@github.com:acme/widgets.git");
        let got = resolve(&Tools::default(), "Acme/Widgets", &root.join("other"), None).await;
        assert_eq!(
            dir_how(&got),
            Some((root.join("widgets"), "sibling".into()))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn the_found_checkouts_branch_and_commit_are_recorded() {
        let root = scratch("branch");
        repo_at(&root.join("other"), "git@github.com:acme/other.git");
        let widgets = root.join("widgets");
        repo_at(&widgets, "git@github.com:acme/widgets.git");
        for args in [
            vec!["checkout", "-q", "-b", "stale-branch"],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x",
            ],
        ] {
            let out = Command::new("git")
                .args(&args)
                .current_dir(&widgets)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
        }
        let got = resolve(&Tools::default(), "acme/widgets", &root.join("other"), None).await;
        let RepoCwd::Resolved { branch, head, .. } = &got else {
            panic!("not resolved: {got:?}");
        };
        assert_eq!(branch.as_deref(), Some("stale-branch"));
        assert!(head.as_deref().is_some_and(|h| h.len() >= 7), "{head:?}");
        let json = got.to_json("acme/widgets", &root.join("other"));
        assert_eq!(json["branch"], "stale-branch");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_checkout_one_level_further_up_is_found_too() {
        let root = scratch("grand");
        repo_at(&root.join("a/other"), "git@github.com:acme/other.git");
        repo_at(&root.join("widgets"), "git@github.com:acme/widgets.git");
        let got = resolve(
            &Tools::default(),
            "acme/widgets",
            &root.join("a/other"),
            None,
        )
        .await;
        assert_eq!(got.state(), "resolved");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn nothing_found_says_what_the_working_directory_is() {
        let root = scratch("none");
        let cwd = root.join("ws/other");
        repo_at(&cwd, "git@github.com:acme/other.git");
        let got = resolve(&Tools::default(), "acme/widgets", &cwd, None).await;
        assert_eq!(
            got,
            RepoCwd::Unresolved {
                cwd_origin: Some("acme/other".into()),
                searched: 0
            }
        );
        let json = got.to_json("acme/widgets", &cwd);
        assert_eq!(json["state"], "unresolved");
        assert_eq!(json["cwdOrigin"], "acme/other");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn the_environment_directory_wins_over_a_sibling() {
        let root = scratch("env");
        repo_at(&root.join("other"), "git@github.com:acme/other.git");
        repo_at(&root.join("widgets"), "git@github.com:acme/widgets.git");
        let far = scratch("env-far");
        repo_at(&far.join("elsewhere"), "git@github.com:acme/widgets.git");
        let got = resolve(
            &Tools::default(),
            "acme/widgets",
            &root.join("other"),
            far.join("elsewhere").to_str(),
        )
        .await;
        assert_eq!(
            dir_how(&got),
            Some((far.join("elsewhere"), ENV_REPO_DIR.into()))
        );
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(far);
    }

    #[tokio::test]
    async fn a_directory_named_like_the_repo_is_tried_first() {
        let root = scratch("named");
        repo_at(&root.join("other"), "git@github.com:acme/other.git");
        repo_at(&root.join("aaa-copy"), "git@github.com:acme/widgets.git");
        repo_at(&root.join("widgets"), "git@github.com:acme/widgets.git");
        let got = resolve(&Tools::default(), "acme/widgets", &root.join("other"), None).await;
        assert_eq!(
            dir_how(&got),
            Some((root.join("widgets"), "sibling".into()))
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
