//! `lib/reconcile-votes.mjs` against `tribunal_context::reconcile_votes`:
//! the same vote directory, arguments and stdin on both sides, compared on
//! stdout byte for byte. The script classifies with `bash
//! lib/classify-verdict.sh` under `LC_ALL=en_US.UTF-8`; the port runs the
//! classifier in process. A classifier that cannot run is stood in on both
//! sides: `TRIBUNAL_CLASSIFIER` naming a missing file for the script, a
//! classifier returning the same error text for the port.
//!
//! The script's side is read from `tests/goldens/reconcile-votes.json` (the
//! generated cases) and `tests/goldens/reconcile-votes-witnesses.json` (the
//! declared differences), so a run starts only the port. Each keeps the text
//! of both scripts, and the cases file a SHA-256 of each case's input; the
//! scratch directories and the plugin are stored as placeholders. A script or
//! generator edited without a regeneration fails here; a script no longer on
//! disk has been retired and its golden is final. Regenerate after changing
//! either: `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test
//! diff_reconcile_votes`.
mod common;
#[path = "../../tribunal-core/tests/common/mod.rs"]
mod core_common;

use common::{plugin_root, scratch};
use core_common::{assert_same, sample};

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use proptest::prelude::*;
use serde_json::{json, Map, Value};
use tribunal_context::harness_version::sha256_hex;
use tribunal_context::reconcile_votes::{classify_in_process, reconcile, run, Printed};

const SCRIPT: &str = "lib/reconcile-votes.mjs";
/// What the cases ran: the script, then the classifier it shells out to.
const SCRIPTS: [&str; 2] = [SCRIPT, "lib/classify-verdict.sh"];
const MISSING_CLASSIFIER: &str = "/nonexistent/tribunal/classify-verdict.sh";
const GOLDEN: &str = "reconcile-votes.json";
const WITNESS_GOLDEN: &str = "reconcile-votes-witnesses.json";
const REGENERATE: &str =
    "regenerate: TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_reconcile_votes";

// ---------------------------------------------------------------- goldens

fn regenerating() -> bool {
    std::env::var("TRIBUNAL_REGEN_GOLDENS").is_ok_and(|v| v == "1")
}

fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens")
        .join(name)
}

fn script_texts() -> Value {
    let texts: BTreeMap<&str, String> = SCRIPTS
        .iter()
        .map(|s| (*s, std::fs::read_to_string(plugin_root().join(s)).unwrap()))
        .collect();
    json!(texts)
}

/// The scripts in `recorded` (`{path: text}`) or `scripts` whose text under
/// `root` is not what was recorded. A script no longer on disk was retired
/// with the Workflow and its golden is final, so it is skipped; when
/// `scripts[0]`, the one the cases run, is gone, the rest are only what it
/// read and are skipped too.
fn stale_scripts(recorded: &Value, scripts: &[&str], root: &Path) -> Vec<String> {
    let on_disk = |s: &str| match std::fs::read_to_string(root.join(s)) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => panic!("{s}: {e}"),
    };
    if on_disk(scripts[0]).is_none() {
        return Vec::new();
    }
    let mut names: BTreeSet<String> = scripts.iter().map(|s| (*s).to_owned()).collect();
    names.extend(
        recorded
            .as_object()
            .into_iter()
            .flat_map(|m| m.keys().cloned()),
    );
    names
        .into_iter()
        .filter(|s| on_disk(s).is_some_and(|text| recorded.get(s) != Some(&json!(text))))
        .collect()
}

