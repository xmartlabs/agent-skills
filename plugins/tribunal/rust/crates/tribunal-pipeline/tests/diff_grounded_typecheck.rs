//! The native grounded typecheck against the workflow's own `grounded-typecheck`
//! bash, both run on the same git fixture repos: the text they print must be
//! the same once the worktree paths are named alike.
//!
//! The port runs a real `tsc`. `TRIBUNAL_TS_FROM` names a directory whose
//! `node_modules` holds typescript; the comparison is `#[ignore]` without it.
//!
//! The bash's side is read from `tests/goldens/grounded-typecheck.json`: the
//! workflow and the `rust/crates/tribunal-context/src/js/relay.mjs` its bash runs are being retired, so a run
//! starts only the port. The file keeps each fixture, the bash the span oracle
//! cut out for it, the typescript version it ran, and a SHA-256 of the
//! workflow code that bash came from, of the span oracle's own files and of
//! `rust/crates/tribunal-context/src/js/relay.mjs`, so editing any of them without a regeneration fails here.
//! A source that is gone from disk is retired and is not checked; the golden
//! is final. The scratch directory and each fixture's head commit are stored
//! as `<ROOT>` and `<SHA>`. Regenerate after changing them:
//! `TRIBUNAL_REGEN_GOLDENS=1 TRIBUNAL_TS_FROM=<dir> cargo test -p tribunal-pipeline --test diff_grounded_typecheck -- --include-ignored`.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{span, Span};
use serde_json::{json, Value};
use tribunal_context::exec::Tools;
use tribunal_context::grounded_typecheck::{remove_tree, run};
use tribunal_context::harness_version::sha256_hex;

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

fn ts_version(ts: &Path) -> String {
    let text = std::fs::read_to_string(ts.join("node_modules/typescript/package.json")).unwrap();
    let pkg: Value = serde_json::from_str(&text).unwrap();
    pkg["version"].as_str().unwrap_or_default().to_owned()
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

const TYPECHECK: Span<'static> = Span {
    start: "if (!typecheckHandledInline && groundedEligible && wantsHeadWorktree) {",
    end: "  applyTypecheckResult(text);\n}",
    deps: &["VERIFIED-RELAY", "SCHEMA-DRIFT-GUARD", "TYPECHECK-COVERAGE"],
    pre: &[],
    fns: &["agent", "applyTypecheckResult", "coverageListLimit"],
    ret: "globalThis.__prompt",
};

