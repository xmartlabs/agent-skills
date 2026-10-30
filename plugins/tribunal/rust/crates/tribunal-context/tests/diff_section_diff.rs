//! `lib/section-diff.mjs` against `tribunal_context::section_diff`: the same
//! patch file and arguments on both sides, compared on the relayed JSON line
//! (as text) and on every file written to the output directory, byte for byte.
//!
//! The script's side is read from `tests/goldens/section-diff.json`, keyed by
//! test: each keeps the SHA-256 of every module the script reaches through a
//! relative import and of the two `workflows/tribunal.js` regions it slices
//! at run time, and, for each script run, the SHA-256 of what the run read
//! (arguments, patch) next to what it printed and the SHA-256 and size of
//! each file it wrote, with the scratch directory and the plugin root stored
//! as placeholders. The history test keeps the commits it was recorded on and
//! replays those, not the last 25 of HEAD. Editing the script, a region or a
//! generator without regenerating fails here; a source that is deleted is
//! retired and only its check is skipped. Regenerate after changing them:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_section_diff`.
//! The patches under `TRIBUNAL_HARNESS_DATA` come from reviewed repositories,
//! which no golden may hold, so that ignored test still runs the script.
mod common;
mod patchgen;
use common::*;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use proptest::prelude::*;
use proptest::sample::select;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use serde_json::{json, Value};
use tribunal_context::harness_version::sha256_hex;
use tribunal_context::section_diff::{count_arg, run};
use tribunal_core::profile::js_ext::json_stringify;

const SCRIPT: &str = "lib/section-diff.mjs";

// ---------------------------------------------------------------------------
// Goldens: the script's side recorded once, so a run starts only the port.

const REGENERATE: &str =
    "regenerate: TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_section_diff";
/// The workflow `loadSectioning` reads at run time, and the regions it slices.
const WORKFLOW: &str = "workflows/tribunal.js";
const REGIONS: [&str; 2] = ["DIFF-USABLE", "DIFF-SECTIONING"];

fn regenerating() -> bool {
    std::env::var("TRIBUNAL_REGEN_GOLDENS").is_ok_and(|v| v == "1")
}

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/section-diff.json")
}

/// `extractRegion` in `lib/check-report-fidelity.mjs`; `None` when the region
/// is not in `src`.
fn region<'a>(src: &'a str, name: &str) -> Option<&'a str> {
    let begin = format!("// ===== BEGIN {name} =====");
    let b = src.find(&begin)?;
    let e = b + src[b..].find(&format!("// ===== END {name} ====="))?;
    Some(&src[b + begin.len()..e])
}

