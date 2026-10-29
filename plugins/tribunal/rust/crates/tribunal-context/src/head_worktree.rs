//! Port of the HEAD-WORKTREE region of `workflow-src/tribunal.js` and of the
//! part of `fetch-pr-context.mjs --prep` that runs it: the checkout pinned to
//! the reviewed commit, built natively with `git` and `gh` through [`Tools`]
//! instead of by writing the region's bash to disk and running it.
//!
//! [`head_worktree_bash`] still returns the bash, byte for byte, so the
//! differential test can run it as the oracle.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use tribunal_core::js;

use crate::exec::Tools;
use crate::fetch_pr_context::js_string;

/// `STALE_WORKTREE_MS`: a review lasts minutes, so a checkout older than this
/// belongs to a run that died.
pub const STALE_WORKTREE_MS: f64 = 2.0 * 60.0 * 60.0 * 1000.0;

/// Ports `headWorktreeBash(sha, repo, prNumber)`.
pub fn head_worktree_bash(sha: &str, repo: &str, pr_number: &str) -> String {
    format!(
        r#"WT=$(mktemp -d "${{TMPDIR:-/tmp}}/peer-review-wt-{pr_number}-XXXXXX") && rmdir "$WT"
AMBIENT_MATCH=$(git remote get-url origin 2>/dev/null | grep -qi "{repo}" && echo yes || echo no)
if [ "$AMBIENT_MATCH" = yes ]; then
  git worktree add --force --detach "$WT" {sha} >/dev/null 2>&1 \
    || {{ git fetch origin {sha} >/dev/null 2>&1; git worktree add --force --detach "$WT" {sha} >/dev/null 2>&1; }}
fi
if git -C "$WT" rev-parse HEAD >/dev/null 2>&1; then
  echo "CREATED_WORKTREE $WT"
else
  rm -rf "$WT" 2>/dev/null
  if gh repo clone {repo} "$WT" -- --quiet --depth 1 --no-single-branch >/dev/null 2>&1 \
    && git -C "$WT" fetch --quiet --depth 1 origin {sha} >/dev/null 2>&1 \
    && git -C "$WT" checkout --quiet --detach {sha} >/dev/null 2>&1; then
    echo "CLONED:$WT"
  else
    echo WORKTREE_FAILED
  fi
fi"#
    )
}

/// What `parseHeadWorktree` returns. `kind` is `worktree`, `clone` or `none`;
/// teardown differs between the first two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadPin {
    pub path: Option<String>,
    pub kind: &'static str,
    pub ok: bool,
}

impl HeadPin {
    /// No pinned checkout.
    pub fn none() -> Self {
        HeadPin {
            path: None,
            kind: "none",
            ok: false,
        }
    }

    /// `{path, kind, ok}`, the object the workflow passes around.
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert(
            "path".into(),
            self.path.clone().map_or(Value::Null, Value::String),
        );
        m.insert("kind".into(), Value::String(self.kind.into()));
        m.insert("ok".into(), Value::Bool(self.ok));
        Value::Object(m)
    }
}

/// Ports `parseHeadWorktree(text)`; `text` is read as `String(text || "")`.
/// Only the first line with each marker is looked at.
///
/// # Errors
/// Where `String(text)` throws: an object with its own `toString` key.
pub fn parse_head_worktree(text: Option<&Value>) -> Result<HeadPin, String> {
    let s = match text {
        Some(v) if js::truthy(Some(v)) => js_string(v)?,
        _ => String::new(),
    };
    let lines: Vec<&str> = s
        .split('\n')
        .map(js::trim)
        .filter(|l| !l.is_empty())
        .collect();
    for (marker, kind) in [("CREATED_WORKTREE ", "worktree"), ("CLONED:", "clone")] {
        if let Some(line) = lines.iter().find(|l| l.starts_with(marker)) {
            let path = js::trim(&line[marker.len()..]);
            if !path.is_empty() {
                return Ok(HeadPin {
                    path: Some(path.to_owned()),
                    kind,
                    ok: true,
                });
            }
        }
    }
    Ok(HeadPin::none())
}

