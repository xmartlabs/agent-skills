//! `tribunal panel` refuses a bad `--findings` before it starts anything. The
//! binary runs with an empty `PATH`, so a check that let bad input through
//! could not reach a real `claude` or `gh` either.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("tribunal-cli-panel-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("empty-path")).unwrap();
    dir
}

fn panel(dir: &Path, extra: &[&str]) -> Output {
    let mut args = vec![
        "panel",
        "--pr",
        "7",
        "--repo",
        "acme/widgets",
        "--head-sha",
        "abc123",
    ];
    args.extend_from_slice(extra);
    Command::new(env!("CARGO_BIN_EXE_tribunal"))
        .args(&args)
        .env("PATH", dir.join("empty-path"))
        .env("TRIBUNAL_DATA_DIR", dir.join("data"))
        .env(
            "TRIBUNAL_PLUGIN_ROOT",
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."),
        )
        .output()
        .expect("tribunal runs")
}

fn refused(o: &Output, why: &str) {
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "exit {:?}: {err}", o.status);
    let first = err.lines().next().unwrap_or("");
    assert!(first.starts_with("tribunal: "), "{err}");
    assert!(first.contains(why), "{err}");
    assert_eq!(String::from_utf8_lossy(&o.stdout), "");
}

fn findings_file(dir: &Path, text: &str) -> String {
    let p = dir.join("findings.json");
    std::fs::write(&p, text).unwrap();
    p.to_string_lossy().into_owned()
}

#[test]
fn a_missing_findings_file_is_refused() {
    let dir = scratch("missing");
    let gone = dir.join("nope.json");
    let o = panel(&dir, &["--findings", gone.to_str().unwrap()]);
    refused(&o, "cannot read --findings");
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn findings_that_are_not_an_array_are_refused() {
    let dir = scratch("object");
    let f = findings_file(&dir, r#"{"title": "t", "file": "src/a.ts"}"#);
    refused(
        &panel(&dir, &["--findings", &f]),
        "must be a JSON array of findings",
    );
}

#[test]
fn a_finding_without_a_title_is_refused() {
    let dir = scratch("untitled");
    let f = findings_file(
        &dir,
        r#"[{"title": "ok", "file": "src/a.ts"}, {"file": "src/b.ts", "claim": "c"}]"#,
    );
    refused(&panel(&dir, &["--findings", &f]), "finding 1 has no title");
}

#[test]
fn the_pinned_commit_and_the_findings_are_required() {
    let dir = scratch("flags");
    let f = findings_file(&dir, r#"[{"title": "t", "file": "src/a.ts"}]"#);
    let o = Command::new(env!("CARGO_BIN_EXE_tribunal"))
        .args([
            "panel",
            "--pr",
            "7",
            "--repo",
            "acme/widgets",
            "--findings",
            &f,
        ])
        .env("PATH", dir.join("empty-path"))
        .output()
        .unwrap();
    refused(&o, "--head-sha <sha> is required");
    assert_eq!(o.status.code(), Some(2));
    refused(&panel(&dir, &[]), "--findings <file.json> is required");
}

#[test]
fn nothing_is_written_to_the_data_dir_on_a_refusal() {
    let dir = scratch("nodata");
    let f = findings_file(&dir, "[]");
    refused(&panel(&dir, &["--findings", &f]), "empty array");
    assert!(!dir.join("data").exists());
}
