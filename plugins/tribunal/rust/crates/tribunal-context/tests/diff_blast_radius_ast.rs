//! The parser half of `rust/crates/tribunal-context/src/js/blast-radius.mjs` and `lib/attention-facts.mjs`
//! against the scripts WITH `typescript` loaded, which is how production runs
//! them. The port parses with oxc; these tests hold its output to the
//! script's, whole, on git fixture repos and on generated sources.
//!
//! What the scripts printed is read from `tests/goldens/blast-radius-ast/`,
//! one file per test. Each keeps a SHA-256 of every module the scripts reach
//! through a relative import, the `typescript` version they ran with, and for
//! each time a test asks them, a SHA-256 of the input (the cases, or the
//! fixture repo's files, HEAD and diff) with what they printed, the scratch
//! directory and the plugin root stored as placeholders. A script or generator
//! edited without a regeneration fails; a script that is gone is retired and
//! its golden final, so only its staleness check is skipped. Regenerate on a
//! Mac, with `TRIBUNAL_TS_FROM` naming a directory whose `node_modules` holds
//! `typescript`, as for `eval/replay-attention-facts.mjs`:
//! `TRIBUNAL_REGEN_GOLDENS=1 TRIBUNAL_TS_FROM=<repo> cargo test -p tribunal-context --test diff_blast_radius_ast -- --include-ignored`.
//!
//! The tests that compare oxc's own answers need nothing else. The ones where
//! the port hands code oxc cannot parse cleanly to `typescript` need the
//! package for the port, so they are `#[ignore]`; run with `--include-ignored`
//! and the variable unset, they fail and say why. The two corpus tests read a
//! repository `TRIBUNAL_AST_CORPUS` or `TRIBUNAL_TS_FROM` names, which no
//! golden can hold, so they still ask the scripts. `TRIBUNAL_AST_SEED` asks
//! them too: its inputs are not in any golden.

mod common;
use common::*;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use tribunal_context::ast_bindings::{self, PARSER};
use tribunal_context::attention_facts::{self, Added};
use tribunal_context::blast_radius;
use tribunal_context::exec::Tools;
use tribunal_context::harness_version::sha256_hex;
use tribunal_context::typescript_fallback::{self, TypeScriptFallback};

const BLAST: &str = "rust/crates/tribunal-context/src/js/blast-radius.mjs";
const ATTENTION: &str = "rust/crates/tribunal-context/src/js/attention-facts.mjs";

// ---------------------------------------------------------------------------
// Goldens

const REGENERATE: &str = "regenerate on a Mac: TRIBUNAL_REGEN_GOLDENS=1 TRIBUNAL_TS_FROM=<repo> cargo test -p tribunal-context --test diff_blast_radius_ast -- --include-ignored";

fn regenerating() -> bool {
    std::env::var("TRIBUNAL_REGEN_GOLDENS").is_ok_and(|v| v == "1")
}

fn exploring() -> bool {
    std::env::var_os("TRIBUNAL_AST_SEED").is_some()
}

/// A SHA-256 of each module the oracle scripts reach through a relative
/// import, by path in the plugin.
fn module_hashes() -> BTreeMap<String, String> {
    let import = regex::Regex::new(r#"(?:from|import)\s*\(?\s*"(\.{1,2}/[^"]+)""#).unwrap();
    let root = plugin_root().canonicalize().unwrap();
    let mut seen = BTreeMap::new();
    let mut todo: Vec<PathBuf> = [BLAST, ATTENTION]
        .iter()
        .map(|s| {
            root.join(s).canonicalize().unwrap_or_else(|e| {
                panic!("{s}: {e}; the oracle is retired, its goldens are final")
            })
        })
        .collect();
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
    seen
}

/// The scripts in `recorded` (path in `root` to SHA-256) whose text changed.
/// A script no longer there is retired: the golden is its final record, so
/// it is not checked.
fn stale_in(root: &Path, recorded: &Value) -> Vec<String> {
    recorded
        .as_object()
        .expect("recordedWith is an object")
        .iter()
        .filter_map(|(path, sha)| match std::fs::read(root.join(path)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => panic!("{path}: {e}"),
            Ok(text) => (json!(sha256_hex(&text)) != *sha).then(|| path.clone()),
        })
        .collect()
}

/// This run's machine-specific paths and the placeholders a golden keeps
/// them as, longest first so a path inside another is replaced after it.
fn path_pairs(root: Option<&Path>, ts: Option<&Path>) -> Vec<(String, &'static str)> {
    let both = |p: &Path, raw: &'static str, real: &'static str| {
        let real_path = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        [
            (p.to_string_lossy().into_owned(), raw),
            (real_path.to_string_lossy().into_owned(), real),
        ]
    };
    let mut v: Vec<(String, &'static str)> =
        both(&plugin_root(), "<PLUGIN>", "<PLUGIN_REAL>").into();
    if let Some(r) = root {
        v.extend(both(r, "<ROOT>", "<ROOT_REAL>"));
    }
    if let Some(t) = ts {
        v.extend(both(t, "<TS>", "<TS_REAL>"));
    }
    v.sort_by_key(|(p, _)| std::cmp::Reverse(p.len()));
    v
}

fn to_golden(v: &Value, paths: &[(String, &str)]) -> Value {
    let text = paths
        .iter()
        .fold(v.to_string(), |t, (p, ph)| t.replace(p.as_str(), ph));
    serde_json::from_str(&text).unwrap()
}

fn from_golden(v: &Value, paths: &[(String, &str)]) -> Value {
    let text = paths
        .iter()
        .fold(v.to_string(), |t, (p, ph)| t.replace(ph, p.as_str()));
    serde_json::from_str(&text).unwrap()
}

/// One test's golden: what the scripts printed, in the order the test asks.
/// Regenerating, each answer is the scripts' and is written as it comes;
/// exploring, it is the scripts' and nothing is written; otherwise it is the
/// recorded one, checked against the input the test asks it for.
struct Golden {
    path: PathBuf,
    mode: Mode,
    with: Value,
    typescript: String,
    records: Vec<Value>,
    next: usize,
}

#[derive(PartialEq)]
enum Mode {
    Record,
    Live,
    Replay,
}

impl Golden {
    fn open(name: &str) -> Golden {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/goldens/blast-radius-ast")
            .join(format!("{name}.json"));
        if regenerating() {
            assert!(
                !exploring(),
                "unset TRIBUNAL_AST_SEED to regenerate: the goldens hold the default seeds"
            );
            return Golden {
                path,
                mode: Mode::Record,
                with: json!(module_hashes()),
                typescript: ts_parser_name(&ts_from()),
                records: Vec::new(),
                next: 0,
            };
        }
        if exploring() {
            return Golden::live();
        }
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e}; {REGENERATE}", path.display()));
        let golden: Value = serde_json::from_str(&text).unwrap();
        let stale = stale_in(&plugin_root(), &golden["recordedWith"]);
        assert!(
            stale.is_empty(),
            "{stale:?} changed since {} was recorded; {REGENERATE}",
            path.display()
        );
        Golden {
            path,
            mode: Mode::Replay,
            with: golden["recordedWith"].clone(),
            typescript: golden["typescript"].as_str().unwrap().to_owned(),
            records: golden["records"].as_array().unwrap().clone(),
            next: 0,
        }
    }

    /// Always the scripts' answers, never recorded: for inputs no golden can
    /// hold.
    fn live() -> Golden {
        Golden {
            path: PathBuf::new(),
            mode: Mode::Live,
            with: Value::Null,
            typescript: ts_parser_name(&ts_from()),
            records: Vec::new(),
            next: 0,
        }
    }

    /// The scripts' answer to `input`, which `run` asks them for.
    fn answer(
        &mut self,
        input: &Value,
        paths: &[(String, &str)],
        run: impl FnOnce() -> Value,
    ) -> Value {
        let fingerprint = json!(sha256_hex(to_golden(input, paths).to_string().as_bytes()));
        match self.mode {
            Mode::Live => {
                assert_not_retired();
                run()
            }
            Mode::Record => {
                assert_not_retired();
                let v = run();
                self.records
                    .push(json!({"input": fingerprint, "js": to_golden(&v, paths)}));
                std::fs::create_dir_all(self.path.parent().unwrap()).unwrap();
                std::fs::write(&self.path, self.text()).unwrap();
                v
            }
            Mode::Replay => {
                let r = self.records.get(self.next).unwrap_or_else(|| {
                    panic!(
                        "{} holds {} answers and the test asks for more; {REGENERATE}",
                        self.path.display(),
                        self.records.len()
                    )
                });
                assert_eq!(
                    r["input"],
                    fingerprint,
                    "answer {} of {}: the generator changed; {REGENERATE}",
                    self.next,
                    self.path.display()
                );
                self.next += 1;
                from_golden(&r["js"], paths)
            }
        }
    }

    /// The golden as written: one line per case of a batch, compact, so the
    /// biggest stay small and a regeneration's diff names the cases it moved.
    fn text(&self) -> String {
        let line = |v: &Value| serde_json::to_string(v).unwrap();
        let records: Vec<String> = self
            .records
            .iter()
            .map(|r| {
                let js = match r["js"].as_array() {
                    Some(cases) => format!(
                        "[\n{}\n    ]",
                        cases
                            .iter()
                            .map(|c| format!("      {}", line(c)))
                            .collect::<Vec<_>>()
                            .join(",\n")
                    ),
                    None => line(&r["js"]),
                };
                format!("    {{\"input\": {}, \"js\": {js}}}", line(&r["input"]))
            })
            .collect();
        let text = format!(
            "{{\n  \"recordedWith\": {},\n  \"typescript\": {},\n  \"records\": [\n{}\n  ]\n}}\n",
            line(&self.with),
            line(&json!(self.typescript)),
            records.join(",\n")
        );
        let back: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(back["records"], json!(self.records));
        text
    }

    /// Every recorded answer was asked for.
    fn done(&self) {
        if self.mode == Mode::Replay {
            assert_eq!(
                self.next,
                self.records.len(),
                "{} holds answers the test no longer asks for; {REGENERATE}",
                self.path.display()
            );
        }
    }
}

fn assert_not_retired() {
    for s in [BLAST, ATTENTION] {
        assert!(
            plugin_root().join(s).exists(),
            "{s} is retired: this run needs the scripts' answers, and only the goldens hold them"
        );
    }
}

/// A file's text as a golden's input sees it: a hash, or that it is missing.
fn file_fingerprint(p: &Path) -> Value {
    std::fs::read(p).map_or(json!(null), |b| json!(sha256_hex(&b)))
}

/// A fixture repo as the script sees it: each file under it but `.git`, and
/// the commit HEAD names, which fixes every commit before it.
fn repo_fingerprint(repo: &Path) -> Value {
    let mut out = BTreeMap::new();
    let mut stack = vec![repo.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n != ".git") {
                    stack.push(p);
                }
            } else {
                let rel = p.strip_prefix(repo).unwrap().to_string_lossy().into_owned();
                out.insert(rel, file_fingerprint(&p));
            }
        }
    }
    let head = repo
        .join(".git")
        .exists()
        .then(|| git(repo, &["rev-parse", "HEAD"]).trim().to_owned());
    json!({"files": out, "head": head})
}