/// Ports `pinnedCheckoutBlock(pin, sha)`, with `pin` and `sha` as the JSON
/// values the workflow holds. Lenses and refuters read this text.
///
/// # Errors
/// Where the template literal throws printing `sha` or `pin.path`.
pub fn pinned_checkout_block(pin: Option<&Value>, sha: Option<&Value>) -> Result<String, String> {
    let head = match sha {
        Some(v) if js::truthy(Some(v)) => js_string(v)?,
        _ => "the PR head".to_owned(),
    };
    let field = |k: &str| pin.and_then(|p| js::get(p, k));
    let pinned = js::truthy(pin) && js::truthy(field("ok")) && js::truthy(field("path"));
    if !pinned {
        return Ok(format!(
            "Any read BEYOND the diff — surrounding code, call sites, sibling surfaces — must be pinned to HEAD ({head}) with `git show <sha>:<path>` or `gh api repos/<repo>/contents/<path>?ref=<sha>`. NO PINNED CHECKOUT IS AVAILABLE on this run, so the ordinary clone is the only tree present and it sits on the base branch: an unpinned read there will confirm code the PR deleted and miss code the PR added."
        ));
    }
    let path = field("path").map_or(Ok(String::new()), js_string)?;
    Ok(format!(
        "A checkout PINNED to the commit under review ({head}) is already prepared at {path}. Read source THERE — not in any other clone of this repo, which sits on a different branch and will show you code that does not exist at this commit. The repo's own conventions (CLAUDE.md, AGENTS.md), its schema and its sibling modules are all in that directory at the right version, so read them there too rather than reaching for another tree."
    ))
}

/// `${TMPDIR:-/tmp}`.
pub fn bash_tmpdir(tmpdir_env: Option<&str>) -> String {
    match tmpdir_env {
        Some(t) if !t.is_empty() => t.to_owned(),
        _ => crate::proc::fallback_tmpdir(),
    }
}

/// An unquoted `${sha}` in bash: split on blanks, empty words dropped.
pub fn words(s: &str) -> Vec<&str> {
    s.split([' ', '\t', '\n'])
        .filter(|w| !w.is_empty())
        .collect()
}

/// `grep -qi "<repo>"` over one line of `git remote get-url origin`: the
/// pattern is a basic regex, where `.` is the only metacharacter a GitHub
/// owner or repository name can hold.
pub fn grep_i(line: &str, pattern: &str) -> bool {
    let hay: Vec<char> = line.chars().collect();
    let pat: Vec<char> = pattern.chars().collect();
    if pat.is_empty() {
        return true;
    }
    (0..hay.len()).any(|start| {
        start + pat.len() <= hay.len()
            && pat
                .iter()
                .zip(&hay[start..])
                .all(|(p, h)| *p == '.' || p.eq_ignore_ascii_case(h))
    })
}

/// The lines `grep` reads: none in empty input, and no empty last line after a
/// trailing newline.
pub fn input_lines(s: &str) -> impl Iterator<Item = &str> {
    let body = s.strip_suffix('\n').unwrap_or(s);
    body.split('\n')
        .take(if s.is_empty() { 0 } else { usize::MAX })
}

/// `path` as a process running in `cwd` reaches it: the bash resolves a
/// relative `$TMPDIR` against the directory it runs in, not this process's.
pub fn in_cwd(cwd: Option<&Path>, path: &str) -> std::path::PathBuf {
    match cwd {
        Some(dir) if Path::new(path).is_relative() => dir.join(path),
        _ => Path::new(path).to_path_buf(),
    }
}

