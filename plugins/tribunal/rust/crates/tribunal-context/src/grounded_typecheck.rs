//! The grounded typecheck, run natively: what the workflow's typecheck bash
//! prints, marker for marker, so the Verify phase reads the text the same way
//! whichever produced it. Ports the standalone `grounded-typecheck` dispatch
//! and the typecheck half of the fused availability probe, which reuses the
//! pinned checkout when there is one.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::exec::{run as exec, Tools};
use crate::head_worktree::{grep_i, in_cwd, input_lines, mktemp_then_rmdir, rm_rf, words};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypecheckRun {
    pub text: String,
    /// A worktree this run made, which teardown removes.
    pub created_worktree: Option<String>,
}

fn is_file(p: &Path) -> bool {
    p.metadata().is_ok_and(|m| m.is_file())
}

/// `{ [ -f A ] || [ -f B ]; } && ! cmp -s A B`.
fn schema_drift(repo: &Path, wt: &Path) -> bool {
    let a = repo.join("prisma/schema.prisma");
    let b = wt.join("prisma/schema.prisma");
    if !is_file(&a) && !is_file(&b) {
        return false;
    }
    match (std::fs::read(&a), std::fs::read(&b)) {
        (Ok(x), Ok(y)) => x != y,
        _ => true,
    }
}

async fn output_of(program: &str, args: &[&str], cwd: &Path, tools: &Tools) -> String {
    match exec(program, args, Some(cwd), &tools.env).await {
        Ok(o) => format!("{}{}", o.stdout, o.stderr),
        Err(_) => String::new(),
    }
}

/// `npx tsc --showConfig | python3 …`: the files the program compiles, with a
/// leading `./` dropped, or nothing when the config does not parse.
fn covered_files(show_config: &str) -> Vec<String> {
    let Ok(Value::Object(d)) = serde_json::from_str::<Value>(show_config) else {
        return Vec::new();
    };
    let Some(Value::Array(files)) = d.get("files") else {
        return Vec::new();
    };
    files
        .iter()
        .map_while(Value::as_str)
        .map(|f| f.strip_prefix("./").unwrap_or(f).to_owned())
        .collect()
}

/// `typecheckCoverageBash`: the program's size, then the PR files in it.
fn coverage_lines(cov: &[String], pr_files: &[String]) -> Vec<String> {
    let cov_text = cov.join("\n");
    let cov_text = cov_text.trim_end_matches('\n');
    let count = input_lines(&format!("{cov_text}\n"))
        .filter(|l| !l.is_empty())
        .count();
    let patterns: Vec<&str> = if pr_files.is_empty() {
        vec![""]
    } else {
        pr_files.iter().map(String::as_str).collect()
    };
    let mut out = vec![format!("---COVERED--- {count}")];
    out.extend(
        input_lines(&format!("{cov_text}\n"))
            .filter(|l| patterns.contains(l))
            .map(str::to_owned),
    );
    out
}

