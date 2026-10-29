//! The native grounded typecheck against the workflow's own `grounded-typecheck`
//! bash, both run on the same git fixture repos: the text they print must be
//! the same once the worktree paths are named alike.
//!
//! Both sides run a real `tsc`. `TRIBUNAL_TS_FROM` names a directory whose
//! `node_modules` holds typescript; the tests are `#[ignore]` without it.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{span, Span};
use serde_json::json;
use tribunal_context::exec::Tools;
use tribunal_context::grounded_typecheck::{remove_tree, run};

const REPO: &str = "acme/widgets";
const PR: &str = "77";

fn ts_from() -> PathBuf {
    let dir = std::env::var_os("TRIBUNAL_TS_FROM")
        .filter(|d| !d.is_empty())
        .expect("TRIBUNAL_TS_FROM must name a directory whose node_modules holds typescript");
    let dir = PathBuf::from(dir);
    assert!(
        dir.join("node_modules/typescript").is_dir(),
        "no typescript under {}",
        dir.display()
    );
    dir
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

struct Fixture {
    name: &'static str,
    remote: &'static str,
    files: Vec<(&'static str, &'static str)>,
    /// Written after the commit, so the ambient tree differs from the head.
    dirty: Vec<(&'static str, &'static str)>,
    pr_files: Vec<&'static str>,
}

const TSCONFIG: &str = r#"{"compilerOptions": {"strict": true, "noEmit": true, "skipLibCheck": true}, "include": ["src"]}"#;

fn fixtures() -> Vec<Fixture> {
    let remote = "https://github.com/Acme/Widgets.git";
    vec![
        Fixture {
            name: "diagnostics",
            remote,
            files: vec![
                ("tsconfig.json", TSCONFIG),
                (
                    "src/a.ts",
                    "export const n: number = 'x';\nexport const m: string = 3;\n",
                ),
                ("src/b.ts", "export const ok = 1;\n"),
                ("scripts/x.ts", "const y: number = 'z';\n"),
            ],
            dirty: vec![],
            pr_files: vec!["src/a.ts", "scripts/x.ts"],
        },
        Fixture {
            name: "clean",
            remote,
            files: vec![
                ("tsconfig.json", TSCONFIG),
                ("src/a.ts", "export const n = 1;\n"),
            ],
            dirty: vec![],
            pr_files: vec!["src/a.ts"],
        },
        Fixture {
            name: "script",
            remote,
            files: vec![
                ("tsconfig.json", TSCONFIG),
                (
                    "package.json",
                    r#"{"name": "w", "scripts": {"typecheck": "echo 'lint error: not a diagnostic' && tsc --noEmit --pretty false"}}"#,
                ),
                ("src/a.ts", "export const n: number = 'x';\n"),
            ],
            dirty: vec![],
            pr_files: vec![],
        },
        Fixture {
            name: "no-tsconfig",
            remote,
            files: vec![("src/a.ts", "export const n = 1;\n")],
            dirty: vec![],
            pr_files: vec!["src/a.ts"],
        },
        Fixture {
            name: "wrong-repo",
            remote: "https://github.com/other/thing.git",
            files: vec![
                ("tsconfig.json", TSCONFIG),
                ("src/a.ts", "export const n = 1;\n"),
            ],
            dirty: vec![],
            pr_files: vec!["src/a.ts"],
        },
        Fixture {
            name: "schema-drift",
            remote,
            files: vec![
                ("tsconfig.json", TSCONFIG),
                ("prisma/schema.prisma", "model A { id Int @id }\n"),
                ("src/a.ts", "export const n = 1;\n"),
            ],
            dirty: vec![("prisma/schema.prisma", "model B { id Int @id }\n")],
            pr_files: vec!["src/a.ts"],
        },
    ]
}

fn build(root: &Path, f: &Fixture, ts: &Path) -> (PathBuf, String) {
    let dir = root.join(f.name);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["remote", "add", "origin", f.remote]);
    for (p, text) in &f.files {
        let at = dir.join(p);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, text).unwrap();
    }
    git(&dir, &["add", "-A"]);
    git(
        &dir,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "head",
        ],
    );
    for (p, text) in &f.dirty {
        std::fs::write(dir.join(p), text).unwrap();
    }
    std::os::unix::fs::symlink(ts.join("node_modules"), dir.join("node_modules")).unwrap();
    let sha = git(&dir, &["rev-parse", "HEAD"]);
    (dir, sha)
}