/// The workflow's standalone typecheck prompt, with the bash cut out of it.
fn workflow_bash(sha: &str, pr_files: &[&str]) -> String {
    let out = span(
        &TYPECHECK,
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

// ---------------------------------------------------------------------------
// Golden: the bash's side recorded once, so a run starts only the port.

const GOLDEN: &str = "grounded-typecheck.json";
const REGENERATE: &str = "regenerate with `TRIBUNAL_REGEN_GOLDENS=1 TRIBUNAL_TS_FROM=<dir> cargo test -p tribunal-pipeline --test diff_grounded_typecheck -- --include-ignored`";
const WORKFLOW: &str = "workflow-src/tribunal.js";
/// The span oracle's own files and the script the bash runs through
/// `tribunal-root`, by path in the plugin.
const ORACLE_FILES: &[&str] = &[
    "rust/oracle/span-oracle.mjs",
    "skills/review-pr/eval/extract-pure-region.mjs",
    "rust/crates/tribunal-context/src/js/relay.mjs",
];

fn regenerating() -> bool {
    std::env::var("TRIBUNAL_REGEN_GOLDENS").is_ok_and(|v| v == "1")
}

/// The workflow code the span oracle runs for `s`, cut out the way it cuts it;
/// `None` when an anchor is no longer in `src`.
fn span_source(src: &str, s: &Span) -> Option<String> {
    let cut = |start: &str, end: &str| -> Option<String> {
        let a = src.find(start)?;
        let e = a + src[a..].find(end)?;
        Some(src[a..e + end.len()].to_owned())
    };
    let mut parts = Vec::new();
    for d in s.deps {
        let (begin, end) = (
            format!("// ===== BEGIN {d} ====="),
            format!("// ===== END {d} ====="),
        );
        let mut from = 0;
        while let Some(b) = src[from..].find(&begin).map(|b| b + from) {
            let e = b + src[b..].find(&end)?;
            parts.push(src[b + begin.len()..e].to_owned());
            from = e + end.len();
        }
    }
    for (a, b) in s.pre {
        parts.push(cut(a, b)?);
    }
    parts.push(cut(s.start, s.end)?);
    Some(parts.join("\n"))
}

/// The SHA-256 of what the recorded bash came from, by path in the plugin
/// under `root`: the span's code for the workflow, the whole file otherwise.
/// A file that is gone maps to `None`: that oracle is retired.
fn sources(root: &Path) -> BTreeMap<String, Option<String>> {
    let mut out = BTreeMap::new();
    let workflow = std::fs::read_to_string(root.join(WORKFLOW)).ok();
    out.insert(
        WORKFLOW.to_owned(),
        workflow.map(|w| sha256_hex(span_source(&w, &TYPECHECK).unwrap_or_default().as_bytes())),
    );
    for f in ORACLE_FILES {
        let sha = std::fs::read(root.join(f)).ok().map(|b| sha256_hex(&b));
        out.insert((*f).to_owned(), sha);
    }
    out
}

/// The sources still on disk that differ from `recorded`; a retired one is
/// skipped, since nothing can regenerate from it.
fn stale(recorded: &Value, now: &BTreeMap<String, Option<String>>) -> Vec<String> {
    let mut keys: Vec<&String> = recorded
        .as_object()
        .map(|o| o.keys().collect())
        .unwrap_or_default();
    keys.sort();
    if keys != now.keys().collect::<Vec<_>>() {
        return vec!["the list of sources".to_owned()];
    }
    now.iter()
        .filter(|(k, sha)| {
            sha.as_ref()
                .is_some_and(|s| recorded[k.as_str()] != json!(s))
        })
        .map(|(k, _)| k.clone())
        .collect()
}

/// The bash's side of each fixture and the typescript version it ran: run by
/// `record` and written to the golden when regenerating, else read back after
/// checking the sources and the fixtures. `swaps` pairs this run's paths with
/// their placeholders.
fn recorded(
    swaps: &[(&str, &str)],
    typescript: &str,
    inputs: &[Value],
    record: impl FnOnce() -> Vec<Value>,
) -> (Vec<Value>, String) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens")
        .join(GOLDEN);
    if regenerating() {
        let with: BTreeMap<String, String> = sources(&plugin_root())
            .into_iter()
            .map(|(k, sha)| {
                let sha = sha.unwrap_or_else(|| panic!("{k} is gone: nothing to regenerate from"));
                (k, sha)
            })
            .collect();
        let js = record();
        let cases: Vec<Value> = inputs
            .iter()
            .zip(&js)
            .map(|(input, js)| json!({"input": input, "js": js}))
            .collect();
        let golden = json!({"recordedWith": with, "typescript": typescript, "cases": cases});
        let text = swaps.iter().fold(
            serde_json::to_string_pretty(&golden).unwrap(),
            |t, (real, placeholder)| t.replace(real, placeholder),
        );
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text + "\n").unwrap();
        return (js, typescript.to_owned());
    }
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; {REGENERATE}", path.display()));
    let text = swaps
        .iter()
        .fold(text, |t, (real, placeholder)| t.replace(placeholder, real));
    let golden: Value = serde_json::from_str(&text).unwrap();
    let stale = stale(&golden["recordedWith"], &sources(&plugin_root()));
    assert!(
        stale.is_empty(),
        "{} changed since {} was recorded; {REGENERATE}",
        stale.join(", "),
        path.display()
    );
    let cases = golden["cases"].as_array().unwrap();
    assert_eq!(
        cases.len(),
        inputs.len(),
        "the fixtures changed; {REGENERATE}"
    );
    let js = cases
        .iter()
        .zip(inputs)
        .map(|(c, input)| {
            assert_eq!(&c["input"], input, "the fixtures changed; {REGENERATE}");
            c["js"].clone()
        })
        .collect();
    (
        js,
        golden["typescript"].as_str().unwrap_or_default().to_owned(),
    )
}

