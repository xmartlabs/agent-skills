//! A parser panic costs only the file that caused it, as a throw inside the
//! script's per-file `try { … } catch {}` does: the run finishes and every
//! other file still counts. The panic is injected with
//! `ast_bindings::set_panic_marker`, which makes parsing any file holding the
//! marker panic.

use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tribunal_context::ast_bindings;
use tribunal_context::blast_radius;
use tribunal_context::exec::Tools;

const MARKER: &str = "PARSER-PANIC-7f3e";
const CHANGED_MARKER: &str = "PARSER-PANIC-changed-91c2";

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-C", dir.to_str().unwrap()])
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn repo() -> (std::path::PathBuf, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("tribunal-parser-panic-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let repo = root.join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(
        repo.join("src/svc.ts"),
        format!("// {CHANGED_MARKER}\nexport async function load(id: string) {{\n  return id.trim();\n}}\n"),
    )
    .unwrap();
    std::fs::write(
        repo.join("src/a.ts"),
        "import { load } from \"./svc\";\nexport async function a() {\n  await load(\"a\");\n}\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("src/b.ts"),
        format!("// {MARKER}\nimport {{ load }} from \"./svc\";\nexport function b() {{\n  load(\"b\");\n}}\n"),
    )
    .unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "fixture"]);
    let diff = root.join("pr.diff");
    std::fs::write(
        &diff,
        "diff --git a/src/svc.ts b/src/svc.ts\n--- a/src/svc.ts\n+++ b/src/svc.ts\n@@ -1,3 +1,3 @@\n export async function load(id: string) {\n-  return id;\n+  return id.trim();\n }\n",
    )
    .unwrap();
    (repo, diff)
}