/// A recorded script that is gone is retired and skipped; one still there is
/// checked, whatever else is gone.
#[test]
fn a_retired_script_is_not_checked_for_staleness() {
    let root = scratch("ast-retired");
    std::fs::create_dir_all(root.join("lib")).unwrap();
    std::fs::write(root.join("lib/kept.mjs"), "export const a = 1;\n").unwrap();
    let recorded = json!({
        "lib/kept.mjs": sha256_hex(b"export const a = 1;\n"),
        "lib/retired.mjs": sha256_hex(b"export const b = 1;\n"),
    });
    assert_eq!(stale_in(&root, &recorded), Vec::<String>::new());
    std::fs::write(root.join("lib/kept.mjs"), "export const a = 2;\n").unwrap();
    assert_eq!(stale_in(&root, &recorded), vec!["lib/kept.mjs".to_owned()]);
    std::fs::remove_file(root.join("lib/kept.mjs")).unwrap();
    assert_eq!(stale_in(&root, &recorded), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// The typescript package

/// The directory `TRIBUNAL_TS_FROM` names, checked to hold `typescript`.
/// Panics, failing the test, when it is unset or holds none.
fn ts_from() -> PathBuf {
    let Some(dir) = std::env::var_os("TRIBUNAL_TS_FROM").filter(|d| !d.is_empty()) else {
        panic!(
            "TRIBUNAL_TS_FROM is not set: these differential tests compare against the script \
             WITH typescript loaded. Point it at a directory whose node_modules holds typescript \
             (any TypeScript repo you review), e.g. TRIBUNAL_TS_FROM=<repo> cargo test -- --include-ignored"
        );
    };
    let dir = PathBuf::from(dir);
    let probe = Command::new("node")
        .arg("-e")
        .arg("require(require('node:path').join(process.argv[1], 'node_modules', 'typescript'))")
        .arg(&dir)
        .output()
        .expect("node must be on PATH");
    assert!(
        probe.status.success(),
        "TRIBUNAL_TS_FROM={} has no loadable node_modules/typescript: {}",
        dir.display(),
        String::from_utf8_lossy(&probe.stderr)
    );
    dir
}

/// `typescript <version>`, as the script names its parser.
fn ts_parser_name(dir: &Path) -> String {
    let out = Command::new("node")
        .arg("-e")
        .arg("process.stdout.write('typescript ' + require(require('node:path').join(process.argv[1], 'node_modules', 'typescript')).version)")
        .arg(dir)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

// ---------------------------------------------------------------------------
// Function-level batches: the script's exported functions on many inputs

const HARNESS: &str = r#"
import path from "node:path";
import { pathToFileURL } from "node:url";
const root = process.argv[2];
const br = await import(pathToFileURL(path.join(root, "rust/crates/tribunal-context/src/js/blast-radius.mjs")).href);
const af = await import(pathToFileURL(path.join(root, "rust/crates/tribunal-context/src/js/attention-facts.mjs")).href);
const ts = af.loadTypescript([process.env.TRIBUNAL_TS_FROM]);
if (!ts) {
  process.stderr.write("no typescript under TRIBUNAL_TS_FROM\n");
  process.exit(3);
}
let input = "";
for await (const c of process.stdin) input += c;
const run = (c) => {
  switch (c.fn) {
    case "bindingRefs":
      return br.bindingRefs(ts, c.file, c.content, c.target, c.name, c.aliases);
    case "returnsPromise":
      return br.returnsPromise(ts, c.file, c.content, c.name);
    case "factsForFile":
      return af.factsForFile(ts, c.file, c.content, new Set(c.added));
    case "statusOrderFacts":
      return af.statusOrderFacts(ts, c.file, c.content, new Set(c.added));
    case "testFacts":
      return af.testFacts(ts, c.file, c.content, new Set(c.added));
    case "ownFacts":
      return [...af.factsForFile(ts, c.file, c.content, new Set(c.added)), ...af.statusOrderFacts(ts, c.file, c.content, new Set(c.added))];
    case "contractFacts":
      return af.contractFacts(ts, c.file, c.base, c.content);
    case "exportedShapes":
      return [...af.exportedShapes(ts, c.file, c.content)];
    case "addedLines":
      return [...af.addedLines(c.diff)].map(([f, s]) => [f, [...s]]);
    case "baseBlobs":
      return [...af.baseBlobs(c.diff)];
  }
  throw new Error("unknown fn " + c.fn);
};
const kindOf = (f) => (/\.tsx$/.test(f) ? ts.ScriptKind.TSX : /\.jsx$/.test(f) ? ts.ScriptKind.JSX : /\.(js|mjs|cjs)$/.test(f) ? ts.ScriptKind.JS : ts.ScriptKind.TS);
const diag = (c) =>
  typeof c.content === "string"
    ? ts.createSourceFile(c.file, c.content, ts.ScriptTarget.Latest, true, kindOf(c.file)).parseDiagnostics.map((d) => ts.flattenDiagnosticMessageText(d.messageText, " "))
    : [];
const out = JSON.parse(input).map((c) => {
  try {
    return { ok: run(c) ?? null, diag: diag(c) };
  } catch (e) {
    return { err: String(e && e.message) };
  }
});
process.stdout.write(JSON.stringify(out));
"#;

/// The script's answers to every case, from `g`.
fn js_batch(g: &mut Golden, cases: &[Value]) -> Vec<Value> {
    let input = json!({"harness": HARNESS, "cases": cases});
    let v = g.answer(&input, &path_pairs(None, None), || {
        Value::Array(run_harness(cases))
    });
    v.as_array().unwrap().clone()
}

/// Runs every case through the script's functions in one node process.
fn run_harness(cases: &[Value]) -> Vec<Value> {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let ts = ts_from();
    let dir = scratch(&format!(
        "ast-harness-{}",
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let script = dir.join("harness.mjs");
    std::fs::write(&script, HARNESS).unwrap();
    let input = dir.join("cases.json");
    std::fs::write(&input, serde_json::to_string(cases).unwrap()).unwrap();
    let out = Command::new("node")
        .arg(&script)
        .arg(plugin_root())
        .env("TRIBUNAL_TS_FROM", &ts)
        .stdin(std::fs::File::open(&input).unwrap())
        .output()
        .expect("node must be on PATH");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        out.status.success(),
        "harness failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    v.as_array().unwrap().clone()
}

fn fact_list(v: Vec<attention_facts::Fact>) -> Value {
    Value::Array(v.iter().map(attention_facts::Fact::to_json).collect())
}

fn added_of(c: &Value) -> Added {
    let lines: Vec<usize> = c["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| usize::try_from(x.as_u64().unwrap()).unwrap())
        .collect();
    Added::of(&lines)
}

/// The port's answer to one case, shaped as the harness prints the script's.
fn rust_case(c: &Value) -> Value {
    let s = |k: &str| c[k].as_str().unwrap_or("");
    let ok = match s("fn") {
        "bindingRefs" => {
            let aliases: Vec<(String, String)> = c["aliases"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
                        .collect()
                })
                .unwrap_or_default();
            match ast_bindings::binding_refs(
                s("file"),
                s("content"),
                s("target"),
                s("name"),
                &aliases,
            ) {
                None => Value::Null,
                Some(b) => json!({
                    "refs": b.refs.iter().map(|r| json!({
                        "line": r.line,
                        "call": r.call,
                        "awaited": r.awaited,
                        "inTry": r.in_try,
                        "text": r.text,
                    })).collect::<Vec<_>>(),
                    "reexports": b.reexports,
                }),
            }
        }
        "returnsPromise" => json!(ast_bindings::returns_promise(
            s("file"),
            s("content"),
            s("name")
        )),
        "factsForFile" => fact_list(attention_facts::facts_for_file(
            s("file"),
            s("content"),
            &added_of(c),
        )),
        "statusOrderFacts" => fact_list(attention_facts::status_order_facts(
            s("file"),
            s("content"),
            &added_of(c),
        )),
        "testFacts" => fact_list(attention_facts::test_facts(
            s("file"),
            s("content"),
            &added_of(c),
        )),
        "contractFacts" => fact_list(attention_facts::contract_facts(
            s("file"),
            s("base"),
            s("content"),
        )),
        "exportedShapes" => Value::Array(
            attention_facts::exported_shapes(s("file"), s("content"))
                .into_iter()
                .map(|(k, sh)| {
                    json!([k, {
                        "params": sh.params,
                        "required": sh.required,
                        "sentinels": sh.sentinels,
                        "throws": sh.throws,
                        "line": sh.line,
                    }])
                })
                .collect(),
        ),
        "addedLines" => Value::Array(
            attention_facts::added_lines(s("diff"))
                .into_iter()
                .map(|(f, a)| json!([f, a.lines().into_iter().map(js_number).collect::<Vec<_>>()]))
                .collect(),
        ),
        "baseBlobs" => Value::Array(
            attention_facts::base_blobs_ordered(s("diff"))
                .into_iter()
                .map(|(f, b)| json!([f, b]))
                .collect(),
        ),
        other => panic!("unknown fn {other}"),
    };
    json!({ "ok": ok })
}

/// Every case gives the same answer on both sides; a mismatch names the case.
/// Returns the script's answers.
fn assert_batch(g: &mut Golden, label: &str, cases: &[Value]) -> Vec<Value> {
    let js = js_batch(g, cases);
    assert_eq!(js.len(), cases.len());
    let mut bad = Vec::new();
    for (i, (c, j)) in cases.iter().zip(&js).enumerate() {
        let r = rust_case(c);
        if r["ok"] != j["ok"] {
            let file = c["file"].as_str().unwrap_or("x.ts");
            let content = c["content"].as_str().unwrap_or("");
            let (oxc, fatal) =
                ast_bindings::parse_errors(file, content, ast_bindings::ScriptKind::of(file));
            bad.push(format!(
                "case {i} ({}, {}):\n--- source ---\n{}\n--- script ---\n{}\n--- port ---\n{}\n--- typescript parse errors: {}\n--- oxc parse errors (fatal {fatal}): {oxc:?}",
                c["fn"],
                c["file"],
                c["content"].as_str().unwrap_or(c["diff"].as_str().unwrap_or("")),
                j["ok"],
                r["ok"],
                j["diag"],
            ));
        }
    }
    assert!(
        bad.is_empty(),
        "{label}: {} of {} cases differ\n{}",
        bad.len(),
        cases.len(),
        bad.iter().take(4).cloned().collect::<Vec<_>>().join("\n\n")
    );
    js
}

/// Generated code is valid code: TypeScript's parser reports nothing on it,
/// so a disagreement there is a port defect, not error recovery.
fn assert_valid(label: &str, cases: &[Value], js: &[Value]) {
    for (c, j) in cases.iter().zip(js) {
        assert!(
            j["diag"].as_array().is_none_or(Vec::is_empty),
            "{label}: the generator wrote code typescript does not parse: {}\n{}",
            j["diag"],
            c["content"]
        );
    }
}

/// The references of a batch of `bindingRefs` answers.
fn all_refs(js: &[Value]) -> Vec<Value> {
    js.iter()
        .filter_map(|r| r["ok"]["refs"].as_array())
        .flatten()
        .cloned()
        .collect()
}

/// The facts of a batch, by kind.
fn fact_counts(js: &[Value]) -> std::collections::BTreeMap<String, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for f in js.iter().filter_map(|r| r["ok"].as_array()).flatten() {
        if let Some(k) = f["kind"].as_str() {
            *counts.entry(k.to_owned()).or_default() += 1;
        }
    }
    counts
}

fn all_lines(src: &str) -> Vec<usize> {
    (1..=src.split('\n').count()).collect()
}

fn bind_case(file: &str, content: &str, target: &str, name: &str) -> Value {
    json!({
        "fn": "bindingRefs",
        "file": file,
        "content": content,
        "target": target,
        "name": name,
        "aliases": {"@services/*": "src/services/*", "@lib": "src/lib/index.ts"},
    })
}

fn facts_case(f: &str, file: &str, content: &str, added: &[usize]) -> Value {
    json!({"fn": f, "file": file, "content": content, "added": added})
}

/// A JavaScript number as `JSON.stringify` writes it: integral values
/// without a fraction.
fn js_number(x: f64) -> Value {
    #[allow(clippy::cast_possible_truncation)]
    if x.fract() == 0.0 && x.abs() < 9.0e15 {
        json!(x as i64)
    } else {
        json!(x)
    }
}

// ---------------------------------------------------------------------------
// Hand-written cases: the replays' seeds, then each syntax form on its own

/// The importers `eval/replay-binding-graph.mjs` builds, each a form of
/// import, plus forms it does not cover.
const IMPORTERS: &[(&str, &str)] = &[
    (
        "src/controllers/aliased.ts",
        "import { getAccount as fetchAccount } from '@services/auth';\n\nexport async function handle(req) {\n  const a = await fetchAccount(req.id);\n  try {\n    fetchAccount(req.other);\n  } catch (e) {}\n  return a;\n}\n",
    ),
    (
        "src/controllers/namespace.ts",
        "import * as authSvc from '../services/auth';\n\nexport function poke(id) {\n  authSvc.getAccount(id);\n}\n",
    ),
    (
        "src/legacy/destructured.js",
        "const {\n  getAccount\n} = require('../services/auth');\n\nexports.run = (id) => getAccount(id);\n",
    ),
    (
        "src/legacy/module.js",
        "const svc = require('../services/auth');\n\nexports.go = (id) => {\n  svc.getAccount(id);\n};\n",
    ),
    (
        "src/controllers/barrel.ts",
        "import { getAccount } from '../services';\n\nexport const viaBarrel = (id) => getAccount(id);\n",
    ),
    (
        "src/other/collision.ts",
        "import { getAccount } from './elsewhere';\n\nexport const unrelated = () => getAccount(9);\n",
    ),
    (
        "src/routes/r.ts",
        "import { makeMw } from '../services/auth';\n\nrouter.get('/a', makeMw(['admin']), handler);\nrouter.get('/b', makeMw(['user']), handler);\n",
    ),
    (
        "src/services/index.ts",
        "export { getAccount } from './auth';\nexport * from './auth';\nexport * as everything from './auth';\nexport { getAccount as other } from './auth';\n",
    ),
    (
        "src/widgets/contexts.ts",
        r"import { getAccount, type Account } from '../services/auth';
import type { getAccount as T } from '../services/auth';
import def, { getAccount as g2 } from '../services/auth';

export async function contexts(id: string): Promise<Account> {
  getAccount(id);
  await getAccount(id);
  const a = getAccount(id);
  getAccount(id).then(() => 1);
  getAccount(id).catch(() => 1);
  getAccount(id).finally(() => 1);
  wrap(getAccount(id));
  (getAccount(id));
  void getAccount(id);
  let b; b = getAccount(id);
  getAccount?.(id);
  getAccount(id)?.then(x);
  const c = () => getAccount(id);
  const d = async () => { getAccount(id); };
  const e = () => (getAccount(id));
  try { getAccount(id); } catch { getAccount(id); } finally { getAccount(id); }
  try { try { getAccount(id); } finally {} } catch {}
  try { [1].map(() => getAccount(id)); } catch {}
  new getAccount(id);
  getAccount`tpl`;
  getAccount!(id);
  getAccount<string>(id);
  const arr = [getAccount(id), { k: getAccount(id), getAccount }];
  if (await g2(id)) return getAccount(id) as any;
  const f: typeof getAccount = getAccount;
  label: for (;;) { break label; }
  return getAccount(id), getAccount(id);
}
",
    ),
    (
        "src/widgets/classes.ts",
        r"import * as auth from '../services/auth';
import { getAccount } from '../services/auth';

@Injectable()
export class WidgetService {
  @Decorated(getAccount)
  field = getAccount(1);
  constructor() {
    try { auth.getAccount(1); } catch {}
  }
  get account() { try { return getAccount(2); } finally {} }
  static { getAccount(3); }
  async load() {
    try {
      return auth.getAccount(4);
    } catch (e) {
      auth?.getAccount(5);
    }
  }
  getAccount() { return 1; }
}
const o = { async m() { try { getAccount(6); } catch {} }, n: function () { getAccount(7); } };
export default getAccount;
export { getAccount };
",
    ),
    (
        "src/widgets/view.tsx",
        r"import React from 'react';
import { Widget, getAccount } from '../services/auth';
import * as auth from '../services/auth';

export function View() {
  const x = import.meta.url;
  return (
    <div onClick={() => getAccount(1)}>
      <Widget getAccount={getAccount} />
      <auth.Widget>{getAccount(2)}</auth.Widget>
    </div>
  );
}
",
    ),
    (
        "src/legacy/requires.js",
        r"const { getAccount: fetch, other } = require('../services/auth');
const { getAccount = fallback } = require(`../services/auth`);
const { ...getAccount } = require('../services/auth');
const { 'getAccount': quoted } = require('../services/auth');
const svc = require('../services/auth'), x = 1;
let late;
late = require('../services/auth');

module.exports.run = async function () {
  fetch(1);
  await getAccount(2);
  quoted(3);
  svc.getAccount(4);
  late.getAccount(5);
  require('../services/auth').getAccount(6);
  require('../services/auth').getAccount;
  return svc?.getAccount(7);
};
",
    ),
    (
        "src/legacy/view.jsx",
        "const { getAccount } = require('../services/auth');\nexport const V = () => <p>{getAccount(1)}</p>;\n",
    ),
    (
        "src/legacy/esm.mjs",
        "import { getAccount } from '../services/auth.js';\nexport default async () => { await getAccount(1); getAccount(2); };\n",
    ),
    (
        "src/legacy/cjs.cjs",
        "const { getAccount } = require('../services/auth');\nif (!module.parent) return;\ngetAccount(1);\n",
    ),
    (
        "src/widgets/crlf.ts",
        "import { getAccount } from '../services/auth';\r\n\r\nexport function f() {\r\n  getAccount(1);\r\n  await getAccount(2);\r\n}\r\n",
    ),
    (
        "src/widgets/separators.ts",
        "import { getAccount } from '../services/auth';\u{2028}getAccount(1);\rgetAccount(2);\u{2029}getAccount(3);\n",
    ),
    (
        "src/widgets/bom.ts",
        "\u{feff}import { getAccount } from '@services/auth';\n// getAccount(0)\n/* getAccount(0) */ getAccount(1);\n",
    ),
    (
        "src/widgets/long.ts",
        "import { getAccount } from '../services/auth';\nconst veryLongLineThatGoesOnAndOnAndOnAndOnAndOnAndOnAndOnAndOnAndOnAndOnAndOnAndOnAndOnAndOnAndOnAndOn = getAccount('ééé🚀🚀🚀');\n",
    ),
    (
        "src/widgets/types.ts",
        "import type { getAccount } from '../services/auth';\nimport type * as authTypes from '../services/auth';\nlet f: typeof getAccount;\nexport type R = ReturnType<typeof getAccount>;\nexport const g = (x: Parameters<typeof authTypes.getAccount>) => x;\n",
    ),
    (
        "src/widgets/rare.ts",
        r"import { getAccount, meta, target, label, Widget } from '../services/auth';
import * as authSvc from '../services/auth';

getAccount(1);
const u = import.meta.url;
function F() { return new.target; }
label: for (;;) { break label; }
getAccount: for (;;) { continue getAccount; }
interface Rows { [getAccount: string]: number; getAccount(): void; meta: string }
enum E { getAccount = 1, target }
type Q = typeof authSvc.getAccount;
({ getAccount = 1 } = obj);
[getAccount] = list;
class P { #getAccount = 1; m() { return this.#getAccount; } static getAccount() {} }
authSvc.getAccount?.();
authSvc.getAccount!();
await using res = getAccount(2);
declare module 'x' { export function getAccount(): void; }
export { getAccount as default, Widget };
export = authSvc;
",
    ),
    (
        "src/widgets/rare.tsx",
        r"import { getAccount, Widget } from '../services/auth';
import * as authSvc from '../services/auth';
const a = <svg:rect getAccount={1} />;
const b = <authSvc.Widget.Inner />;
const c = <>{getAccount}</>;
const d = <Widget {...getAccount(1)} />;
",
    ),
    (
        "src/widgets/import-equals.ts",
        "import auth = require('../services/auth');\nimport { getAccount } from \"../services/auth\";\nauth.getAccount(1);\nexport = getAccount;\n",
    ),
];

#[test]
fn binding_refs_match_the_script_on_each_import_form() {
    let mut g = Golden::open("binding_refs_match_the_script_on_each_import_form");
    let mut cases = Vec::new();
    for (file, content) in IMPORTERS {
        for target in [
            "src/services/auth.ts",
            "src/services/index.ts",
            "src/services/other.ts",
        ] {
            for name in [
                "getAccount",
                "makeMw",
                "Widget",
                "other",
                "meta",
                "label",
                "target",
            ] {
                cases.push(bind_case(file, content, target, name));
            }
        }
    }
    // The batch is only a test if it reached the forms it was built for.
    let js = assert_batch(&mut g, "bindingRefs", &cases);
    g.done();
    let refs: usize = js
        .iter()
        .filter_map(|r| r["ok"]["refs"].as_array())
        .map(Vec::len)
        .sum();
    assert!(refs > 70, "only {refs} references found");
    for (k, v) in [
        ("awaited", json!(false)),
        ("awaited", json!(true)),
        ("inTry", json!(true)),
        ("call", json!(false)),
    ] {
        assert!(
            js.iter()
                .filter_map(|r| r["ok"]["refs"].as_array())
                .flatten()
                .any(|r| r[k] == v),
            "no reference with {k} = {v}"
        );
    }
    assert!(js.iter().any(|r| r["ok"]["reexports"] == json!(true)));
}

const PROMISES: &[(&str, &str)] = &[
    ("x.ts", "export const f = async () => 1;"),
    (
        "x.ts",
        "export function f(): Promise<number> { return g(); }",
    ),
    (
        "x.ts",
        "export function f(): PromiseLike<number> { return g(); }",
    ),
    (
        "x.ts",
        "export function f(): (Promise<number>) { return g(); }",
    ),
    ("x.ts", "export const f = (e): boolean => e.status === 404;"),
    ("x.ts", "export const f = (roles) => async (req) => roles;"),
    ("x.ts", "export const f = (input) => queue.add('x', input);"),
    ("x.ts", "export function f(x) { cache.delete(x); }"),
    ("x.js", "exports.f = async (x) => x;"),
    ("x.js", "module.exports.f = function (x) { return [x]; };"),
    ("x.js", "(module.exports.f) = function (x) { return [x]; };"),
    ("x.js", "module.exports . f = (x) => { return; };"),
    ("x.ts", "export const g = 1;"),
    (
        "x.ts",
        "export function f(a: string): void;\nexport function f(a: any) { return a; }",
    ),
    ("x.ts", "declare function f(): number;"),
    // No return type and no body: `ts.isBlock(undefined)` throws in the
    // script, which its caller reads as not known to be synchronous.
    (
        "x.ts",
        "export function f(a: string);\nexport function f(a: any) { return a; }",
    ),
    ("x.ts", "declare function f();"),
    ("x.d.ts", "export function f(a);"),
    ("x.ts", "export declare function f(): Promise<void>;"),
    ("x.ts", "export async function* f() {}"),
    ("x.ts", "export function f() { return void 0; }"),
    ("x.ts", "export function f() { return delete o.k; }"),
    ("x.ts", "export function f() { return a ? 1 : 2; }"),
    ("x.ts", "export function f() { return <number>x; }"),
    ("x.ts", "export function f() { return x in y; }"),
    ("x.ts", "export function f() { return import.meta; }"),
    (
        "x.ts",
        "export function f(): /* c */ Promise<void> { return g(); }",
    ),
    ("x.js", "exports.f = exports.g = async () => 1;"),
    ("x.js", "exports.g.f = () => 1;"),
    ("x.ts", "function outer() { function f() { return g(); } }"),
    (
        "x.ts",
        "export function f() { return { get() { return g(); } }; }",
    ),
    (
        "x.ts",
        "export function f() { const h = () => g(); return null; }",
    ),
    (
        "x.ts",
        "export function f() { if (a) return 1; return `t${x}`; }",
    ),
    ("x.ts", "export function f() { return -1; } "),
    ("x.ts", "export function f() { return typeof x; }"),
    ("x.ts", "export function f() { return ++x; }"),
    ("x.ts", "export function f() { return x++; }"),
    ("x.ts", "export function f() { return a ?? b; }"),
    ("x.ts", "export function f() { return (1); }"),
    ("x.ts", "export function f() { return /re/; }"),
    ("x.ts", "export function f() { return 10n; }"),
    ("x.ts", "export function f() { return class {}; }"),
    ("x.ts", "export function f() { return #p in o; }"),
    ("x.ts", "export const f = function* () { yield 1; };"),
    ("x.ts", "export const f = (async () => 1);"),
    ("x.ts", "const f = () => <T,>(x: T) => x;"),
    ("x.tsx", "export const f = () => <div />;"),
    ("x.jsx", "export const f = () => <div />;"),
    ("x.jsx", "export const f = (x) => <T>x;"),
    ("x.ts", "export class A { f() { return g(); } }"),
    ("x.ts", "const f = 1; function f() { return 2; }"),
    ("x.ts", "export default function f() { return null; }"),
    (
        "x.ts",
        "export const f = () => { class K { constructor() { return g(); } } };",
    ),
];

#[test]
fn returns_promise_matches_the_script() {
    let mut g = Golden::open("returns_promise_matches_the_script");
    let cases: Vec<Value> = PROMISES
        .iter()
        .map(|(file, content)| json!({"fn": "returnsPromise", "file": file, "content": content, "name": "f"}))
        .collect();
    let js = assert_batch(&mut g, "returnsPromise", &cases);
    g.done();
    for v in [json!(true), json!(false), Value::Null] {
        assert!(js.iter().any(|r| r["ok"] == v), "no case answered {v}");
    }
}

/// The fixtures of `eval/replay-attention-facts.mjs`, with the added lines it
/// gives them.
fn attention_seeds() -> Vec<Value> {
    let upload = "export async function upload(req, res) {\n  const filesToClean = [];\n  try {\n    if (!req.ok) {\n      return res.status(403).json({});\n    }\n    req.files.forEach((f) => filesToClean.push(f.path));\n    return res.status(200).json({});\n  } finally {\n    cleanup(filesToClean);\n  }\n}";
    let set_above = "export async function render(p) {\n  const tmp = make(p);\n  try {\n    return await read(tmp);\n  } finally {\n    await rm(tmp);\n  }\n}";
    let hook = "export async function hook(req, res) {\n  addSyncJob(req.body);\n  await addOtherJob(req.body);\n  return enqueueThing(req.body);\n}";
    let sentinel = "const getByExternalId = async (vid) => {\n  try {\n    return await db.find(vid);\n  } catch (e) {\n    return null;\n  }\n};\n\nexport async function resolve(vid) {\n  for (let i = 0; i < 3; i++) {\n    const a = await getByExternalId(vid);\n    if (!a) return null;\n  }\n  return null;\n}";
    let both = "export function parse(s) {\n  if (!s) return false;\n  try {\n    return JSON.parse(s).ok;\n  } catch {\n    return false;\n  }\n}";
    let distinct = "export function parse(s) {\n  if (!s) return null;\n  try {\n    return JSON.parse(s).ok;\n  } catch {\n    throw new Error(\"bad\");\n  }\n}";
    let authz = "module.exports = (req, res, next) => {\n  if (!allowed(req)) {\n    return res.status(403).json({ code: 403 });\n  }\n  if (account.isActive === 0) {\n    return res.status(StatusCodes.UNAUTHORIZED).json({ code: 401 });\n  }\n  next();\n};";
    let tests = "import { vi } from \"vitest\";\nvi.mock(\"./billing\");\nit(\"refunds the payment\", async () => {\n  await refund(1);\n});\nit(\"rejects a bad id\", () => {\n  expect(() => refund(-1)).toThrow();\n});\nit(\"uses a helper\", () => {\n  assertRefunded(1);\n});";
    let void_fn = "export async function save(x) {\n  if (!x) return;\n  try {\n    await db.put(x);\n  } catch (e) {\n    log(e);\n    return;\n  }\n}";
    let calls = "const get = async (id) => {\n  try { return await db.get(id); } catch { return null; }\n};\nexport async function a() { return get(1); }\nexport async function b() { return get(2); }";
    let mixed = (0..14)
        .map(|i| format!("const g{i} = async () => {{ try {{ return await q(); }} catch {{ return null; }} }};\nexport const c{i} = () => g{i}();"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\nexport const h = (req, res) => {\n  if (x) return res.status(403);\n  return res.status(401);\n};";
    let mut out = vec![
        facts_case("factsForFile", "src/x.ts", upload, &[5]),
        facts_case("factsForFile", "src/x.ts", upload, &[8]),
        facts_case("factsForFile", "src/x.ts", upload, &[2]),
        facts_case("factsForFile", "src/x.ts", set_above, &all_lines(set_above)),
        facts_case("factsForFile", "src/x.ts", hook, &all_lines(hook)),
        facts_case("factsForFile", "src/x.ts", "queue.addSyncJob(x);", &[1]),
        facts_case("factsForFile", "src/x.ts", "addNumbers(1, 2);", &[1]),
        facts_case("factsForFile", "src/x.ts", sentinel, &[11]),
        facts_case(
            "factsForFile",
            "src/x.ts",
            "const get = async (vid) => db.find(vid);\nconst a = get(1);",
            &[1, 2],
        ),
        facts_case("factsForFile", "src/x.ts", both, &all_lines(both)),
        facts_case("factsForFile", "src/x.ts", distinct, &all_lines(distinct)),
        facts_case(
            "factsForFile",
            "src/x.ts",
            "io.to(room).emit(\"xls:done\", { id });",
            &[1],
        ),
        facts_case(
            "factsForFile",
            "src/x.ts",
            "io.emit(\"a\", 1);\nfoo();",
            &[2],
        ),
        facts_case("statusOrderFacts", "src/legacy/authorize.js", authz, &[6]),
        facts_case(
            "statusOrderFacts",
            "a.js",
            &authz.replace("return res.status(403)", "return res.status(400)"),
            &[6],
        ),
        facts_case("statusOrderFacts", "a.js", authz, &[3]),
        facts_case(
            "testFacts",
            "src/services/billing.test.ts",
            tests,
            &[2, 3, 6, 9],
        ),
        facts_case(
            "testFacts",
            "src/services/billing.test.ts",
            "vi.mock(\"./logger\");",
            &[1],
        ),
        facts_case("factsForFile", "src/x.ts", void_fn, &all_lines(void_fn)),
        facts_case("factsForFile", "src/x.ts", calls, &[4, 5]),
        facts_case("factsForFile", "src/z.ts", &mixed, &all_lines(&mixed)),
        facts_case("statusOrderFacts", "src/z.ts", &mixed, &all_lines(&mixed)),
    ];
    let base = "export const find = async (id) => {\n  const r = await db.get(id);\n  if (!r) throw new NotFound(id);\n  return r;\n};\nexport function sum(a, b) { return a + b; }\nexport const opt = (a) => a;\n";
    let head = "export const find = async (id) => {\n  const r = await db.get(id);\n  if (!r) return null;\n  return r;\n};\nexport function sum(a, b, c) { return a + b + c; }\nexport const opt = (a, b = 1) => a + b;\n";
    let cjs_base = "function g(x) { return compute(x); }\nmodule.exports = { g };\n";
    let cjs_head = "function g(x) { try { return compute(x); } catch { return null; } }\nmodule.exports = { g };\n";
    for (file, b, h) in [
        ("src/x.ts", base, head),
        ("a.js", cjs_base, cjs_base),
        ("a.js", cjs_base, cjs_head),
        ("a.js", cjs_head, cjs_base),
    ] {
        out.push(json!({"fn": "contractFacts", "file": file, "base": b, "content": h}));
        out.push(json!({"fn": "exportedShapes", "file": file, "content": h}));
    }
    let decorated = "export class Svc {\n  @Get()\n  @Auth('admin')\n  public async load(\n  id) {\n    if (!id) return null;\n    try { return await db.get(id); } catch { return null; }\n  }\n  static\n  async *gen() { try { yield 1; } catch { return false; } return false; }\n}\nconst o = {\n  async m(\n  ) { if (a) return []; try { return x; } catch { return []; } },\n};\nexport\nasync function top() { if (a) return undefined; try { return 1; } catch { return undefined; } }\nexport default async function () { let x; try { x = 1; return y; } finally { use(x); } }\n";
    for added in [
        &[2usize][..],
        &[4],
        &[5],
        &[10],
        &[11],
        &[13],
        &[14],
        &[16],
        &[17],
        &[18],
    ] {
        out.push(facts_case("factsForFile", "src/svc.ts", decorated, added));
    }
    out.push(json!({"fn": "exportedShapes", "file": "src/svc.ts", "content": decorated}));
    out.push(json!({"fn": "contractFacts", "file": "src/svc.ts", "base": decorated.replace("top()", "top(a, b)"), "content": decorated}));
    let fills = "export async function save(req) {\n  let x = 0, list = [], m = new Map();\n  try {\n    if (a) return 1;\n    if (b) throw new Error();\n    x += 1;\n    [x] = y;\n    this.list.push(1);\n    list.unshift(2);\n    m.set(1, 2);\n    x = 3;\n    list?.push(4);\n    function inner() { return 5; }\n  } finally {\n    done(x, list, m.size);\n  }\n}\n";
    out.push(facts_case(
        "factsForFile",
        "src/fills.ts",
        fills,
        &[4, 5, 13],
    ));
    let emits = "io.emit('a', 1);\nio.emit(`b`, {});\nio.emit(`c${x}`);\nio.emit(...args);\nio.emit();\nthis.#emit('d');\nio?.emit('e');\nio.emit('\\u0066');\nemit('g');\n";
    out.push(facts_case(
        "factsForFile",
        "src/emits.ts",
        emits,
        &all_lines(emits),
    ));
    let status = "export function h(req, res) {\n  res.status(403);\n  res.sendStatus(401);\n  res.status(StatusCodes . UNAUTHORIZED);\n  res.status(HttpStatus.UNAUTHORIZED, 1);\n  new Forbidden();\n  new Unauthorised();\n  res.status(/* x */ 401);\n  function inner() { res.status(403); res.status(401); }\n  class K { constructor() { res.status(401); } }\n}\n";
    out.push(facts_case(
        "statusOrderFacts",
        "src/status.ts",
        status,
        &all_lines(status),
    ));
    let enq = "addJob(x);\nadd_Job(x);\nenqueue(x);\nenqueue$x(x);\nq.enqueueAll(x)\n;(addSyncJob)(x);\naddSyncJob?.(x);\nawait addSyncJob(x);\n";
    out.push(facts_case(
        "factsForFile",
        "src/enq.ts",
        enq,
        &all_lines(enq),
    ));
    let diff = "diff --git a/src/x.ts b/src/x.ts\nindex 1a2b3c4..5d6e7f8 100644\n--- a/src/x.ts\n+++ b/src/x.ts\ndiff --git a/src/n.ts b/src/n.ts\nnew file mode 100644\nindex 0000000..9a9a9a9\n--- /dev/null\n+++ b/src/n.ts\n";
    out.push(json!({"fn": "baseBlobs", "diff": diff}));
    out.push(json!({"fn": "addedLines", "diff": "diff --git a/src/a.ts b/src/a.ts\n--- a/src/a.ts\n+++ b/src/a.ts\n@@ -1,2 +1,3 @@\n export function f() {\n+  addSyncJob(1);\n }\n-gone\n+++ b/\n@@ -1 +99999999999999999999 @@\n+x\n"}));
    out
}

#[test]
fn attention_facts_match_the_script_on_the_replay_seeds() {
    let mut g = Golden::open("attention_facts_match_the_script_on_the_replay_seeds");
    let cases = attention_seeds();
    let js = assert_batch(&mut g, "attention seeds", &cases);
    g.done();
    let kinds: std::collections::HashSet<String> = js
        .iter()
        .filter_map(|r| r["ok"].as_array())
        .flatten()
        .filter_map(|f| f["kind"].as_str().map(str::to_owned))
        .collect();
    for k in [
        "exit-before-finally",
        "unawaited-enqueue",
        "sentinel-from-catch",
        "new-emit",
        "status-order",
        "test-without-assertion",
        "mocks-subject",
        "contract-change",
    ] {
        assert!(kinds.contains(k), "no {k} fact among the seeds");
    }
}

// ---------------------------------------------------------------------------
// Generated sources

/// A small deterministic generator, so a failing case can be replayed from its
/// seed.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n as u64).unwrap()
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len())]
    }
    fn chance(&mut self, pct: usize) -> bool {
        self.below(100) < pct
    }
}

/// A generator's seed: its fixed default, or mixed with
/// `TRIBUNAL_AST_SEED` to explore other inputs. A failure prints the source,
/// so a case found that way becomes a hand-written one.
fn seed(default: u64) -> u64 {
    std::env::var("TRIBUNAL_AST_SEED")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map_or(default, |s| default ^ s.wrapping_mul(0x2545_F491_4F6C_DD1D))
}

const EXTS: &[&str] = &[".ts", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"];

#[derive(Clone, Copy)]
struct Lang {
    ts: bool,
    jsx: bool,
}

fn lang_of(ext: &str) -> Lang {
    Lang {
        ts: ext == ".ts" || ext == ".tsx",
        jsx: ext != ".ts",
    }
}

/// Glue between tokens: a space, sometimes a line break or a comment, which
/// moves the line a site lands on.
fn gap(rng: &mut Rng) -> &'static str {
    rng.pick(&[" ", " ", " ", " ", "\n  ", " /* c */ ", "\n// note\n"])
}

const SPECS: &[&str] = &[
    "../services/auth",
    "../services/auth",
    "@services/auth",
    "../services/auth.js",
    "../services",
    "./auth",
    "../services/other",
    "@lib",
];

fn import_stmt(rng: &mut Rng, l: Lang) -> String {
    let spec = *rng.pick(SPECS);
    let q = if rng.chance(20) { "\"" } else { "'" };
    let s = format!("{q}{spec}{q}");
    let mut forms = vec![
        format!("import {{ getAccount }} from {s};"),
        format!("import {{ getAccount as fetchAccount }} from {s};"),
        format!(
            "import {{{}getAccount,{}other }} from {s};",
            gap(rng),
            gap(rng)
        ),
        format!("import * as authSvc from {s};"),
        format!("import getAccount from {s};"),
        format!("import def, {{ getAccount }} from {s};"),
        format!("import def, * as authSvc from {s};"),
        format!("import {{ \"getAccount\" as quoted }} from {s};"),
        format!("export {{ getAccount }} from {s};"),
        format!("export {{ getAccount as fetchAccount }} from {s};"),
        format!("export * from {s};"),
        format!("export * as authSvc from {s};"),
        format!("const {{ getAccount }} = require({s});"),
        format!(
            "const {{{}getAccount: fetchAccount,{}other }} = require({s});",
            gap(rng),
            gap(rng)
        ),
        format!("const {{ getAccount = noop }} = require({s});"),
        format!("let {{ ...getAccount }} = require({s});"),
        format!("const {{ inner: {{ getAccount }} }} = require({s});"),
        format!("const svc = require({s});"),
        format!("const svc = require(`{spec}`);"),
        format!("var svc = (require({s}));"),
        format!("const a = 1, svc = require({s});"),
        format!("import {s};"),
    ];
    if l.ts {
        forms.extend([
            format!("import type {{ getAccount }} from {s};"),
            format!("import {{ type getAccount }} from {s};"),
            format!("import svc = require({s});"),
            format!("export type {{ getAccount }} from {s};"),
        ]);
    }
    rng.pick(&forms).clone()
}

#[derive(Clone, Copy)]
struct Ctx {
    l: Lang,
    /// Inside an async function: `await` is allowed.
    can_await: bool,
    /// Inside a function body (not a static block): `return` is allowed.
    can_return: bool,
    /// A module (ESM syntax or TS): `import.meta` is allowed.
    module: bool,
}

/// A reference to the bound name in one of the shapes the script tells apart.
fn reference(rng: &mut Rng, cx: Ctx) -> String {
    let l = cx.l;
    let mut forms: Vec<String> = vec![
        "getAccount(id)".into(),
        "getAccount(id)".into(),
        "fetchAccount(id)".into(),
        "authSvc.getAccount(id)".into(),
        "svc.getAccount(id)".into(),
        "quoted(id)".into(),
        "require('../services/auth').getAccount(id)".into(),
        "getAccount".into(),
        "authSvc.getAccount".into(),
        "svc . getAccount (id)".into(),
        "getAccount?.(id)".into(),
        "authSvc?.getAccount(id)".into(),
        "svc?.getAccount?.(id)".into(),
        "authSvc['getAccount'](id)".into(),
        "(getAccount)(id)".into(),
        "getAccount(getAccount(id))".into(),
        "getAccount(id)(x)".into(),
        "x.getAccount(id)".into(),
        "this.getAccount(id)".into(),
        "getAccount`tpl`".into(),
        "new getAccount(id)".into(),
        "import('../services/auth')".into(),
        "import(getAccount(id))".into(),
        format!("getAccount({}id,{}x)", gap(rng), gap(rng)),
    ];
    if cx.module {
        forms.push("import.meta".into());
    }
    if l.ts {
        forms.extend([
            "getAccount!(id)".into(),
            "getAccount<string>(id)".into(),
            "(getAccount(id) as any)".into(),
            "getAccount(id)!".into(),
            "(getAccount(id) satisfies unknown)".into(),
        ]);
        if !l.jsx {
            forms.push("(<any>getAccount(id))".into());
        }
    }
    if l.jsx {
        forms.extend([
            "(<getAccount />)".into(),
            "(<authSvc.getAccount x={1} />)".into(),
            "(<div onClick={() => getAccount(id)}>{getAccount(id)}</div>)".into(),
            "(<Widget getAccount={getAccount} />)".into(),
            "(<authSvc.getAccount></authSvc.getAccount>)".into(),
        ]);
    }
    rng.pick(&forms).clone()
}

/// A statement around a reference: awaited, returned, chained, handed over,
/// or dropped.
fn statement(rng: &mut Rng, cx: Ctx, depth: usize) -> String {
    let e = reference(rng, cx);
    let g = gap(rng);
    let mut forms: Vec<String> = vec![
        format!("{e};"),
        format!("const v ={g}{e};"),
        format!("{e}.then(done);"),
        format!("{e}{g}.catch(done);"),
        format!("{e}.finally(done);"),
        format!("{e}?.then(done);"),
        format!("wrap({e});"),
        format!("({e});"),
        format!("void {e};"),
        format!("v = {e};"),
        format!("[{e}];"),
        format!("({{ k: {e} }});"),
        "({ getAccount });".to_owned(),
        format!("x ? {e} : y;"),
        format!("{e}, 1;"),
        format!("`${{{e}}}`;"),
        format!("throw {e};"),
        format!("if ({e}) {{}}"),
        format!("for (const a of {e}) {{}}"),
        format!("const p = async () => {e};"),
        format!("const p = () =>{g}({e});"),
        format!("const p = () => {{ {e}; }};"),
        format!("module.exports = {{ getAccount, run: {e} }};"),
        format!("exports.run = (id) => {e};"),
    ];
    if cx.can_await {
        forms.push(format!("await{g}{e};"));
        forms.push(format!("await Promise.all([{e}]);"));
    }
    if cx.can_return {
        forms.push(format!("return {e};"));
        forms.push(format!("return{g}{e};"));
    }
    if cx.l.ts {
        forms.push(format!("let w: any ={g}{e};"));
        forms.push("let t: typeof getAccount;".into());
        forms.push("type T = ReturnType<typeof authSvc.getAccount>;".into());
    }
    let s = rng.pick(&forms).clone();
    if depth >= 3 || rng.chance(40) {
        return s;
    }
    let sync = Ctx {
        can_await: false,
        ..cx
    };
    let async_fn = Ctx {
        can_await: true,
        can_return: true,
        ..cx
    };
    let no_return = Ctx {
        can_await: false,
        can_return: false,
        ..cx
    };
    let wrap = |rng: &mut Rng, k: Ctx| statement(rng, k, depth + 1);
    let body_ctx = Ctx {
        can_return: true,
        ..sync
    };
    match rng.below(12) {
        0 => format!("try {{\n{}\n}} catch (e) {{\n{}\n}}", wrap(rng, cx), wrap(rng, cx)),
        1 => format!("try {{ {} }} catch {{}} finally {{ {} }}", wrap(rng, cx), wrap(rng, cx)),
        2 => format!(
            "try {{\n  try {{ {} }} finally {{}}\n}} catch {{ {} }}",
            wrap(rng, cx),
            wrap(rng, cx)
        ),
        3 => format!(
            "async function inner() {{\n{}\n{}\n}}",
            wrap(rng, async_fn),
            wrap(rng, async_fn)
        ),
        4 => format!("const inner = async () => {{\n{}\n}};", wrap(rng, async_fn)),
        5 => format!("[1].forEach(function () {{ {} }});", wrap(rng, body_ctx)),
        6 => format!("if (ok) {{\n{}\n}} else {{ {} }}", wrap(rng, cx), wrap(rng, cx)),
        7 => format!(
            "switch (k) {{\ncase getAccount: {}\ndefault: {}\n}}",
            wrap(rng, cx),
            wrap(rng, cx)
        ),
        8 => format!("for (;;) {{ {} break; }}", wrap(rng, cx)),
        9 => format!(
            "class K {{\n  async m() {{ {} }}\n  get g() {{ {} return 1; }}\n  constructor() {{ try {{ {} }} catch {{}} }}\n  static {{ {} }}\n  f = () => {{ {} }};\n}}",
            wrap(rng, async_fn),
            wrap(rng, body_ctx),
            wrap(rng, body_ctx),
            wrap(rng, no_return),
            wrap(rng, body_ctx),
        ),
        10 => format!(
            "const o = {{\n  async m() {{ {} }},\n  get g() {{ {} return 1; }},\n  n: function () {{ {} }},\n}};",
            wrap(rng, async_fn),
            wrap(rng, body_ctx),
            wrap(rng, body_ctx),
        ),
        _ => format!("{{ {} }}", wrap(rng, cx)),
    }
}

fn gen_importer(rng: &mut Rng) -> (String, String) {
    let ext = *rng.pick(EXTS);
    let l = lang_of(ext);
    let mut src = String::new();
    if rng.chance(10) {
        src.push('\u{feff}');
    }
    let mut imports = String::new();
    for _ in 0..=rng.below(3) {
        imports.push_str(&import_stmt(rng, l));
        imports.push('\n');
    }
    src.push_str(&imports);
    let module =
        l.ts || ext == ".mjs" || imports.contains("import ") || imports.contains("export ");
    let cx = Ctx {
        l,
        can_await: true,
        can_return: true,
        module,
    };
    let body: Vec<String> = (0..=rng.below(4)).map(|_| statement(rng, cx, 0)).collect();
    let body = body.join("\n");
    match rng.below(6) {
        0 => src.push_str(&format!(
            "\nexport async function handle(id) {{\n{body}\n}}\n"
        )),
        1 => src.push_str(&format!(
            "\nexport const handle = async (id) => {{\n{body}\n}};\n"
        )),
        2 => src.push_str(&format!(
            "\nexports.handle = async function (id) {{\n{body}\n}};\n"
        )),
        3 => src.push_str(&format!(
            "\nexport\nasync function handle(id) {{\n{body}\n}}\n"
        )),
        4 => src.push_str(&format!(
            "\nasync function* handle(id) {{\n{body}\n}}\nmodule.exports = {{ handle }};\n"
        )),
        _ => {
            let top = Ctx {
                can_await: module,
                can_return: false,
                ..cx
            };
            let body: Vec<String> = (0..=rng.below(3)).map(|_| statement(rng, top, 0)).collect();
            src.push_str(&body.join("\n"));
            src.push_str("\nexport default getAccount;\n");
        }
    }
    let mut src = if rng.chance(10) {
        src.replace('\n', "\r\n")
    } else {
        src
    };
    if rng.chance(5) {
        src = src.replace('\n', "\u{2028}");
    }
    (format!("src/widgets/w{}{ext}", rng.below(3)), src)
}

#[test]
fn binding_refs_match_the_script_on_generated_importers() {
    let mut g = Golden::open("binding_refs_match_the_script_on_generated_importers");
    let mut rng = Rng::new(seed(0xB1D));
    let mut cases = Vec::new();
    for _ in 0..1500 {
        let (file, src) = gen_importer(&mut rng);
        let target = if rng.chance(80) {
            "src/services/auth.ts"
        } else {
            "src/services/index.ts"
        };
        let name = if rng.chance(90) { "getAccount" } else { "meta" };
        cases.push(bind_case(&file, &src, target, name));
    }
    let js = assert_batch(&mut g, "generated importers", &cases);
    g.done();
    assert_valid("generated importers", &cases, &js);
    let refs = all_refs(&js);
    let count = |k: &str, v: Value| refs.iter().filter(|r| r[k] == v).count();
    for (k, v, min) in [
        ("call", json!(true), 500),
        ("call", json!(false), 200),
        ("awaited", json!(true), 200),
        ("awaited", json!(false), 200),
        ("inTry", json!(true), 100),
        ("inTry", json!(false), 300),
    ] {
        let n = count(k, v.clone());
        assert!(n >= min, "only {n} references with {k} = {v}");
    }
    let nulls = js.iter().filter(|r| r["ok"].is_null()).count();
    assert!(
        nulls > 100 && nulls < 1200,
        "{nulls} importers bind nothing"
    );
    assert!(
        js.iter()
            .filter(|r| r["ok"]["reexports"] == json!(true))
            .count()
            > 50
    );
}

fn gen_returned(rng: &mut Rng, l: Lang) -> String {
    let mut forms: Vec<&str> = vec![
        "",
        "1",
        "null",
        "true",
        "'s'",
        "`t`",
        "`t${x}`",
        "[]",
        "[x]",
        "{}",
        "{ a: 1 }",
        "x",
        "g()",
        "queue.add('x', input)",
        "a + b",
        "a ?? b",
        "a && b",
        "x = 1",
        "(a, b)",
        "-1",
        "!x",
        "~x",
        "+x",
        "typeof x",
        "void x",
        "delete x.y",
        "++x",
        "x++",
        "(1)",
        "(g())",
        "/re/g",
        "10n",
        "() => 1",
        "async () => 1",
        "function () {}",
        "class {}",
        "new Promise(r)",
        "this",
        "x ? 1 : 2",
        "g`x`",
        "Promise.resolve(1)",
    ];
    if l.ts {
        forms.extend(["x as any", "x!", "x satisfies number"]);
        if !l.jsx {
            forms.push("<any>x");
        }
    }
    if l.jsx && !l.ts {
        forms.push("<div />");
    }
    (*rng.pick(&forms)).to_owned()
}

fn gen_promise_decl(rng: &mut Rng) -> (String, String) {
    let ext = *rng.pick(EXTS);
    let mut l = lang_of(ext);
    // returnsPromise parses a .jsx file as TS: JSX there is a parse error.
    if ext == ".jsx" {
        l = Lang {
            ts: false,
            jsx: false,
        };
    }
    let ret = if l.ts {
        *rng.pick(&[
            "",
            "",
            ": Promise<void>",
            ": Promise",
            ": void",
            ": PromiseLike<number>",
            ": (Promise<number>)",
            ": Promise<number> | null",
            ": Promised",
        ])
    } else {
        ""
    };
    let asy = if rng.chance(30) { "async " } else { "" };
    let mut body = String::new();
    for _ in 0..rng.below(4) {
        let r = gen_returned(rng, l);
        let stmt = match rng.below(7) {
            0 => format!("if (c) return {r};\n"),
            1 => format!("try {{ return {r}; }} catch {{ return null; }}\n"),
            2 => "const h = () => { return g(); };\n".to_owned(),
            3 => "const o = { m() { return g(); } };\n".to_owned(),
            4 => "return;\n".to_owned(),
            _ => format!("return {r};\n"),
        };
        body.push_str(&stmt);
    }
    let expr = gen_returned(rng, l);
    let expr = if expr.is_empty() {
        "undefined".to_owned()
    } else {
        expr
    };
    let paren = |e: &str| {
        if e.starts_with('{') {
            format!("({e})")
        } else {
            e.to_owned()
        }
    };
    let decl = match rng.below(9) {
        0 => format!("export {asy}function f(x){ret} {{\n{body}}}\n"),
        1 => format!("export const f = {asy}(x){ret} => {{\n{body}}};\n"),
        2 => format!("export const f = {asy}(x){ret} => {};\n", paren(&expr)),
        3 => format!("exports.f = {asy}function (x) {{\n{body}}};\n"),
        4 => format!("module.exports.f = {asy}(x) => {};\n", paren(&expr)),
        5 => format!("function outer() {{\n  {asy}function f(x){ret} {{\n{body}}}\n}}\n"),
        6 => format!("export default {asy}function f(x){ret} {{\n{body}}}\n"),
        7 => format!("const f = ({asy}(x) => {});\n", paren(&expr)),
        _ => format!("let f = 1;\nvar g = {asy}function f() {{ {body} }};\n"),
    };
    let decl = if l.ts && rng.chance(15) {
        format!("export function f(a: string): Promise<void>;\n{decl}")
    } else {
        decl
    };
    (format!("src/services/s{ext}"), decl)
}

#[test]
fn returns_promise_matches_the_script_on_generated_declarations() {
    let mut g = Golden::open("returns_promise_matches_the_script_on_generated_declarations");
    let mut rng = Rng::new(seed(0x9_0A1));
    let cases: Vec<Value> = (0..1200)
        .map(|_| {
            let (file, content) = gen_promise_decl(&mut rng);
            json!({"fn": "returnsPromise", "file": file, "content": content, "name": "f"})
        })
        .collect();
    let js = assert_batch(&mut g, "generated declarations", &cases);
    g.done();
    assert_valid("generated declarations", &cases, &js);
    for v in [json!(true), json!(false), Value::Null] {
        let n = js.iter().filter(|r| r["ok"] == v).count();
        assert!(n > 50, "only {n} cases answered {v}");
    }
}

fn sentinel(rng: &mut Rng) -> &'static str {
    rng.pick(&[
        "null",
        "false",
        "[]",
        "undefined",
        "0",
        "x",
        "",
        "true",
        "[1]",
        "(null)",
    ])
}

/// A statement for the attention-fact classes: exits, fills, enqueues,
/// emits, status answers, calls to helpers that hide errors.
fn fact_stmt(rng: &mut Rng, can_await: bool, can_return: bool, depth: usize) -> String {
    let s = sentinel(rng);
    let mut forms: Vec<String> = vec![
        "throw new Error('x');".to_owned(),
        "addSyncJob(req.body);".to_owned(),
        "queue.addOtherJob(req.body);".to_owned(),
        "enqueueThing(x);".to_owned(),
        "queue?.enqueueLater(x);".to_owned(),
        "void addSyncJob(x);".to_owned(),
        "const j = addMailJob(x);".to_owned(),
        "io.emit('widgets:done', { id });".to_owned(),
        "socket.to(room).emit(`widgets:${x}`, payload);".to_owned(),
        "socket.emit(\"widgets:ping\");".to_owned(),
        "socket.emit(`widgets:pong`, {});".to_owned(),
        "this.emit('evt', 1);".to_owned(),
        "files.push(f);".to_owned(),
        "toClean.add(p);".to_owned(),
        "list = more;".to_owned(),
        "(list) = more;".to_owned(),
        "items.forEach((i) => files.push(i));".to_owned(),
        "res.status( 401 ).send();".to_owned(),
        "res.sendStatus(StatusCodes.FORBIDDEN);".to_owned(),
        "res.status(HttpStatus.FORBIDDEN);".to_owned(),
        "res.status(...codes);".to_owned(),
        "throw new UnauthorizedError();".to_owned(),
        "throw new ForbiddenError('no');".to_owned(),
        "throw new AuthUnauthorisedException();".to_owned(),
        "getByExternalId(1);".to_owned(),
        "helper(2);".to_owned(),
        "noCatch(3);".to_owned(),
    ];
    if can_await {
        forms.push("await addSyncJob(req.body);".to_owned());
        forms.push("const a = await getByExternalId(vid);".to_owned());
    }
    if can_return {
        forms.push(format!("return {s};"));
        forms.push(format!("if (!ok) return {s};"));
        forms.push("return addSyncJob(x);".to_owned());
        forms.push("return res.status(403).json({});".to_owned());
        forms.push("return res.status(StatusCodes.UNAUTHORIZED).json({});".to_owned());
    }
    let st = rng.pick(&forms).clone();
    if depth >= 3 || rng.chance(45) {
        return st;
    }
    let block = |rng: &mut Rng, aw: bool, ret: bool| {
        (0..=rng.below(3))
            .map(|_| fact_stmt(rng, aw, ret, depth + 1))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let (aw, ret) = (can_await, can_return);
    match rng.below(8) {
        0 => format!(
            "try {{\n{}\n}} finally {{\ncleanup(files, toClean, list);\n}}",
            block(rng, aw, ret)
        ),
        1 => format!(
            "try {{\n{}\n}} catch (e) {{\n{}\n}} finally {{\n{}\n}}",
            block(rng, aw, ret),
            block(rng, aw, ret),
            block(rng, aw, ret)
        ),
        2 => format!("try {{\n{}\n}} catch {{\n{}\n}}", block(rng, aw, ret), block(rng, aw, ret)),
        3 => format!("if (x) {{\n{}\n}} else {{\n{}\n}}", block(rng, aw, ret), block(rng, aw, ret)),
        4 => format!("const cb = async () => {{\n{}\n}};", block(rng, true, true)),
        5 => format!("[1].map(function () {{\n{}\n}});", block(rng, false, true)),
        6 => format!("for (const f of fs) {{\n{}\n}}", block(rng, aw, ret)),
        _ => format!(
            "class K {{\n  constructor() {{\n{}\n  }}\n  async m() {{\n{}\n  }}\n  get g() {{\n{}\n  }}\n}}",
            block(rng, false, true),
            block(rng, true, true),
            block(rng, false, true)
        ),
    }
}

fn gen_fact_file(rng: &mut Rng) -> (String, String) {
    let ext = *rng.pick(&[".ts", ".ts", ".js", ".tsx", ".cjs"]);
    let mut src = String::new();
    let helpers = [
        "const getByExternalId = async (vid) => {\n  try {\n    return await db.find(vid);\n  } catch (e) {\n    return null;\n  }\n};\n",
        "function helper(x) {\n  try { return compute(x); } catch { return false; }\n}\n",
        "const noCatch = async (x) => db.find(x);\n",
        "export const getByExternalId = function (vid) {\n  if (!vid) return [];\n  try { return db.find(vid); } catch { return []; }\n};\n",
    ];
    for h in helpers {
        if rng.chance(50) {
            src.push_str(h);
        }
    }
    for i in 0..=rng.below(3) {
        let body = |rng: &mut Rng, aw: bool, ret: bool| {
            (0..=rng.below(4))
                .map(|_| fact_stmt(rng, aw, ret, 0))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let decl = match rng.below(6) {
            0 => format!(
                "export async function handler{i}(req, res) {{\nconst files = [];\nlet list;\n{}\n}}\n",
                body(rng, true, true)
            ),
            1 => format!("export const handler{i} = async (req, res, next) => {{\n{}\n}};\n", body(rng, true, true)),
            2 => format!("module.exports.handler{i} = function (req, res) {{\n{}\n}};\n", body(rng, false, true)),
            3 => format!("class H{i} {{\n  async handle(req, res) {{\n{}\n  }}\n}}\n", body(rng, true, true)),
            4 => format!("export\nfunction handler{i}(req, res) {{\n{}\n}}\n", body(rng, false, true)),
            _ => format!("{}\n", body(rng, false, false)),
        };
        src.push_str(&decl);
    }
    (format!("src/controllers/c{ext}"), src)
}

fn added_subset(rng: &mut Rng, src: &str) -> Vec<usize> {
    let n = src.split('\n').count();
    if rng.chance(30) {
        return (1..=n).collect();
    }
    (1..=n).filter(|_| rng.chance(40)).collect()
}

#[test]
fn attention_facts_match_the_script_on_generated_files() {
    let mut g = Golden::open("attention_facts_match_the_script_on_generated_files");
    let mut rng = Rng::new(seed(0xFAC7));
    let mut cases = Vec::new();
    for _ in 0..900 {
        let (file, src) = gen_fact_file(&mut rng);
        let added = added_subset(&mut rng, &src);
        cases.push(facts_case("factsForFile", &file, &src, &added));
        cases.push(facts_case("statusOrderFacts", &file, &src, &added));
    }
    let js = assert_batch(&mut g, "generated fact files", &cases);
    g.done();
    assert_valid("generated fact files", &cases, &js);
    let counts = fact_counts(&js);
    for k in [
        "exit-before-finally",
        "unawaited-enqueue",
        "sentinel-from-catch",
        "new-emit",
        "status-order",
    ] {
        assert!(
            counts.get(k).copied().unwrap_or(0) > 20,
            "too few {k}: {counts:?}"
        );
    }
}

fn gen_test_file(rng: &mut Rng) -> (String, String) {
    let ext = *rng.pick(&[".ts", ".js", ".tsx"]);
    let subject = *rng.pick(&["billing", "widgets", "acme"]);
    let file = format!(
        "src/services/{}{subject}.{}{ext}",
        if rng.chance(30) { "__tests__/" } else { "" },
        rng.pick(&["test", "spec"])
    );
    let mut src = String::from("import { vi } from 'vitest';\n");
    for _ in 0..=rng.below(5) {
        let title = rng.pick(&[
            "'refunds'",
            "\"rejects\"",
            "`works`",
            "`with ${x}`",
            "name",
            "'a very long test title that goes on and on and on and on and on and on and on and on'",
        ]);
        let body = rng.pick(&[
            "await refund(1)",
            "expect(x).toBe(1)",
            "expect (x).toBe(1)",
            "assertRefunded(1)",
            "assert.equal(1, 1)",
            "x.should.equal(1)",
            "expect.assertions(1)",
            "toMatchInlineSnapshot()",
            "reexpect(1)",
            "/* expect(x) */ run()",
            "expectTypeOf(x).toBeString()",
        ]);
        let callee = rng.pick(&[
            "it",
            "test",
            "it.only",
            "test.skip",
            "describe",
            "it.each([1])",
        ]);
        let cb = match rng.below(4) {
            0 => format!("async () => {{\n  {body};\n}}"),
            1 => format!("async function () {{\n  {body};\n}}"),
            2 => format!("async () => {body}"),
            _ => "done".to_owned(),
        };
        src.push_str(&format!("{callee}({title}, {cb});\n"));
        if rng.chance(30) {
            let target = rng.pick(&[
                "'./billing'",
                "\"../widgets\"",
                "'./acme.ts'",
                "'./logger'",
                "`./billing`",
                "'@services/billing'",
            ]);
            let mocker = rng.pick(&["vi", "jest", "vitest"]);
            src.push_str(&format!("{mocker}.mock({target});\n"));
        }
    }
    (file, src)
}

#[test]
fn test_facts_match_the_script_on_generated_tests() {
    let mut g = Golden::open("test_facts_match_the_script_on_generated_tests");
    let mut rng = Rng::new(seed(0x7E57));
    let mut cases = Vec::new();
    for _ in 0..600 {
        let (file, src) = gen_test_file(&mut rng);
        let added = added_subset(&mut rng, &src);
        cases.push(facts_case("testFacts", &file, &src, &added));
    }
    let js = assert_batch(&mut g, "generated tests", &cases);
    g.done();
    assert_valid("generated tests", &cases, &js);
    let counts = fact_counts(&js);
    for k in ["test-without-assertion", "mocks-subject"] {
        assert!(
            counts.get(k).copied().unwrap_or(0) > 30,
            "too few {k}: {counts:?}"
        );
    }
}

fn gen_contract_fn(rng: &mut Rng, name: &str, ext: &str) -> String {
    let ts = ext.starts_with(".ts");
    let mut params: Vec<String> = Vec::new();
    let n = rng.below(4);
    let mut optional = false;
    for i in 0..n {
        let last = i + 1 == n;
        let kind = rng.below(if ts { 5 } else { 3 });
        params.push(match kind {
            2 => format!("p{i} = {i}"),
            3 if ts => {
                optional = true;
                format!("p{i}?: number")
            }
            4 if last => format!("...rest{i}"),
            _ if optional => format!("p{i}?: string"),
            _ => format!("p{i}"),
        });
    }
    let arrow = rng.below(5);
    if ts && rng.chance(10) && !matches!(arrow, 1 | 4) {
        params.insert(0, "this: Ctx".to_owned());
    }
    let mut body = String::new();
    for _ in 0..rng.below(4) {
        body.push_str(match rng.below(6) {
            0 => "if (!x) return null;\n",
            1 => "if (!x) throw new NotFound();\n",
            2 => "try { return run(); } catch { return false; }\n",
            3 => "const h = () => { throw e; };\n",
            4 => "if (y) return [];\n",
            _ => "return x;\n",
        });
    }
    let p = params.join(", ");
    match arrow {
        0 => format!("export function {name}({p}) {{\n{body}}}\n"),
        1 => format!("export const {name} = async ({p}) => {{\n{body}}};\n"),
        2 => format!("exports.{name} = function ({p}) {{\n{body}}};\n"),
        3 => format!("function {name}({p}) {{\n{body}}}\nmodule.exports = {{ {name} }};\n"),
        _ => format!("export const {name} = ({p}) => x;\n"),
    }
}

#[test]
fn contract_facts_match_the_script_on_generated_pairs() {
    let mut g = Golden::open("contract_facts_match_the_script_on_generated_pairs");
    let mut rng = Rng::new(seed(0xC0DE));
    let mut cases = Vec::new();
    for _ in 0..600 {
        let ext = *rng.pick(&[".ts", ".js", ".cjs"]);
        let file = format!("src/helpers/h{ext}");
        let names = ["find", "sum", "opt"];
        let base: String = names
            .iter()
            .map(|n| gen_contract_fn(&mut rng, n, ext))
            .collect();
        let head: String = names
            .iter()
            .map(|n| gen_contract_fn(&mut rng, n, ext))
            .collect();
        cases.push(json!({"fn": "contractFacts", "file": file, "base": base, "content": head}));
        cases.push(json!({"fn": "exportedShapes", "file": file, "content": head}));
    }
    let js = assert_batch(&mut g, "generated contracts", &cases);
    g.done();
    assert_valid("generated contracts", &cases, &js);
    let n = js
        .iter()
        .filter_map(|r| r["ok"].as_array())
        .flatten()
        .filter(|f| f["kind"] == "contract-change")
        .count();
    assert!(n > 100, "only {n} contract facts");
}

// ---------------------------------------------------------------------------
// The whole script on git fixture repos

fn real_git() -> String {
    let out = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "init.defaultBranch=main",
            "-c",
            "diff.noprefix=false",
            "-c",
            "diff.mnemonicPrefix=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn write_files(dir: &Path, files: &[(String, Option<String>)]) {
    for (rel, content) in files {
        let p = dir.join(rel);
        match content {
            Some(c) => {
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, c).unwrap();
            }
            None => {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

type Files = Vec<(String, Option<String>)>;

fn files(v: &[(&str, &str)]) -> Files {
    v.iter()
        .map(|(f, c)| ((*f).to_owned(), Some((*c).to_owned())))
        .collect()
}

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    diff: PathBuf,
    head: String,
}

/// A repo with `base` committed, then `head` applied and committed, and the
/// unified diff between the two.
fn fixture(name: &str, base: &Files, head: &Files) -> Fixture {
    let root = scratch(name);
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    write_files(&repo, base);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
    let base_sha = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    write_files(&repo, head);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "head"]);
    let head_sha = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    let diff = git(
        &repo,
        &["diff", "--no-color", "--no-ext-diff", &base_sha, &head_sha],
    );
    let diff_path = root.join("pr.diff");
    std::fs::write(&diff_path, diff).unwrap();
    Fixture {
        root,
        repo,
        diff: diff_path,
        head: head_sha,
    }
}

/// A `git` that logs each call's arguments, then runs the real one.
fn git_wrapper(root: &Path) -> PathBuf {
    let dir = root.join("gitwrap");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("git");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\037' \"$a\"; done >> \"$GIT_CALL_LOG\"\nprintf '\\n' >> \"$GIT_CALL_LOG\"\nexec '{}' \"$@\"\n",
            real_git()
        ),
    )
    .unwrap();
    Command::new("chmod").arg("+x").arg(&path).status().unwrap();
    dir
}

struct Pair {
    js: Value,
    rust: Value,
    js_calls: String,
    rust_calls: String,
    /// `typescript <version>`, the parser the script ran with.
    ts_name: String,
}

/// The script, run from `TRIBUNAL_TS_FROM` so its `loadTypescript([repoDir,
/// process.cwd()])` finds the parser, and the port, each through the logging
/// `git`.
async fn pair(g: &mut Golden, root: &Path, diff: &Path, repo: &Path, rev: Option<&str>) -> Pair {
    pair_with(g, root, diff, repo, rev, true).await
}

/// [`pair`], the port finding `typescript` for the files oxc cannot parse
/// cleanly where the script finds it, or nowhere.
async fn pair_with(
    g: &mut Golden,
    root: &Path,
    diff: &Path,
    repo: &Path,
    rev: Option<&str>,
    with_typescript: bool,
) -> Pair {
    let ts = ts_from();
    let wrap = git_wrapper(root);
    let rec = recordings(root, vec![]);
    let js_log = root.join("git-js.log");
    let rust_log = root.join("git-rust.log");
    let _ = std::fs::remove_file(&js_log);
    let _ = std::fs::remove_file(&rust_log);
    // A live run records nothing: the corpus repos are too big to hash.
    let input = if g.mode == Mode::Live {
        Value::Null
    } else {
        json!({
            "diff": [diff.to_string_lossy(), file_fingerprint(diff)],
            "repo": [repo.to_string_lossy(), repo_fingerprint(repo)],
            "rev": rev,
        })
    };
    let script = g.answer(&input, &path_pairs(Some(root), Some(&ts)), || {
        let mut args = vec![diff.to_str().unwrap(), repo.to_str().unwrap()];
        if let Some(r) = rev {
            args.extend(["--rev", r]);
        }
        let path = std::env::join_paths([wrap.clone(), shim_dir()].into_iter().chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        let out = Command::new("node")
            .arg(plugin_root().join(BLAST))
            .args(&args)
            .current_dir(&ts)
            .env("PATH", path)
            .env("GH_RECORDINGS", &rec)
            .env("GIT_CALL_LOG", &js_log)
            .env_remove("CLAUDE_PLUGIN_ROOT")
            .env_remove("NODE_PATH")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("node must be on PATH");
        json!({
            "code": out.status.code(),
            "stdout": String::from_utf8_lossy(&out.stdout),
            "stderr": String::from_utf8_lossy(&out.stderr),
            "calls": std::fs::read_to_string(&js_log).unwrap_or_default(),
        })
    });
    let stdout = script["stdout"].as_str().unwrap();
    assert_eq!(
        script["code"],
        json!(0),
        "script failed: {stdout}{}",
        script["stderr"].as_str().unwrap()
    );
    let mut lines = stdout.lines();
    assert!(
        lines.next().unwrap_or("").starts_with("RELAY-CRC "),
        "{stdout}"
    );
    let js: Value = serde_json::from_str(lines.next().expect("a JSON line")).unwrap();
    let mut t: Tools = tools(&rec);
    t.git = wrap.join("git").to_string_lossy().into_owned();
    t.env.push(("GIT_CALL_LOG".into(), rust_log.clone().into()));
    let mut dirs = vec![repo.to_path_buf()];
    if with_typescript {
        dirs.push(ts.clone());
    }
    let parser = TypeScriptFallback::new(dirs);
    let rust = blast_radius::run_in(
        diff.to_str().unwrap(),
        repo.to_str().unwrap(),
        rev,
        &t,
        blast_radius::Resolver::Bindings,
        parser,
    )
    .await
    .unwrap_or_else(|e| panic!("port failed: {e}"));
    Pair {
        js,
        rust,
        js_calls: script["calls"].as_str().unwrap().to_owned(),
        rust_calls: std::fs::read_to_string(&rust_log).unwrap_or_default(),
        ts_name: g.typescript.clone(),
    }
}

/// The whole relayed object is the script's, except `attention.parser`,
/// which names the parser (declared); and the git calls are the same, in the
/// same order. Returns the script's object.
fn assert_same(case: &str, p: &Pair) -> Value {
    assert_same_resolving(case, p, Some(&[]))
}

/// [`assert_same`] where TypeScript answered for exactly `files`, the ones
/// oxc does not parse cleanly, or for any files when `files` is `None`. A
/// clean file never reaches it, and every file that needed it got it.
fn assert_same_resolving(case: &str, p: &Pair, files: Option<&[&str]>) -> Value {
    let mut rust = p.rust.clone();
    let fallback = rust
        .as_object_mut()
        .unwrap()
        .remove("typescriptFallback")
        .expect("the port reports typescriptFallback");
    if let Some(files) = files {
        assert_eq!(fallback["files"], json!(files), "{case}: {fallback}");
    }
    assert_eq!(fallback["unresolved"], json!([]), "{case}: {fallback}");
    let p = &Pair {
        js: p.js.clone(),
        rust,
        js_calls: p.js_calls.clone(),
        rust_calls: p.rust_calls.clone(),
        ts_name: p.ts_name.clone(),
    };
    assert_eq!(
        p.js["resolver"], "bindings",
        "{case}: the script found no typescript"
    );
    assert_eq!(p.rust["resolver"], "bindings", "{case}");
    assert_eq!(p.js["attention"]["parser"], json!(p.ts_name), "{case}");
    assert_eq!(p.rust["attention"]["parser"], json!(PARSER), "{case}");
    let mut js = p.js.clone();
    js["attention"]["parser"] = json!(PARSER);
    let (a, b) = (
        serde_json::to_string(&js).unwrap(),
        serde_json::to_string(&p.rust).unwrap(),
    );
    if a != b {
        let pretty = |v: &Value| serde_json::to_string_pretty(v).unwrap();
        panic!(
            "{case}: output differs\n--- script ---\n{}\n--- port ---\n{}",
            pretty(&js),
            pretty(&p.rust)
        );
    }
    assert_eq!(p.rust_calls, p.js_calls, "{case}: git calls differ");
    p.js.clone()
}

const TSCONFIG: &str = r#"{
  // Path aliases, as a backend declares them
  "compilerOptions": {
    "baseUrl": ".",
    "paths": {
      "@services/*": ["src/services/*"],
      "@lib": ["src/lib/index.ts"],
    },
  },
  "include": ["src/**/*"]
}
"#;

fn queue_src(timeout: &str, extra: &str) -> String {
    format!(
        r"import {{ Queue }} from 'bullmq';

const queue = new Queue('widgets', {{ connection, timeout: {timeout} }});

/** Enqueue a widget sync. */
export const enqueueSync = (data: SyncData) => queue.add('sync', data, {{ attempts: {timeout} }});

export async function drainQueue(): Promise<void> {{
  await queue.drain({timeout});
}}

export const requireRole = (roles: string[]) => async (req, res, next) => {{
  if (!roles.includes(req.role)) return res.status(403).json({{}});
  next({timeout});
}};

export function retryDelay(attempt: number) {{
  return attempt * {timeout};
}}

export const enqueueAll = (items) => items.map((i) => enqueueSync(i));
{extra}
export type SyncData = {{ id: string }};
"
    )
}

const WIDGET_CONTROLLER: &str = r"import { Request, Response } from 'express';
import { enqueueSync, retryDelay } from '@services/queue';
import * as queues from '../services/queue';
import type { SyncData } from '../services/queue';

export const handleWebhook = async (req: Request, res: Response) => {
  enqueueSync(req.body);
  try {
    queues.enqueueSync(req.body);
  } catch (e) {
    await queues.drainQueue();
  }
  return res.status(200).json({ delay: retryDelay(1) });
};

export async function handleBatch(items: SyncData[]) {
  const all = queues.enqueueAll(items);
  return Promise.all(all).then(() => queues.drainQueue());
}
";

const WIDGET_ROUTES: &str = r"import { Router } from 'express';
import { requireRole } from '../services/queue';
import { handleWebhook } from '../controllers/widgets.controller';

const router = Router();
router.post('/a', requireRole(['admin']), handleWebhook);
router.post('/b', requireRole(['user']), handleWebhook);
router.post('/c', requireRole(['user']), handleWebhook);
export const factories = [requireRole];
export default router;
";

const WIDGET_JOB: &str = r"import {
  enqueueSync as push,
  drainQueue,
} from '../services';

export async function runSync(ids: string[]) {
  for (const id of ids) {
    await push({ id });
  }
  const pending =
    push({ id: 'last' });
  drainQueue().then(() => undefined);
  return pending;
}
";

const WIDGET_WORKER: &str = r"const q = require('../services/queue');
const { drainQueue: drain, ...rest } = require('@services/queue');

async function consume(job) {
  q.enqueueSync(job.data);
  await drain();
  return q?.retryDelay(2);
}

module.exports = { consume };
";

const WIDGET_VIEW: &str = r"import React from 'react';
import { enqueueSync } from '../services/queue';

@observer
export class WidgetPanel extends React.Component {
  @action
  save() {
    enqueueSync({ id: this.props.id });
  }
  render() {
    return <button onClick={() => enqueueSync({ id: 'x' })}>Save</button>;
  }
}
";

const ACME_CONTROLLER_BASE: &str = r"import { prisma } from '../lib/prisma';

export async function uploadEvidence(req, res) {
  const filesToClean = [];
  try {
    req.files.forEach((f) => filesToClean.push(f.path));
    return res.status(200).json({});
  } finally {
    cleanup(filesToClean);
  }
}

export const authorize = (req, res, next) => {
  if (!allowed(req)) {
    return res.status(403).json({ code: 403 });
  }
  next();
};

export const createResult = async (input) => {
  await prisma.results.create({ data: { orderId: input.orderId, sheetId: input.sheetId, contactId: input.contactId, status: 'pending' } });
  socketIO.emit(WEB_SOCKET.RESULT, { kind: 'created', orderId: input.orderId, sheetId: input.sheetId });
};
";

const ACME_CONTROLLER_HEAD: &str = r"import { prisma } from '../lib/prisma';

export async function uploadEvidence(req, res) {
  const filesToClean = [];
  try {
    if (!req.ok) {
      return res.status(403).json({});
    }
    req.files.forEach((f) => filesToClean.push(f.path));
    return res.status(200).json({});
  } finally {
    cleanup(filesToClean);
  }
}

export const authorize = (req, res, next) => {
  if (!allowed(req)) {
    return res.status(403).json({ code: 403 });
  }
  if (account.isActive === 0) {
    return res.status(StatusCodes.UNAUTHORIZED).json({ code: 401 });
  }
  next();
};

export const createResult = async (input) => {
  await prisma.results.create({ data: { orderId: input.orderId, sheetId: input.sheetId, contactId: input.contactId, status: 'pending' } });
  socketIO.emit(WEB_SOCKET.RESULT, { kind: 'created', orderId: input.orderId, sheetId: input.sheetId });
};

export const createTerminal = async (input) => {
  await prisma.results.create({ data: { orderId: input.orderId, status: input.status } });
  socketIO.emit(WEB_SOCKET.RESULT, { kind: 'created', orderId: input.orderId });
  addResultJob(input);
  await queue.add(ResultJobNames.SYNC, { id: 1 });
  queue.add(ResultJobNames.LOST, { id: 2 });
  io.emit('acme:done', { id: 1 });
};
";

const ACME_READER: &str = r"export const listResults = (input) => {
  const where = {};
  if (input.contactId) where.contactId = { contains: input.contactId };
  return prisma.results.findMany({ where });
};
export const mark = () => {
  socketIO.emit(WEB_SOCKET.RESULT, { kind: 'updated', orderId: 1, sheetId: 2 });
  return prisma.results.create({ data: { orderId: 1, sheetId: 2, contactId: 3 } });
};
";