/// Runs the typecheck the workflow's bash runs and returns what it prints.
/// `cwd` is the ambient checkout; `pinned` the head worktree when there is
/// one, else this makes its own at `head_sha`.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    tools: &Tools,
    repo: &str,
    pr_number: &str,
    head_sha: &str,
    pr_files: &[String],
    pinned: Option<&str>,
    cwd: &Path,
    tmpdir: &str,
) -> TypecheckRun {
    let mut lines: Vec<String> = Vec::new();
    let mut created = None;
    let done = |lines: Vec<String>, created: Option<String>| TypecheckRun {
        text: lines.join("\n"),
        created_worktree: created,
    };
    let ambient = tools
        .git(&["remote", "get-url", "origin"], Some(cwd))
        .await
        .is_ok_and(|o| input_lines(&o.stdout).any(|l| grep_i(l, repo)));
    if !ambient {
        return done(vec!["DEGRADED_WRONG_REPO".into()], None);
    }
    let top = match tools
        .git(&["rev-parse", "--show-toplevel"], Some(cwd))
        .await
    {
        Ok(o) if o.ok() => o.stdout.trim_end_matches('\n').to_owned(),
        _ => String::new(),
    };
    let repo_dir = PathBuf::from(&top);
    if !is_file(&PathBuf::from(format!("{top}/tsconfig.json"))) {
        return done(vec!["NO_TSCONFIG".into()], None);
    }
    let wt = match pinned.filter(|p| !p.is_empty()) {
        Some(p) => p.to_owned(),
        None => {
            let wt = mktemp_then_rmdir(tmpdir, pr_number, Some(cwd));
            let sha = words(head_sha);
            let add: Vec<&str> = ["worktree", "add", "--force", "--detach", wt.as_str()]
                .into_iter()
                .chain(sha.iter().copied())
                .collect();
            if !tools.git(&add, Some(cwd)).await.is_ok_and(|o| o.ok()) {
                let fetch: Vec<&str> = ["fetch", "origin"]
                    .into_iter()
                    .chain(sha.iter().copied())
                    .collect();
                let _ = tools.git(&fetch, Some(cwd)).await;
                let _ = tools.git(&add, Some(cwd)).await;
            }
            if tools
                .git(&["-C", &wt, "rev-parse", "HEAD"], Some(cwd))
                .await
                .is_ok_and(|o| o.ok())
            {
                lines.push(format!("CREATED_WORKTREE {wt}"));
                created = Some(wt.clone());
                wt
            } else {
                lines.push("DEGRADED_NO_WORKTREE".into());
                return done(lines, None);
            }
        }
    };
    let wt_dir = in_cwd(Some(cwd), &wt);
    if schema_drift(&repo_dir, &wt_dir) {
        lines.push("DEGRADED_SCHEMA_DRIFT".into());
        return done(lines, created);
    }
    let nm = wt_dir.join("node_modules");
    if !nm.exists() {
        let _ = crate::proc::link_dir(&repo_dir.join("node_modules"), &nm);
    }
    if !wt_dir.is_dir() {
        lines.push("DEGRADED_NO_CD".into());
        return done(lines, created);
    }
    let has_script = std::fs::read_to_string(wt_dir.join("package.json"))
        .is_ok_and(|p| p.contains("\"typecheck\""));
    let out = if has_script {
        output_of("npm", &["run", "--silent", "typecheck"], &wt_dir, tools).await
    } else {
        output_of(
            "npx",
            &["tsc", "--noEmit", "--pretty", "false"],
            &wt_dir,
            tools,
        )
        .await
    };
    let out = format!("{}\n", out.trim_end_matches('\n'));
    let tsc: Vec<&str> = input_lines(&out)
        .filter(|l| l.contains("): error TS"))
        .take(500)
        .collect();
    if tsc.is_empty() {
        lines.push("NO_DIAGNOSTICS".into());
    } else {
        lines.extend(tsc.iter().map(|l| (*l).to_owned()));
    }
    let show = match exec("npx", &["tsc", "--showConfig"], Some(&wt_dir), &tools.env).await {
        Ok(o) => o.stdout,
        Err(_) => String::new(),
    };
    lines.extend(coverage_lines(&covered_files(&show), pr_files));
    done(lines, created)
}

/// The teardown of a tree the run made: `git worktree remove --force` for a
/// worktree, `rm -rf` for a plain clone. Best effort, as in the workflow.
pub async fn remove_tree(tools: &Tools, path: &str, plain_clone: bool, cwd: &Path) {
    if plain_clone {
        rm_rf(path, Some(cwd));
    } else {
        let _ = tools
            .git(&["worktree", "remove", "--force", path], Some(cwd))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_counts_the_program_and_lists_the_pr_files_in_it() {
        let cov = vec!["src/a.ts".to_owned(), "src/b.ts".to_owned()];
        assert_eq!(
            coverage_lines(&cov, &["src/b.ts".to_owned(), "x.ts".to_owned()]),
            vec!["---COVERED--- 2", "src/b.ts"]
        );
        assert_eq!(coverage_lines(&[], &[]), vec!["---COVERED--- 0", ""]);
        assert_eq!(
            covered_files(r#"{"files": ["./src/a.ts", "b.ts", 3, "c.ts"]}"#),
            vec!["src/a.ts", "b.ts"]
        );
    }
}
