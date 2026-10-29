//! `skills/review-pr/scripts/harness-version.mjs` against
//! `tribunal_context::harness_version`: generated plugin trees named by
//! `CLAUDE_PLUGIN_ROOT` on the script's side and passed as the root to the
//! port, plus this checkout itself. Compared on stdout byte for byte, and on
//! `batteryVersion()` imported from the script as a module.
//!
//! The script's side of the generated trees is read from
//! `tests/goldens/harness-version.json`: running node twice for each of them
//! made this one of the slowest test binaries. The file keeps the script's
//! text and each case's input, so editing either without a regeneration
//! fails here. Regenerate after changing them:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_harness_version`.
mod common;
#[path = "../../tribunal-core/tests/common/mod.rs"]
mod core_common;

use common::{plugin_root, scratch};
use core_common::{assert_same, sample};

use std::collections::BTreeMap;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use proptest::prelude::*;
use serde_json::{json, Value};
use tribunal_context::harness_version::{battery_version, run, surface_files};

const SCRIPT: &str = "skills/review-pr/scripts/harness-version.mjs";

const DIRS: [&str; 7] = [
    "skills/review-pr/references",
    "skills/review-pr/scripts",
    "workflows",
    "lib",
    "agents",
    "methodology",
    "skills/review-pr/eval",
];

/// File names; a trailing `/` makes a subdirectory holding `inner.md`,
/// `inner.sh` and `inner.txt`.
const NAMES: [&str; 26] = [
    "a.md",
    "b.mjs",
    "c.sh",
    "d.js",
    "e.txt",
    "f.md.bak",
    ".hidden.md",
    "x.MD",
    "\u{ff5e}.md",
    "\u{ff5e}.sh",
    "\u{1f600}.md",
    "\u{1f600}.sh",
    "\u{e9}.mjs",
    "Z.js",
    "_.md",
    "10.md",
    "9.md",
    "a b.mjs",
    "sh",
    "dir.md/",
    "sub/",
    "deep/",
    "@link-file.md",
    "@link-dir.md",
    "@dead.sh",
    "@link-file.js",
];

const BODIES: [&[u8]; 6] = [
    b"",
    b"hello\n",
    b"hello\n",
    &[0xff, 0xfe, 0x00, 0x80, 0xc3],
    b"caf\xc3\xa9 \xe2\x80\x94 line\r\n",
    &[0x61; 200],
];

#[derive(Debug, Clone)]
struct Case {
    /// Per directory: 0 present, 1 missing, 2 a plain file.
    dirs: Vec<usize>,
    files: Vec<Vec<(usize, usize)>>,
    /// 0 present, 1 missing, 2 dangling symlink, 3 a directory.
    skill: usize,
    args: usize,
    root_form: usize,
}

const ARGS: [&[&str]; 5] = [
    &[],
    &["--short"],
    &["--manifest"],
    &["--manifest", "--short"],
    &["--x"],
];

fn case() -> impl Strategy<Value = Case> {
    (
        prop::collection::vec(
            prop_oneof![90 => Just(0usize), 1 => Just(1usize), 1 => Just(2usize)],
            7,
        ),
        prop::collection::vec(
            prop::collection::vec(
                (
                    prop_oneof![25 => 0..22usize, 1 => 22..NAMES.len()],
                    0..BODIES.len(),
                ),
                0..6,
            ),
            7,
        ),
        prop_oneof![30 => Just(0usize), 1 => 1..4usize],
        0..ARGS.len(),
        0..3usize,
    )
        .prop_map(|(dirs, files, skill, args, root_form)| Case {
            dirs,
            files,
            skill,
            args,
            root_form,
        })
}