/// The workflow's standalone typecheck prompt, with the bash cut out of it.
fn workflow_bash(sha: &str, pr_files: &[&str]) -> String {
    let out = span(
        &Span {
            start: "if (!typecheckHandledInline && groundedEligible && wantsHeadWorktree) {",
            end: "  applyTypecheckResult(text);\n}",
            deps: &["VERIFIED-RELAY", "SCHEMA-DRIFT-GUARD", "TYPECHECK-COVERAGE"],
            pre: &[],
            fns: &["agent", "applyTypecheckResult", "coverageListLimit"],
            ret: "globalThis.__prompt",
        },
        &[json!({
            "typecheckHandledInline": false, "groundedEligible": true, "wantsHeadWorktree": true,
            "repo": REPO, "prNumber": PR,
            "prContext": {"headRefOid": sha, "files": pr_files},
            "agent": "async (p) => { globalThis.__prompt = p; return ''; }",
            "applyTypecheckResult": "() => {}",
            "coverageListLimit": "Infinity",
            "MECHANICAL": {},
            "groundedWorktreePath": null,
        })],
    )
    .remove(0);
    let prompt = out.as_str().unwrap();
    let marker = "Run EXACTLY this bash and return its output verbatim:\n";
    prompt[prompt.find(marker).unwrap() + marker.len()..].to_owned()
}

fn plugin_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

fn normalize(text: &str) -> (String, Option<String>) {
    let mut wt = None;
    let lines: Vec<String> = text
        .trim_end_matches('\n')
        .split('\n')
        .map(|l| match l.strip_prefix("CREATED_WORKTREE ") {
            Some(p) => {
                wt = Some(p.trim().to_owned());
                "CREATED_WORKTREE <wt>".to_owned()
            }
            None => l.to_owned(),
        })
        .collect();
    (lines.join("\n"), wt)
}

#[tokio::test]
#[ignore = "needs TRIBUNAL_TS_FROM"]
async fn the_native_typecheck_prints_what_the_workflow_bash_prints() {
    let ts = ts_from();
    let root = std::env::temp_dir().join(format!("tribunal-tsc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let tmp = root.join("tmp");
    let bin = root.join("bin");
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    let tr = bin.join("tribunal-root");
    std::fs::write(
        &tr,
        format!("#!/bin/sh\necho '{}'\n", plugin_root().display()),
    )
    .unwrap();
    std::fs::set_permissions(&tr, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let tools = Tools::default();
    let mut kinds = Vec::new();
    for f in fixtures() {
        let (dir, sha) = build(&root, &f, &ts);
        let bash = workflow_bash(&sha, &f.pr_files);
        let js = Command::new("bash")
            .arg("-c")
            .arg(&bash)
            .current_dir(&dir)
            .env("TMPDIR", &tmp)
            .env("PATH", &path)
            .output()
            .unwrap();
        let (js_text, js_wt) = normalize(&String::from_utf8_lossy(&js.stdout));
        let pr_files: Vec<String> = f.pr_files.iter().map(|s| (*s).to_owned()).collect();
        let ours = run(
            &tools,
            REPO,
            PR,
            &sha,
            &pr_files,
            None,
            &dir,
            &tmp.to_string_lossy(),
        )
        .await;
        let (rs_text, rs_wt) = normalize(&ours.text);
        assert_eq!(rs_text, js_text, "fixture {}", f.name);
        assert_eq!(rs_wt.is_some(), ours.created_worktree.is_some());
        for wt in [js_wt, rs_wt].into_iter().flatten() {
            remove_tree(&tools, &wt, false, &dir).await;
            assert!(!Path::new(&wt).exists(), "{wt} left behind");
        }
        kinds.push(json!({"fixture": f.name, "text": rs_text}));
    }
    let text = |name: &str| -> String {
        kinds.iter().find(|k| k["fixture"] == name).unwrap()["text"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert!(text("diagnostics").contains("src/a.ts(1,14): error TS2322"));
    assert!(text("diagnostics").contains("---COVERED--- 2\nsrc/a.ts"));
    assert!(text("clean").contains("NO_DIAGNOSTICS"));
    assert!(text("script").contains("error TS2322"));
    assert_eq!(text("no-tsconfig"), "NO_TSCONFIG");
    assert_eq!(text("wrong-repo"), "DEGRADED_WRONG_REPO");
    assert!(text("schema-drift").ends_with("DEGRADED_SCHEMA_DRIFT"));
    let _ = std::fs::remove_dir_all(&root);
}
