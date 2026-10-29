//! `lib/section-diff.mjs` against `tribunal_context::section_diff`: the same
//! patch file and arguments on both sides, compared on the relayed JSON line
//! (as text) and on every file written to the output directory, byte for byte.
mod common;
mod patchgen;
use common::*;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use proptest::prelude::*;
use proptest::sample::select;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use serde_json::Value;
use tribunal_context::section_diff::{count_arg, run};
use tribunal_core::profile::js_ext::json_stringify;

const SCRIPT: &str = "lib/section-diff.mjs";

/// Every file under `dir`, by path relative to it.
fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(dir).unwrap().to_string_lossy().into_owned();
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

#[derive(Default, Debug)]
struct Seen {
    cases: usize,
    usable: usize,
    unusable: usize,
    multi_section: usize,
    unparsed: usize,
    truncated: usize,
    cut_in_pair: usize,
    no_ranges: usize,
}

/// Runs both sides on `diff` (raw bytes) and fails on any difference.
/// `out` is the output directory relative to the case directory, as typed.
fn check_case(dir: &Path, diff: &[u8], args: &[String], out: &str, seen: &mut Seen) {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    let diff_path = dir.join("diff.patch");
    std::fs::write(&diff_path, diff).unwrap();
    let diff_arg = diff_path.to_str().unwrap();
    let js_out = format!("{}/js/{out}", dir.display());
    let rs_out = format!("{}/rs/{out}", dir.display());

    let mut js_args = vec![diff_arg, js_out.as_str()];
    js_args.extend(args.iter().map(String::as_str));
    let rec = recordings(dir, vec![]);
    let js = run_script(SCRIPT, &js_args, dir, &rec, None);
    let rust = run(
        diff_arg,
        &rs_out,
        count_arg(args.first().map(String::as_str)),
        count_arg(args.get(1).map(String::as_str)),
    );

    let ctx = || format!("case {} args {args:?} out {out:?}", dir.display());
    assert_eq!(js.code, Some(0), "{}: script failed: {}", ctx(), js.stderr);
    let rust = rust.unwrap_or_else(|e| panic!("{}: port failed: {e}", ctx()));
    let lines: Vec<&str> = js.stdout.split('\n').collect();
    assert_eq!(lines.len(), 3, "{}: {:?}", ctx(), js.stdout);
    assert!(lines[0].starts_with("RELAY-CRC "), "{}", ctx());
    let js_line = lines[1].replace(
        &format!("{}/js/", dir.display()),
        &format!("{}/rs/", dir.display()),
    );
    assert_eq!(json_stringify(Some(&rust)), js_line, "{}", ctx());

    let js_tree = tree(&dir.join("js"));
    let rs_tree = tree(&dir.join("rs"));
    assert_eq!(
        js_tree.keys().collect::<Vec<_>>(),
        rs_tree.keys().collect::<Vec<_>>(),
        "{}: written files",
        ctx()
    );
    for (name, bytes) in &js_tree {
        assert!(
            &rs_tree[name] == bytes,
            "{}: {name} differs ({} vs {} bytes)",
            ctx(),
            bytes.len(),
            rs_tree[name].len()
        );
    }

    seen.cases += 1;
    if rust["usable"] == Value::Bool(true) {
        seen.usable += 1;
    } else {
        seen.unusable += 1;
    }
    let sections = rust["sections"].as_array().unwrap();
    seen.multi_section += usize::from(sections.len() > 1);
    seen.unparsed += usize::from(!rust["unparsed"].as_array().unwrap().is_empty());
    seen.no_ranges += usize::from(sections.iter().any(|s| s["added"] == Value::Array(vec![])));
    for (name, bytes) in &rs_tree {
        if name.ends_with(".slice") && bytes.windows(9).any(|w| w == b"truncated") {
            seen.truncated += 1;
            seen.cut_in_pair += usize::from(bytes.ends_with("\u{FFFD}".as_bytes()));
        }
    }
}