fn build(root: &Path, case: &Case) {
    std::fs::create_dir_all(root.join("skills/review-pr")).unwrap();
    std::fs::create_dir_all(root.join("target-dir")).unwrap();
    std::fs::write(root.join("target-file"), b"through a link\n").unwrap();
    let skill = root.join("skills/review-pr/SKILL.md");
    match case.skill {
        0 => std::fs::write(&skill, b"# skill\n").unwrap(),
        1 => {}
        2 => symlink(root.join("nowhere"), &skill).unwrap(),
        _ => std::fs::create_dir_all(&skill).unwrap(),
    }
    for (i, dir) in DIRS.iter().enumerate() {
        let abs = root.join(dir);
        match case.dirs[i] {
            1 => continue,
            2 => {
                std::fs::write(&abs, b"not a directory\n").unwrap();
                continue;
            }
            _ => std::fs::create_dir_all(&abs).unwrap(),
        }
        for &(n, b) in &case.files[i] {
            let name = NAMES[n];
            if let Some(sub) = name.strip_suffix('/') {
                let d = abs.join(sub);
                std::fs::create_dir_all(&d).unwrap();
                for inner in ["inner.md", "inner.sh", "inner.txt", "inner.js"] {
                    std::fs::write(d.join(inner), BODIES[b]).unwrap();
                }
                if sub == "deep" {
                    std::fs::create_dir_all(d.join("more")).unwrap();
                    std::fs::write(d.join("more/leaf.mjs"), BODIES[b]).unwrap();
                }
            } else if let Some(link) = name.strip_prefix('@') {
                let target = match link {
                    "link-dir.md" => root.join("target-dir"),
                    "dead.sh" => root.join("nowhere"),
                    _ => root.join("target-file"),
                };
                let _ = symlink(target, abs.join(link));
            } else {
                std::fs::write(abs.join(name), BODIES[b]).unwrap();
            }
        }
    }
}

/// The root as the operator might spell it: plain, with a trailing slash, or
/// through a directory that does not exist and `..`.
fn root_arg(root: &Path, form: usize) -> String {
    let r = root.to_string_lossy();
    match form {
        0 => r.into_owned(),
        1 => format!("{r}/"),
        _ => format!("{r}/no-such-dir/.."),
    }
}

struct NodeRun {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn node(args: &[&str], root: Option<&str>, eval: Option<&str>) -> NodeRun {
    let mut cmd = Command::new("node");
    match eval {
        Some(code) => {
            cmd.args(["--input-type=module", "-e", code]);
        }
        None => {
            cmd.arg(plugin_root().join(SCRIPT)).args(args);
        }
    }
    match root {
        Some(r) => cmd.env("CLAUDE_PLUGIN_ROOT", r),
        None => cmd.env_remove("CLAUDE_PLUGIN_ROOT"),
    };
    let out = cmd.output().expect("node must be on PATH");
    NodeRun {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn battery_js() -> String {
    let url = format!(
        "file://{}",
        plugin_root().canonicalize().unwrap().join(SCRIPT).display()
    );
    format!(
        "import {{ batteryVersion }} from {url:?};\n\
         try {{ const v = batteryVersion(); console.log(JSON.stringify({{ id: v.id, manifest: v.manifest }})); }}\n\
         catch (e) {{ console.log(JSON.stringify({{ error: e.message }})); }}"
    )
}

fn battery_rust(root: &str) -> String {
    match battery_version(root) {
        Ok(v) => json!({"id": v.id, "manifest": v.manifest}).to_string(),
        Err(e) => json!({ "error": e }).to_string(),
    }
}

/// An error the script does not raise itself carries Node's or Rust's own
/// wording; only that it failed is compared.
fn io_masked(line: &str) -> String {
    let v: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}"));
    match v.get("error").and_then(Value::as_str) {
        Some(e) if !e.starts_with("harness surface") => json!({"error": "<io>"}).to_string(),
        _ => v.to_string(),
    }
}

/// Exit and stdout on both sides; a thrown error must carry the same message
/// when it is one the script raises itself.
fn compare_cli(args: &[&str], js: NodeRun, root_rust: &str) -> (Value, Value, Option<String>) {
    let rust = run(args, root_rust);
    let js_text = if js.code == Some(0) {
        js.stdout
    } else {
        format!("<exit {:?}>", js.code)
    };
    let (rust_text, err) = match rust {
        Ok(s) => (s, None),
        Err(e) => {
            if e.starts_with("harness surface") {
                assert!(
                    js.stderr.contains(&format!("Error: {e}")),
                    "{e} vs {}",
                    js.stderr
                );
            }
            ("<exit Some(1)>".to_owned(), Some(e))
        }
    };
    (Value::String(rust_text), Value::String(js_text), err)
}

struct Outcome {
    input: Value,
    rust: Value,
    js: Value,
    battery_rust: Value,
    battery_js: Value,
    error: Option<String>,
    case: Case,
    order_differs: bool,
}

/// The script's side of one case: the CLI run and what `batteryVersion()`
/// printed.
struct JsSide {
    cli: NodeRun,
    battery: String,
}

impl JsSide {
    fn run(args: &[&str], root: &str, battery: &str) -> Self {
        Self {
            cli: node(args, Some(root), None),
            battery: node(&[], Some(root), Some(battery)).stdout,
        }
    }

