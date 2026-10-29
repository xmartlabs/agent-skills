//! What a reviewer or refuter is told about the diff and the checkout, and the
//! full text a lens is dispatched with. Ports `diffBody`, `scopedDiffBody`,
//! the diff tails and `lensDiffBlockFor` from the workflow's Context phase.

use serde_json::{json, Value};
use tribunal_context::head_worktree::pinned_checkout_block;
use tribunal_core::lens::lens_prompt::lens_prompt;
use tribunal_core::profile::conventions_channel::ConventionsChannel;

/// The workflow's `SCOPE_CLAUDE_RECIPIENTS`. While it is false every lens and
/// refuter reads the whole patch, so the sectioned branch of `scopedDiffBody`
/// is not ported; `tests/diff_prompts.rs` fails if the workflow flips it.
pub const SCOPE_CLAUDE_RECIPIENTS: bool = false;

pub const SELF_FETCH_INSTRUCTION: &str = r#"Fetch the diff yourself per your own "PR number provided" scope rule (gh pr view, gh pr diff)."#;

/// `repo.replace(/[^A-Za-z0-9]+/g, "-")`.
fn repo_slug(repo: &str) -> String {
    let mut slug = String::new();
    let mut in_run = false;
    for c in repo.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
            in_run = false;
        } else if !in_run {
            slug.push('-');
            in_run = true;
        }
    }
    slug
}

fn run_file(tmpdir: &str, kind: &str, repo: &str, pr_number: &str) -> String {
    format!(
        "{tmpdir}/tribunal-{kind}-{}-{pr_number}.md",
        repo_slug(repo)
    )
}

/// `METHODOLOGY_PATH`: where the run's assembled methodology lives.
#[must_use]
pub fn methodology_path(repo: &str, pr_number: &str) -> String {
    run_file(
        &tribunal_context::proc::fallback_tmpdir(),
        "methodology",
        repo,
        pr_number,
    )
}

/// Where load-conventions writes the run's convention file, beside the
/// methodology.
#[must_use]
pub fn conventions_path(repo: &str, pr_number: &str) -> String {
    run_file(
        &tribunal_context::proc::fallback_tmpdir(),
        "conventions",
        repo,
        pr_number,
    )
}

#[must_use]
pub fn methodology_line(path: &str) -> String {
    format!("METHODOLOGY FILE: {path}")
}

/// Counters behind the metrics row's `diffExposure`, counted where a block
/// is actually placed in a prompt.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiffExposure {
    pub lens_receivers: u32,
    pub refuter_receivers: u32,
    pub sectioned_receivers: u32,
    pub full_diff_receivers: u32,
    pub delivered_bytes: f64,
}

/// How this run hands the diff out: by path when fetch-pr wrote one, else
/// each agent fetches it.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffDelivery {
    pub diff_path: Option<String>,
    /// `prContext.diffBytes` when it is a number.
    pub diff_bytes: Option<f64>,
    /// `headPin` as the workflow holds it: `{path, kind, ok}`.
    pub head_pin: Value,
    /// `prContext.headRefOid`.
    pub head_sha: Option<Value>,
}

impl DiffDelivery {
    /// `diffBody(tail)`.
    #[must_use]
    pub fn diff_body(&self, tail: &str) -> String {
        match &self.diff_path {
            Some(p) => format!("The full PR diff has already been fetched to {p} — read that file instead of re-running gh pr view / gh pr diff. It is the authoritative copy of this PR's changes.\n{tail}"),
            None => format!("{SELF_FETCH_INSTRUCTION}\n{tail}"),
        }
    }

    fn full_diff_bytes(&self) -> f64 {
        match (&self.diff_path, self.diff_bytes) {
            (Some(_), Some(b)) => b,
            _ => 0.0,
        }
    }

    fn scoped_diff_body(&self, tail: &str, exposure: &mut DiffExposure) -> String {
        exposure.full_diff_receivers += 1;
        exposure.delivered_bytes += self.full_diff_bytes();
        self.diff_body(tail)
    }

    /// # Errors
    /// Where printing the pin or the sha throws in JavaScript.
    pub fn pinned_block(&self) -> Result<String, String> {
        pinned_checkout_block(Some(&self.head_pin), self.head_sha.as_ref())
    }