const ACME_WORKER: &str = "switch (job.name) {\n  case ResultJobNames.SYNC:\n    break;\n}\n";

const LOOKUP_BASE: &str = r"function findAccount(id) {
  const r = db.get(id);
  if (!r) throw new NotFound(id);
  return r;
}

function getAccountByExternalId(vid) {
  return db.byVid(vid);
}

module.exports = { findAccount, getAccountByExternalId };
";

const LOOKUP_HEAD: &str = r"function findAccount(id) {
  const r = db.get(id);
  if (!r) return null;
  return r;
}

function getAccountByExternalId(vid, opts) {
  try {
    return db.byVid(vid, opts);
  } catch (e) {
    return null;
  }
}

module.exports = { findAccount, getAccountByExternalId };
";

const LOOKUP_USER: &str = r"const { findAccount, getAccountByExternalId } = require('./lookup');
const lookup = require('./lookup');

exports.resolve = async (vid) => {
  const a = findAccount(vid);
  try {
    return getAccountByExternalId(vid) || lookup.findAccount(vid);
  } catch (e) {
    return null;
  }
};
";

const BILLING_TEST_HEAD: &str = r"import { vi } from 'vitest';
vi.mock('./billing');
it('refunds the payment', async () => {
  await refund(1);
});
it('rejects a bad id', () => {
  expect(() => refund(-1)).toThrow();
});
";