    /// As the golden keeps it, with the scratch directory as `<SCRATCH>` and
    /// the plugin, which node's stack traces name, as `<PLUGIN>`.
    fn portable(&self, scratch: &str) -> Value {
        let text = json!({
            "code": self.cli.code,
            "stdout": self.cli.stdout,
            "stderr": self.cli.stderr,
            "battery": self.battery,
        })
        .to_string()
        .replace(scratch, "<SCRATCH>")
        .replace(&plugin(), "<PLUGIN>");
        serde_json::from_str(&text).unwrap()
    }

    fn local(v: &Value, scratch: &str) -> Self {
        let text = v
            .to_string()
            .replace("<SCRATCH>", scratch)
            .replace("<PLUGIN>", &plugin());
        let v: Value = serde_json::from_str(&text).unwrap();
        let s = |k: &str| v[k].as_str().unwrap().to_owned();
        Self {
            cli: NodeRun {
                code: v["code"].as_i64().map(|c| i32::try_from(c).unwrap()),
                stdout: s("stdout"),
                stderr: s("stderr"),
            },
            battery: s("battery"),
        }
    }
}

fn plugin() -> String {
    plugin_root()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn check(root: &Path, case: &Case, js: JsSide) -> Outcome {
    let r = root_arg(root, case.root_form);
    let args = ARGS[case.args];
    let b_js = js.battery;
    let (rust, js, error) = compare_cli(args, js.cli, &r);
    let order_differs = surface_files(&r).is_ok_and(|f| {
        let mut bytes = f.clone();
        bytes.sort();
        let mut units = f;
        units.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
        bytes != units
    });
    let out = Outcome {
        input: json!({ "case": format!("{case:?}"), "args": args }),
        rust,
        js,
        battery_rust: Value::String(io_masked(&battery_rust(&r))),
        battery_js: Value::String(io_masked(b_js.trim_end())),
        error,
        case: case.clone(),
        order_differs,
    };
    let _ = std::fs::remove_dir_all(root);
    out
}

const REGENERATE: &str =
    "regenerate: TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_harness_version";

fn golden_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/harness-version.json")
}

fn script_text() -> String {
    std::fs::read_to_string(plugin_root().join(SCRIPT)).unwrap()
}

fn regenerating() -> bool {
    std::env::var("TRIBUNAL_REGEN_GOLDENS").is_ok_and(|v| v == "1")
}

/// The golden, checked against the script on disk; `None` when regenerating.
fn recorded() -> Option<Value> {
    if regenerating() {
        return None;
    }
    let path = golden_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; {REGENERATE}", path.display()));
    let golden: Value = serde_json::from_str(&text).unwrap();
    assert!(
        golden["script"] == script_text(),
        "the script changed since {} was recorded; {REGENERATE}",
        path.display()
    );
    Some(golden)
}

fn outcomes() -> &'static [Outcome] {
    static CELL: OnceLock<Vec<Outcome>> = OnceLock::new();
    CELL.get_or_init(|| {
        let cases = sample(case());
        let root = scratch("harness-version");
        let scratch_text = root.to_string_lossy().into_owned();
        let battery = battery_js();
        let golden = recorded();
        let replay: Option<&Vec<Value>> = golden.as_ref().map(|g| {
            let recorded = g["cases"].as_array().unwrap();
            assert_eq!(
                recorded.len(),
                cases.len(),
                "the generator changed; {REGENERATE}"
            );
            recorded
        });
        let next = AtomicUsize::new(0);
        let slots: Vec<Mutex<Option<(Outcome, Value)>>> =
            cases.iter().map(|_| Mutex::new(None)).collect();
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
        std::thread::scope(|s| {
            for _ in 0..workers {
                s.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(case) = cases.get(i) else { break };
                    let dir = root.join(i.to_string());
                    build(&dir, case);
                    let js = match replay {
                        Some(r) => {
                            assert_eq!(
                                r[i]["input"],
                                json!(format!("{case:?}")),
                                "the generator changed; {REGENERATE}"
                            );
                            JsSide::local(&r[i]["js"], &scratch_text)
                        }
                        None => {
                            JsSide::run(ARGS[case.args], &root_arg(&dir, case.root_form), &battery)
                        }
                    };
                    let kept = json!({
                        "input": format!("{case:?}"),
                        "js": js.portable(&scratch_text),
                    });
                    let o = check(&dir, case, js);
                    *slots[i].lock().unwrap() = Some((o, kept));
                });
            }
        });
        let _ = std::fs::remove_dir_all(&root);
        let (out, kept): (Vec<Outcome>, Vec<Value>) = slots
            .into_iter()
            .map(|m| m.into_inner().unwrap().unwrap())
            .unzip();
        if replay.is_none() {
            let text =
                serde_json::to_string_pretty(&json!({"script": script_text(), "cases": kept}))
                    .unwrap();
            std::fs::write(golden_path(), text + "\n").unwrap();
        }
        out
    })
}