/// A fresh `mktemp -d <dir>/peer-review-wt-<pr>-XXXXXX` name, created and at
/// once removed as the bash does, or `""` when it cannot be created (the bash
/// then runs on with an empty `$WT`). The name is returned as the template
/// spells it, relative when `tmpdir` is.
pub fn mktemp_then_rmdir(tmpdir: &str, pr_number: &str, cwd: Option<&Path>) -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    #[cfg(unix)]
    use std::os::unix::fs::DirBuilderExt;
    const ALNUM: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    for attempt in 0u32..100 {
        let mut h = RandomState::new().build_hasher();
        h.write_u32(attempt);
        h.write_u128(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        let mut bits = h.finish();
        let suffix: String = (0..6)
            .map(|_| {
                let c = ALNUM[(bits % ALNUM.len() as u64) as usize] as char;
                bits /= ALNUM.len() as u64;
                c
            })
            .collect();
        let wt = format!("{tmpdir}/peer-review-wt-{pr_number}-{suffix}");
        let at = in_cwd(cwd, &wt);
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        builder.mode(0o700);
        match builder.create(&at) {
            Ok(()) => {
                let _ = std::fs::remove_dir(&at);
                return wt;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return String::new(),
        }
    }
    String::new()
}

/// `rm -rf "$WT"`, run in `cwd`.
pub fn rm_rf(path: &str, cwd: Option<&Path>) {
    if path.is_empty() {
        return;
    }
    let path = in_cwd(cwd, path);
    match std::fs::symlink_metadata(&path) {
        Ok(m) if m.is_dir() => {
            let _ = std::fs::remove_dir_all(&path);
        }
        Ok(_) => {
            let _ = std::fs::remove_file(&path);
        }
        Err(_) => {}
    }
}

async fn ok(fut: impl std::future::Future<Output = std::io::Result<crate::exec::Output>>) -> bool {
    fut.await.is_ok_and(|o| o.ok())
}

/// Builds the pinned checkout as [`head_worktree_bash`] does, run in `cwd`
/// with `${TMPDIR:-/tmp}` = `tmpdir`, and returns what the bash prints:
/// `CREATED_WORKTREE <path>`, `CLONED:<path>` or `WORKTREE_FAILED`, with its
/// newline.
pub async fn build_head_worktree(
    tools: &Tools,
    sha: &str,
    repo: &str,
    pr_number: &str,
    cwd: Option<&Path>,
    tmpdir: &str,
) -> String {
    let wt = mktemp_then_rmdir(tmpdir, pr_number, cwd);
    let sha_words = words(sha);
    let repo_words = words(repo);
    let add_args: Vec<&str> = ["worktree", "add", "--force", "--detach", wt.as_str()]
        .into_iter()
        .chain(sha_words.iter().copied())
        .collect();

    let ambient = tools
        .git(&["remote", "get-url", "origin"], cwd)
        .await
        .is_ok_and(|o| input_lines(&o.stdout).any(|l| grep_i(l, repo)));
    if ambient && !ok(tools.git(&add_args, cwd)).await {
        let fetch: Vec<&str> = ["fetch", "origin"]
            .into_iter()
            .chain(sha_words.iter().copied())
            .collect();
        let _ = tools.git(&fetch, cwd).await;
        let _ = tools.git(&add_args, cwd).await;
    }
    if ok(tools.git(&["-C", &wt, "rev-parse", "HEAD"], cwd)).await {
        return format!("CREATED_WORKTREE {wt}\n");
    }
    rm_rf(&wt, cwd);
    let clone: Vec<&str> = ["repo", "clone"]
        .into_iter()
        .chain(repo_words.iter().copied())
        .chain([
            wt.as_str(),
            "--",
            "--quiet",
            "--depth",
            "1",
            "--no-single-branch",
        ])
        .collect();
    let fetch: Vec<&str> = [
        "-C",
        wt.as_str(),
        "fetch",
        "--quiet",
        "--depth",
        "1",
        "origin",
    ]
    .into_iter()
    .chain(sha_words.iter().copied())
    .collect();
    let checkout: Vec<&str> = ["-C", wt.as_str(), "checkout", "--quiet", "--detach"]
        .into_iter()
        .chain(sha_words.iter().copied())
        .collect();
    if ok(tools.gh(&clone, cwd)).await
        && ok(tools.git(&fetch, cwd)).await
        && ok(tools.git(&checkout, cwd)).await
    {
        format!("CLONED:{wt}\n")
    } else {
        "WORKTREE_FAILED\n".to_owned()
    }
}

/// `/^peer-review-wt-\d+-[A-Za-z0-9]+$/`.
fn is_review_worktree(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("peer-review-wt-") else {
        return false;
    };
    let Some((pr, suffix)) = rest.split_once('-') else {
        return false;
    };
    !pr.is_empty()
        && pr.bytes().all(|b| b.is_ascii_digit())
        && !suffix.is_empty()
        && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn ms_since_epoch(t: SystemTime) -> f64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64() * 1000.0,
        Err(e) => -e.duration().as_secs_f64() * 1000.0,
    }
}