fn realistic_base() -> Files {
    files(&[
        ("tsconfig.json", TSCONFIG),
        ("src/services/queue.ts", &queue_src("1000", "")),
        ("src/services/index.ts", "export * from './queue';\n"),
        ("src/services/queue.test.ts", "import { enqueueSync } from './queue';\nit('enqueues', async () => { await enqueueSync({ id: '1' }); });\n"),
        ("src/controllers/widgets.controller.ts", WIDGET_CONTROLLER),
        ("src/routes/widgets.ts", WIDGET_ROUTES),
        ("src/jobs/sync.job.ts", WIDGET_JOB),
        ("src/workers/consumer.js", WIDGET_WORKER),
        ("src/views/WidgetPanel.tsx", WIDGET_VIEW),
        ("src/controllers/acme.controller.ts", ACME_CONTROLLER_BASE),
        ("src/services/results.ts", ACME_READER),
        ("src/jobs/worker.ts", ACME_WORKER),
        ("src/legacy/lookup.js", LOOKUP_BASE),
        ("src/legacy/resolve.js", LOOKUP_USER),
        ("src/services/billing.test.ts", "it('noop', () => { expect(1).toBe(1); });\n"),
        ("README.md", "enqueueSync is documented here\n"),
    ])
}

fn realistic_head() -> Files {
    files(&[
        (
            "src/services/queue.ts",
            &queue_src("2000", "\nexport function unusedHelper() {}\n"),
        ),
        ("src/controllers/acme.controller.ts", ACME_CONTROLLER_HEAD),
        ("src/legacy/lookup.js", LOOKUP_HEAD),
        ("src/services/billing.test.ts", BILLING_TEST_HEAD),
    ])
}