#[test]
fn harness_version_matches_js_on_generated_trees() {
    let out = outcomes();
    let inputs: Vec<Value> = out.iter().map(|o| o.input.clone()).collect();
    let rust: Vec<Value> = out.iter().map(|o| o.rust.clone()).collect();
    let js: Vec<Value> = out.iter().map(|o| o.js.clone()).collect();
    assert_same("harness-version", &inputs, &rust, &js);
    let rust: Vec<Value> = out.iter().map(|o| o.battery_rust.clone()).collect();
    let js: Vec<Value> = out.iter().map(|o| o.battery_js.clone()).collect();
    assert_same("batteryVersion", &inputs, &rust, &js);
}

#[test]
fn harness_version_matches_js_on_this_checkout() {
    let root = plugin_root().canonicalize().unwrap();
    let root = root.to_string_lossy();
    for args in ARGS {
        let (rust, js, err) = compare_cli(args, node(args, None, None), &root);
        assert_eq!(err, None);
        assert_eq!(rust, js, "{args:?}");
    }
    let js = node(&[], None, Some(&battery_js()));
    assert_eq!(battery_rust(&root), js.stdout.trim_end());
}

#[test]
fn the_generated_trees_reach_every_branch() {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    let mut hit = |k: &'static str, cond: bool| {
        *seen.entry(k).or_default() += usize::from(cond);
    };
    for o in outcomes() {
        let e = o.error.as_deref().unwrap_or("");
        let js = o.js.as_str().unwrap();
        hit("ok", o.error.is_none());
        hit(
            "dir-missing",
            e.starts_with("harness surface directory missing"),
        );
        hit(
            "file-missing",
            e.starts_with("harness surface file missing"),
        );
        hit(
            "io-error",
            !e.is_empty() && !e.starts_with("harness surface"),
        );
        hit(
            "short",
            js.starts_with("pr-harness-") && js.lines().count() == 1 && !js.contains("files)"),
        );
        hit("manifest", js.lines().count() > 1);
        hit("summary", js.contains(" files)"));
        hit("utf16-order-differs", o.order_differs && o.error.is_none());
        hit(
            "battery-ok",
            o.battery_js.as_str().unwrap().contains("pr-battery-"),
        );
        hit(
            "battery-error",
            o.battery_js.as_str().unwrap().contains("\"error\""),
        );
        hit("dotted-root", o.case.root_form == 2 && o.error.is_none());
        hit(
            "nested-dir",
            js.contains("/more/leaf.mjs") || js.contains("dir.md/inner.md"),
        );
        hit(
            "symlinked-file",
            js.contains("@link-file") || js.contains("link-file.md"),
        );
        hit("skill-is-dir", o.case.skill == 3);
    }
    eprintln!("{seen:#?}");
    for (k, n) in &seen {
        assert!(*n >= 3, "{k} reached only {n} times: {seen:#?}");
    }
}