#[test]
fn a_retired_source_is_not_checked_but_a_changed_one_is() {
    let root = std::env::temp_dir().join(format!("tribunal-tsc-retired-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let gone = sources(&root);
    assert!(gone.values().all(Option::is_none));
    let recorded = json!(gone
        .keys()
        .map(|k| (k, "recorded"))
        .collect::<BTreeMap<_, _>>());
    assert!(stale(&recorded, &gone).is_empty());
    for (i, f) in [WORKFLOW, "rust/crates/tribunal-context/src/js/relay.mjs"]
        .into_iter()
        .enumerate()
    {
        let at = root.join(f);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(&at, "edited").unwrap();
        assert_eq!(stale(&recorded, &sources(&root)).len(), i + 1, "{f}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
#[ignore = "needs TRIBUNAL_TS_FROM"]
async fn the_native_typecheck_prints_what_the_workflow_bash_prints() {
    let ts = ts_from();
    let root = std::env::temp_dir().join(format!("tribunal-tsc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let tmp = root.join("tmp");
    let bin = root.join("bin");
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    let tools = Tools::default();
    let built: Vec<(Fixture, PathBuf, String)> = fixtures()
        .into_iter()
        .map(|f| {
            let (dir, sha) = build(&root, &f, &ts);
            (f, dir, sha)
        })
        .collect();
    let inputs: Vec<Value> = built
        .iter()
        .map(|(f, _, _)| {
            json!({"name": f.name, "remote": f.remote, "files": f.files, "dirty": f.dirty, "prFiles": f.pr_files})
        })
        .collect();
    let root_text = root.to_string_lossy();
    let mut js_worktrees: Vec<(String, PathBuf)> = Vec::new();
    let (js, typescript) = recorded(&[(&root_text, "<ROOT>")], &ts_version(&ts), &inputs, || {
        let tr = bin.join("tribunal-root");
        std::fs::write(
            &tr,
            format!("#!/bin/sh\necho '{}'\n", plugin_root().display()),
        )
        .unwrap();
        std::fs::set_permissions(&tr, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        built
                .iter()
                .map(|(f, dir, sha)| {
                    let bash = workflow_bash(sha, &f.pr_files);
                    let js = Command::new("bash")
                        .arg("-c")
                        .arg(&bash)
                        .current_dir(dir)
                        .env("TMPDIR", &tmp)
                        .env("PATH", &path)
                        .output()
                        .unwrap();
                    let (js_text, js_wt) = normalize(&String::from_utf8_lossy(&js.stdout));
                    js_worktrees.extend(js_wt.map(|wt| (wt, dir.clone())));
                    json!({"bash": bash.replace(sha.as_str(), "<SHA>"), "text": js_text.replace(sha.as_str(), "<SHA>")})
                })
                .collect()
    });
    for (wt, dir) in js_worktrees {
        remove_tree(&tools, &wt, false, &dir).await;
        assert!(!Path::new(&wt).exists(), "{wt} left behind");
    }
    let mut kinds = Vec::new();
    for ((f, dir, sha), js) in built.iter().zip(&js) {
        let pr_files: Vec<String> = f.pr_files.iter().map(|s| (*s).to_owned()).collect();
        let ours = run(
            &tools,
            REPO,
            PR,
            sha,
            &pr_files,
            None,
            dir,
            &tmp.to_string_lossy(),
        )
        .await;
        let (rs_text, rs_wt) = normalize(&ours.text);
        assert_eq!(
            json!(rs_text.replace(sha.as_str(), "<SHA>")),
            js["text"],
            "fixture {} (recorded with typescript {typescript}, this run has {})",
            f.name,
            ts_version(&ts)
        );
        assert_eq!(rs_wt.is_some(), ours.created_worktree.is_some());
        if let Some(wt) = rs_wt {
            remove_tree(&tools, &wt, false, dir).await;
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