#[tokio::test]
#[ignore = "needs TRIBUNAL_TS_FROM"]
async fn blast_radius_matches_the_script_with_typescript_on_a_realistic_repo() {
    let mut g = Golden::open("blast_radius_matches_the_script_with_typescript_on_a_realistic_repo");
    let f = fixture("ast-real", &realistic_base(), &realistic_head());
    let p = pair(&mut g, &f.root, &f.diff, &f.repo, None).await;
    g.done();
    let out = assert_same("checkout", &p);
    // The fixture is only a test if it reaches the paths it was built for.
    let callers = out["callers"].as_array().unwrap();
    assert!(callers.len() >= 15, "{out}");
    for (k, v) in [
        ("awaited", json!(false)),
        ("awaited", json!(true)),
        ("awaited", Value::Null),
        ("inTry", json!(true)),
        ("inTry", json!(false)),
        ("call", json!(false)),
    ] {
        assert!(
            callers.iter().any(|c| c[k] == v),
            "no caller with {k} = {v}: {out}"
        );
    }
    let vias: Vec<&str> = callers.iter().map(|c| c["via"].as_str().unwrap()).collect();
    for via in [
        "enqueueSync",
        "requireRole",
        "drainQueue",
        "findAccount",
        "enqueueSync -> enqueueAll",
    ] {
        assert!(vias.contains(&via), "no caller via {via}: {out}");
    }
    let kinds: Vec<&str> = out["attention"]["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["kind"].as_str().unwrap())
        .collect();
    for k in [
        "exit-before-finally",
        "status-order",
        "payload-parity",
        "writer-parity",
        "job-without-handler",
        "contract-change",
        "unawaited-enqueue",
        "test-without-assertion",
    ] {
        assert!(kinds.contains(&k), "no {k} fact: {kinds:?}");
    }
}

#[tokio::test]
#[ignore = "needs TRIBUNAL_TS_FROM"]
async fn blast_radius_matches_the_script_with_typescript_at_a_revision() {
    let mut g = Golden::open("blast_radius_matches_the_script_with_typescript_at_a_revision");
    let f = fixture("ast-rev", &realistic_base(), &realistic_head());
    // The working tree moves on; --rev must read the commit, not the files.
    write_files(
        &f.repo,
        &files(&[
            (
                "src/controllers/widgets.controller.ts",
                "export const gone = 1;\n",
            ),
            ("src/services/results.ts", "export {};\n"),
        ]),
    );
    let p = pair(&mut g, &f.root, &f.diff, &f.repo, Some(&f.head)).await;
    g.done();
    let out = assert_same("--rev", &p);
    assert!(p.js_calls.contains("cat-file"), "no base blob read");
    assert!(p.js_calls.contains("show"), "no file read at the revision");
    assert!(out["callers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["file"] == "src/controllers/widgets.controller.ts"));
}

#[tokio::test]
#[ignore = "needs TRIBUNAL_TS_FROM"]
async fn blast_radius_matches_the_script_with_typescript_when_truncated() {
    let mut g = Golden::open("blast_radius_matches_the_script_with_typescript_when_truncated");
    let mut base = realistic_base();
    let mut extra = Vec::new();
    for i in 0..30 {
        extra.push((
            format!("src/controllers/widget{i}.controller.ts"),
            Some(format!(
                "import {{ enqueueSync, drainQueue }} from '../services/queue';\nexport const h{i} = async () => {{\n  enqueueSync({{ id: {i} }});\n  await drainQueue();\n}};\n"
            )),
        ));
    }
    base.extend(extra);
    let many: String = (0..20).map(|i| format!("addSyncJob({i});\n")).collect();
    let mut head = realistic_head();
    head.push(("src/jobs/many.ts".to_owned(), Some(many)));
    let f = fixture("ast-trunc", &base, &head);
    let p = pair(&mut g, &f.root, &f.diff, &f.repo, None).await;
    g.done();
    let out = assert_same("truncated", &p);
    assert_eq!(out["truncated"], true);
    assert_eq!(out["attention"]["truncated"], true);
}

/// The repos `eval/replay-binding-graph.mjs` and
/// `eval/replay-attention-facts.mjs` build, through the whole script.
#[tokio::test]
#[ignore = "needs TRIBUNAL_TS_FROM"]
async fn blast_radius_matches_the_script_with_typescript_on_the_replay_repos() {
    let mut g = Golden::open("blast_radius_matches_the_script_with_typescript_on_the_replay_repos");
    let auth = |body: &str| {
        format!("export const getAccount = async (id) => {{\n  return db.get(id, {body});\n}};\n\nexport const makeMw = (roles) => async (req, res, next) => {{\n  if (!roles.includes(req.role)) return res.status({body});\n  next();\n}};\n")
    };
    let mut base: Files = vec![
        ("tsconfig.json".into(), Some("{ \"compilerOptions\": { \"paths\": { \"@services/*\": [\"./src/services/*\"] } } }\n".into())),
        ("src/services/auth.ts".into(), Some(auth("1"))),
        ("src/services/index.ts".into(), Some("export { getAccount } from './auth';\n".into())),
    ];
    for (file, content) in &IMPORTERS[..8] {
        base.push(((*file).to_owned(), Some((*content).to_owned())));
    }
    let head: Files = vec![("src/services/auth.ts".into(), Some(auth("2")))];
    let f = fixture("ast-replay-bindings", &base, &head);
    let p = pair(&mut g, &f.root, &f.diff, &f.repo, None).await;
    let out = assert_same("replay-binding-graph", &p);
    assert!(out["callers"].as_array().unwrap().len() >= 6, "{out}");

    let results_base = "export const createPending = async (input) => {\n  await prisma.results.create({ data: { orderId: input.orderId, sheetId: input.sheetId, contactId: input.contactId ?? null, status: \"pending\" } });\n  socketIO.emit(WEB_SOCKET.RESULT, { kind: \"created\", orderId: input.orderId, sheetId: input.sheetId });\n};\nexport const listResults = (input) => {\n  const where = {};\n  if (input.contactId) where.contactId = { contains: input.contactId };\n  return prisma.results.findMany({ where });\n};\n";
    let results_head = "export const createPending = async (input) => {\n  await prisma.results.create({ data: { orderId: input.orderId, sheetId: input.sheetId, contactId: input.contactId ?? null, status: \"pending\" } });\n  socketIO.emit(WEB_SOCKET.RESULT, { kind: \"created\", orderId: input.orderId, sheetId: input.sheetId });\n};\nexport const createTerminal = async (input) => {\n  await prisma.results.create({ data: { orderId: input.orderId, status: input.status } });\n  socketIO.emit(WEB_SOCKET.RESULT, { kind: \"created\", orderId: input.orderId });\n};\nexport const listResults = (input) => {\n  const where = {};\n  if (input.contactId) where.contactId = { contains: input.contactId };\n  return prisma.results.findMany({ where });\n};\n";
    let other = "export const mark = () => {\n  socketIO.emit(WEB_SOCKET.RESULT, { kind: \"updated\", orderId: 1, sheetId: 2 });\n  return queue.add(ResultJobNames.SYNC, { id: 1 });\n};\nexport const spread = () => socketIO.emit(WEB_SOCKET.RESULT, { ...base });\n";
    let base = files(&[
        ("src/services/results.ts", results_base),
        ("src/services/other.ts", other),
        (
            "src/jobs/worker.ts",
            "switch (job.name) {\n  case ResultJobNames.OTHER:\n}\n",
        ),
    ]);
    let head = files(&[
        ("src/services/results.ts", results_head),
        (
            "src/services/other.ts",
            &format!(
                "{other}export const again = () => queue.add(ResultJobNames.SYNC, {{ id: 2 }});\n"
            ),
        ),
    ]);
    let f = fixture("ast-replay-edges", &base, &head);
    let p = pair(&mut g, &f.root, &f.diff, &f.repo, None).await;
    g.done();
    let out = assert_same("replay-attention-facts edges", &p);
    let kinds: Vec<&str> = out["attention"]["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["kind"].as_str().unwrap())
        .collect();
    for k in ["payload-parity", "writer-parity", "job-without-handler"] {
        assert!(kinds.contains(&k), "no {k}: {kinds:?}");
    }
}

fn auth_module(rng: &mut Rng, version: usize) -> String {
    let forms = [
        format!("export const getAccount = async (id) => {{\n  return db.get(id, {version});\n}};\n"),
        format!("export async function getAccount(id) {{\n  return db.get(id, {version});\n}}\n"),
        format!("export function getAccount(id): Promise<Account> {{\n  return db.get(id, {version});\n}}\n"),
        format!("export const getAccount = (id) => db.get(id, {version});\n"),
        format!("export const getAccount = (roles) => async (req) => roles.includes({version});\n"),
        format!("export function getAccount(id) {{\n  cache.delete(id, {version});\n}}\n"),
        format!("exports.getAccount = async function (id) {{\n  return db.get(id, {version});\n}};\n"),
    ];
    let mut src = rng.pick(&forms).clone();
    src.push_str("\nexport const Widget = () => null;\n\nexport function useAccount(id) {\n  return getAccount(id);\n}\n");
    src
}

fn gen_repo(rng: &mut Rng) -> (Files, Files) {
    let seed = rng.next();
    let mut a = Rng::new(seed);
    let mut b = Rng::new(seed);
    let base_auth = auth_module(&mut a, 1);
    let head_auth = auth_module(&mut b, 2);
    let mut base: Files = vec![
        ("tsconfig.json".into(), Some(TSCONFIG.into())),
        ("src/services/auth.ts".into(), Some(base_auth)),
        (
            "src/services/index.ts".into(),
            Some(
                rng.pick(&[
                    "export * from './auth';\n",
                    "export { getAccount } from './auth';\n",
                    "export { useAccount } from './auth';\n",
                ])
                .to_string(),
            ),
        ),
    ];
    let dirs = [
        "src/widgets",
        "src/controllers",
        "src/routes",
        "src/jobs",
        "src/legacy",
    ];
    for i in 0..(3 + rng.below(8)) {
        let (file, src) = gen_importer(rng);
        let ext = &file[file.rfind('.').unwrap()..];
        base.push((format!("{}/w{i}{ext}", rng.pick(&dirs)), Some(src)));
    }
    let mut head: Files = vec![("src/services/auth.ts".into(), Some(head_auth))];
    if rng.chance(50) {
        let (_, src) = gen_fact_file(rng);
        head.push(("src/controllers/added.controller.ts".into(), Some(src)));
    }
    if rng.chance(30) {
        // A changed importer is in the PR: its own references drop out.
        let (file, src) = gen_importer(rng);
        let ext = &file[file.rfind('.').unwrap()..];
        head.push((format!("src/widgets/changed{ext}"), Some(src)));
    }
    (base, head)
}

#[tokio::test]
#[ignore = "needs TRIBUNAL_TS_FROM"]
async fn blast_radius_matches_the_script_with_typescript_on_generated_repos() {
    let mut g = Golden::open("blast_radius_matches_the_script_with_typescript_on_generated_repos");
    let mut rng = Rng::new(seed(0x0005_EED5));
    let mut listed = 0;
    let mut facts = 0;
    for i in 0..30 {
        let (base, head) = gen_repo(&mut rng);
        let f = fixture(&format!("ast-gen-{i}"), &base, &head);
        let rev = if i % 4 == 0 {
            Some(f.head.clone())
        } else {
            None
        };
        let p = pair(&mut g, &f.root, &f.diff, &f.repo, rev.as_deref()).await;
        // oxc reports some early errors TypeScript's parser leaves to its
        // checker (a duplicated export): those files go to TypeScript.
        let out = assert_same_resolving(&format!("generated repo {i}"), &p, None);
        listed += out["callers"].as_array().unwrap().len();
        facts += out["attention"]["facts"].as_array().unwrap().len();
        let _ = std::fs::remove_dir_all(&f.root);
    }
    g.done();
    assert!(
        listed > 150,
        "only {listed} callers across the generated repos"
    );
    assert!(facts > 20, "only {facts} facts across the generated repos");
}

/// Emits, writers and jobs across files: the comparisons `edgeFacts` makes
/// with the rest of the repo, including a token more than 40 files mention.
#[tokio::test]
#[ignore = "needs TRIBUNAL_TS_FROM"]
async fn edge_facts_match_the_script_on_a_repo_of_writers_and_emits() {
    let mut g = Golden::open("edge_facts_match_the_script_on_a_repo_of_writers_and_emits");
    let mut base: Files = files(&[
        (
            "src/services/widgets.ts",
            "export const a = () => {\n  socket.emit('widgets:sync', { id, tenant, $meta: 1, 'acme-key': 2 });\n  prisma.widget.upsert({ where: { id }, create: { id, tenant, sku: 1, $meta: 1, 'acme-key': 2 }, update: {} });\n  prisma.widget.createMany({ data: [{ id, tenant, sku: 2, $meta: 1, 'acme-key': 2 }] });\n  return queue.add('widgets-sync', { id });\n};\n",
        ),
        (
            "src/services/readers.ts",
            "export const find = (id) => prisma.widget.findMany({ where: {\n  tenant: id, sku: 1 } });\nexport const by = (w) => w.where.$meta;\nconst f = { where : { 'acme-key': 1 } };\n",
        ),
        (
            "src/jobs/worker.ts",
            "switch (job.name) {\n  case \"widgets-sync\":\n    break;\n  case WidgetJobs.RENDER:\n    break;\n}\n",
        ),
        (
            "src/services/spread.ts",
            "export const s = () => {\n  socket.emit('widgets:sync', { ...rest });\n  prisma.widget.create({ data: { ...rest } });\n  prisma.widget.create({ data });\n  prisma.widget.create({ data() { return 1; } });\n};\n",
        ),
    ]);
    for i in 0..45 {
        base.push((
            format!("src/services/many{i:02}.ts"),
            Some(format!(
                "export const m{i} = () => socket.emit(WIDGET_EVENTS.MANY, {{ id: {i}, tenant, ['computed']: 1, 7: 2 }});\n"
            )),
        ));
    }
    let head = files(&[(
        "src/services/widgets.ts",
        "export const a = () => {\n  socket.emit('widgets:sync', { id, tenant, $meta: 1, 'acme-key': 2 });\n  prisma.widget.upsert({ where: { id }, create: { id, tenant, sku: 1, $meta: 1, 'acme-key': 2 }, update: {} });\n  prisma.widget.createMany({ data: [{ id, tenant, sku: 2, $meta: 1, 'acme-key': 2 }] });\n  return queue.add('widgets-sync', { id });\n};\nexport const b = () => {\n  socket.emit('widgets:sync', { id });\n  socket.emit(\"widgets:sync\", { id });\n  prisma.widget.create({ data: { id } });\n  prisma.widget.upsert({ create: [{ id, tenant, sku: 3, $meta: 1, 'acme-key': 2 }] });\n  queue.add('widgets-sync', { id });\n  queue.add(\"widgets-sync\", { id });\n  queue.add(WidgetJobs.RENDER, {});\n  queue.add(WidgetJobs.LOST, {});\n  queue.add('', {});\n  queue.add(`widgets-${x}`, {});\n  socket.emit(WIDGET_EVENTS.MANY, { id: 99 });\n  this.#bus.add(this.#job, {});\n};\n",
    )]);
    let f = fixture("ast-edges", &base, &head);
    let p = pair(&mut g, &f.root, &f.diff, &f.repo, None).await;
    let out = assert_same("edges", &p);
    let kinds: Vec<&str> = out["attention"]["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["kind"].as_str().unwrap())
        .collect();
    for k in ["payload-parity", "writer-parity", "job-without-handler"] {
        assert!(kinds.contains(&k), "no {k}: {out}");
    }
    let p = pair(&mut g, &f.root, &f.diff, &f.repo, Some(&f.head)).await;
    g.done();
    assert_same("edges at --rev", &p);
}

// ---------------------------------------------------------------------------
// Failure paths, with the parser loaded

#[tokio::test]
#[ignore = "needs TRIBUNAL_TS_FROM"]
async fn failure_paths_match_the_script_with_typescript() {
    let mut g = Golden::open("failure_paths_match_the_script_with_typescript");
    let f = fixture("ast-fail", &realistic_base(), &realistic_head());
    // An empty diff, a diff of only non-source files, a repo that is not one.
    let empty = f.root.join("empty.diff");
    std::fs::write(&empty, "").unwrap();
    let p = pair(&mut g, &f.root, &empty, &f.repo, None).await;
    assert_same("empty diff", &p);
    let docs = f.root.join("docs.diff");
    std::fs::write(&docs, "diff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n@@ -1 +1 @@\n-a\n+b\n").unwrap();
    let p = pair(&mut g, &f.root, &docs, &f.repo, None).await;
    assert_same("docs only", &p);
    let not_a_repo = f.root.join("plain");
    std::fs::create_dir_all(not_a_repo.join("src/services")).unwrap();
    std::fs::write(not_a_repo.join("src/services/queue.ts"), queue_src("1", "")).unwrap();
    let p = pair(&mut g, &f.root, &f.diff, &not_a_repo, None).await;
    assert_same("not a git repo", &p);
    let p = pair(
        &mut g,
        &f.root,
        &f.diff,
        &f.repo,
        Some("0000000000000000000000000000000000000000"),
    )
    .await;
    assert_same("unknown revision", &p);
    // A diff that cannot be read: the script prints FATAL and exits 1.
    let missing = f.root.join("missing.diff");
    let rust = blast_radius::run(
        missing.to_str().unwrap(),
        f.repo.to_str().unwrap(),
        None,
        &Tools::default(),
    )
    .await;
    let err = rust.expect_err("a missing diff is fatal");
    let ts = ts_from();
    let input = json!({
        "missing": missing.to_string_lossy(),
        "repo": [f.repo.to_string_lossy(), repo_fingerprint(&f.repo)],
    });
    let js = g.answer(&input, &path_pairs(Some(&f.root), Some(&ts)), || {
        let out = Command::new("node")
            .arg(plugin_root().join(BLAST))
            .arg(&missing)
            .arg(&f.repo)
            .current_dir(&ts)
            .output()
            .unwrap();
        json!({"code": out.status.code(), "stdout": String::from_utf8_lossy(&out.stdout)})
    });
    g.done();
    assert_eq!(js["code"], json!(1));
    assert_eq!(js["stdout"], json!(format!("{err}\n")));
}

/// Nested past what either parser takes: TypeScript's parser overflows the
/// stack and the script catches it. The port hands the file to the same
/// parser, which yields nothing for it the same way, and nothing crashes.
#[tokio::test]
#[ignore = "needs TRIBUNAL_TS_FROM"]
async fn deep_nesting_yields_nothing_on_both_sides() {
    let mut g = Golden::open("deep_nesting_yields_nothing_on_both_sides");
    let deep = |n: usize| {
        format!(
            "import {{ getAccount }} from '../services/queue';\nexport const deep = () => {}getAccount(1){};\nexport async function ok() {{ try {{ getAccount(1); }} finally {{ addSyncJob(1); }} }}\n",
            "(".repeat(n),
            ")".repeat(n)
        )
    };
    let mut base = realistic_base();
    base.push(("src/widgets/deep.ts".into(), Some(deep(3000))));
    base.push(("src/widgets/deeper.ts".into(), Some(deep(20_000))));
    let mut head = realistic_head();
    head.push(("src/widgets/added-deep.ts".into(), Some(deep(5000))));
    let f = fixture("ast-deep", &base, &head);
    let p = pair(&mut g, &f.root, &f.diff, &f.repo, None).await;
    g.done();
    let out = assert_same_resolving(
        "deep nesting",
        &p,
        Some(&[
            "src/widgets/added-deep.ts",
            "src/widgets/deep.ts",
            "src/widgets/deeper.ts",
        ]),
    );
    assert!(!out["callers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["file"].as_str().unwrap().contains("deep")));
    assert!(ast_bindings::nesting_depth(&deep(3000)) > ast_bindings::MAX_NESTING);
}

// ---------------------------------------------------------------------------
// Declared differences, each with its witness

/// `attention.parser` names the parser: `typescript <version>` in the script,
/// oxc in the port. Everything else in the relayed object is the same.
#[tokio::test]
#[ignore = "needs TRIBUNAL_TS_FROM"]
async fn declared_difference_parser_name() {
    let mut g = Golden::open("declared_difference_parser_name");
    let f = fixture("ast-parser-name", &realistic_base(), &realistic_head());
    let p = pair(&mut g, &f.root, &f.diff, &f.repo, None).await;
    g.done();
    let name = p.ts_name.clone();
    assert!(name.starts_with("typescript "), "{name}");
    assert_eq!(p.js["attention"]["parser"], json!(name));
    assert_eq!(p.rust["attention"]["parser"], json!("oxc_parser 0.155.0"));
    assert_eq!(PARSER, "oxc_parser 0.155.0");
}

/// The version `PARSER` names is the oxc_parser the workspace locks.
#[test]
fn the_parser_name_matches_the_locked_oxc() {
    let lock =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock"))
            .unwrap();
    let at = lock
        .find("name = \"oxc_parser\"\nversion = \"")
        .expect("oxc_parser in Cargo.lock");
    let rest = &lock[at + "name = \"oxc_parser\"\nversion = \"".len()..];
    let version = &rest[..rest.find('"').unwrap()];
    assert_eq!(PARSER, format!("oxc_parser {version}"));
}

/// Code that does not compile: TypeScript's parser recovers a tree from it
/// (and from TypeScript syntax in a `.js` file, which it parses and its
/// checker refuses); oxc stops, and the file would yield nothing. The port
/// hands such a file to the script's own function, so it answers as the
/// script does.
#[tokio::test]
#[ignore = "needs TRIBUNAL_TS_FROM"]
async fn error_recovery_follows_the_script() {
    let mut g = Golden::open("error_recovery_follows_the_script");
    let witnesses = [
        ("src/widgets/w.ts", "import { getAccount } from '../services/auth';\ngetAccount(1);\nconst = ;\n"),
        ("src/widgets/w.js", "const { getAccount } = require('../services/auth');\nfunction f(x: number) { getAccount(x); }\n"),
        ("src/widgets/w.ts", "import { getAccount } from '../services/auth';\nexport async function f() {\n  getAccount(1);\n  const o = { async\n  m() {} };\n}\n"),
    ];
    let cases: Vec<Value> = witnesses
        .iter()
        .map(|(f, c)| bind_case(f, c, "src/services/auth.ts", "getAccount"))
        .collect();
    let js = js_batch(&mut g, &cases);
    g.done();
    let mut ts = TypeScriptFallback::new(vec![ts_from()]);
    for ((file, content), j) in witnesses.iter().zip(&js) {
        let refs = j["ok"]["refs"]
            .as_array()
            .expect("the script recovers references");
        assert!(!refs.is_empty(), "{j}");
        let (errors, fatal) =
            ast_bindings::parse_errors(file, content, ast_bindings::ScriptKind::of(file));
        assert!(fatal && !errors.is_empty(), "oxc accepted {content}");
        assert_eq!(
            ast_bindings::binding_refs(file, content, "src/services/auth.ts", "getAccount", &[]),
            None
        );
        let routed = ts
            .binding_refs(file, content, "src/services/auth.ts", "getAccount", &[])
            .await;
        assert_eq!(bound_json(routed), j["ok"], "{content}");
    }
    assert!(js[0]["diag"].as_array().is_some_and(|d| !d.is_empty()));
    assert_eq!(
        ts.report()["files"],
        json!(["src/widgets/w.js", "src/widgets/w.ts"])
    );
}

/// `returnsPromise` reads a `.jsx` file as JSX, as `bindingRefs` does. Until
/// 2026-10-22 the script parsed it as TS, so TypeScript's error recovery
/// decided the answer for a component; both sides now parse the JSX.
#[test]
fn returns_promise_reads_jsx_as_jsx() {
    let mut g = Golden::open("returns_promise_reads_jsx_as_jsx");
    let cases = [
        ("export const f = async () => <div />;", "f", Some(true)),
        // A returned JSX element is not on the script's list of plainly
        // non-promise values, so a component stays unknown on both sides.
        (
            "export function A() { const x = 1; return <p>{x}</p>; }",
            "A",
            None,
        ),
        (
            "export const load = async (id) => { const r = await fetch(id); return <ul>{r}</ul>; };",
            "load",
            Some(true),
        ),
    ];
    let calls: Vec<Value> = cases
        .iter()
        .map(|(content, name, _)| {
            json!({"fn": "returnsPromise", "file": "src/w.jsx", "content": content, "name": name})
        })
        .collect();
    let js = js_batch(&mut g, &calls);
    g.done();
    for ((content, name, want), j) in cases.iter().zip(&js) {
        let rust = ast_bindings::returns_promise("src/w.jsx", content, name);
        assert_eq!(rust, *want, "{content}");
        assert_eq!(j["ok"], json!(want), "script on {content}: {j}");
    }
}

/// Between where V8's stack runs out under TypeScript's parser (about 611
/// levels of `[` in a fresh node process) and `MAX_NESTING`, the script
/// throws (and catches it: no references) while oxc parses. A file nested
/// past `TS_NESTING` goes to TypeScript in the embedded engine, whose stack is
/// sized to run out at the same depth, so the port throws and catches with
/// it.
#[tokio::test]
#[ignore = "needs TRIBUNAL_TS_FROM"]
async fn nesting_band_follows_the_script() {
    let mut g = Golden::open("nesting_band_follows_the_script");
    let content = format!(
        "import {{ getAccount }} from '../services/auth';\nconst a = {}getAccount(1){};\n",
        "[".repeat(990),
        "]".repeat(990)
    );
    assert!(ast_bindings::nesting_depth(&content) <= ast_bindings::MAX_NESTING);
    assert!(ast_bindings::nesting_depth(&content) > typescript_fallback::TS_NESTING);
    let js = js_batch(
        &mut g,
        &[bind_case(
            "src/widgets/w.ts",
            &content,
            "src/services/auth.ts",
            "getAccount",
        )],
    );
    g.done();
    assert!(
        js[0]["err"]
            .as_str()
            .is_some_and(|e| e.contains("call stack")),
        "{}",
        js[0]
    );
    let oxc = ast_bindings::binding_refs(
        "src/widgets/w.ts",
        &content,
        "src/services/auth.ts",
        "getAccount",
        &[],
    )
    .unwrap();
    assert_eq!(oxc.refs.len(), 1);
    let mut ts = TypeScriptFallback::new(vec![ts_from()]);
    let routed = ts
        .binding_refs(
            "src/widgets/w.ts",
            &content,
            "src/services/auth.ts",
            "getAccount",
            &[],
        )
        .await;
    assert_eq!(routed, None);
    assert_eq!(ts.report()["files"], json!(["src/widgets/w.ts"]));
}

/// A string literal holding a lone surrogate: JavaScript keeps it (and
/// `JSON.stringify` escapes it), a Rust string cannot, so the port writes
/// U+FFFD in its place.
#[test]
fn declared_difference_lone_surrogate_in_a_literal() {
    let mut g = Golden::open("declared_difference_lone_surrogate_in_a_literal");
    let src = "io.emit('\\uD800', 1);";
    let text = format!(
        "const af = await import({:?});\nconst ts = af.loadTypescript([process.env.TRIBUNAL_TS_FROM]);\nprocess.stdout.write(JSON.stringify(af.factsForFile(ts, 'x.ts', {src:?}, new Set([1]))));\n",
        format!("file://{}", plugin_root().join(ATTENTION).display())
    );
    let js = run_module(&mut g, "ast-surrogate", &text);
    g.done();
    assert!(js.contains("emits \\\"\\ud800\\\""), "{js}");
    let rust = attention_facts::facts_for_file("x.ts", src, &Added::of(&[1]));
    assert_eq!(rust[0].detail, "emits \"\u{fffd}\"");
}

/// What `text`, a module run by node from a scratch directory, printed, from
/// `g`. Kept as text: the witnesses print lone surrogates, which a JSON parser
/// refuses.
fn run_module(g: &mut Golden, name: &str, text: &str) -> String {
    let v = g.answer(&json!({"module": text}), &path_pairs(None, None), || {
        let dir = scratch(name);
        let script = dir.join("s.mjs");
        std::fs::write(&script, text).unwrap();
        let out = Command::new("node")
            .arg(&script)
            .env("TRIBUNAL_TS_FROM", ts_from())
            .output()
            .unwrap();
        json!(String::from_utf8(out.stdout).unwrap())
    });
    v.as_str().unwrap().to_owned()
}

/// writer-parity names the missing keys readers filter on by building a
/// regular expression from each key. Keys that are identifiers (with `$`,
/// the one regular-expression character an identifier holds) match as the
/// script's do; any other syntax in a string-literal key is matched as text
/// by the port, while the script compiles it, and a key that does not
/// compile (`(`) throws and drops every edge fact of the run.
#[tokio::test]
#[ignore = "needs TRIBUNAL_TS_FROM"]
async fn declared_difference_regex_syntax_in_a_key() {
    let mut g = Golden::open("declared_difference_regex_syntax_in_a_key");
    let reader = "export const r = (widget) => widget.where.aXb;\n";
    let mk = |key: &str| {
        files(&[
            ("src/services/readers.ts", reader),
            (
                "src/services/writer.ts",
                &format!("export const w = () => prisma.widget.create({{ data: {{ id, '{key}': 1 }} }});\n"),
            ),
        ])
    };
    let head = |key: &str| {
        files(&[(
            "src/services/writer.ts",
            &format!("export const w = () => prisma.widget.create({{ data: {{ id, '{key}': 1 }} }});\nexport const v = () => {{\n  prisma.widget.create({{ data: {{ id }} }});\n  queue.add(WidgetJobs.LOST, {{}});\n}};\n"),
        )])
    };
    // `.` matches the `X` in `where.aXb` in the script, not in the port.
    let f = fixture("ast-key-dot", &mk("a.b"), &head("a.b"));
    let p = pair(&mut g, &f.root, &f.diff, &f.repo, None).await;
    let detail = |v: &Value| {
        v["attention"]["facts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["kind"] == "writer-parity")
            .map(|x| x["detail"].as_str().unwrap().to_owned())
    };
    assert!(
        detail(&p.js).unwrap().ends_with("readers filter on a.b"),
        "{}",
        p.js
    );
    assert!(
        !detail(&p.rust).unwrap().contains("readers filter"),
        "{}",
        p.rust
    );
    // `(` does not compile: the script loses every edge fact, the port keeps them.
    let f = fixture("ast-key-paren", &mk("("), &head("("));
    let p = pair(&mut g, &f.root, &f.diff, &f.repo, None).await;
    g.done();
    let kinds = |v: &Value| -> Vec<String> {
        v["attention"]["facts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["kind"].as_str().unwrap().to_owned())
            .collect()
    };
    assert!(
        !kinds(&p.js)
            .iter()
            .any(|k| k == "writer-parity" || k == "job-without-handler"),
        "{}",
        p.js
    );
    assert!(
        kinds(&p.rust).iter().any(|k| k == "writer-parity"),
        "{}",
        p.rust
    );
    assert!(
        kinds(&p.rust).iter().any(|k| k == "job-without-handler"),
        "{}",
        p.rust
    );
}

/// A cut through a surrogate pair at the 100th (caller text) or 80th (test
/// title) UTF-16 unit keeps the lead surrogate in JavaScript; the port drops
/// the pair, as the word resolver's port already declares.
#[test]
fn declared_difference_cut_through_a_surrogate_pair() {
    let mut g = Golden::open("declared_difference_cut_through_a_surrogate_pair");
    let title = format!("{}\u{1F680}tail", "a".repeat(79));
    let src = format!("it('{title}', () => {{ run(); }});\n");
    let rust = attention_facts::test_facts("src/a.test.ts", &src, &Added::of(&[1]));
    assert_eq!(
        rust[0].detail,
        format!(
            "the test \"{}\" asserts nothing, so it passes whatever the code does",
            "a".repeat(79)
        )
    );
    let text = format!(
        "const af = await import({:?});\nconst ts = af.loadTypescript([process.env.TRIBUNAL_TS_FROM]);\nprocess.stdout.write(JSON.stringify(af.testFacts(ts, 'src/a.test.ts', {src:?}, new Set([1]))[0].detail));\n",
        format!("file://{}", plugin_root().join(ATTENTION).display())
    );
    let js = run_module(&mut g, "ast-cut", &text);
    g.done();
    assert!(
        js.contains(&format!("{}\\ud83d\\\"", "a".repeat(79))),
        "{js}"
    );
}

// ---------------------------------------------------------------------------
// Forms where TypeScript's tree and oxc's differ in shape

/// Importers whose syntax TypeScript and oxc shape differently: heritage
/// clauses (an expression to TypeScript, a type name to oxc), optional
/// `require`, a class constructor's key, and more.
const SHAPED: &[(&str, &str)] = &[
    (
        "src/w/heritage.ts",
        r"import * as ns from '../services/b';
import { Foo, Inner } from '../services/b';
class X implements ns.Foo {}
class Y extends Base implements ns.Foo, ns.Foo.Inner, Other<ns.Foo> {}
interface I extends ns.Foo {}
interface J extends ns.Foo.Inner, Foo {}
class Z implements y.Foo, y.Inner.Foo {}
interface K extends y.Foo {}
class W extends ns.Foo<string> implements ns.Foo<number> {}
import Q = ns.Foo;
type T = ns.Foo;
let v: typeof ns.Foo;
class V implements Foo.Inner {}
const e = class implements ns.Foo {};
",
    ),
    (
        "src/w/optional-require.js",
        "const svc = require?.('../services/b');\nsvc.Foo();\nconst { Foo } = require?.('../services/b');\nFoo();\nconst m = (require)('../services/b');\nm.Foo();\n",
    ),
    (
        "src/w/ctor.ts",
        "import { constructor, Foo } from '../services/b';\nclass X { constructor() { Foo(); } }\nclass Y { 'constructor'() {} }\nconst o = { constructor() {} };\nclass Z { static constructor() { Foo(); } }\n",
    ),
    (
        "src/w/calls.ts",
        r"import { Foo } from '../services/b';
export default Foo();
enum E { A = Foo() }
for (const x of Foo()) {}
switch (Foo()) { case Foo(): break; }
if (Foo()) {}
Foo() && x;
x[Foo()];
({ [Foo()]: 1 });
Foo()();
Foo().x;
Foo()[0];
throw Foo();
function* g() { yield Foo(); yield* Foo(); }
function h(a = Foo()) {}
const { a = Foo() } = o;
`${Foo()}`;
label: Foo();
Foo() satisfies X;
<X>Foo();
class C { x = Foo(); static y = Foo(); accessor z = Foo(); }
try { function g2() { Foo(); } } catch {}
try { class A { static { Foo(); } } } catch {}
try { const o2 = { get x() { return Foo(); }, set x(v) { Foo(); } }; } catch {}
try { class A2 { x = Foo(); } } catch {}
try { @dec(Foo()) class A3 {} } catch {}
try { (function () { Foo(); })(); } catch {}
try { lbl: { Foo(); } } catch {}
const z = Foo?.()?.then(f);
await (Foo());
const w = <T,>() => Foo();
",
    ),
    (
        "src/w/calls.tsx",
        "import { Foo } from '../services/b';\nconst w = <T,>() => Foo();\nconst j = <Foo.Bar x={Foo()} />;\nconst k = <a:Foo />;\n",
    ),
    (
        "src/w/tla.js",
        "const { Foo } = require('../services/b');\nawait Foo(1);\nFoo(2);\n",
    ),
    (
        "src/w/names.ts",
        r"import { Foo } from '../services/b';
import * as ns from '../services/b';
export { Foo as 'Foo' };
type M = { [Foo in keyof X]: Foo };
type N = X extends infer Foo ? Foo : never;
type L = [Foo: string];
function p(x): asserts Foo is string {}
@ns.Foo() class D { @Foo m(@Foo() p) {} }
declare global { function Foo(): void; }
namespace Foo.Inner { }
type I2 = import('../services/b').Foo;
let s: `a${Foo}`;
",
    ),
];

#[test]
fn binding_refs_match_the_script_on_differently_shaped_forms() {
    let mut g = Golden::open("binding_refs_match_the_script_on_differently_shaped_forms");
    let mut cases = Vec::new();
    for (file, content) in SHAPED {
        for name in ["Foo", "Inner", "constructor", "Bar"] {
            cases.push(bind_case(file, content, "src/services/b.ts", name));
        }
    }
    let js = assert_batch(&mut g, "bindingRefs (shaped)", &cases);
    g.done();
    assert!(all_refs(&js).len() > 40);
}

/// Attention facts over forms whose shape differs between the two parsers.
#[test]
fn attention_facts_match_the_script_on_differently_shaped_forms() {
    let mut g = Golden::open("attention_facts_match_the_script_on_differently_shaped_forms");
    let ctor = "export class A {\n  static constructor() {\n    if (a) return null;\n    try { return x(); } catch { return null; }\n  }\n  'constructor'() { if (a) return false; try { return 1; } catch { return false; } }\n  get v() { if (a) return []; try { return 1; } catch { return []; } }\n}\nexport function outer() {\n  const o = { get v() { if (a) return null; try { return 1; } catch { return null; } } };\n  class K { constructor() { if (a) return undefined; try { return 1; } catch { return undefined; } } }\n}\nconst o2 = { constructor() { if (a) return null; try { return 1; } catch { return null; } } };\n";
    let calls = "export default function lookup(id) {\n  try { return db.get(id); } catch { return null; }\n}\nexport function run() {\n  lookup(1);\n  lookup(2);\n  addJob(x)!;\n  addJob(x) as any;\n  addJob?.(x);\n  if (a) addJob(x);\n  else enqueueIt(x);\n  lbl: addJob(x);\n  const f = () => addJob(x);\n  io.emit('a' as const, {});\n  io.emit(<const>'b', {});\n  io?.emit(`c\\u0064`, {});\n  io.emit('e')!;\n}\n";
    let fills = "export async function save(req) {\n  let x, list = [], m;\n  try {\n    switch (req.kind) {\n      case 1: return 1;\n      case 2: throw new Error();\n    }\n    lbl: { if (a) break lbl; return 2; }\n    (x) = 1;\n    x! = 2;\n    ({ x } = req);\n    list?.push(1);\n    (list).push(2);\n    m = new Map();\n    const cb = () => { x = 3; };\n  } finally {\n    done(x, list, m);\n  }\n}\n";
    let tests = "import { vi } from 'vitest';\nvi.mock(`./billing`);\njest.mock('./billing.js');\nvi.mock('../billing/index');\nit.each([1])('x', () => {});\ntest.only('only', function () { run(); });\nit('timeout', () => {}, 1000);\nit(`tpl`, () => { run(); });\nit('rejects', async () => { await expect(p).rejects.toThrow(); });\nit('should', () => { x.should.equal(1); });\nit('assertLike', () => { assertThat (x); });\ndescribe('d', () => { it('inner', () => {}); });\nit('snap', () => { toMatchInlineSnapshot; });\n";
    let status = "export function h(req, res) {\n  res.status(403);\n  res.status(401 as const);\n  res . status(401);\n  res?.status(401);\n  res.status((401));\n  throw new Unauthorized<string>();\n}\nexport class C {\n  static constructor(res) { res.status(403); res.status(401); }\n  m(res) { res.status(403); res.status(401); }\n}\n";
    let shapes = "export function f(this: X, a?, b = 1, ...c) {}\nexport function g(a: string): void;\nexport function g(a, b) { if (a) return null; throw new Error(); }\nexport default function d(x) { return false; }\nexport const h = function (a, ...b) {};\nexport class K { static constructor(a, b) {} }\nmodule.exports.m = (a) => a;\nexports.n = async function (a, b) { return []; };\nmodule.exports = { f, k };\nfunction k(a) {}\n";
    let shapes_base = shapes
        .replace(
            "export function g(a, b) { if (a) return null; throw new Error(); }",
            "export function g(a) { throw new Error(); }",
        )
        .replace("function k(a) {}", "function k(a, b) {}");
    let mut cases = vec![
        facts_case("factsForFile", "src/ctor.ts", ctor, &all_lines(ctor)),
        facts_case("statusOrderFacts", "src/ctor.ts", ctor, &all_lines(ctor)),
        facts_case("factsForFile", "src/calls.ts", calls, &all_lines(calls)),
        facts_case("factsForFile", "src/fills.ts", fills, &all_lines(fills)),
        facts_case("testFacts", "src/billing.test.ts", tests, &all_lines(tests)),
        facts_case(
            "statusOrderFacts",
            "src/status.ts",
            status,
            &all_lines(status),
        ),
        facts_case("factsForFile", "src/status.ts", status, &all_lines(status)),
        json!({"fn": "exportedShapes", "file": "src/shapes.ts", "content": shapes}),
        json!({"fn": "contractFacts", "file": "src/shapes.ts", "base": shapes_base, "content": shapes}),
        json!({"fn": "contractFacts", "file": "src/shapes.ts", "base": shapes, "content": shapes_base}),
    ];
    for f in ["src/ctor.ts", "src/calls.ts", "src/fills.ts"] {
        let content = cases.iter().find(|c| c["file"] == f).unwrap()["content"].clone();
        cases.push(json!({"fn": "exportedShapes", "file": f, "content": content}));
    }
    let js = assert_batch(&mut g, "attention facts (shaped)", &cases);
    g.done();
    assert_valid("attention facts (shaped)", &cases, &js);
    assert!(fact_counts(&js).len() >= 5, "{:?}", fact_counts(&js));
}

/// Grammar ambiguities each parser resolves on its own: generic calls,
/// arrows in a conditional, ASI around contextual keywords.
const AMBIGUOUS: &[(&str, &str)] = &[
    ("a.ts", "const r = a ? (b) : (c) => Foo(c);"),
    ("a.ts", "const r = a ? (b) : c => Foo(c);"),
    ("a.ts", "const r = a ? (b): T => Foo(b) : d;"),
    ("a.ts", "const r = a ? (b) : (c): T => Foo(c);"),
    ("a.ts", "const r = a ? b ? (c) : (d) => Foo(d) : e;"),
    ("a.ts", "const r = Foo < T > (x);"),
    ("a.ts", "const r = Foo < a, b > (c);"),
    ("a.ts", "const r = Foo < T >\n(x);"),
    ("a.ts", "const r = (Foo < 1) > (2);"),
    ("a.ts", "const r = Foo < a > b;"),
    ("a.ts", "const r = Foo<T>;\nFoo<T>\n(x);"),
    ("a.ts", "const r = new Foo<T>;"),
    ("a.ts", "const r = async (x) => Foo(x);\nconst s = async(Foo(1));"),
    ("a.ts", "const r = <T>(x: T) => Foo(x);"),
    ("a.ts", "let type = 1; type\nFoo = 1;\nFoo();"),
    ("a.ts", "declare\nconst x = Foo();"),
    ("a.ts", "abstract\nclass X { m() { return Foo(); } }"),
    ("a.ts", "namespace\nFoo\n{}"),
    ("a.ts", "x\n++Foo;"),
    ("a.ts", "const o = { get: Foo(), set() { Foo(); }, async, await: Foo(), static: 1 };"),
    ("a.ts", "class A { static async *[Foo()]() {} get = Foo(); set; static = 1; async\nm() { return Foo(); } }"),
    ("a.ts", "function f() {} /Foo/.test(x); Foo();"),
    ("a.ts", "const r = a as any as typeof Foo;\nfor (const x of Foo() as any) {}"),
    ("a.ts", "const r = Foo!\n(x);"),
    ("a.ts", "let x = y < Foo > (1);"),
    ("a.ts", "f(a < b, c > (Foo()));"),
    ("a.ts", "f(a < b, c >= (Foo()));"),
    ("a.ts", "const r = a?.b < c > (Foo());"),
    ("a.ts", "const r = Foo?.<T>(x);"),
    ("a.ts", "label: function g() { return Foo(); }"),
    ("a.ts", "if (a) function g() { Foo(); }"),
    ("a.ts", "const r = a < b > c ? Foo() : 1;"),
    ("a.ts", "type A = typeof Foo<string>;\nconst b = Foo<string>;"),
    ("a.ts", "enum E { A = Foo(), B = A << 1 }"),
    ("a.tsx", "const r = <T,>(x: T) => Foo(x);\nconst s = <T extends {}>(x: T) => Foo(x);"),
    ("a.tsx", "const r = <div>{/* Foo */}{Foo()}</div>;\nconst f = <>{Foo}</>;"),
    ("a.tsx", "const r = a ? <A /> : (b) => Foo(b);"),
    ("a.js", "var yield = 1, let = 2; Foo(yield, let);"),
    ("a.js", "const r = a ? (b) : (c) => Foo(c);"),
    ("a.js", "const r = Foo < T > (x);"),
    ("a.js", "label: { Foo(); }\nlet\n[x] = Foo();"),
    ("a.js", "async function f() { for await (const x of Foo()) {} }"),
    ("a.mjs", "const x = await Foo();\nexport { x };"),
    ("a.cjs", "'use strict';\nFoo();\nreturn Foo();"),
    ("a.d.ts", "declare function g(): typeof Foo;\nexport function h() { return Foo(); }\ndeclare const k: ReturnType<typeof Foo>;"),
    ("a.d.ts", "export declare namespace N { const x: typeof Foo; }\nexport = Foo;"),
    ("a.ts", "import data from './d.json' with { type: 'json' };\nFoo(data);"),
    ("a.ts", "import data from './d.json' assert { type: 'json' };\nFoo(data);"),
    ("a.ts", "import defer * as lazy from '../services/b';\nlazy.Foo();"),
    ("a.ts", "module M { Foo(); }"),
    ("a.ts", "class A { declare x: typeof Foo; override m(): void { Foo(); } }"),
    ("a.ts", "let x!: number; const y = <const>[Foo()];"),
    ("a.ts", "function f<const T, in out U>(this: Window, a?: T): asserts a is T { Foo(); }"),
    ("a.ts", "type U = unique symbol; abstract class B { abstract m(): void; protected constructor(private readonly f = Foo()) { super(); } }"),
];

#[test]
fn binding_refs_match_the_script_on_grammar_ambiguities() {
    let mut g = Golden::open("binding_refs_match_the_script_on_grammar_ambiguities");
    let cases: Vec<Value> = AMBIGUOUS
        .iter()
        .map(|(ext, body)| {
            let file = format!("src/w/{ext}");
            let import = if ext.ends_with(".js") || ext.ends_with(".cjs") {
                "const { Foo } = require('../services/b');\n"
            } else {
                "import { Foo } from '../services/b';\n"
            };
            bind_case(
                &file,
                &format!("{import}{body}\n"),
                "src/services/b.ts",
                "Foo",
            )
        })
        .collect();
    let js = assert_batch(&mut g, "bindingRefs (ambiguities)", &cases);
    g.done();
    assert!(all_refs(&js).len() > 40);
}

/// A long file whose brackets are shallow but whose regular expressions and
/// JSX text hold quotes and brackets: the nesting guard must not read those
/// as unclosed brackets, or the port drops a file TypeScript parses.
#[test]
fn shallow_files_with_quotes_in_regexes_and_text_are_parsed() {
    let mut g = Golden::open("shallow_files_with_quotes_in_regexes_and_text_are_parsed");
    let lines = |l: &str| l.repeat(1100);
    let bodies = [
        ("src/w/quotes.ts", lines("s = s.replace(/'/g, '');\n")),
        ("src/w/dquotes.ts", lines("if (/[\"']/.test(s)) { n++; }\n")),
        ("src/w/parens.ts", lines("s = s.replace(/\\(/g, '');\n")),
        ("src/w/class.ts", lines("s = s.replace(/[([{]/g, '');\n")),
        (
            "src/w/urls.ts",
            lines("u = u.replace(/^https?:\\/\\//, '');\n"),
        ),
        (
            "src/w/text.tsx",
            format!(
                "const V = () => (\n  <div>\n{}  </div>\n);\n",
                lines("    <p>(you can't)</p>\n")
            ),
        ),
    ];
    let cases: Vec<Value> = bodies
        .iter()
        .map(|(file, body)| {
            bind_case(
                file,
                &format!("import {{ Foo }} from '../services/b';\n{body}Foo();\n"),
                "src/services/b.ts",
                "Foo",
            )
        })
        .collect();
    let js = assert_batch(&mut g, "bindingRefs (shallow, quoted)", &cases);
    g.done();
    assert_valid("bindingRefs (shallow, quoted)", &cases, &js);
    assert!(js
        .iter()
        .all(|r| r["ok"]["refs"].as_array().is_some_and(|a| a.len() == 1)));
}

// ---------------------------------------------------------------------------
// A real repository as the corpus

/// The source files of `TRIBUNAL_AST_CORPUS`, or of the repo
/// `TRIBUNAL_TS_FROM` names: real code, with the forms generators miss.
fn corpus_files() -> (PathBuf, Vec<String>) {
    let root = std::env::var_os("TRIBUNAL_AST_CORPUS")
        .filter(|d| !d.is_empty())
        .map_or_else(ts_from, PathBuf::from);
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            if p.is_dir() {
                if !name.starts_with('.')
                    && !["node_modules", "dist", "build", "coverage", "target"]
                        .contains(&name.as_str())
                {
                    stack.push(p);
                }
            } else if [".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"]
                .iter()
                .any(|e| name.ends_with(e))
                && std::fs::metadata(&p).is_ok_and(|m| m.len() < 400_000)
            {
                out.push(
                    p.strip_prefix(&root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    out.sort();
    (root, out)
}

fn idents(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_alphanumeric() || c == '_' || c == '$' {
            cur.push(c);
        } else if !cur.is_empty() {
            if !cur.starts_with(|d: char| d.is_ascii_digit()) {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
        }
    }
    out
}

/// The char boundary at or before `i`, within `s`.
fn boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// `(spec, names)` for each import or require in `src`: the names written in
/// the statement, and the properties read through a namespace or module
/// binding.
fn import_names(src: &str) -> Vec<(String, Vec<String>)> {
    const SKIP: &[&str] = &[
        "import", "from", "as", "type", "const", "let", "var", "require", "default", "export",
        "await", "typeof",
    ];
    let mut out = Vec::new();
    for marker in ["from '", "from \"", "require('", "require(\""] {
        let mut at = 0;
        while let Some(k) = src[at..].find(marker) {
            let s = at + k + marker.len();
            let q = marker.chars().last().unwrap();
            let Some(e) = src[s..].find(q) else { break };
            let spec = src[s..s + e].to_owned();
            at = s + e;
            if !(spec.starts_with('.') || spec.starts_with('@')) {
                continue;
            }
            let back = boundary(
                src,
                src[..s - marker.len()]
                    .rfind(';')
                    .map_or(0, |p| p + 1)
                    .max((s - marker.len()).saturating_sub(400)),
            );
            let mut names: Vec<String> = Vec::new();
            for w in idents(&src[back..s - marker.len()]) {
                if !SKIP.contains(&w.as_str()) && !names.contains(&w) {
                    names.push(w);
                }
            }
            let bound: Vec<String> = names.clone();
            for b in bound {
                let dot = format!("{b}.");
                let mut from = 0;
                while let Some(p) = src[from..].find(&dot) {
                    let w = idents(
                        &src[from + p + dot.len()..boundary(src, from + p + dot.len() + 60)],
                    );
                    if let Some(w) = w.first() {
                        if !names.contains(w) && names.len() < 10 {
                            names.push(w.clone());
                        }
                    }
                    from += p + dot.len();
                    if names.len() >= 10 {
                        break;
                    }
                }
            }
            names.truncate(10);
            out.push((spec, names));
        }
    }
    out
}

/// Every parser-backed function of both scripts over a real repository's
/// files: each file's facts with every line added, its exported shapes, the
/// promise verdict of each declaration, and the bindings of each import.
/// The corpus is whatever repository the run names, so the scripts are asked
/// every time: no golden holds it, and one would copy its code.
#[test]
#[ignore = "needs TRIBUNAL_TS_FROM and the scripts"]
fn every_function_matches_the_script_on_a_real_repository() {
    let mut g = Golden::live();
    let (root, files) = corpus_files();
    assert!(
        files.len() > 20,
        "too few source files under {}",
        root.display()
    );
    let tsconfig = std::fs::read_to_string(root.join("tsconfig.json")).ok();
    let aliases: serde_json::Map<String, Value> = blast_radius::read_aliases(tsconfig.as_deref())
        .into_iter()
        .map(|(k, v)| (k, json!(v)))
        .collect();
    let mut cases: Vec<Value> = Vec::new();
    for f in &files {
        let Ok(content) = std::fs::read_to_string(root.join(f)) else {
            continue;
        };
        let all = all_lines(&content);
        if blast_radius::is_test(f) {
            cases.push(json!({"fn": "testFacts", "file": f, "content": content, "added": all}));
        } else {
            cases.push(json!({"fn": "factsForFile", "file": f, "content": content, "added": all}));
            cases.push(
                json!({"fn": "statusOrderFacts", "file": f, "content": content, "added": all}),
            );
            cases.push(json!({"fn": "exportedShapes", "file": f, "content": content}));
        }
        for d in blast_radius::declarations(&content) {
            cases.push(
                json!({"fn": "returnsPromise", "file": f, "content": content, "name": d.name}),
            );
        }
        for (spec, names) in import_names(&content) {
            let target = if spec.starts_with('.') {
                let dir = Path::new(f).parent().unwrap_or(Path::new(""));
                let mut parts: Vec<String> = Vec::new();
                for seg in dir.join(&spec).components() {
                    match seg {
                        std::path::Component::ParentDir => {
                            parts.pop();
                        }
                        std::path::Component::Normal(s) => {
                            parts.push(s.to_string_lossy().into_owned())
                        }
                        _ => {}
                    }
                }
                format!("{}.ts", parts.join("/"))
            } else {
                aliases
                    .iter()
                    .find_map(|(k, v)| {
                        let v = v.as_str().unwrap();
                        k.strip_suffix('*')
                            .and_then(|p| spec.strip_prefix(p))
                            .map(|rest| format!("{}{rest}.ts", v.trim_end_matches('*')))
                    })
                    .unwrap_or_else(|| format!("{spec}.ts"))
            };
            for name in names {
                cases.push(json!({
                    "fn": "bindingRefs", "file": f, "content": content,
                    "target": target, "name": name, "aliases": aliases,
                }));
            }
        }
    }
    let js = js_batch(&mut g, &cases);
    assert_eq!(js.len(), cases.len());
    let mut bad = Vec::new();
    let mut parsed_clean = 0usize;
    for (c, j) in cases.iter().zip(&js) {
        if j["diag"].as_array().is_some_and(Vec::is_empty) {
            parsed_clean += 1;
        }
        let r = rust_case(c);
        if r["ok"] != j["ok"] {
            // A file oxc does not parse cleanly goes to TypeScript in the
            // port (`typescript_fallback`); this compares oxc's own answers.
            let file = c["file"].as_str().unwrap_or("");
            let routed = ["content", "base"].iter().any(|k| {
                c[*k]
                    .as_str()
                    .is_some_and(|t| typescript_fallback::needs_typescript(file, t))
            });
            if routed {
                continue;
            }
            bad.push(format!(
                "{} {} name={} target={}\n--- script ---\n{}\n--- port ---\n{}",
                c["fn"], c["file"], c["name"], c["target"], j["ok"], r["ok"]
            ));
        }
    }
    eprintln!(
        "{} cases over {} files ({} parsed without errors by typescript)",
        cases.len(),
        files.len(),
        parsed_clean
    );
    assert!(
        bad.is_empty(),
        "{} of {} cases differ\n{}",
        bad.len(),
        cases.len(),
        bad.iter().take(6).cloned().collect::<Vec<_>>().join("\n\n")
    );
    // The corpus is only a test if it reached references and facts.
    assert!(
        all_refs(&js).len() > 100,
        "{} references",
        all_refs(&js).len()
    );
    assert!(!fact_counts(&js).is_empty());
    eprintln!(
        "{} references, facts {:?}",
        all_refs(&js).len(),
        fact_counts(&js)
    );
}

/// The whole run, both resolvers' halves and every attention fact, on the
/// corpus repository's own recent commits, each at its revision: real diffs
/// over real code. Read-only: `git log`, `diff`, `show`, `grep`, `cat-file`.
/// Each commit's git calls are logged under the scratch directory. Like the
/// test above, it asks the scripts every time.
#[tokio::test]
#[ignore = "needs TRIBUNAL_TS_FROM and the scripts"]
async fn blast_radius_matches_the_script_on_the_corpus_history() {
    let mut g = Golden::live();
    let (repo, _) = corpus_files();
    // `TRIBUNAL_AST_HISTORY` widens the sample for an exploration run.
    let n = std::env::var("TRIBUNAL_AST_HISTORY").unwrap_or_else(|_| "8".into());
    let shas = git(
        &repo,
        &[
            "log",
            "--no-merges",
            "-n",
            &n,
            "--format=%H",
            "--",
            "*.ts",
            "*.tsx",
            "*.js",
            "*.jsx",
        ],
    );
    let shas: Vec<&str> = shas.lines().filter(|l| !l.is_empty()).collect();
    assert!(shas.len() >= 4, "{} has too few commits", repo.display());
    let mut callers = 0;
    let mut facts = 0;
    for sha in shas {
        let root = scratch(&format!("ast-history-{}", &sha[..8]));
        let diff = root.join("pr.diff");
        std::fs::write(&diff, git(&repo, &["diff", &format!("{sha}^"), sha])).unwrap();
        let p = pair(&mut g, &root, &diff, &repo, Some(sha)).await;
        let js = assert_same(&format!("commit {sha}"), &p);
        callers += js["callers"].as_array().map_or(0, Vec::len);
        facts += js["attention"]["facts"].as_array().map_or(0, Vec::len);
    }
    assert!(callers + facts > 0, "no caller and no fact on any commit");
    eprintln!("{callers} callers, {facts} facts");
}

// ---------------------------------------------------------------------------
// Code oxc cannot parse cleanly: the reviewed repo's `typescript` answers

const JUNK: &[&str] = &[
    "const = ;",
    "@@",
    "function (",
    "if (x {",
    "} }",
    "let x = <",
    "a ? : b;",
    "import {",
    "'unterminated",
    "x: number;",
    "=> =>",
    "enum",
    "const o = { async\n m() {} };",
    "f(a b);",
];

/// `src` with one syntax error put in, at a place `rng` picks.
fn mutate(rng: &mut Rng, src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let lines: Vec<&str> = src.split('\n').collect();
    match rng.below(5) {
        0 => {
            let closers: Vec<usize> = (0..chars.len())
                .filter(|&i| matches!(chars[i], ')' | ']' | '}'))
                .collect();
            if closers.is_empty() {
                return format!("{src}\n(");
            }
            let at = *rng.pick(&closers);
            chars[..at].iter().chain(&chars[at + 1..]).collect()
        }
        1 => {
            let at = rng.below(lines.len() + 1);
            let mut v: Vec<&str> = lines.clone();
            v.insert(at, rng.pick(JUNK));
            v.join("\n")
        }
        2 => chars[..rng.below(chars.len().max(1))].iter().collect(),
        3 => {
            let mut v = lines.clone();
            if v.len() > 1 {
                v.remove(rng.below(v.len()));
            }
            v.join("\n")
        }
        _ => {
            let openers: Vec<usize> = (0..chars.len())
                .filter(|&i| matches!(chars[i], '(' | '{'))
                .collect();
            if openers.is_empty() {
                return format!("{src}\n{{");
            }
            let at = *rng.pick(&openers);
            let mut v = chars.clone();
            v.insert(at, chars[at]);
            v.into_iter().collect()
        }
    }
}

fn aliases_of(c: &Value) -> Vec<(String, String)> {
    c["aliases"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

fn bound_json(b: Option<ast_bindings::Bound>) -> Value {
    b.map_or(Value::Null, |b| {
        json!({
            "refs": b.refs.iter().map(|r| json!({
                "line": r.line, "call": r.call, "awaited": r.awaited, "inTry": r.in_try, "text": r.text,
            })).collect::<Vec<_>>(),
            "reexports": b.reexports,
        })
    })
}

fn facts_or_threw(v: Option<Vec<attention_facts::Fact>>) -> Value {
    v.map_or(json!("threw"), fact_list)
}

/// The script's answer as the port's routed steps see it: a throw is a
/// dropped step, which for `bindingRefs` is the same as binding nothing.
fn script_answer(c: &Value, j: &Value) -> Value {
    if j.get("err").is_some() {
        return if c["fn"] == "bindingRefs" {
            Value::Null
        } else {
            json!("threw")
        };
    }
    j["ok"].clone()
}

async fn routed_case(ts: &mut TypeScriptFallback, c: &Value) -> Value {
    let s = |k: &str| c[k].as_str().unwrap_or("").to_owned();
    match s("fn").as_str() {
        "bindingRefs" => bound_json(
            ts.binding_refs(
                &s("file"),
                &s("content"),
                &s("target"),
                &s("name"),
                &aliases_of(c),
            )
            .await,
        ),
        "returnsPromise" => ts
            .returns_promise(&s("file"), &s("content"), &s("name"))
            .await
            .map_or(json!("threw"), |v| json!(v)),
        "ownFacts" => facts_or_threw(ts.own_facts(&s("file"), &s("content"), &added_of(c)).await),
        "testFacts" => facts_or_threw(ts.test_facts(&s("file"), &s("content"), &added_of(c)).await),
        "contractFacts" => facts_or_threw(
            ts.contract_facts(&s("file"), &s("base"), &s("content"))
                .await,
        ),
        other => panic!("unknown fn {other}"),
    }
}

/// The port's answer with oxc alone, shaped as [`routed_case`] shapes it.
fn oxc_case(c: &Value) -> Value {
    let s = |k: &str| c[k].as_str().unwrap_or("");
    match s("fn") {
        "bindingRefs" => bound_json(
            ast_bindings::guarded(|| {
                ast_bindings::binding_refs(
                    s("file"),
                    s("content"),
                    s("target"),
                    s("name"),
                    &aliases_of(c),
                )
            })
            .flatten(),
        ),
        "returnsPromise" => ast_bindings::guarded(|| {
            ast_bindings::returns_promise(s("file"), s("content"), s("name"))
        })
        .map_or(json!("threw"), |v| json!(v)),
        "ownFacts" => facts_or_threw(ast_bindings::guarded(|| {
            let mut v = attention_facts::facts_for_file(s("file"), s("content"), &added_of(c));
            v.extend(attention_facts::status_order_facts(
                s("file"),
                s("content"),
                &added_of(c),
            ));
            v
        })),
        "testFacts" => facts_or_threw(ast_bindings::guarded(|| {
            attention_facts::test_facts(s("file"), s("content"), &added_of(c))
        })),
        "contractFacts" => facts_or_threw(ast_bindings::guarded(|| {
            attention_facts::contract_facts(s("file"), s("base"), s("content"))
        })),
        other => panic!("unknown fn {other}"),
    }
}

/// Generated sources with one syntax error each, through every per-file
/// step: the routed answer is the script's on every case, and the case set
/// is one oxc alone gets wrong often, so it can tell the routing is there.
#[tokio::test]
#[ignore = "needs TRIBUNAL_TS_FROM"]
async fn every_step_matches_the_script_on_code_with_syntax_errors() {
    let mut g = Golden::open("every_step_matches_the_script_on_code_with_syntax_errors");
    let mut rng = Rng::new(seed(0xB20C));
    let mut cases = Vec::new();
    for _ in 0..250 {
        let (file, src) = gen_importer(&mut rng);
        let src = mutate(&mut rng, &src);
        cases.push(bind_case(&file, &src, "src/services/auth.ts", "getAccount"));
        let (file, src) = gen_promise_decl(&mut rng);
        let src = mutate(&mut rng, &src);
        cases.push(json!({"fn": "returnsPromise", "file": file, "content": src, "name": "f"}));
        let (file, src) = gen_fact_file(&mut rng);
        let added = added_subset(&mut rng, &src);
        let src = mutate(&mut rng, &src);
        cases.push(facts_case("ownFacts", &file, &src, &added));
        let (file, src) = gen_test_file(&mut rng);
        let added = added_subset(&mut rng, &src);
        let src = mutate(&mut rng, &src);
        cases.push(facts_case("testFacts", &file, &src, &added));
        let ext = *rng.pick(&[".ts", ".js", ".cjs"]);
        let file = format!("src/helpers/h{ext}");
        let names = ["find", "sum", "opt"];
        let mut base: String = names
            .iter()
            .map(|n| gen_contract_fn(&mut rng, n, ext))
            .collect();
        let mut head: String = names
            .iter()
            .map(|n| gen_contract_fn(&mut rng, n, ext))
            .collect();
        if rng.chance(50) {
            head = mutate(&mut rng, &head);
        } else {
            base = mutate(&mut rng, &base);
        }
        cases.push(json!({"fn": "contractFacts", "file": file, "base": base, "content": head}));
    }
    let js = js_batch(&mut g, &cases);
    g.done();
    assert_eq!(js.len(), cases.len());
    let mut ts = TypeScriptFallback::new(vec![ts_from()]);
    let (mut bad, mut routed, mut oxc_wrong, mut ts_diag, mut ts_only) = (Vec::new(), 0, 0, 0, 0);
    for (c, j) in cases.iter().zip(&js) {
        let want = script_answer(c, j);
        let file = c["file"].as_str().unwrap();
        let needs = typescript_fallback::needs_typescript(file, c["content"].as_str().unwrap())
            || c["base"]
                .as_str()
                .is_some_and(|b| typescript_fallback::needs_typescript(file, b));
        routed += usize::from(needs);
        let diag = j["diag"].as_array().is_some_and(|d| !d.is_empty());
        ts_diag += usize::from(diag);
        // TypeScript reports an error oxc does not: these stay on oxc.
        ts_only += usize::from(diag && !needs);
        oxc_wrong += usize::from(oxc_case(c) != want);
        let got = routed_case(&mut ts, c).await;
        if got != want {
            bad.push(format!(
                "{} {file} (routed to typescript: {needs})\n--- source ---\n{}\n--- script ---\n{want}\n--- port ---\n{got}",
                c["fn"], c["content"]
            ));
        }
    }
    eprintln!("{}", ts.report());
    eprintln!(
        "{} cases: {routed} routed to typescript, {ts_diag} with typescript parse errors ({ts_only} of them parsed cleanly by oxc), {oxc_wrong} wrong with oxc alone",
        cases.len()
    );
    assert!(
        bad.is_empty(),
        "{} of {} cases differ:\n{}",
        bad.len(),
        cases.len(),
        bad.iter().take(3).cloned().collect::<Vec<_>>().join("\n\n")
    );
    assert_eq!(ts.report()["unresolved"], json!([]));
    assert!(routed * 10 >= cases.len() * 7, "only {routed} cases routed");
    assert!(
        oxc_wrong >= 100,
        "only {oxc_wrong} cases oxc alone gets wrong"
    );
}

/// `files` with `junk` appended to `file`.
fn break_file(files: &mut Files, file: &str, junk: &str) {
    let entry = files
        .iter_mut()
        .find(|(f, _)| f == file)
        .unwrap_or_else(|| panic!("no {file}"));
    let text = entry.1.as_deref().unwrap();
    entry.1 = Some(format!("{text}\n{junk}\n"));
}

/// The realistic repo with a syntax error in an importer, TypeScript syntax
/// in a `.js` importer, and errors in the changed service, controller and
/// test. With a `typescript` package, every step on those files is the
/// script's; without one, the port keeps oxc's answers, which differ, and
/// says so.
#[tokio::test]
#[ignore = "needs TRIBUNAL_TS_FROM"]
async fn blast_radius_matches_the_script_on_a_repo_with_syntax_errors() {
    let mut g = Golden::open("blast_radius_matches_the_script_on_a_repo_with_syntax_errors");
    let mut base = realistic_base();
    break_file(
        &mut base,
        "src/controllers/widgets.controller.ts",
        "const = ;",
    );
    break_file(
        &mut base,
        "src/workers/consumer.js",
        "function typed(x: number) { return x; }",
    );
    let mut head = realistic_head();
    break_file(&mut head, "src/services/queue.ts", "export const half = (;");
    break_file(
        &mut head,
        "src/services/billing.test.ts",
        "it('open', () => {",
    );
    break_file(&mut head, "src/controllers/acme.controller.ts", "@@");
    let f = fixture("ast-broken", &base, &head);
    let p = pair(&mut g, &f.root, &f.diff, &f.repo, None).await;
    let out = assert_same_resolving(
        "syntax errors",
        &p,
        Some(&[
            "src/controllers/acme.controller.ts",
            "src/controllers/widgets.controller.ts",
            "src/services/billing.test.ts",
            "src/services/queue.ts",
            "src/workers/consumer.js",
        ]),
    );
    assert!(!out["callers"].as_array().unwrap().is_empty(), "{out}");

    let q = pair_with(&mut g, &f.root, &f.diff, &f.repo, None, false).await;
    g.done();
    let fallback = &q.rust["typescriptFallback"];
    assert_eq!(fallback["typescript"], Value::Null, "{fallback}");
    assert_eq!(fallback["files"], json!([]), "{fallback}");
    assert_eq!(
        fallback["unresolved"],
        p.rust["typescriptFallback"]["files"]
    );
    let mut oxc = q.rust.clone();
    oxc.as_object_mut().unwrap().remove("typescriptFallback");
    oxc["attention"]["parser"] = q.js["attention"]["parser"].clone();
    assert_ne!(
        oxc, q.js,
        "oxc alone already matched the script on this repo"
    );
}

/// `resolve_typescript` finds the file `require("typescript")` loads, from
/// the repo and from a directory under it, and nothing where node finds
/// nothing.
#[test]
#[ignore = "needs TRIBUNAL_TS_FROM"]
fn typescript_resolves_where_node_resolves_it() {
    let ts = ts_from();
    let nowhere = scratch("ts-nowhere");
    for dir in [
        ts.clone(),
        ts.join("src"),
        ts.join("node_modules"),
        nowhere.clone(),
    ] {
        let node = Command::new("node")
            .arg("-e")
            .arg("try { process.stdout.write(require('node:fs').realpathSync(require.resolve('typescript', { paths: [process.argv[1]] }))) } catch { }")
            .arg(&dir)
            .env_remove("NODE_PATH")
            .output()
            .unwrap();
        let node = String::from_utf8(node.stdout).unwrap();
        let port = typescript_fallback::resolve_typescript(&dir)
            .map(|p| std::fs::canonicalize(p).unwrap().display().to_string())
            .unwrap_or_default();
        assert_eq!(port, node, "{}", dir.display());
    }
    let _ = std::fs::remove_dir_all(&nowhere);
}

/// The engine's stack is sized from one construct (`[`); TypeScript's
/// recursion is the same code under both engines, so the other constructs run
/// out within a few percent of where they do under a fresh node.
#[tokio::test]
#[ignore = "needs TRIBUNAL_TS_FROM"]
async fn the_engine_stack_runs_out_where_node_does() {
    use tribunal_context::js_engine::{JsEngine, Setup, Stack};
    const KINDS: &str = r#"globalThis.mk = {
  arr: (n) => "const a = " + "[".repeat(n) + "1" + "]".repeat(n) + ";",
  paren: (n) => "const a = " + "(".repeat(n) + "1" + ")".repeat(n) + ";",
  obj: (n) => "const a = " + "{a:".repeat(n) + "1" + "}".repeat(n) + ";",
  block: (n) => "function f(){" + "{".repeat(n) + "}".repeat(n) + "}",
  call: (n) => "const a = " + "f(".repeat(n) + "1" + ")".repeat(n) + ";",
};
globalThis.ok = (k, n) => { try { ts.createSourceFile("d.ts", mk[k](n), ts.ScriptTarget.Latest, true, ts.ScriptKind.TS); return true; } catch { return false; } };
"#;
    let ts = ts_from();
    let entry = typescript_fallback::resolve_typescript(&ts).unwrap();
    // A fresh node per probe, as the script runs.
    let node_ok = |k: &str, n: u32| {
        let script = format!(
            "const ts = require({:?});\n{KINDS}\nprocess.exit(ok({k:?}, {n}) ? 0 : 1);",
            entry.display().to_string()
        );
        Command::new("node")
            .args(["-e", &script])
            .status()
            .unwrap()
            .success()
    };
    let setup = Setup {
        scripts: vec![("ts".into(), std::fs::read_to_string(&entry).unwrap())],
        modules: Vec::new(),
        entry: (
            "entry.mjs".into(),
            format!("{KINDS}\nglobalThis.probe = (n) => ok('arr', n);\nglobalThis.__call = (l) => {{ const [k, n] = l.split(':'); return String(ok(k, +n)); }};"),
        ),
        hello: "ts.version".into(),
        call: "__call".into(),
        stack: Stack::Calibrated {
            probe: "probe".into(),
            target: typescript_fallback::V8_ARRAY_DEPTH,
        },
    };
    let (engine, _) = JsEngine::start(setup).await.unwrap();
    let mut rows = Vec::new();
    for k in ["arr", "paren", "obj", "block", "call"] {
        let (mut lo, mut hi) = (10u32, 5000u32);
        while hi - lo > 2 {
            let mid = (lo + hi) / 2;
            if node_ok(k, mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let node = hi;
        let (mut lo, mut hi) = (10u32, 5000u32);
        while hi - lo > 2 {
            let mid = (lo + hi) / 2;
            if engine.call(format!("{k}:{mid}")).await.unwrap() == "true" {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        rows.push((k, node, hi));
    }
    eprintln!("(construct, node, engine): {rows:?}");
    for (k, node, engine) in rows {
        let off = (f64::from(engine) - f64::from(node)).abs() / f64::from(node);
        assert!(off < 0.08, "{k}: node {node}, engine {engine}");
    }
}

/// The fallback answers with no `node` anywhere on PATH: the test reruns
/// itself in a child with an empty PATH, where the inner half routes broken
/// code to TypeScript and checks it got TypeScript's recovery.
#[test]
#[ignore = "needs TRIBUNAL_TS_FROM"]
fn the_fallback_runs_without_node() {
    if std::env::var_os("TRIBUNAL_INNER_NO_NODE").is_some() {
        assert!(
            Command::new("node").arg("-v").output().is_err(),
            "node is still reachable"
        );
        let rt = tokio::runtime::Runtime::new().unwrap();
        // `ts_from` asks node, which is gone here.
        let dir = PathBuf::from(std::env::var_os("TRIBUNAL_TS_FROM").unwrap());
        let mut ts = TypeScriptFallback::new(vec![dir]);
        let content = "import { getAccount } from '../services/auth';\ngetAccount(1);\nconst = ;\n";
        let routed = rt.block_on(ts.binding_refs(
            "src/widgets/w.ts",
            content,
            "src/services/auth.ts",
            "getAccount",
            &[],
        ));
        let report = ts.report();
        assert_eq!(report["files"], json!(["src/widgets/w.ts"]), "{report}");
        let want: Value =
            serde_json::from_str(&std::env::var("TRIBUNAL_INNER_EXPECT").unwrap()).unwrap();
        assert!(!want.is_null());
        assert_eq!(bound_json(routed), want, "{report}");
        println!("INNER-NO-NODE-OK {}", report["typescript"]);
        return;
    }
    let content = "import { getAccount } from '../services/auth';\ngetAccount(1);\nconst = ;\n";
    let mut g = Golden::open("the_fallback_runs_without_node");
    let want = js_batch(
        &mut g,
        &[bind_case(
            "src/widgets/w.ts",
            content,
            "src/services/auth.ts",
            "getAccount",
        )],
    )[0]["ok"]
        .to_string();
    g.done();
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "the_fallback_runs_without_node",
            "--include-ignored",
            "--nocapture",
        ])
        .env("PATH", "")
        .env("TRIBUNAL_INNER_NO_NODE", "1")
        .env("TRIBUNAL_INNER_EXPECT", want)
        .env("TRIBUNAL_TS_FROM", ts_from())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("INNER-NO-NODE-OK"),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