fn caller_files(out: &Value) -> Vec<String> {
    out["callers"]
        .as_array()
        .unwrap_or_else(|| panic!("no callers in {out}"))
        .iter()
        .map(|c| c["file"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[tokio::test]
async fn a_parser_panic_drops_only_its_file() {
    let (repo, diff) = repo();
    let tools = Tools::default();
    let run = || blast_radius::run(diff.to_str().unwrap(), repo.to_str().unwrap(), None, &tools);

    let clean = run().await.expect("run without the marker");
    let clean_callers = caller_files(&clean);
    assert!(clean_callers.contains(&"src/a.ts".to_owned()), "{clean}");
    assert!(clean_callers.contains(&"src/b.ts".to_owned()), "{clean}");

    ast_bindings::set_panic_marker(MARKER, true);
    let panicked = run().await;
    ast_bindings::set_panic_marker(MARKER, false);

    let out = panicked.expect("the run finishes although one file's parse panicked");
    let callers = caller_files(&out);
    assert!(callers.contains(&"src/a.ts".to_owned()), "{out}");
    assert!(!callers.contains(&"src/b.ts".to_owned()), "{out}");

    // The changed file itself: returnsPromise and its attention facts fail,
    // the importers still parse.
    ast_bindings::set_panic_marker(CHANGED_MARKER, true);
    let changed = run().await;
    ast_bindings::set_panic_marker(CHANGED_MARKER, false);
    let out = changed.expect("the run finishes although the changed file's parse panicked");
    assert_eq!(caller_files(&out), clean_callers, "{out}");
}

#[test]
fn guarded_turns_a_panic_into_none() {
    assert_eq!(ast_bindings::guarded(|| 7), Some(7));
    assert_eq!(ast_bindings::guarded(|| -> i32 { panic!("boom") }), None);
}

const EMITTER_MARKER: &str = "PARSER-PANIC-emitter-5b07";

#[tokio::test]
async fn a_parser_panic_in_another_emitter_drops_only_its_edges() {
    let root = std::env::temp_dir().join(format!(
        "tribunal-parser-panic-edges-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let repo = root.join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(
        repo.join("src/notify.ts"),
        "export function notify(io: any) {\n  io.emit(\"order:updated\", { id: 1 });\n}\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("src/other.ts"),
        format!(
            "// {EMITTER_MARKER}\nexport function other(io: any) {{\n  io.emit(\"order:updated\", {{ id: 2, status: \"x\" }});\n}}\n"
        ),
    )
    .unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "fixture"]);
    let diff = root.join("pr.diff");
    std::fs::write(
        &diff,
        "diff --git a/src/notify.ts b/src/notify.ts\n--- a/src/notify.ts\n+++ b/src/notify.ts\n@@ -1,2 +1,3 @@\n export function notify(io: any) {\n+  io.emit(\"order:updated\", { id: 1 });\n }\n",
    )
    .unwrap();
    let tools = Tools::default();
    let run = || blast_radius::run(diff.to_str().unwrap(), repo.to_str().unwrap(), None, &tools);

    let clean = run().await.expect("run without the marker").to_string();
    assert!(clean.contains("payload-parity"), "{clean}");

    ast_bindings::set_panic_marker(EMITTER_MARKER, true);
    let marked = run().await;
    ast_bindings::set_panic_marker(EMITTER_MARKER, false);
    let marked = marked
        .expect("the run finishes although the other emitter's parse panicked")
        .to_string();
    assert!(!marked.contains("payload-parity"), "{marked}");
}

const TEST_MARKER: &str = "PARSER-PANIC-test-file-2d44";
const BASE_MARKER: &str = "PARSER-PANIC-base-blob-8e19";

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-C", dir.to_str().unwrap()])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// A changed test file and a changed file whose base version is what panics:
/// the script's per-file `try` covers `testFacts` and `contractFacts` too.
#[tokio::test]
async fn a_parser_panic_in_a_test_file_or_a_base_blob_drops_only_that_step() {
    let root = std::env::temp_dir().join(format!(
        "tribunal-parser-panic-steps-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let repo = root.join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(
        repo.join("src/shape.ts"),
        format!("// {BASE_MARKER}\nexport function shape() {{\n  return {{ a: 1 }};\n}}\n"),
    )
    .unwrap();
    std::fs::write(
        repo.join("src/shape.test.ts"),
        format!("// {TEST_MARKER}\nit(\"works\", () => {{\n  expect(1).toBe(1);\n}});\n"),
    )
    .unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    let base = git_out(&repo, &["rev-parse", "HEAD:src/shape.ts"]);
    std::fs::write(
        repo.join("src/shape.ts"),
        "export function shape() {\n  return { a: 1, b: 2 };\n}\n",
    )
    .unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "head"]);
    let head = git_out(&repo, &["rev-parse", "HEAD:src/shape.ts"]);
    let diff = root.join("pr.diff");
    std::fs::write(
        &diff,
        format!(
            "diff --git a/src/shape.ts b/src/shape.ts\nindex {}..{} 100644\n--- a/src/shape.ts\n+++ b/src/shape.ts\n@@ -1,4 +1,3 @@\n-// {BASE_MARKER}\n export function shape() {{\n-  return {{ a: 1 }};\n+  return {{ a: 1, b: 2 }};\n }}\ndiff --git a/src/shape.test.ts b/src/shape.test.ts\n--- a/src/shape.test.ts\n+++ b/src/shape.test.ts\n@@ -1,3 +1,4 @@\n // {TEST_MARKER}\n it(\"works\", () => {{\n+  expect(2).toBe(2);\n   expect(1).toBe(1);\n }});\n",
            &base[..7],
            &head[..7]
        ),
    )
    .unwrap();
    let tools = Tools::default();
    let run = || blast_radius::run(diff.to_str().unwrap(), repo.to_str().unwrap(), None, &tools);

    run().await.expect("run without markers");
    for marker in [TEST_MARKER, BASE_MARKER] {
        ast_bindings::set_panic_marker(marker, true);
        let out = run().await;
        ast_bindings::set_panic_marker(marker, false);
        out.unwrap_or_else(|e| panic!("the run fails with {marker} set: {e}"));
    }
}