    /// `lensDiffBlockFor(files)`.
    ///
    /// # Errors
    /// As [`DiffDelivery::pinned_block`].
    pub fn lens_diff_block(&self, exposure: &mut DiffExposure) -> Result<String, String> {
        exposure.lens_receivers += 1;
        let tail = format!(
            "{} Any read BEYOND the diff — surrounding code, call sites, sibling surfaces — belongs there.",
            self.pinned_block()?
        );
        Ok(self.scoped_diff_body(&tail, exposure))
    }

    /// `refuterDiffBlockFor(findings)`.
    ///
    /// # Errors
    /// As [`DiffDelivery::pinned_block`].
    pub fn refuter_diff_block(&self, exposure: &mut DiffExposure) -> Result<String, String> {
        exposure.refuter_receivers += 1;
        let tail = format!(
            "{} Read the surrounding code and call sites there to check whether the claim actually bites — the diff alone is not enough to confirm or refute it.",
            self.pinned_block()?
        );
        Ok(self.scoped_diff_body(&tail, exposure))
    }
}

/// The per-lens inputs of `lensPrompt`.
pub struct LensPromptInput<'a> {
    pub pr_number: &'a Value,
    pub repo: &'a str,
    pub jira_ticket: Option<&'a Value>,
    pub context_block: &'a str,
    pub dependents: &'a str,
    pub high_risk_surface: Option<&'a Value>,
    pub focus: &'a Value,
    pub files: Option<&'a [String]>,
}

/// The whole prompt a lens is dispatched with: the methodology line, the
/// conventions line, then `lensPrompt`.
///
/// # Errors
/// Where the JavaScript would throw building it.
pub fn lens_dispatch_prompt(
    input: &LensPromptInput,
    delivery: &DiffDelivery,
    conventions: &ConventionsChannel,
    methodology_line: &str,
    exposure: &mut DiffExposure,
) -> Result<String, String> {
    let mut p = json!({
        "prNumber": input.pr_number,
        "repo": input.repo,
        "diffBlock": delivery.lens_diff_block(exposure)?,
        "contextBlock": input.context_block,
        "dependents": input.dependents,
        "focus": input.focus,
        "files": input.files,
    });
    if let Some(j) = input.jira_ticket {
        p["jiraTicket"] = j.clone();
    }
    if let Some(h) = input.high_risk_surface {
        p["highRiskSurface"] = h.clone();
    }
    Ok(conventions.reviewer_prompt(methodology_line, &lens_prompt(&p)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tribunal_context::proc::fallback_tmpdir;
    use tribunal_core::profile::conventions_channel::conventions_channel;

    #[test]
    fn a_windows_tmpdir_keeps_the_repo_slug() {
        let tmp = r"C:\Users\dev\AppData\Local\Temp";
        let repo = "acme/Web_app_FE";
        assert_eq!(
            run_file(tmp, "methodology", repo, "12"),
            r"C:\Users\dev\AppData\Local\Temp/tribunal-methodology-acme-Web-app-FE-12.md"
        );
        assert_eq!(
            run_file(tmp, "conventions", repo, "12"),
            r"C:\Users\dev\AppData\Local\Temp/tribunal-conventions-acme-Web-app-FE-12.md"
        );
    }

    // Runs on every CI platform, so on Windows it reads the real temp dir.
    #[test]
    fn conventions_sit_beside_the_methodology_and_pass_the_channel() {
        let tmp = fallback_tmpdir();
        assert_eq!(
            methodology_path("acme/widgets-api", "12"),
            format!("{tmp}/tribunal-methodology-acme-widgets-api-12.md")
        );
        let conventions = conventions_path("acme/widgets-api", "12");
        assert_eq!(
            conventions,
            format!("{tmp}/tribunal-conventions-acme-widgets-api-12.md")
        );
        let args = json!({"conventionsPath": conventions, "conventionsSha256": "a".repeat(64)});
        conventions_channel(&args, &json!({"conventionFiles": ["CLAUDE.md"]})).unwrap();
    }
}