fn arg_pool() -> Vec<String> {
    [
        "0", "0", "12", "50", "200", "3000", "abc", "1e2", " 7 ", "0x20", "-5", "Infinity", "NaN",
        "",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

#[test]
fn section_diff_matches_js_on_generated_patches() {
    let root = scratch("section-diff-gen");
    let strategy = (
        patchgen::diff(),
        proptest::collection::vec(select(arg_pool()), 0..=2),
        select(&["out", "out/./nested/", "a/../out2", "deep/x/y/"][..]),
    );
    let mut runner = TestRunner::deterministic();
    let mut seen = Seen::default();
    for i in 0..300 {
        let ((diff, _), args, out) = strategy.new_tree(&mut runner).unwrap().current();
        check_case(
            &root.join(i.to_string()),
            diff.as_bytes(),
            &args,
            out,
            &mut seen,
        );
    }
    eprintln!("{seen:?}");
    // The generator has to reach every branch the script has.
    assert!(seen.usable >= 100 && seen.unusable >= 20, "{seen:?}");
    assert!(seen.multi_section >= 30 && seen.unparsed >= 30, "{seen:?}");
    assert!(seen.truncated >= 20 && seen.cut_in_pair >= 3, "{seen:?}");
    assert!(seen.no_ranges >= 10, "{seen:?}");
}

#[test]
fn section_diff_matches_js_on_edge_inputs() {
    let root = scratch("section-diff-edges");
    let mut seen = Seen::default();
    let header =
        "diff --git a/src/a.ts b/src/a.ts\n--- a/src/a.ts\n+++ b/src/a.ts\n@@ -1,2 +1,3 @@\n";
    let body = "+const total = computeTotal(items);\n".repeat(8);
    let mut invalid_utf8 = format!("{header}{body}").into_bytes();
    invalid_utf8.extend_from_slice(b"+bad \xff\xfe bytes \xc3\n");
    let crossing = format!("{header}+{}😀😀\n", "a".repeat(6000 - js_len(header) - 2));
    // One section of exactly the excerpt size, and one unit over.
    let at_cut = |n: usize| format!("{header}+{}", "a".repeat(n - js_len(header) - 1)).into_bytes();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("at-cut", at_cut(6000)),
        ("past-cut", at_cut(6001)),
        ("empty", Vec::new()),
        ("plain", format!("{header}{body}").into_bytes()),
        ("invalid-utf8", invalid_utf8),
        ("bom", format!("\u{FEFF}{header}{body}").into_bytes()),
        ("crossing", crossing.into_bytes()),
        (
            "duplicate-path",
            format!("{header}{body}{header}+again\n").into_bytes(),
        ),
    ];
    for (i, (name, diff)) in cases.into_iter().enumerate() {
        for args in [
            vec![],
            vec!["0".to_owned(), "0".to_owned()],
            vec!["9".to_owned()],
        ] {
            check_case(
                &root.join(format!("{name}-{i}-{}", args.len())),
                &diff,
                &args,
                "out",
                &mut seen,
            );
        }
    }
    assert!(seen.cut_in_pair >= 1, "{seen:?}");
}

fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

#[test]
fn section_diff_matches_js_on_repo_history() {
    let root = scratch("section-diff-history");
    let mut seen = Seen::default();
    let patches = patchgen::repo_patches(25);
    for (sha, diff) in &patches {
        check_case(
            &root.join(sha),
            diff.as_bytes(),
            &["0".into(), "0".into()],
            "sections",
            &mut seen,
        );
    }
    assert!(seen.usable >= 1, "{seen:?}");
}

/// Recorded patches in the harness data, as `replay-verified-relay.mjs`
/// cuts them.
#[test]
#[ignore = "needs TRIBUNAL_HARNESS_DATA"]
fn section_diff_matches_js_on_recorded_patches() {
    let data = std::env::var("TRIBUNAL_HARNESS_DATA").expect("TRIBUNAL_HARNESS_DATA is not set");
    let dir = PathBuf::from(data).join("tribunal/fixtures/incremental-scope");
    let root = scratch("section-diff-recorded");
    let mut seen = Seen::default();
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "patch") {
            let name = p.file_name().unwrap().to_string_lossy().replace('.', "_");
            check_case(
                &root.join(name),
                &std::fs::read(&p).unwrap(),
                &["0".into(), "0".into()],
                "out",
                &mut seen,
            );
        }
    }
    assert!(seen.cases > 0, "no patches in {}", dir.display());
}