/// The SHA-256 of what the script runs under `root`, by path: the script, every
/// module it reaches through a relative import, and each workflow region as
/// `<workflow>#<region>`. `None` when the script is not there, that is, it is
/// retired; a region of a workflow that is gone is left out, since it is
/// retired too.
fn sources(root: &Path) -> Option<BTreeMap<String, String>> {
    let import = regex::Regex::new(r#"(?:from|import)\s*\(?\s*"(\.{1,2}/[^"]+)""#).unwrap();
    let root = root.canonicalize().unwrap();
    let entry = root.join(SCRIPT);
    if !entry.exists() {
        return None;
    }
    let mut seen = BTreeMap::new();
    let mut todo = vec![entry.canonicalize().unwrap()];
    while let Some(file) = todo.pop() {
        let key = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if seen.contains_key(&key) {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for m in import.captures_iter(&text) {
            let dep = file.parent().unwrap().join(&m[1]);
            todo.push(
                dep.canonicalize()
                    .unwrap_or_else(|e| panic!("{}: {e}", dep.display())),
            );
        }
        seen.insert(key, sha256_hex(text.as_bytes()));
    }
    if let Ok(src) = std::fs::read_to_string(root.join(WORKFLOW)) {
        for r in REGIONS {
            let sha =
                region(&src, r).map_or_else(|| "missing".to_owned(), |t| sha256_hex(t.as_bytes()));
            seen.insert(format!("{WORKFLOW}#{r}"), sha);
        }
    }
    Some(seen)
}

/// Whether what the script runs under `root` is what `recorded` names. A
/// retired script passes: nothing can run it, so the golden is final; so does
/// a recorded source that is gone.
fn check_sources(root: &Path, recorded: &Value) -> Result<(), String> {
    let Some(now) = sources(root) else {
        return Ok(());
    };
    let was: BTreeMap<String, String> =
        serde_json::from_value(recorded.clone()).unwrap_or_default();
    let changed: Vec<&String> = now
        .iter()
        .filter(|(k, sha)| was.get(*k) != Some(sha))
        .map(|(k, _)| k)
        .collect();
    if changed.is_empty() {
        Ok(())
    } else {
        Err(format!("{SCRIPT}: changed since recorded: {changed:?}"))
    }
}

/// One test's recordings: what the script printed for each run, in order.
struct Oracle {
    key: String,
    subs: Vec<(String, &'static str)>,
    replay: Option<Vec<Value>>,
    kept: Vec<Value>,
}

static GOLDEN_WRITE: Mutex<()> = Mutex::new(());

impl Oracle {
    /// `dir` is the test's scratch directory, which the golden keeps as a
    /// placeholder, as it keeps the plugin root.
    fn new(key: &str, dir: &Path) -> Self {
        let mut subs: Vec<(String, &'static str)> = Vec::new();
        let mut add = |p: &Path, real: &'static str, given: &'static str| {
            // Both on every platform: a golden recorded where the two forms
            // differ (macOS's /var/folders) holds both placeholders.
            let canonical = p.canonicalize().unwrap().to_string_lossy().into_owned();
            subs.push((p.to_string_lossy().into_owned(), given));
            subs.push((canonical, real));
        };
        add(&plugin_root(), "<ROOT_REAL>", "<ROOT>");
        add(dir, "<DIR_REAL>", "<DIR>");
        // A path that contains another is replaced first.
        subs.sort_by_key(|(p, _)| std::cmp::Reverse(p.len()));
        let replay = (!regenerating()).then(|| {
            let path = golden_path();
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}; {REGENERATE}", path.display()));
            let golden: Value = serde_json::from_str(&text).unwrap();
            let entry = &golden["cases"][key];
            assert!(
                entry.is_object(),
                "{key}: not in {}; {REGENERATE}",
                path.display()
            );
            if let Err(e) = check_sources(&plugin_root(), &entry["sources"]) {
                panic!("{e}; {REGENERATE}");
            }
            entry["runs"].as_array().unwrap().clone()
        });
        Oracle {
            key: key.to_owned(),
            subs,
            replay,
            kept: Vec::new(),
        }
    }

    fn portable(&self, text: &str) -> String {
        self.subs
            .iter()
            .fold(text.to_owned(), |t, (from, to)| t.replace(from, to))
    }

    fn local(&self, v: &Value) -> Value {
        let text = self
            .subs
            .iter()
            .fold(v.to_string(), |t, (to, from)| t.replace(from, to));
        serde_json::from_str(&text).unwrap()
    }

    /// What the script printed for `input`, a description of everything the
    /// run reads; `spawn` runs it, and only when regenerating.
    fn answer(&mut self, input: &str, spawn: impl FnOnce() -> Value) -> Value {
        // One hash on every platform: on Linux the two forms of a path are one.
        let input = sha256_hex(self.portable(input).replace("_REAL>", ">").as_bytes());
        let i = self.kept.len();
        let out = match &self.replay {
            Some(runs) => {
                let run = runs.get(i).unwrap_or_else(|| {
                    panic!("{}: run {i} was not recorded; {REGENERATE}", self.key)
                });
                assert!(
                    run["input"] == json!(input),
                    "{}: run {i}'s input changed since it was recorded; {REGENERATE}",
                    self.key
                );
                self.kept.push(Value::Null);
                return self.local(&run["out"]);
            }
            None => spawn(),
        };
        let portable: Value = serde_json::from_str(&self.portable(&out.to_string())).unwrap();
        self.kept.push(json!({"input": input, "out": portable}));
        out
    }

    /// `run_script` through [`Oracle::answer`], with the SHA-256 and size of
    /// each file the run wrote under `wrote`; `read` holds the bytes of the
    /// files `args` name.
    fn script(&mut self, args: &[&str], read: &[&[u8]], cwd: &Path, wrote: &Path) -> Recorded {
        let read: Vec<String> = read.iter().map(|b| sha256_hex(b)).collect();
        let input = format!("{args:?}\n{read:?}");
        let out = self.answer(&input, || {
            let rec = recordings(cwd, vec![]);
            let run = run_script(SCRIPT, args, cwd, &rec, None);
            let files = hashed(tree(wrote));
            json!({"code": run.code, "stdout": run.stdout, "stderr": run.stderr, "files": files})
        });
        Recorded {
            run: ScriptRun {
                code: out["code"].as_i64().map(|c| i32::try_from(c).unwrap()),
                stdout: out["stdout"].as_str().unwrap().to_owned(),
                stderr: out["stderr"].as_str().unwrap().to_owned(),
            },
            files: serde_json::from_value(out["files"].clone()).unwrap(),
        }
    }

    /// Checks every recorded run was replayed, or writes this test's entry.
    fn finish(self) {
        if let Some(runs) = &self.replay {
            assert_eq!(
                runs.len(),
                self.kept.len(),
                "{}: recorded runs; {REGENERATE}",
                self.key
            );
            return;
        }
        let entry = json!({
            "sources": sources(&plugin_root()).expect("the script to record"),
            "runs": self.kept,
        });
        let tmp = std::env::temp_dir();
        for t in [tmp.clone(), tmp.canonicalize().unwrap()] {
            let t = t.to_string_lossy();
            assert!(
                !entry.to_string().contains(t.trim_end_matches('/')),
                "{}: a run printed a scratch path the placeholders missed ({t})",
                self.key
            );
        }
        let _lock = GOLDEN_WRITE.lock().unwrap_or_else(|e| e.into_inner());
        let path = golden_path();
        let golden: Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let mut cases: BTreeMap<String, Value> =
            serde_json::from_value(golden["cases"].clone()).unwrap_or_default();
        cases.insert(self.key.clone(), entry);
        let golden = json!({"script": SCRIPT, "cases": cases});
        std::fs::write(&path, serde_json::to_string_pretty(&golden).unwrap() + "\n").unwrap();
    }
}

/// A script run as recorded: what it printed, and each file it wrote by path
/// under the output root, as `{sha256, bytes}`.
struct Recorded {
    run: ScriptRun,
    files: BTreeMap<String, Value>,
}

/// `git show` of `sha` the way `patchgen::repo_patches` takes it, for the
/// commits a golden was recorded on.
fn show(sha: &str) -> String {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../..");
    let out = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args([
            "show",
            "-M",
            "--format=",
            "--no-color",
            "--no-ext-diff",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            sha,
        ])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git must be on PATH");
    assert!(
        out.status.success(),
        "commit {sha}, recorded in the golden, is not in this history (a shallow checkout?): {}; {REGENERATE}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The last 25 commits of HEAD when regenerating, else the ones recorded: the
/// history moves with every commit, the golden does not.
fn history(oracle: &mut Oracle) -> Vec<(String, String)> {
    let mut fresh = None;
    let shas = oracle.answer("the commits", || {
        let patches = patchgen::repo_patches(25);
        let shas = json!(patches.iter().map(|(s, _)| s).collect::<Vec<_>>());
        fresh = Some(patches);
        shas
    });
    fresh.unwrap_or_else(|| {
        shas.as_array()
            .unwrap()
            .iter()
            .map(|s| {
                let s = s.as_str().unwrap();
                (s.to_owned(), show(s))
            })
            .collect()
    })
}

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

/// Each file of a [`tree`] as `{sha256, bytes}`.
fn hashed(tree: BTreeMap<String, Vec<u8>>) -> BTreeMap<String, Value> {
    tree.into_iter()
        .map(|(name, b)| (name, json!({"sha256": sha256_hex(&b), "bytes": b.len()})))
        .collect()
}

/// Runs both sides on `diff` (raw bytes) and fails on any difference.
/// `out` is the output directory relative to the case directory, as typed.
/// The script's side comes from `oracle`, or from the script itself without
/// one.
fn check_case(
    oracle: Option<&mut Oracle>,
    dir: &Path,
    diff: &[u8],
    args: &[String],
    out: &str,
    seen: &mut Seen,
) {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    let diff_path = dir.join("diff.patch");
    std::fs::write(&diff_path, diff).unwrap();
    let diff_arg = diff_path.to_str().unwrap();
    let js_out = format!("{}/js/{out}", dir.display());
    let rs_out = format!("{}/rs/{out}", dir.display());

    let mut js_args = vec![diff_arg, js_out.as_str()];
    js_args.extend(args.iter().map(String::as_str));
    let (js, js_files) = match oracle {
        Some(o) => {
            let r = o.script(&js_args, &[diff], dir, &dir.join("js"));
            (r.run, r.files)
        }
        None => {
            let js = run_script(SCRIPT, &js_args, dir, &recordings(dir, vec![]), None);
            (js, hashed(tree(&dir.join("js"))))
        }
    };
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

    let rs_tree = tree(&dir.join("rs"));
    assert_eq!(
        js_files.keys().collect::<Vec<_>>(),
        rs_tree.keys().collect::<Vec<_>>(),
        "{}: written files",
        ctx()
    );
    for (name, js_file) in &js_files {
        let bytes = &rs_tree[name];
        assert!(
            js_file["sha256"] == json!(sha256_hex(bytes)),
            "{}: {name} differs ({} vs {} bytes)",
            ctx(),
            js_file["bytes"],
            bytes.len()
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
    let mut oracle = Oracle::new("generated", &root);
    for i in 0..300 {
        let ((diff, _), args, out) = strategy.new_tree(&mut runner).unwrap().current();
        check_case(
            Some(&mut oracle),
            &root.join(i.to_string()),
            diff.as_bytes(),
            &args,
            out,
            &mut seen,
        );
    }
    oracle.finish();
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
    let mut oracle = Oracle::new("edges", &root);
    for (i, (name, diff)) in cases.into_iter().enumerate() {
        for args in [
            vec![],
            vec!["0".to_owned(), "0".to_owned()],
            vec!["9".to_owned()],
        ] {
            check_case(
                Some(&mut oracle),
                &root.join(format!("{name}-{i}-{}", args.len())),
                &diff,
                &args,
                "out",
                &mut seen,
            );
        }
    }
    oracle.finish();
    assert!(seen.cut_in_pair >= 1, "{seen:?}");
}

fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// The last 25 commits of this repository when the golden was recorded.
#[test]
fn section_diff_matches_js_on_repo_history() {
    let root = scratch("section-diff-history");
    let mut seen = Seen::default();
    let mut oracle = Oracle::new("repo-history", &root);
    let patches = history(&mut oracle);
    for (sha, diff) in &patches {
        check_case(
            Some(&mut oracle),
            &root.join(sha),
            diff.as_bytes(),
            &["0".into(), "0".into()],
            "sections",
            &mut seen,
        );
    }
    oracle.finish();
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
                None,
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
    let mut oracle = Oracle::new("failures", &dir);
    let missing = dir.join("missing.patch");
    let missing = missing.to_str().unwrap();
    let out = format!("{}/out", dir.display());

    let js = oracle
        .script(&[missing, &out, "0", "0"], &[], &dir, Path::new(&out))
        .run;
    assert_eq!(js.code, Some(1));
    let prefix = format!("FATAL: cannot read the diff at {missing}");
    assert!(
        js.stdout.starts_with(&format!("{prefix}: ")),
        "{}",
        js.stdout
    );
    assert_eq!(run(missing, &out, 0.0, 0.0), Err(prefix));

    let js = oracle.script(&[missing], &[], &dir, Path::new(&out)).run;
    assert_eq!(js.code, Some(2));
    assert!(run(missing, "", 0.0, 0.0)
        .unwrap_err()
        .starts_with("usage: "));

    let patch = dir.join("p.patch");
    std::fs::write(&patch, "x").unwrap();
    let blocked = dir.join("a-file");
    std::fs::write(&blocked, "").unwrap();
    let blocked_out = format!("{}/sub", blocked.display());
    let js = oracle
        .script(
            &[patch.to_str().unwrap(), &blocked_out],
            &[b"x", b""],
            &dir,
            Path::new(&blocked_out),
        )
        .run;
    oracle.finish();
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
    let js_out = format!("{}/js", dir.display());
    let rs_out = format!("{}/rs", dir.display());
    let mut oracle = Oracle::new("witness", &dir);
    let js = oracle
        .script(
            &[patch.to_str().unwrap(), &js_out],
            &[diff.as_bytes()],
            &dir,
            Path::new(&js_out),
        )
        .run;
    oracle.finish();
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

// ---------------------------------------------------------------------------
// The golden's source check

#[test]
fn a_retired_source_skips_only_its_own_check() {
    let root = scratch("section-diff-retired");
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::create_dir_all(root.join("workflows")).unwrap();
    std::fs::write(
        lib.join("section-diff.mjs"),
        "import { relayLines } from \"./relay.mjs\";\n",
    )
    .unwrap();
    std::fs::write(lib.join("relay.mjs"), "export const relayLines = 1;\n").unwrap();
    let workflow = |usable: &str| {
        let text = format!(
            "x\n// ===== BEGIN DIFF-USABLE =====\n{usable}\n// ===== END DIFF-USABLE =====\n\
             // ===== BEGIN DIFF-SECTIONING =====\ns\n// ===== END DIFF-SECTIONING =====\n"
        );
        std::fs::write(root.join(WORKFLOW), text).unwrap();
    };
    workflow("u");
    let recorded = serde_json::to_value(sources(&root).unwrap()).unwrap();
    assert_eq!(recorded.as_object().unwrap().len(), 4);
    assert_eq!(check_sources(&root, &recorded), Ok(()));
    // Outside the regions the workflow can change freely.
    std::fs::write(
        root.join(WORKFLOW),
        std::fs::read_to_string(root.join(WORKFLOW)).unwrap() + "more\n",
    )
    .unwrap();
    assert_eq!(check_sources(&root, &recorded), Ok(()));
    workflow("u edited");
    let err = check_sources(&root, &recorded).unwrap_err();
    assert!(err.contains("workflows/tribunal.js#DIFF-USABLE"), "{err}");
    std::fs::remove_file(root.join(WORKFLOW)).unwrap();
    assert_eq!(check_sources(&root, &recorded), Ok(()));
    std::fs::write(lib.join("relay.mjs"), "export const relayLines = 2;\n").unwrap();
    let err = check_sources(&root, &recorded).unwrap_err();
    assert!(err.contains("lib/relay.mjs"), "{err}");
    std::fs::remove_file(lib.join("section-diff.mjs")).unwrap();
    assert_eq!(check_sources(&root, &recorded), Ok(()));
    // Every recorded test replays on a tree that does not hold the script.
    if regenerating() {
        return;
    }
    let golden: Value =
        serde_json::from_str(&std::fs::read_to_string(golden_path()).unwrap()).unwrap();
    for (key, case) in golden["cases"].as_object().unwrap() {
        assert_eq!(check_sources(&root, &case["sources"]), Ok(()), "{key}");
    }
    let _ = std::fs::remove_dir_all(&root);
}