/// Milliseconds since the epoch, as `Date.now()`.
pub fn now_ms() -> f64 {
    ms_since_epoch(SystemTime::now()).floor()
}

/// Ports `pruneStaleWorktrees`: removes, in `cwd`'s repository, every review
/// checkout older than `max_age_ms` by its birth time (its modification time
/// where there is none), then prunes once if anything went. Returns the paths
/// removed.
pub async fn prune_stale_worktrees(
    tools: &Tools,
    cwd: Option<&Path>,
    now: f64,
    max_age_ms: f64,
) -> Vec<String> {
    let Ok(list) = tools.git(&["worktree", "list", "--porcelain"], cwd).await else {
        return Vec::new();
    };
    if !list.ok() {
        return Vec::new();
    }
    let mut removed = Vec::new();
    for line in list.stdout.split('\n') {
        let Some(rest) = line.strip_prefix("worktree ") else {
            continue;
        };
        let path = js::trim(rest);
        if !is_review_worktree(path.rsplit('/').next().unwrap_or("")) {
            continue;
        }
        // statSync resolves a relative path against the process, not `cwd`.
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        let birth = meta.created().map(ms_since_epoch).unwrap_or(0.0);
        let born = if birth != 0.0 {
            birth
        } else {
            meta.modified().map(ms_since_epoch).unwrap_or(f64::NAN)
        };
        if now - born <= max_age_ms {
            continue;
        }
        if ok(tools.git(&["worktree", "remove", "--force", path], cwd)).await {
            removed.push(path.to_owned());
        }
    }
    if !removed.is_empty() {
        let _ = tools.git(&["worktree", "prune"], cwd).await;
    }
    removed
}

/// The worktree half of `prepareReview`: nothing for a deployment PR or a
/// context without `headRefOid`; otherwise prune stale checkouts and build the
/// pinned one at `headRefOid`. Returns `prep.worktree`, the bash's output
/// trimmed, which [`parse_head_worktree`] reads.
///
/// # Errors
/// Where `prepareReview` rejects after pruning: Node refuses a `PREP_SHA` it
/// cannot convert to a string, or one holding a NUL byte.
pub async fn prepare_head_worktree(
    tools: &Tools,
    context: &Value,
    repo: &str,
    pr_number: &str,
    cwd: Option<&Path>,
    tmpdir: &str,
) -> Result<Option<String>, String> {
    let sha = js::get(context, "headRefOid");
    if js::truthy(js::get(context, "isDeploymentPR")) || !js::truthy(sha) {
        return Ok(None);
    }
    prune_stale_worktrees(tools, cwd, now_ms(), STALE_WORKTREE_MS).await;
    let sha = sha.map_or(Ok(String::new()), js_string)?;
    if sha.contains('\0') {
        return Err(
            "The property 'options.env['PREP_SHA']' must be a string without null bytes".into(),
        );
    }
    let out = build_head_worktree(tools, &sha, repo, pr_number, cwd, tmpdir).await;
    Ok(Some(js::trim(&out).to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_empty_marker_is_not_a_pin() {
        assert_eq!(
            parse_head_worktree(Some(&json!("CREATED_WORKTREE "))),
            Ok(HeadPin::none())
        );
        assert_eq!(
            parse_head_worktree(Some(&json!("CLONED:"))),
            Ok(HeadPin::none())
        );
        let p = parse_head_worktree(Some(&json!("CLONED:/a\nCREATED_WORKTREE /b"))).unwrap();
        assert_eq!((p.path.as_deref(), p.kind), (Some("/b"), "worktree"));
    }

    #[test]
    fn review_worktree_names() {
        assert!(is_review_worktree("peer-review-wt-12-aB3xYz"));
        assert!(!is_review_worktree("peer-review-wt-x-aB3xYz"));
        assert!(!is_review_worktree("peer-review-wt-12-a_b"));
        assert!(!is_review_worktree("peer-review-wt-12-"));
    }

    #[test]
    fn repo_pattern_dots_match_any_char() {
        assert!(grep_i("git@github.com:Acme/Widgets.git", "acme/widgets"));
        assert!(grep_i("x/acmeXwidgets", "acme.widgets"));
        assert!(!grep_i("x/acme/widget", "acme/widgets"));
    }
}