#[test]
fn section_diff_fails_where_the_script_does() {
    let dir = scratch("section-diff-failures");
    let rec = recordings(&dir, vec![]);
    let missing = dir.join("missing.patch");
    let missing = missing.to_str().unwrap();
    let out = format!("{}/out", dir.display());

    let js = run_script(SCRIPT, &[missing, &out, "0", "0"], &dir, &rec, None);
    assert_eq!(js.code, Some(1));
    let prefix = format!("FATAL: cannot read the diff at {missing}");
    assert!(
        js.stdout.starts_with(&format!("{prefix}: ")),
        "{}",
        js.stdout
    );
    assert_eq!(run(missing, &out, 0.0, 0.0), Err(prefix));

    let js = run_script(SCRIPT, &[missing], &dir, &rec, None);
    assert_eq!(js.code, Some(2));
    assert!(run(missing, "", 0.0, 0.0)
        .unwrap_err()
        .starts_with("usage: "));

    let patch = dir.join("p.patch");
    std::fs::write(&patch, "x").unwrap();
    let blocked = dir.join("a-file");
    std::fs::write(&blocked, "").unwrap();
    let blocked_out = format!("{}/sub", blocked.display());
    let js = run_script(
        SCRIPT,
        &[patch.to_str().unwrap(), &blocked_out],
        &dir,
        &rec,
        None,
    );
    assert_eq!(js.code, Some(1), "{}", js.stderr);
    assert!(run(patch.to_str().unwrap(), &blocked_out, 0.0, 0.0).is_err());
}

/// Declared difference: an unparsed header is cut at 120 UTF-16 units, and a
/// cut through a surrogate pair leaves the script a lone surrogate, which
/// `JSON.stringify` escapes. A Rust `String` cannot hold one; the port has
/// U+FFFD there. Nothing else in the index differs.
#[test]
fn a_header_cut_inside_a_pair_is_a_declared_difference() {
    let dir = scratch("section-diff-witness");
    let header = format!("diff --git \"a/{}😀\" \"b/x\"", "p".repeat(105));
    assert_eq!(js_len(&header[..header.find('😀').unwrap()]), 119);
    let diff = format!(
        "{header}\ndiff --git a/src/a.ts b/src/a.ts\n@@ -1 +1 @@\n{}",
        "+const total = computeTotal(items);\n".repeat(8)
    );
    let patch = dir.join("w.patch");
    std::fs::write(&patch, &diff).unwrap();
    let rec = recordings(&dir, vec![]);
    let js_out = format!("{}/js", dir.display());
    let rs_out = format!("{}/rs", dir.display());
    let js = run_script(
        SCRIPT,
        &[patch.to_str().unwrap(), &js_out],
        &dir,
        &rec,
        None,
    );
    let js_line = js.stdout.split('\n').nth(1).unwrap().replace("/js", "/rs");
    let rust = json_stringify(Some(
        &run(patch.to_str().unwrap(), &rs_out, 0.0, 0.0).unwrap(),
    ));
    assert!(js_line.contains("\\ud83d\""), "{js_line}");
    assert!(rust.contains("\u{FFFD}\""), "{rust}");
    assert_eq!(js_line.replace("\\ud83d", "\u{FFFD}"), rust);
}

/// A shallow checkout fails `repo_patches` with what it found, not with a
/// thin sample a later assertion reads as a wrong port.
#[test]
fn a_shallow_history_is_named_rather_than_sampled() {
    let root = scratch("section-diff-shallow");
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../..");
    let clone = root.join("clone");
    let url = format!("file://{}", repo.canonicalize().unwrap().display());
    let ok = std::process::Command::new("git")
        .args(["clone", "-q", "--depth", "2", "--no-checkout", &url])
        .arg(&clone)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .unwrap()
        .success();
    assert!(ok, "git clone --depth 2 failed");
    let err = std::panic::catch_unwind(|| patchgen::repo_patches_in(&clone, 25))
        .expect_err("a two-commit history passed");
    let message = err.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(message.contains("25 wanted; shallow: true"), "{message}");
}