/// The golden `name`, checked against the scripts on disk; `None` when
/// regenerating.
fn recorded(name: &str) -> Option<Value> {
    if regenerating() {
        return None;
    }
    let path = golden_path(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; {REGENERATE}", path.display()));
    let golden: Value = serde_json::from_str(&text).unwrap();
    let stale = stale_scripts(&golden["scripts"], &SCRIPTS, &plugin_root());
    assert!(
        stale.is_empty(),
        "{stale:?} changed since {} was recorded; {REGENERATE}",
        path.display()
    );
    Some(golden)
}

fn write_golden(name: &str, golden: Value) {
    let text = serde_json::to_string_pretty(&golden).unwrap();
    std::fs::write(golden_path(name), text + "\n").unwrap();
}

fn plugin_text() -> String {
    plugin_root()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

/// `dir` and the plugin as placeholders, both ways: node names the real path
/// (`/private/var/…` on macOS), so it goes before the one `dir` is spelled as.
fn placeholders(dir: &Path, name: &'static str) -> [(String, &'static str); 3] {
    let real = dir.canonicalize().map_or_else(
        |_| dir.to_string_lossy().into_owned(),
        |d| d.to_string_lossy().into_owned(),
    );
    [
        (real, name),
        (dir.to_string_lossy().into_owned(), name),
        (plugin_text(), "<PLUGIN>"),
    ]
}

fn to_golden(v: &Value, pairs: &[(String, &'static str)]) -> Value {
    let text = pairs
        .iter()
        .fold(v.to_string(), |t, (from, to)| t.replace(from.as_str(), to));
    serde_json::from_str(&text).unwrap()
}

fn from_golden(v: &Value, pairs: &[(String, &'static str)]) -> Value {
    let text = pairs[1..]
        .iter()
        .fold(v.to_string(), |t, (to, from)| t.replace(from, to));
    serde_json::from_str(&text).unwrap()
}

// ---------------------------------------------------------------- pools

/// A vote field: its JSON value (`None` = absent) and its template string.
type Field = (Option<Value>, &'static str);

fn slot_pool() -> Vec<Field> {
    vec![
        (Some(json!(1)), "1"),
        (Some(json!(2)), "2"),
        (Some(json!(3)), "3"),
        (Some(json!(12)), "12"),
        (Some(json!("4")), "4"),
        (Some(json!("a")), "a"),
        (Some(json!("q r1-b")), "q r1-b"),
        (Some(json!("q r1-b")), "q r1-b"),
        (Some(json!("A")), "A"),
        (Some(json!("x/../2")), "x/../2"),
        (Some(json!("sub/5")), "sub/5"),
        (Some(json!(" 7")), " 7"),
        (Some(json!("0x1f")), "0x1f"),
        (Some(json!("Infinity")), "Infinity"),
        (Some(json!(1.5)), "1.5"),
        (Some(json!(-0.0)), "0"),
        (Some(json!(true)), "true"),
        (Some(json!([1, 2])), "1,2"),
        (Some(json!([null, [3]])), ",3"),
        (Some(json!({})), "[object Object]"),
        (Some(Value::Null), "null"),
        (None, "undefined"),
    ]
}

fn source_pool() -> Vec<Field> {
    vec![
        (Some(json!("codex")), "codex"),
        (Some(json!("codex")), "codex"),
        (Some(json!("codex")), "codex"),
        (Some(json!("gemini")), "gemini"),
        (Some(json!("gemini")), "gemini"),
        (Some(json!("claude")), "claude"),
        (Some(json!("other")), "other"),
        (Some(json!(7)), "7"),
        (None, "undefined"),
    ]
}

/// A round: its value and the key infix it gives.
fn round_pool() -> Vec<Field> {
    vec![
        (None, ""),
        (None, ""),
        (Some(json!(0)), ""),
        (Some(json!(1)), "r1"),
        (Some(json!(1)), "r1"),
        (Some(json!(2)), "r2"),
        (Some(json!("1")), "r1"),
        (Some(Value::Null), ""),
        (Some(json!("")), ""),
        (Some(json!(true)), "rtrue"),
        (Some(json!(10)), "r10"),
    ]
}

fn verdict_pool() -> Vec<Option<Value>> {
    vec![
        Some(json!("REFUTED")),
        Some(json!("REFUTED")),
        Some(json!("NOT_REFUTED")),
        Some(json!("NOT_REFUTED")),
        Some(json!("refuted")),
        Some(json!(["REFUTED"])),
        Some(json!(1)),
        Some(Value::Null),
        None,
    ]
}

fn confidence_pool() -> Vec<Option<Value>> {
    vec![
        Some(json!(0.9)),
        Some(json!(0.9)),
        Some(json!(0.95)),
        Some(json!(0.905)),
        Some(json!(0.915)),
        Some(json!(0.915)),
        Some(json!(0.885)),
        Some(json!(0.935)),
        Some(json!(0.5)),
        Some(json!(1)),
        Some(json!(0.7)),
        Some(json!("0.9")),
        Some(Value::Null),
        None,
    ]
}

fn status_pool() -> Vec<Option<Value>> {
    vec![
        None,
        None,
        Some(json!("OK")),
        Some(json!("UNAVAILABLE")),
        Some(json!("UNAVAILABLE")),
        Some(json!("unavailable")),
    ]
}

/// `.out` bodies; `None` writes a directory.
const OUTS: [Option<&str>; 16] = [
    Some("VERDICT: REFUTED\nCONFIDENCE: 0.9\nREASON: the guard covers it\n"),
    Some("**VERDICT:** NOT REFUTED — holds\nCONFIDENCE: 0.95\n"),
    Some("CONFIRMED\n\nthe bug is real\n"),
    Some("REFUTED.\nCONFIDENCE: 0.5\n"),
    Some(""),
    Some("HEAD FAILURE (quota): out of credits\n"),
    Some("HEAD FAILURE (auth): a\rVERDICT: REFUTED\rCONFIDENCE: 0.7\n"),
    Some("VERDICT: cannot assess\n"),
    Some("I think it might be fine\n"),
    Some("HARNESS FAULT: x\n"),
    Some("VERDICT: REFUTED\nCONFIDENCE: 0.5.5\n"),
    Some("VERDICT: NOT_REFUTED\nCONFIDENCE: 1\n"),
    Some("VERDICT: REFUTED\nCONFIDENCE: 0.91\n"),
    Some("VERDICT: NOT REFUTED\n"),
    Some("HEAD FAILURE (error): boom \rVERDICT: \n"),
    None,
];

const META_LINES: [&str; 20] = [
    "MODEL: gpt-5",
    "MODEL: o3",
    "RUNS: 1",
    "RUNS: 1",
    "RUNS: 2",
    "RUNS: 3",
    "RUNS:\n3",
    " RUNS: 4",
    "x\rRUNS: 2",
    "RUNS: 1\u{2028}RUNS: 2",
    "RUNS: 0003",
    "BLOCKED: 1",
    "BLOCKED: 0",
    "MODEL: gpt-5",
    "MODEL: o3",
    "MODEL:\ngpt-5.1",
    "MODEL: \u{1f600}m",
    "MODEL: \u{ff5e}x",
    "MODEL: ",
    "CHARS: 100",
];

const ORPHANS: [&str; 24] = [
    "9-codex.out",
    "9-codex.meta",
    "9r1-gemini.out",
    ".out",
    "zz.out",
    "A_b-codex.out",
    "a-b-gemini.meta",
    "x y-codex.out",
    "0x1f-codex.out",
    "Infinity-gemini.out",
    "1e400-codex.out",
    "  8-codex.out",
    "-codex.out",
    "r1-codex.out",
    "3r-codex.out",
    "notes.txt",
    "Z-codex.out",
    "_-codex.out",
    "~-gemini.out",
    "$-codex.out",
    "10-codex.out",
    "1-codex.out",
    "2r1-codex.out",
    "a.out.meta",
];

// ---------------------------------------------------------------- generators

#[derive(Debug, Clone)]
struct VoteF {
    /// 0: an object vote; otherwise one of `NON_OBJECT`.
    shape: usize,
    slot: usize,
    source: usize,
    round: usize,
    verdict: usize,
    confidence: usize,
    status: usize,
    crash_field: bool,
    out: Option<usize>,
    meta: Option<Vec<usize>>,
    meta_dir: bool,
    leftover: bool,
    /// A vote copying the artifact's own answer, its confidence moved by
    /// `DELTAS[i]`.
    echo: Option<usize>,
}

const DELTAS: [f64; 8] = [0.0, 0.005, -0.005, 0.0099, 0.011, 0.015, -0.015, 0.03];

/// What `OUTS[i]` classifies as, for an echoing vote.
fn answer_of(out: usize) -> Option<(&'static str, f64)> {
    match out {
        0 => Some(("REFUTED", 0.9)),
        1 => Some(("NOT_REFUTED", 0.95)),
        2 | 13 => Some(("NOT_REFUTED", 0.5)),
        3 | 10 => Some(("REFUTED", 0.5)),
        11 => Some(("NOT_REFUTED", 1.0)),
        12 => Some(("REFUTED", 0.91)),
        _ => None,
    }
}

const NON_OBJECT: [&str; 9] = [
    "", "null", "0", "\"\"", "false", "\"str\"", "5", "[1]", "true",
];

#[derive(Debug, Clone)]
struct Case {
    votes: Vec<VoteF>,
    orphans: Vec<usize>,
    flood: usize,
    stdin: usize,
    args: usize,
    broken_classifier: bool,
}

const STDINS: [&str; 7] = ["", "", "", "", "garbage", "{}", "  "];
const ARGS: [&[&str]; 6] = [
    &["DIR", "--compact"],
    &["DIR", "--compact"],
    &["DIR"],
    &["DIR"],
    &[],
    &[""],
];

fn vote_f() -> impl Strategy<Value = VoteF> {
    (
        (
            prop_oneof![9 => Just(0usize), 1 => 1..NON_OBJECT.len()],
            0..slot_pool().len(),
            0..source_pool().len(),
            0..round_pool().len(),
            0..verdict_pool().len(),
            0..confidence_pool().len(),
            0..status_pool().len(),
        ),
        (
            prop::bool::weighted(0.02),
            prop::option::weighted(0.85, 0..OUTS.len()),
            prop::option::weighted(0.45, prop::collection::vec(0..META_LINES.len(), 0..4)),
            prop::bool::weighted(0.02),
            prop::bool::weighted(0.25),
            prop::option::weighted(0.35, 0..DELTAS.len()),
        ),
    )
        .prop_map(
            |(
                (shape, slot, source, round, verdict, confidence, status),
                (crash_field, out, meta, meta_dir, leftover, echo),
            )| VoteF {
                shape,
                slot,
                source,
                round,
                verdict,
                confidence,
                status,
                crash_field,
                out,
                meta,
                meta_dir,
                leftover,
                echo,
            },
        )
}

fn case() -> impl Strategy<Value = Case> {
    (
        prop::collection::vec(vote_f(), 0..7),
        prop::collection::vec(0..ORPHANS.len(), 0..4),
        prop_oneof![30 => Just(0usize), 1 => 38..48usize],
        prop_oneof![12 => Just(0usize), 1 => 1..STDINS.len()],
        prop_oneof![40 => 0..4usize, 1 => 4..ARGS.len()],
        prop::bool::weighted(0.04),
    )
        .prop_map(
            |(mut votes, orphans, flood, stdin, args, broken_classifier)| {
                unfold_case(&mut votes);
                Case {
                    votes,
                    orphans,
                    flood,
                    stdin,
                    args,
                    broken_classifier,
                }
            },
        )
}

/// Slots `a` and `A` in one case name the same artifacts on macOS's
/// case-insensitive disk and different ones on Linux, so a golden recorded on
/// one cannot speak for the other. Each stays covered in cases of its own.
fn unfold_case(votes: &mut [VoteF]) {
    let named = |name: &str| slot_pool().iter().position(|(_, s)| *s == name).unwrap();
    let (lower, upper) = (named("a"), named("A"));
    if votes.iter().any(|v| v.slot == lower) {
        for v in votes.iter_mut().filter(|v| v.slot == upper) {
            v.slot = lower;
        }
    }
}

// ---------------------------------------------------------------- building

fn vote_value(v: &VoteF) -> Value {
    if v.shape > 0 {
        return serde_json::from_str(match NON_OBJECT[v.shape] {
            "" => "null",
            s => s,
        })
        .unwrap();
    }
    let mut m = Map::new();
    let mut put = |k: &str, f: Option<Value>| {
        if let Some(x) = f {
            m.insert(k.to_owned(), x);
        }
    };
    put("slotId", slot_pool()[v.slot].0.clone());
    put("source", source_pool()[v.source].0.clone());
    put("round", round_pool()[v.round].0.clone());
    put("verdict", verdict_pool()[v.verdict].clone());
    put("confidence", confidence_pool()[v.confidence].clone());
    put("slotStatus", status_pool()[v.status].clone());
    if let (Some(d), Some((verdict, conf))) = (v.echo, v.out.and_then(answer_of)) {
        m.insert("verdict".to_owned(), json!(verdict));
        m.insert("confidence".to_owned(), json!(conf + DELTAS[d]));
    }
    if v.crash_field {
        let k = if v.slot.is_multiple_of(2) {
            "slotId"
        } else {
            "verdict"
        };
        m.insert(k.to_owned(), json!({"toString": 1}));
    }
    Value::Object(m)
}

/// The artifact key the script derives, from the pools' own template text.
fn key_of(v: &VoteF) -> String {
    format!(
        "{}{}-{}",
        slot_pool()[v.slot].1,
        round_pool()[v.round].1,
        source_pool()[v.source].1
    )
}

fn write_file(path: &Path, body: &str) {
    if path.is_dir() {
        return;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

fn build(dir: &Path, case: &Case) -> String {
    std::fs::create_dir_all(dir).unwrap();
    for v in &case.votes {
        if v.shape > 0 {
            continue;
        }
        // `x/` is never created, so only a lexically normalized join finds it.
        let key = key_of(v).replace("x/../", "");
        let out = dir.join(format!("{key}.out"));
        if let Some(i) = v.out {
            match OUTS[i] {
                Some(body) => write_file(&out, body),
                None => {
                    let _ = std::fs::create_dir_all(&out);
                }
            }
        }
        if let Some(lines) = &v.meta {
            let meta = dir.join(format!("{key}.meta"));
            if v.meta_dir {
                let _ = std::fs::create_dir_all(&meta);
            } else {
                let mut body: Vec<&str> = lines.iter().map(|&i| META_LINES[i]).collect();
                body.push("");
                write_file(&meta, &body.join("\n"));
            }
        }
        if v.leftover {
            let base = format!("{}-{}", slot_pool()[v.slot].1, source_pool()[v.source].1)
                .replace("x/../", "");
            write_file(&dir.join(format!("{base}.out")), "VERDICT: REFUTED\n");
            write_file(&dir.join(format!("{base}r0-x.meta")), "RUNS: 1\n");
        }
    }
    for &o in &case.orphans {
        if ORPHANS[o] == "notes.txt" {
            std::fs::create_dir_all(dir.join("sub")).unwrap();
        }
        write_file(&dir.join(ORPHANS[o]), "VERDICT: NOT_REFUTED\n");
    }
    for i in 0..case.flood {
        let name = if i % 3 == 0 {
            format!("F{i}-gemini.meta")
        } else {
            format!("f{i}-codex.out")
        };
        write_file(&dir.join(name), "garbage\n");
    }
    match STDINS[case.stdin] {
        "" => serde_json::to_string(&Value::Array(case.votes.iter().map(vote_value).collect()))
            .unwrap(),
        s => s.to_owned(),
    }
}

fn args_for(case: &Case, dir: &Path) -> Vec<String> {
    ARGS[case.args]
        .iter()
        .map(|a| {
            if *a == "DIR" {
                dir.to_string_lossy().into_owned()
            } else {
                (*a).to_owned()
            }
        })
        .collect()
}

// ---------------------------------------------------------------- running

struct NodeRun {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl NodeRun {
    fn to_json(&self) -> Value {
        json!({"code": self.code, "stdout": self.stdout, "stderr": self.stderr})
    }

    fn from_json(v: &Value) -> Self {
        Self {
            code: v["code"].as_i64().map(|c| i32::try_from(c).unwrap()),
            stdout: v["stdout"].as_str().unwrap().to_owned(),
            stderr: v["stderr"].as_str().unwrap().to_owned(),
        }
    }
}

/// Runs the script; only a regeneration reaches it.
fn node(args: &[String], stdin: &str, cwd: &Path, broken: bool) -> NodeRun {
    let mut cmd = Command::new("node");
    cmd.arg(plugin_root().join(SCRIPT))
        .args(args)
        .current_dir(cwd)
        .env("LC_ALL", "en_US.UTF-8")
        .env_remove("TRIBUNAL_CLASSIFIER")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if broken {
        cmd.env("TRIBUNAL_CLASSIFIER", MISSING_CLASSIFIER);
    }
    let mut child = cmd.spawn().expect("node must be on PATH");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    NodeRun {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// The error text Node's `execFileSync` gives for a classifier that is not
/// there, as `String(err)`.
fn broken_classifier(file: &str) -> Result<String, String> {
    Err(format!(
        "Error: Command failed: bash {MISSING_CLASSIFIER} {file}\nbash: {MISSING_CLASSIFIER}: No such file or directory\n"
    ))
}

/// The port's stdout, or its error.
fn rust_side(args: &[String], stdin: &str, broken: bool) -> Result<String, String> {
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    if !broken {
        return run(&argv, stdin).map(|p| p.stdout());
    }
    // Same entry as `run`, with the classifier swapped.
    let Some(dir) = argv.first().copied().filter(|d| !d.is_empty()) else {
        return run(&argv, stdin).map(|p| p.stdout());
    };
    let text = if stdin.is_empty() { "[]" } else { stdin };
    let Ok(votes) = tribunal_context::operator::json_parse(text) else {
        return run(&argv, stdin).map(|p| p.stdout());
    };
    let compact = argv.contains(&"--compact");
    let r = reconcile(dir, &votes, &broken_classifier)?;
    Ok(Printed {
        value: r.to_value(compact),
        indent: if compact { "" } else { "  " },
    }
    .stdout())
}

struct Outcome {
    input: Value,
    rust: Value,
    js: Value,
    js_out: Option<Value>,
    js_stderr: String,
    compact: bool,
    case: Case,
}

/// One case: the script's side from `replay`, the golden's record of it, or
/// from the script when regenerating. Also returns what the golden keeps.
fn check(dir: &Path, root: &Path, case: &Case, replay: Option<&Value>) -> (Outcome, Value) {
    let stdin = build(dir, case);
    let args = args_for(case, dir);
    let pairs = placeholders(root, "<ROOT>");
    let input = json!({ "case": format!("{case:?}"), "args": args, "stdin": stdin });
    // A hash: the cases' stdin would be most of the file.
    let hash = json!(sha256_hex(to_golden(&input, &pairs).to_string().as_bytes()));
    let js = match replay {
        Some(r) => {
            assert_eq!(r["input"], hash, "the generator changed; {REGENERATE}");
            NodeRun::from_json(&from_golden(&r["js"], &pairs))
        }
        None => node(&args, &stdin, dir, case.broken_classifier),
    };
    let kept = json!({"input": hash, "js": to_golden(&js.to_json(), &pairs)});
    let rust = rust_side(&args, &stdin, case.broken_classifier);
    let _ = std::fs::remove_dir_all(dir);
    let js_text = if js.code == Some(0) {
        js.stdout.clone()
    } else {
        assert!(js.stdout.is_empty(), "a crash printed {}", js.stdout);
        "<exit non-zero>".to_owned()
    };
    let rust_text = match &rust {
        Ok(s) => s.clone(),
        Err(e) => {
            if e.starts_with("Cannot convert") {
                assert!(js.stderr.contains(e), "{e} vs {}", js.stderr);
            }
            "<exit non-zero>".to_owned()
        }
    };
    let js_out = (js.code == Some(0)).then(|| serde_json::from_str(js.stdout.trim()).unwrap());
    let outcome = Outcome {
        input,
        rust: Value::String(rust_text),
        js: Value::String(js_text),
        js_out,
        js_stderr: js.stderr,
        compact: args.iter().any(|a| a == "--compact"),
        case: case.clone(),
    };
    (outcome, kept)
}

fn outcomes() -> &'static [Outcome] {
    static CELL: OnceLock<Vec<Outcome>> = OnceLock::new();
    CELL.get_or_init(|| {
        let cases = sample(case());
        let root = scratch("reconcile-votes");
        let golden = recorded(GOLDEN);
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
                    let o = check(&dir, &root, case, replay.map(|r| &r[i]));
                    *slots[i].lock().unwrap() = Some(o);
                });
            }
        });
        let _ = std::fs::remove_dir_all(&root);
        let (out, kept): (Vec<Outcome>, Vec<Value>) = slots
            .into_iter()
            .map(|m| m.into_inner().unwrap().unwrap())
            .unzip();
        if replay.is_none() {
            write_golden(GOLDEN, json!({"scripts": script_texts(), "cases": kept}));
        }
        out
    })
}

#[test]
fn reconcile_votes_matches_js_on_generated_cases() {
    let out = outcomes();
    let inputs: Vec<Value> = out.iter().map(|o| o.input.clone()).collect();
    let rust: Vec<Value> = out.iter().map(|o| o.rust.clone()).collect();
    let js: Vec<Value> = out.iter().map(|o| o.js.clone()).collect();
    assert_same("reconcile-votes", &inputs, &rust, &js);
}

#[test]
fn the_generated_cases_reach_every_branch() {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    let mut hit = |k: &'static str, cond: bool| {
        *seen.entry(k).or_default() += usize::from(cond);
    };
    for o in outcomes() {
        let c = &o.case;
        hit("crash", o.js_out.is_none());
        hit(
            "crash-to-primitive",
            o.js_stderr
                .contains("Cannot convert object to primitive value"),
        );
        hit("crash-meta-dir", o.js_stderr.contains("EISDIR"));
        hit("broken-classifier", c.broken_classifier);
        hit("non-object-vote", c.votes.iter().any(|v| v.shape > 0));
        hit("leftover-round", c.votes.iter().any(|v| v.leftover));
        let Some(out) = &o.js_out else { continue };
        if let Some(e) = out.get("error").and_then(Value::as_str) {
            hit("usage", e.starts_with("usage"));
            hit("bad-json", e.contains("not valid JSON"));
            continue;
        }
        hit("non-array-stdin", STDINS[c.stdin] == "{}");
        hit("compact", o.compact);
        hit("full", !o.compact);
        hit("omitted", out.get("omitted").is_some());
        hit("ok>0", out["ok"].as_i64().unwrap_or(0) > 0);
        hit(
            "two-models",
            out["codexModelsUsed"]
                .as_array()
                .is_some_and(|a| a.len() >= 2),
        );
        for kind in [
            "missing-artifact",
            "repeat-blocked",
            "over-invoked",
            "classifier-failed",
            "abstained-despite-verdict",
            "vote-without-verdict",
            "verdict-mismatch",
            "confidence-mismatch",
            "orphan-artifact",
            "superseded-round",
        ] {
            hit(kind, out["counts"].get(kind).is_some());
        }
        for m in out["mismatches"].as_array().unwrap() {
            let orphan = m["kind"] == "orphan-artifact" || m["kind"] == "superseded-round";
            hit("orphan-null-slot", orphan && m["slotId"].is_null());
            hit("orphan-number-slot", orphan && m["slotId"].is_number());
            hit("orphan-string-slot", orphan && m["slotId"].is_string());
            hit(
                "orphan-no-key",
                orphan && m.get("key").is_none_or(|k| k == ""),
            );
            hit(
                "vote-slot-absent",
                !orphan && m.get("slotId").is_none_or(Value::is_null),
            );
            let actual = m.get("actual");
            hit(
                "nan-confidence",
                actual.is_some_and(|a| a["failclass"] == "none" && a["confidence"].is_null()),
            );
            hit(
                "picked-from-reason",
                actual.is_some_and(|a| a["failclass"] == "auth" && a["verdict"] == "REFUTED"),
            );
            hit(
                "empty-pick",
                actual.is_some_and(|a| a["failclass"] == "error" && a["verdict"] == ""),
            );
        }
    }
    eprintln!("{seen:#?}");
    for (k, n) in &seen {
        assert!(*n >= 3, "{k} reached only {n} times: {seen:#?}");
    }
}

#[test]
fn the_in_process_classifier_never_fails() {
    assert!(classify_in_process("/nonexistent/x.out").is_ok());
}

// ---------------------------------------------------------------------------
// Witnesses for the declared differences.

struct Witness {
    name: &'static str,
    files: &'static [&'static str],
    stdin: &'static str,
    broken: bool,
}

const WITNESSES: [Witness; 4] = [
    Witness {
        name: "rv-collation",
        files: &["\u{e9}-codex.out", "f-codex.out"],
        stdin: "[]",
        broken: false,
    },
    Witness {
        name: "rv-seam",
        files: &["1-codex.out"],
        stdin: r#"[{"slotId":1,"source":"codex","verdict":"REFUTED"}]"#,
        broken: true,
    },
    Witness {
        name: "rv-infinity",
        files: &["Infinity-codex.out"],
        stdin: r#"[{"slotId":1e400,"source":"codex","verdict":"REFUTED"}]"#,
        broken: false,
    },
    Witness {
        name: "rv-surrogate",
        files: &[],
        stdin: r#"[{"slotId":"\ud800","source":"codex"}]"#,
        broken: false,
    },
];

fn witness_dir(w: &Witness, suffix: &str) -> PathBuf {
    let dir = scratch(&format!("{}{suffix}", w.name));
    for f in w.files {
        std::fs::write(dir.join(f), "VERDICT: REFUTED\n").unwrap();
    }
    dir
}

fn witness_args(dir: &Path) -> Vec<String> {
    vec![dir.to_string_lossy().into_owned(), "--compact".to_owned()]
}

/// What the script printed for each witness, by name, with its directory as
/// `<DIR>`: from the golden, or from the script when regenerating, in
/// directories of its own so the tests' are left alone.
fn witness_stdouts() -> &'static BTreeMap<&'static str, Value> {
    static CELL: OnceLock<BTreeMap<&'static str, Value>> = OnceLock::new();
    CELL.get_or_init(|| {
        let input = |w: &Witness| json!({"files": w.files, "stdin": w.stdin, "broken": w.broken});
        if let Some(g) = recorded(WITNESS_GOLDEN) {
            return WITNESSES
                .iter()
                .map(|w| {
                    let r = &g["witnesses"][w.name];
                    assert_eq!(r["input"], input(w), "the witness changed; {REGENERATE}");
                    (w.name, r["stdout"].clone())
                })
                .collect();
        }
        let out: BTreeMap<&'static str, Value> = WITNESSES
            .iter()
            .map(|w| {
                let dir = witness_dir(w, "-js");
                let js = node(&witness_args(&dir), w.stdin, &dir, w.broken);
                let stdout = to_golden(&json!(js.stdout), &placeholders(&dir, "<DIR>"));
                let _ = std::fs::remove_dir_all(&dir);
                (w.name, stdout)
            })
            .collect();
        let kept: BTreeMap<&str, Value> = WITNESSES
            .iter()
            .map(|w| (w.name, json!({"input": input(w), "stdout": out[w.name]})))
            .collect();
        write_golden(
            WITNESS_GOLDEN,
            json!({"scripts": script_texts(), "witnesses": kept}),
        );
        out
    })
}

fn witness_raw(name: &str) -> (Value, String) {
    let w = WITNESSES.iter().find(|w| w.name == name).unwrap();
    let recorded = &witness_stdouts()[name];
    let dir = witness_dir(w, "");
    let js = from_golden(recorded, &placeholders(&dir, "<DIR>"));
    let args = witness_args(&dir);
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let rust = run(&argv, w.stdin).unwrap().value;
    let _ = std::fs::remove_dir_all(&dir);
    (rust, js.as_str().unwrap().to_owned())
}

fn witness(name: &str) -> (Value, Value) {
    let (rust, js) = witness_raw(name);
    (rust, serde_json::from_str(js.trim()).unwrap())
}

fn keys(v: &Value) -> Vec<String> {
    v["mismatches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["key"].as_str().unwrap_or("").to_owned())
        .collect()
}

#[test]
fn orphan_keys_outside_ascii_are_a_declared_difference() {
    let (rust, js) = witness("rv-collation");
    assert_eq!(keys(&js), ["\u{e9}-codex", "f-codex"]);
    assert_eq!(keys(&rust), ["f-codex", "\u{e9}-codex"]);
}

#[test]
fn the_classifier_test_seam_is_a_declared_difference() {
    let (rust, js) = witness("rv-seam");
    assert_eq!(js["classifierFailed"], 1);
    assert_eq!(rust["classifierFailed"], 0);
    assert_eq!(rust["ok"], 1);
}

#[test]
fn an_out_of_range_number_is_a_declared_difference() {
    let (rust, js) = witness("rv-infinity");
    assert_eq!(js["ok"], 1);
    assert_eq!(rust["counts"]["missing-artifact"], 1);
}

#[test]
fn a_lone_surrogate_is_a_declared_difference() {
    let (rust, js) = witness_raw("rv-surrogate");
    assert!(js.contains(r#""counts":{"missing-artifact":1}"#), "{js}");
    assert!(js.contains(r#""slotId":"\ud800""#), "{js}");
    assert_eq!(rust["counts"]["missing-artifact"], 1);
    assert_eq!(rust["mismatches"][0]["slotId"], "\u{fffd}");
}

/// The staleness check on a tree of its own: a script edited since the
/// recording is stale, one deleted (retired with the Workflow) is not, and
/// with the script the cases run deleted nothing is.
#[test]
fn a_retired_script_is_not_stale() {
    let root = scratch("rv-retired");
    std::fs::create_dir_all(root.join("lib")).unwrap();
    let write = |name: &str, text: &str| std::fs::write(root.join(name), text).unwrap();
    write("lib/run.mjs", "run");
    write("lib/dep.sh", "dep edited");
    let recorded = json!({"lib/run.mjs": "run", "lib/dep.sh": "dep", "lib/gone.sh": "gone"});
    let scripts = ["lib/run.mjs", "lib/dep.sh", "lib/gone.sh"];
    assert_eq!(stale_scripts(&recorded, &scripts, &root), ["lib/dep.sh"]);
    write("lib/dep.sh", "dep");
    assert!(stale_scripts(&recorded, &scripts, &root).is_empty());
    write("lib/new.sh", "new");
    assert_eq!(
        stale_scripts(&recorded, &["lib/run.mjs", "lib/new.sh"], &root),
        ["lib/new.sh"]
    );
    std::fs::remove_file(root.join("lib/run.mjs")).unwrap();
    write("lib/dep.sh", "dep edited again");
    assert!(stale_scripts(&recorded, &scripts, &root).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}
