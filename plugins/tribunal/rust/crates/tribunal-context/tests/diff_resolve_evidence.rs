//! `skills/review-pr/scripts/resolve-evidence-line.mjs` against
//! `tribunal_context::resolve_evidence_line`: the same diff file and
//! candidates text on both sides, compared on the printed JSON as text.
//!
//! The script's side is read from `tests/goldens/resolve-evidence-line.json`,
//! keyed by test: each keeps the SHA-256 of every module the script reaches
//! through a relative import and, for each script run, the SHA-256 of what the
//! run read (arguments, candidates, diff) next to what it printed, with the
//! scratch directory and the plugin root stored as placeholders. The history
//! test keeps the commits it was recorded on and replays those, not the last
//! 25 of HEAD. Editing the script or a generator without regenerating fails
//! here; once the script is deleted its golden is final and only the module
//! check is skipped. Regenerate after changing them:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_resolve_evidence`.
//! The fixtures under `TRIBUNAL_HARNESS_DATA` come from reviewed repositories,
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
use tribunal_context::resolve_evidence_line::run;
use tribunal_core::profile::js_ext::json_stringify;

const SCRIPT: &str = "skills/review-pr/scripts/resolve-evidence-line.mjs";

// ---------------------------------------------------------------------------
// Goldens: the script's side recorded once, so a run starts only the port.

const REGENERATE: &str =
    "regenerate: TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_resolve_evidence";

fn regenerating() -> bool {
    std::env::var("TRIBUNAL_REGEN_GOLDENS").is_ok_and(|v| v == "1")
}

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/resolve-evidence-line.json")
}

/// The SHA-256 of `entry` (a path under `root`) and of every module it reaches
/// through a relative import, by path under `root`; `None` when `entry` is not
/// there, that is, the script is retired.
fn module_hashes(root: &Path, entry: &str) -> Option<BTreeMap<String, String>> {
    let import = regex::Regex::new(r#"(?:from|import)\s*\(?\s*"(\.{1,2}/[^"]+)""#).unwrap();
    let root = root.canonicalize().unwrap();
    let entry = root.join(entry);
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
    Some(seen)
}

/// Whether the modules `entry` reaches under `root` are the ones `recorded`
/// names. A retired `entry` passes: nothing can run it, so the golden is final.
fn check_modules(root: &Path, entry: &str, recorded: &Value) -> Result<(), String> {
    let Some(now) = module_hashes(root, entry) else {
        return Ok(());
    };
    if serde_json::to_value(&now).unwrap() == *recorded {
        return Ok(());
    }
    let was: BTreeMap<String, String> =
        serde_json::from_value(recorded.clone()).unwrap_or_default();
    let changed: std::collections::BTreeSet<&String> = now
        .keys()
        .chain(was.keys())
        .filter(|k| now.get(*k) != was.get(*k))
        .collect();
    Err(format!("{entry}: changed since recorded: {changed:?}"))
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
            let canonical = p.canonicalize().unwrap().to_string_lossy().into_owned();
            let p = p.to_string_lossy().into_owned();
            if canonical != p {
                subs.push((p, given));
            }
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
            if let Err(e) = check_modules(&plugin_root(), SCRIPT, &entry["modules"]) {
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
        let input = sha256_hex(self.portable(input).as_bytes());
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

    /// `run_script` through [`Oracle::answer`]; `read` holds the bytes of the
    /// files `args` name.
    fn script(
        &mut self,
        args: &[&str],
        stdin: Option<&str>,
        read: &[&[u8]],
        cwd: &Path,
    ) -> ScriptRun {
        let read: Vec<String> = read.iter().map(|b| sha256_hex(b)).collect();
        let input = format!("{args:?}\n{stdin:?}\n{read:?}");
        let out = self.answer(&input, || {
            let rec = recordings(cwd, vec![]);
            let run = run_script(SCRIPT, args, cwd, &rec, stdin);
            json!({"code": run.code, "stdout": run.stdout, "stderr": run.stderr})
        });
        ScriptRun {
            code: out["code"].as_i64().map(|c| i32::try_from(c).unwrap()),
            stdout: out["stdout"].as_str().unwrap().to_owned(),
            stderr: out["stderr"].as_str().unwrap().to_owned(),
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
            "modules": module_hashes(&plugin_root(), SCRIPT).expect("the script to record"),
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

/// How a candidate's field is built: a fixed value (`None` leaves the key
/// out), or taken from the diff around an anchor line.
#[derive(Debug, Clone)]
enum Field {
    Fixed(Option<Value>),
    /// The anchor's 1-based patch line plus an offset.
    PatchLine(i64),
    /// A guess at a file line.
    FileLine(i64),
    /// The anchor line's text (offset by `delta` lines), dressed by `how`.
    Quote(usize, usize),
    /// One of the paths the generator wrote into a header.
    DiffPath(usize),
    /// The b-side of the last header above the anchor line.
    AnchorPath,
}

#[derive(Debug, Clone)]
struct Spec {
    id: Option<Value>,
    file: Field,
    line: Field,
    evidence: Field,
    anchor: usize,
}

fn ids() -> Vec<Option<Value>> {
    vec![
        Some(json!("c1")),
        Some(json!(1)),
        Some(json!(0)),
        Some(Value::Null),
        None,
        Some(json!({"2": 1, "1": 0, "b": {"10": "x", "a": 1}})),
        Some(json!("é😀")),
        Some(json!(1.5)),
        Some(serde_json::from_str("1e21").unwrap()),
        Some(serde_json::from_str("-0.0").unwrap()),
        Some(json!([1, "two", null])),
    ]
}

fn fixed_files() -> Vec<Option<Value>> {
    vec![
        None,
        Some(Value::Null),
        Some(json!(5)),
        Some(json!(true)),
        Some(json!(["src/a.ts"])),
        Some(json!({"toString": 1})),
        Some(json!({})),
        Some(json!("")),
        Some(json!("nomatch.ts")),
        Some(json!("a.ts")),
        Some(json!("acme/widgets-api/src/a.ts")),
        Some(json!("./src/b.ts")),
        Some(json!("lib/x y.ts")),
    ]
}

fn fixed_lines() -> Vec<Option<Value>> {
    vec![
        None,
        Some(Value::Null),
        Some(json!(0)),
        Some(json!(-1)),
        Some(json!("12")),
        Some(json!(" 7 ")),
        Some(json!(2.5)),
        Some(json!(true)),
        Some(json!([5])),
        Some(json!("abc")),
        Some(serde_json::from_str("1e21").unwrap()),
        Some(json!("0x10")),
    ]
}

fn fixed_evidence() -> Vec<Option<Value>> {
    vec![
        None,
        Some(Value::Null),
        Some(json!(5)),
        Some(json!("")),
        Some(json!("   \n\t")),
        Some(json!("No relevant quotes found")),
        Some(json!("  No relevant quotes found \n")),
        Some(json!("}")),
        Some(json!("return;")),
        Some(json!("short")),
        Some(json!(
            "\n\n  const total = computeTotal(items);\n  return total;"
        )),
        Some(json!("\u{FEFF}const total = computeTotal(items);\u{3000}")),
        Some(json!("const   total =\tcomputeTotal(items);")),
        Some(json!("émoji 😀 line with astral 𝒳 chars")),
        Some(json!("not in any diff at all, anywhere")),
        Some(json!("}\nreturn null;\n{")),
    ]
}

fn field_strategy(
    fixed: Vec<Option<Value>>,
    kinds: &'static [&'static str],
) -> BoxedStrategy<Field> {
    let mut options: Vec<BoxedStrategy<Field>> = Vec::new();
    if !fixed.is_empty() {
        options.push(select(fixed).prop_map(Field::Fixed).boxed());
    }
    for k in kinds {
        options.push(match *k {
            "patch" => prop_oneof![3 => Just(0i64), 1 => -3i64..4]
                .prop_map(Field::PatchLine)
                .boxed(),
            "file" => (1i64..40).prop_map(Field::FileLine).boxed(),
            "quote" => (0usize..3, 0usize..7)
                .prop_map(|(d, h)| Field::Quote(d, h))
                .boxed(),
            "path" => (0usize..8).prop_map(Field::DiffPath).boxed(),
            "anchor-path" => Just(Field::AnchorPath).boxed(),
            _ => unreachable!(),
        });
    }
    proptest::strategy::Union::new(options).boxed()
}

fn spec() -> BoxedStrategy<Spec> {
    (
        select(ids()),
        prop_oneof![
            1 => field_strategy(fixed_files(), &[]),
            2 => field_strategy(vec![], &["path"]),
            3 => field_strategy(vec![], &["anchor-path"]),
        ],
        prop_oneof![
            1 => field_strategy(fixed_lines(), &[]),
            4 => field_strategy(vec![], &["patch"]),
            3 => field_strategy(vec![], &["file"]),
        ],
        prop_oneof![
            1 => field_strategy(fixed_evidence(), &[]),
            4 => field_strategy(vec![], &["quote"]),
        ],
        0usize..10_000,
    )
        .prop_map(|(id, file, line, evidence, anchor)| Spec {
            id,
            file,
            line,
            evidence,
            anchor,
        })
        .boxed()
}

/// The candidate a spec stands for over `lines` (the diff split on `\n`).
fn build(spec: &Spec, lines: &[&str], paths: &[String]) -> Value {
    let n = lines.len().max(1);
    let k = spec.anchor % n;
    let resolve = |f: &Field| -> Option<Value> {
        match f {
            Field::Fixed(v) => v.clone(),
            Field::PatchLine(off) => Some(json!(k as i64 + 1 + off)),
            Field::FileLine(l) => Some(json!(l)),
            Field::AnchorPath => {
                let header = lines[..=k.min(lines.len().saturating_sub(1))]
                    .iter()
                    .rev()
                    .find(|l| l.starts_with("diff --git a/"))
                    .copied()
                    .unwrap_or("");
                Some(json!(header.rsplit(" b/").next().unwrap_or("")))
            }
            Field::DiffPath(i) => Some(match paths.get(i % paths.len().max(1)) {
                Some(p) => json!(p),
                None => json!("src/a.ts"),
            }),
            Field::Quote(delta, how) => {
                let at = |j: usize| lines.get((k + j) % n).copied().unwrap_or("");
                let line = at(*delta);
                let bare = line.strip_prefix(['+', '-', ' ']).unwrap_or(line);
                Some(json!(match how {
                    0 => line.to_owned(),
                    1 => bare.to_owned(),
                    2 => format!("  {bare}\t "),
                    3 => format!("\n  \n{bare}"),
                    4 => format!("{}\n{}", at(*delta), at(delta + 1)),
                    5 => format!("{bare}\r"),
                    _ => format!("{}\n{line}", at(delta + 1)),
                }))
            }
        }
    };
    let mut m = serde_json::Map::new();
    for (key, v) in [
        ("id", spec.id.clone()),
        ("file", resolve(&spec.file)),
        ("line", resolve(&spec.line)),
        ("evidence", resolve(&spec.evidence)),
    ] {
        if let Some(v) = v {
            m.insert(key.into(), v);
        }
    }
    Value::Object(m)
}

/// What the outputs held, so the generator is shown to reach every branch.
#[derive(Default, Debug)]
struct Seen {
    kinds: BTreeMap<String, usize>,
    corrected: usize,
    no_resolved_file: usize,
    removed_match: usize,
    not_ran: usize,
}

impl Seen {
    fn add(&mut self, out: &Value) {
        let Some(results) = out["results"].as_array() else {
            self.not_ran += 1;
            return;
        };
        for r in results {
            *self
                .kinds
                .entry(r["matchKind"].as_str().unwrap_or("?").to_owned())
                .or_default() += 1;
            self.corrected += usize::from(r.get("correctedLine").is_some());
            self.no_resolved_file +=
                usize::from(r.get("resolvedFile").is_none() && r["matchKind"] != "error");
            self.removed_match += usize::from(r["matchedLineKind"] == "removed");
        }
    }

    fn kind(&self, k: &str) -> usize {
        self.kinds.get(k).copied().unwrap_or(0)
    }
}

enum Candidates<'a> {
    File(&'a str),
    Stdin(&'a str),
}

/// Runs both sides and fails on any difference. A script that throws out of
/// the batch exits non-zero; the port must return an error there. The
/// script's side comes from `oracle`, or from the script itself without one.
fn check_case(
    oracle: Option<&mut Oracle>,
    dir: &Path,
    diff: &[u8],
    candidates: Candidates,
    seen: &mut Seen,
) {
    std::fs::create_dir_all(dir).unwrap();
    let diff_path = dir.join("diff.patch");
    std::fs::write(&diff_path, diff).unwrap();
    let cands_path = dir.join("candidates.json");
    let (args, stdin, text) = match candidates {
        Candidates::File(text) => {
            std::fs::write(&cands_path, text).unwrap();
            let args = [diff_path.to_str().unwrap(), cands_path.to_str().unwrap()];
            (args, None, text)
        }
        Candidates::Stdin(text) => ([diff_path.to_str().unwrap(), "-"], Some(text), text),
    };
    let js = match oracle {
        Some(o) => o.script(&args, stdin, &[diff, text.as_bytes()], dir),
        None => run_script(SCRIPT, &args, dir, &recordings(dir, vec![]), stdin),
    };
    let rust = run(&diff_path, text);
    let ctx = || format!("case {}: {text}", dir.display());
    match rust {
        Ok(v) => {
            assert_eq!(js.code, Some(0), "{}: script failed: {}", ctx(), js.stderr);
            assert_eq!(json_stringify(Some(&v)), js.stdout, "{}", ctx());
            seen.add(&v);
        }
        Err(e) => {
            assert_eq!(
                js.code,
                Some(1),
                "{}: port failed ({e}), script printed {}",
                ctx(),
                js.stdout
            );
            assert!(js.stderr.contains(&e), "{}: {e} vs {}", ctx(), js.stderr);
            *seen.kinds.entry("crash".into()).or_default() += 1;
        }
    }
}

#[test]
fn resolve_evidence_matches_js_on_generated_candidates() {
    let root = scratch("resolve-evidence-gen");
    let strategy = (
        patchgen::diff(),
        proptest::collection::vec(spec(), 0..7),
        0usize..20,
    );
    let mut runner = TestRunner::deterministic();
    let mut seen = Seen::default();
    let mut oracle = Oracle::new("generated", &root);
    for i in 0..300 {
        let ((diff, paths), specs, mode) = strategy.new_tree(&mut runner).unwrap().current();
        let lines: Vec<&str> = diff.split('\n').collect();
        let mut cands: Vec<Value> = specs.iter().map(|s| build(s, &lines, &paths)).collect();
        if mode == 0 {
            cands.push(Value::Null);
        }
        let text = Value::Array(cands).to_string();
        let input = if mode == 1 {
            Candidates::Stdin(&text)
        } else {
            Candidates::File(&text)
        };
        check_case(
            Some(&mut oracle),
            &root.join(i.to_string()),
            diff.as_bytes(),
            input,
            &mut seen,
        );
    }
    oracle.finish();
    eprintln!("{seen:?}");
    for k in [
        "exact",
        "shifted",
        "elsewhere",
        "ambiguous",
        "not-found",
        "low-specificity",
        "skipped",
        "error",
        "crash",
    ] {
        assert!(seen.kind(k) >= 5, "{k} reached too rarely: {seen:?}");
    }
    assert!(seen.corrected >= 20 && seen.removed_match >= 5, "{seen:?}");
    assert!(seen.not_ran >= 10, "{seen:?}");
}

/// A candidate with no `file` resolves only when the diff's first path ends
/// in "undefined" (`key.endsWith(undefined)`); otherwise `file.endsWith`
/// throws inside the per-candidate `try`. A resolved one carries no
/// `resolvedFile` key at all.
#[test]
fn resolve_evidence_matches_js_when_the_file_is_missing() {
    let quote = "const total = computeTotal(items);";
    let body = format!("@@ -1 +1,9 @@\n{}", format!("+{quote}\n").repeat(1));
    let first_undefined = format!(
        "diff --git a/src/undefined b/src/undefined\n{body}diff --git a/src/a.ts b/src/a.ts\n{body}{}",
        " filler line that pads the diff past two hundred characters\n".repeat(4)
    );
    let first_other = format!(
        "diff --git a/src/a.ts b/src/a.ts\n{body}diff --git a/src/undefined b/src/undefined\n{body}{}",
        " filler line that pads the diff past two hundred characters\n".repeat(4)
    );
    let cands = json!([
        {"id": "no-file", "line": 1, "evidence": quote},
        {"id": "no-file-far", "line": 30, "evidence": quote},
        {"id": "null-file", "file": null, "line": 1, "evidence": quote},
    ])
    .to_string();
    let root = scratch("resolve-evidence-missing-file");
    let mut seen = Seen::default();
    let mut oracle = Oracle::new("missing-file", &root);
    check_case(
        Some(&mut oracle),
        &root.join("first-undefined"),
        first_undefined.as_bytes(),
        Candidates::File(&cands),
        &mut seen,
    );
    check_case(
        Some(&mut oracle),
        &root.join("first-other"),
        first_other.as_bytes(),
        Candidates::File(&cands),
        &mut seen,
    );
    oracle.finish();
    assert!(
        seen.no_resolved_file >= 2 && seen.kind("error") >= 3,
        "{seen:?}"
    );
}

/// Declared difference: a number literal beyond the double range reads as
/// Infinity in `JSON.parse` and as null here (a `Value` cannot hold it). In a
/// candidate that only changes `file`, where `String(Infinity)` is
/// "Infinity": the script reports `file.endsWith is not a function`, the port
/// the null message. `line`, `id` and `evidence` read the same either way.
#[test]
fn an_infinite_file_is_a_declared_difference() {
    let dir = scratch("resolve-evidence-infinity");
    let diff = format!(
        "diff --git a/src/a.ts b/src/a.ts\n@@ -1 +1,9 @@\n{}",
        "+const total = computeTotal(items);\n".repeat(8)
    );
    let patch = dir.join("d.patch");
    std::fs::write(&patch, &diff).unwrap();
    let cands = r#"[{"id":1e400,"file":1e400,"line":1e400,"evidence":"const total = computeTotal(items);"}]"#;
    let mut oracle = Oracle::new("infinity", &dir);
    let js = oracle.script(
        &[patch.to_str().unwrap(), "-"],
        Some(cands),
        &[diff.as_bytes()],
        &dir,
    );
    oracle.finish();
    let rust = json_stringify(Some(&run(&patch, cands).unwrap()));
    let js_says = r#"{"ran":true,"results":[{"id":null,"matchKind":"error","error":"file.endsWith is not a function"}]}"#;
    assert_eq!(js.stdout, js_says);
    assert_eq!(
        rust,
        js_says.replace(
            "file.endsWith is not a function",
            "Cannot read properties of null (reading 'endsWith')"
        )
    );
}

/// Patches from this repository's history, with candidates quoting their
/// real lines: the last 25 commits when the golden was recorded.
#[test]
fn resolve_evidence_matches_js_on_repo_history() {
    let root = scratch("resolve-evidence-history");
    let mut oracle = Oracle::new("repo-history", &root);
    let patches = history(&mut oracle);
    let strategy = proptest::collection::vec(spec(), 1..12);
    let mut runner = TestRunner::deterministic();
    let mut seen = Seen::default();
    for (sha, diff) in &patches {
        let lines: Vec<&str> = diff.split('\n').collect();
        let paths: Vec<String> = lines
            .iter()
            .filter_map(|l| l.strip_prefix("+++ b/"))
            .map(str::to_owned)
            .collect();
        let specs = strategy.new_tree(&mut runner).unwrap().current();
        let cands: Vec<Value> = specs.iter().map(|s| build(s, &lines, &paths)).collect();
        let text = Value::Array(cands).to_string();
        check_case(
            Some(&mut oracle),
            &root.join(sha),
            diff.as_bytes(),
            Candidates::File(&text),
            &mut seen,
        );
    }
    oracle.finish();
    assert!(
        seen.kind("exact") + seen.kind("shifted") + seen.kind("elsewhere") >= 5,
        "{seen:?}"
    );
}

/// The fixtures of `replay-line-correction.mjs`, every case through the batch.
#[test]
fn resolve_evidence_matches_js_on_the_line_correction_replay() {
    let diff = [
        "diff --git a/src/a.ts b/src/a.ts",
        "index 1111111..2222222 100644",
        "--- a/src/a.ts",
        "+++ b/src/a.ts",
        "@@ -1,3 +1,4 @@",
        " export function first() {",
        "+  const guard = requireAccount(accountId);",
        "   return loadEverything();",
        " }",
        "diff --git a/src/b.ts b/src/b.ts",
        "index 3333333..4444444 100644",
        "--- a/src/b.ts",
        "+++ b/src/b.ts",
        "@@ -40,4 +40,5 @@ export async function second() {",
        "   const rows = await prisma.invoice.findMany({",
        "-    where: { accountId },",
        "+    where: { accountId, deletedAt: null },",
        "+    orderBy: { createdAt: 'desc' },",
        "   });",
        "   return rows;",
        "diff --git a/src/c.ts b/src/c.ts",
        "--- a/src/c.ts",
        "+++ b/src/c.ts",
        "@@ -20,2 +22,3 @@",
        "   const total = sumInvoices(rows);",
        "+  const total2 = sumInvoices(rows);",
        "   return total;",
        "@@ -24,1 +26,1 @@",
        "-const legacyTotal = 0;",
        "+const total2 = sumInvoices(rows);",
    ]
    .join("\n");
    let mut long = vec![
        "diff --git a/src/d.ts b/src/d.ts".to_owned(),
        "--- a/src/d.ts".to_owned(),
        "+++ b/src/d.ts".to_owned(),
        "@@ -100,30 +200,31 @@".to_owned(),
        "+const marker = computeSettlementTotals(invoices);".to_owned(),
    ];
    long.extend((0..30).map(|k| format!(" const filler{k} = {k};")));
    let long = long.join("\n");
    let w = "where: { accountId, deletedAt: null },";
    let cands = json!([
        {"id": 0, "file": "src/b.ts", "line": 17, "evidence": w},
        {"id": 1, "file": "src/b.ts", "line": 18, "evidence": format!("{w}\n    orderBy: {{ createdAt: 'desc' }},")},
        {"id": 2, "file": "src/b.ts", "line": 17, "evidence": format!("+    {w}")},
        {"id": 3, "file": "src/b.ts", "line": 15, "evidence": "const rows = await prisma.invoice.findMany({"},
        {"id": 4, "file": "src/b.ts", "line": 41, "evidence": w},
        {"id": 5, "file": "src/a.ts", "line": 17, "evidence": w},
        {"id": 6, "file": "src/b.ts", "line": 16, "evidence": "where: { accountId },"},
        {"id": 7, "file": "src/a.ts", "line": 9, "evidence": w},
        {"id": 8, "file": "src/b.ts", "line": 19, "evidence": w},
        {"id": 9, "file": "src/a.ts", "line": 9, "evidence": "const guard = requireAccount(accountId);"},
        {"id": 10, "file": "src/a.ts", "line": 9, "evidence": "}"},
        {"id": 11, "file": "src/b.ts", "line": 14, "evidence": "@@ -40,4 +40,5 @@ export async function second() {"},
        {"id": 12, "file": "src/b.ts", "evidence": w},
        {"id": 13, "file": "src/b.ts", "line": 17},
        {"id": 14, "file": "src/c.ts", "line": 26, "evidence": "const total2 = sumInvoices(rows);"},
        {"id": 15, "file": "src/c.ts", "line": 30, "evidence": "const total2 = sumInvoices(rows);"},
        {"id": 16, "file": "src/d.ts", "line": 18 + 31, "evidence": "const marker = computeSettlementTotals(invoices);"},
        {"id": 17, "file": "src/d.ts", "line": 17 + 31, "evidence": "const marker = computeSettlementTotals(invoices);"},
    ]);
    let root = scratch("resolve-evidence-replay");
    let mut seen = Seen::default();
    let both = format!("{diff}\n{long}");
    let mut oracle = Oracle::new("line-correction", &root);
    check_case(
        Some(&mut oracle),
        &root.join("both"),
        both.as_bytes(),
        Candidates::File(&cands.to_string()),
        &mut seen,
    );
    oracle.finish();
    assert!(seen.corrected >= 5, "{seen:?}");
}

#[test]
fn resolve_evidence_matches_js_on_unusable_inputs() {
    let root = scratch("resolve-evidence-edges");
    let mut seen = Seen::default();
    let usable = format!(
        "diff --git a/src/a.ts b/src/a.ts\n@@ -1 +1,9 @@\n{}",
        "+const total = computeTotal(items);\n".repeat(8)
    );
    let cases: Vec<(&str, String, &str)> = vec![
        ("short", "diff --git a/x b/x\n+1".into(), "[]"),
        ("short-any", "x".into(), "{\"not\": \"an array\"}"),
        ("banner", format!("Output too large\n{usable}"), "[]"),
        ("not-json", usable.clone(), "not json"),
        ("bom-json", usable.clone(), "\u{FEFF}[]"),
        ("object", usable.clone(), "{\"a\": 1}"),
        ("string", usable.clone(), "\"abc\""),
        ("null-item", usable.clone(), "[{\"id\": 1}, null]"),
        ("empty", usable.clone(), "[]"),
        ("primitives", usable.clone(), "[0, \"\", false, true, 5, \"abc\", [], {}]"),
        ("dup-keys", usable.clone(), "[{\"id\": 1, \"id\": 2, \"evidence\": \"const total = computeTotal(items);\", \"file\": \"src/a.ts\"}]"),
        ("deep-id", usable.clone(), "[{\"id\": {\"1\": [{\"0\": 1, \"z\": 2, \"3\": 3}]}, \"file\": \"a.ts\"}]"),
        ("big-line", usable.clone(), "[{\"file\": \"src/a.ts\", \"line\": 1e400, \"evidence\": \"const total = computeTotal(items);\"}]"),
    ];
    let mut oracle = Oracle::new("unusable", &root);
    for (name, diff, cands) in cases {
        check_case(
            Some(&mut oracle),
            &root.join(name),
            diff.as_bytes(),
            Candidates::File(cands),
            &mut seen,
        );
    }

    let dir = root.join("unreadable");
    std::fs::create_dir_all(&dir).unwrap();
    let missing = dir.join("missing.patch");
    let js = oracle.script(&[missing.to_str().unwrap(), "-"], Some("[]"), &[], &dir);
    oracle.finish();
    let rust = run(&missing, "[]").unwrap();
    assert_eq!(json_stringify(Some(&rust)), js.stdout);
    assert_eq!(rust, json!({"ran": false, "reason": "diff-unreadable"}));
}

/// The fixtures `replay-evidence-resolver.mjs` replays, one candidate each.
#[test]
#[ignore = "needs TRIBUNAL_HARNESS_DATA"]
fn resolve_evidence_matches_js_on_recorded_fixtures() {
    let data = std::env::var("TRIBUNAL_HARNESS_DATA").expect("TRIBUNAL_HARNESS_DATA is not set");
    let dir = PathBuf::from(data).join("tribunal/fixtures/evidence-resolver");
    let root = scratch("resolve-evidence-recorded");
    let mut seen = Seen::default();
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let fixture: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        let cands = match (&fixture["batchFindings"], &fixture["finding"]) {
            (Value::Array(b), _) => Value::Array(b.clone()),
            (_, f) => json!([f]),
        };
        let name = p.file_stem().unwrap().to_string_lossy().into_owned();
        let diff = fixture["diff"].as_str().unwrap_or("");
        check_case(
            None,
            &root.join(name),
            diff.as_bytes(),
            Candidates::File(&cands.to_string()),
            &mut seen,
        );
    }
    assert!(
        !seen.kinds.is_empty() || seen.not_ran > 0,
        "no fixtures in {}",
        dir.display()
    );
}

// ---------------------------------------------------------------------------
// The golden's module check

#[test]
fn a_retired_script_skips_only_the_module_check() {
    let root = scratch("resolve-evidence-retired");
    let scripts = root.join("skills/review-pr/scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::create_dir_all(root.join("lib")).unwrap();
    std::fs::write(
        scripts.join("resolve-evidence-line.mjs"),
        "import { relayLines } from \"../../../lib/relay.mjs\";\n",
    )
    .unwrap();
    std::fs::write(root.join("lib/relay.mjs"), "export const relayLines = 1;\n").unwrap();
    let recorded = serde_json::to_value(module_hashes(&root, SCRIPT).unwrap()).unwrap();
    assert_eq!(recorded.as_object().unwrap().len(), 2);
    assert_eq!(check_modules(&root, SCRIPT, &recorded), Ok(()));
    std::fs::write(root.join("lib/relay.mjs"), "export const relayLines = 2;\n").unwrap();
    let err = check_modules(&root, SCRIPT, &recorded).unwrap_err();
    assert!(err.contains("lib/relay.mjs"), "{err}");
    std::fs::remove_file(scripts.join("resolve-evidence-line.mjs")).unwrap();
    assert_eq!(check_modules(&root, SCRIPT, &recorded), Ok(()));
    // Every recorded test replays on a tree that does not hold the script.
    if regenerating() {
        return;
    }
    let golden: Value =
        serde_json::from_str(&std::fs::read_to_string(golden_path()).unwrap()).unwrap();
    for (key, case) in golden["cases"].as_object().unwrap() {
        assert_eq!(
            check_modules(&root, SCRIPT, &case["modules"]),
            Ok(()),
            "{key}"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}
