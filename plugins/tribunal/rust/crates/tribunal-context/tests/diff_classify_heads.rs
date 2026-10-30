//! `lib/classify-verdict.sh`, `lib/classify-head-failure.sh` and
//! `lib/classify-artifacts.sh` against their ports, on the same files. Each
//! script runs under `bash` from PATH with `LC_ALL=en_US.UTF-8` (this
//! machine's locale) and a `tribunal-root` that names this checkout, and the
//! comparison is the exit code and stdout, as text.
//!
//! classify-artifacts sorts its entries by the locale's collation and the port
//! by bytes (a declared difference), so the port's entries are put in the
//! script's key order before the line is compared; every byte else is checked.
//!
//! The port matches the scripts as they run on macOS. Elsewhere GNU `cut -c`
//! counts bytes and splits a character, mawk folds case byte by byte, and
//! glibc collates `*.out` in another order. So the comparisons over generated
//! text and the collation witness read what the scripts printed on macOS from
//! `tests/goldens/`, and run on every platform. So do the edge cases and
//! witnesses at the end, from `classify-edge-*.json`: no test runs a script
//! outside a regeneration. A golden whose script is no longer on disk was
//! recorded from a script retired with the Workflow, and is final. Regenerate
//! those files on a Mac after changing a script or a generator:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_classify_heads`.
mod common;
#[path = "../../tribunal-core/tests/common/mod.rs"]
mod core_common;

use common::{plugin_root, scratch};
use core_common::{assert_same, sample};

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use proptest::prelude::*;
use proptest::sample::select;
use serde_json::{json, Value};
use tribunal_context::classify_artifacts::{self, ArtifactScan};
use tribunal_context::classify_head_failure::{self, HeadFailure};
use tribunal_context::classify_verdict::{self, Classification, Failclass, Verdict};

const VERDICT_SH: &str = "lib/classify-verdict.sh";
const HEAD_FAILURE_SH: &str = "lib/classify-head-failure.sh";
const ARTIFACTS_SH: &str = "lib/classify-artifacts.sh";
const NO_REASON: &str = "(no reason line — see the raw output)";

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// A directory holding a `tribunal-root` that prints `root`.
fn stub_bin(name: &str, root: &Path) -> PathBuf {
    let dir = scratch(name);
    let stub = dir.join("tribunal-root");
    std::fs::write(&stub, format!("#!/bin/sh\necho '{}'\n", root.display())).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

fn checkout_bin() -> &'static Path {
    static CELL: OnceLock<PathBuf> = OnceLock::new();
    CELL.get_or_init(|| stub_bin("classify-bin", &plugin_root().canonicalize().unwrap()))
}

fn bash_with(bin: &Path, script: &str, args: &[&str]) -> Run {
    let path = std::env::join_paths(std::iter::once(bin.to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    let out = Command::new("bash")
        .arg(plugin_root().join(script))
        .args(args)
        .env("PATH", path)
        .env("LC_ALL", "en_US.UTF-8")
        .env_remove("CLAUDE_PLUGIN_ROOT")
        .output()
        .expect("bash must be on PATH");
    Run {
        code: out.status.code(),
        stdout: String::from_utf8(out.stdout).expect("UTF-8 stdout"),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn bash(script: &str, args: &[&str]) -> Run {
    bash_with(checkout_bin(), script, args)
}

/// Runs `f` over every case on every core, keeping the order.
fn par_map<T: Sync, R: Send>(cases: &[T], f: impl Fn(usize, &T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<R>>> = cases.iter().map(|_| Mutex::new(None)).collect();
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(case) = cases.get(i) else { break };
                let r = f(i, case);
                *slots[i].lock().unwrap() = Some(r);
            });
        }
    });
    slots
        .into_iter()
        .map(|m| m.into_inner().unwrap().unwrap())
        .collect()
}

// ---------------------------------------------------------------- goldens

const REGENERATE: &str =
    "regenerate on a Mac: TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_classify_heads";

fn regenerating() -> bool {
    std::env::var("TRIBUNAL_REGEN_GOLDENS").is_ok_and(|v| v == "1")
}

/// The scripts in `recorded` (`{path: text}`) or `scripts` whose text under
/// `root` is not what was recorded. A script no longer on disk was retired
/// with the Workflow and its golden is final, so it is skipped; when
/// `scripts[0]`, the one the cases run, is gone, the rest are only what it
/// ran and are skipped too.
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

/// What the script printed for each input, read from `tests/goldens/<set>.json`
/// or, when regenerating, from `run` and written there. The file keeps the
/// text of `scripts` and every input, so a script or generator edited without
/// a regeneration fails here on every platform instead of being compared with
/// a stale oracle; `scripts[0]` is the one `run` starts. `root`, the scratch
/// directory the cases live in, is stored as `<ROOT>`: its name carries the
/// process id. `run` is called only when regenerating.
fn oracle(
    set: &str,
    scripts: &[&str],
    root: &Path,
    inputs: &[Value],
    run: impl Fn(usize) -> Value + Sync,
) -> Vec<Value> {
    let root = root.to_string_lossy().into_owned();
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens")
        .join(format!("{set}.json"));
    if regenerating() {
        if !cfg!(target_os = "macos") {
            panic!("the goldens record the scripts as they run on macOS");
        }
        let texts: BTreeMap<String, String> = scripts
            .iter()
            .map(|s| {
                let text = std::fs::read_to_string(plugin_root().join(s)).unwrap();
                ((*s).to_owned(), text)
            })
            .collect();
        let js = par_map(inputs, |i, _| run(i));
        let cases: Vec<Value> = inputs
            .iter()
            .zip(&js)
            .map(|(input, js)| json!({"input": input, "js": js}))
            .collect();
        let text = serde_json::to_string_pretty(&json!({"scripts": texts, "cases": cases}))
            .unwrap()
            .replace(&root, "<ROOT>");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text + "\n").unwrap();
        return js;
    }
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; {REGENERATE}", path.display()));
    let golden: Value = serde_json::from_str(&text.replace("<ROOT>", &root)).unwrap();
    let stale = stale_scripts(&golden["scripts"], scripts, &plugin_root());
    assert!(
        stale.is_empty(),
        "{stale:?} changed since {} was recorded; {REGENERATE}",
        path.display()
    );
    let cases = golden["cases"].as_array().unwrap();
    assert_eq!(
        cases.len(),
        inputs.len(),
        "the generator changed; {REGENERATE}"
    );
    cases
        .iter()
        .zip(inputs)
        .map(|(c, input)| {
            assert_eq!(&c["input"], input, "the generator changed; {REGENERATE}");
            c["js"].clone()
        })
        .collect()
}

// ---------------------------------------------------------------- generators

const DECOR: [&str; 20] = [
    "",
    "",
    "",
    "**",
    "### ",
    "# ",
    "####### ",
    "- ",
    "> ",
    "+ ",
    "  ",
    "\t",
    "\u{a0}",
    "\u{3000}",
    "`",
    "_",
    "1. ",
    " - ** ",
    "\u{85}",
    "\u{2003}#\u{2003}",
];
const LABELS: [&str; 12] = [
    "",
    "",
    "VERDICT",
    "VERDICT",
    "Verdict",
    "verdict",
    "VEREDICTO",
    "Veredicto",
    "VERDICTS",
    "Final verdict",
    "VERD\u{130}CT",
    "V_ERDICT",
];
const SEPS: [&str; 13] = [
    ": ",
    ":",
    "：",
    " — ",
    " – ",
    " - ",
    " ",
    "",
    " : ",
    "\u{a0}:\u{a0}",
    "::",
    "**: ",
    ":** ",
];
const TOKENS: [&str; 22] = [
    "REFUTED",
    "REFUTED",
    "Refuted",
    "refuted",
    "NOT REFUTED",
    "NOT-REFUTED",
    "NOT_REFUTED",
    "not.refuted",
    "NOTREFUTED",
    "Not\u{2003}Refuted",
    "NOT  REFUTED",
    "CONFIRMED",
    "Confirmed",
    "CANNOT-ASSESS",
    "CANNOT ASSESS",
    "Cannot_Assess",
    "cannotassess",
    "ABSTAIN",
    "abstained",
    "UNREFUTED",
    "REFUTATION",
    "CONFIRMED",
];
const TAILS: [&str; 25] = [
    "",
    "",
    "",
    ".",
    " — the guard on line 12 covers it",
    ": because the caller validates first",
    " (FALSE POSITIVE)",
    " (FALSE POSITIVE)  ",
    "**",
    "** — Reason: the index exists",
    "s",
    "\u{212A}",
    "\u{131}",
    "é and more",
    "1",
    " - REASON: echoed reason",
    " — RAZÓN: la guarda",
    " —",
    ".\u{2003}",
    " :: ",
    "  ",
    " — not refuted after all",
    " — confirmed by the test",
    "\u{a0}(see below)",
    " \u{2014} \"quoted\" \\ back\u{7f}slash\u{1}\u{1b}",
];
const CONF_LABELS: [&str; 8] = [
    "CONFIDENCE",
    "CONFIDENCE",
    "Confidence",
    "confidence",
    "CONFIANZA",
    "CONF\u{130}DENCE",
    "Confidence level",
    "CONFIDENCE_SCORE",
];
const CONF_SEPS: [&str; 10] = [": ", ": ", ":", " — ", " – ", " - ", " ", "", "：", ":** "];
const CONF_VALUES: [&str; 28] = [
    "0.8",
    "0.95",
    "1.0",
    "1",
    "0",
    "95",
    "high",
    "0.5.5",
    "1.05",
    "1.",
    ".5",
    "0.",
    "0.00001",
    "0.000015",
    "0.12345678901234567890",
    "0.9 (high-ish)",
    "0.7 - fairly",
    "１",
    "0.3\u{2003}",
    "1.0000000000000001",
    "0.30000000000000004",
    "0.0001",
    "1.0.2",
    "0.5/1",
    "00.5",
    "0.99999999999999999",
    "10",
    "0.6**",
];
const PROSE: [&str; 44] = [
    "",
    "",
    "  ",
    "\t",
    "\u{a0}",
    " \t ",
    "Analysis:",
    "Reasoning:",
    "## Why",
    "Step 1:",
    "A very long heading that is longer than thirty one chars:",
    "Two words :  ",
    "refutationType: guard-exists",
    "killReason: covered upstream",
    "KILLREASON: x",
    "\u{212A}illreason: kelvin sign",
    "SOURCE: codex",
    "slotStatus: OK",
    "coreStands: yes",
    "decisionQuestioned: whether it bites",
    "Reason: the guard covers it",
    "REASON: echoed reason",
    "razón: la guarda existe",
    "RAZÓN: x",
    "The claim is refuted by the guard at line 12.",
    "I think this is not refuted.",
    "I cannot assess this from the slice.",
    "La función está bien — ok 😀 e\u{301}",
    "I can't help with that.",
    "Here is my analysis of the finding.",
    "    indented argument line",
    "`code` and *emphasis* and snake_case",
    "(parenthetical aside)",
    "ERROR: Your workspace is out of credits",
    "HARNESS FAULT: not on the first line",
    "PROMPT TRUNCATED IN TRANSIT",
    "- bullet argument",
    "> quoted argument",
    "The caller in src/a.ts:40 validates the id first.",
    "\"quoted\" \\ backslash \u{7f} del \u{1} ctl",
    "Verdict reasoning follows",
    "1. first point",
    "   ",
    "\r",
];
const FIRSTS: [&str; 13] = [
    "HEAD FAILURE (quota): ERROR: Your workspace is out of credits",
    "HEAD FAILURE (auth): Error: 401 Unauthorized",
    "HEAD FAILURE (error): ERROR: stream disconnected",
    "HEAD FAILURE (timeout): x",
    "HEAD FAILURE (quota):",
    "HEAD FAILURE (error): ",
    "HARNESS FAULT: prompt missing",
    " HARNESS FAULT: x",
    "bash: PROMPT TRUNCATED IN TRANSIT",
    "head failure (quota): x",
    "HEAD FAILURE (auth): \"quoted\" \\ é😀 del\u{7f} esc\u{1b}",
    "HEAD FAILURE (Quota): x",
    "HEAD FAILURE (error):   spaced reason",
];
const EOLS: [&str; 3] = ["\n", "\n", "\r\n"];

fn long_text(prefix: &str, units: usize) -> String {
    format!("{prefix}{}", "Ünïcödé e\u{301} 😀 text, ".repeat(units))
}

fn long_line() -> impl Strategy<Value = String> {
    (
        select(
            &[
                "",
                "VERDICT: REFUTED — ",
                "Reason: ",
                "  ",
                "HEAD FAILURE (error): ",
                "ERROR: ",
            ][..],
        ),
        8usize..30,
    )
        .prop_map(|(p, n)| long_text(p, n))
}

fn verdict_line() -> impl Strategy<Value = String> {
    (
        select(&DECOR[..]),
        select(&LABELS[..]),
        select(&SEPS[..]),
        select(&TOKENS[..]),
        select(&TAILS[..]),
    )
        .prop_map(|(d, l, s, t, x)| {
            if l.is_empty() {
                format!("{d}{t}{x}")
            } else {
                format!("{d}{l}{s}{t}{x}")
            }
        })
}

fn confidence_line() -> impl Strategy<Value = String> {
    (
        select(&DECOR[..]),
        select(&CONF_LABELS[..]),
        select(&CONF_SEPS[..]),
        select(&CONF_VALUES[..]),
    )
        .prop_map(|(d, l, s, v)| format!("{d}{l}{s}{v}"))
}

fn body_line() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => verdict_line(),
        2 => confidence_line(),
        5 => select(&PROSE[..]).prop_map(str::to_owned),
        1 => long_line(),
    ]
}

/// Lines joined by one line ending, with zero to two of them after the last.
fn join(lines: &[String], eol: &str, ends: usize) -> String {
    let mut text = lines.join(eol);
    if !lines.is_empty() {
        text.push_str(&eol.repeat(ends));
    }
    text
}

const GOOD_CONF: [&str; 15] = [
    "0", "0", "1", "1", "0.8", "0.65", "1.0", "1.00", "0.5", "0.95", "0.5.5", "1.0.2", "0.00001",
    "0.000015", "0.0001",
];
const GAPS: [&str; 10] = [
    "",
    "  ",
    "\t",
    "Analysis:",
    "## Reasoning:",
    "Why it holds:  ",
    "refutationType: guard-exists",
    "CONFIDENCE: 0.8",
    "Source: codex",
    "\u{212A}illReason : x",
];

fn prose() -> impl Strategy<Value = String> {
    select(&PROSE[..]).prop_map(str::to_owned)
}

/// The labelled shape the prompt asks for: argument, then the verdict and
/// confidence lines.
fn labelled_answer() -> impl Strategy<Value = Vec<String>> {
    (
        proptest::collection::vec(prose(), 0..3),
        verdict_line(),
        prop_oneof![
            2 => select(&GOOD_CONF[..]).prop_map(|v| format!("CONFIDENCE: {v}")),
            1 => confidence_line(),
            1 => Just(String::new()),
        ],
        proptest::collection::vec(prose(), 0..2),
    )
        .prop_map(|(before, verdict, conf, after)| {
            let mut lines = before;
            lines.push(verdict);
            lines.push(conf);
            lines.extend(after);
            lines
        })
}

/// A bare headline verdict, whose reason is the next line of argument.
fn headline_answer() -> impl Strategy<Value = Vec<String>> {
    (
        select(&DECOR[..]),
        select(&TOKENS[..]),
        select(&["", ".", "**", " (FALSE POSITIVE)", ":"][..]),
        proptest::collection::vec(select(&GAPS[..]).prop_map(str::to_owned), 0..4),
        proptest::collection::vec(body_line(), 0..3),
    )
        .prop_map(|(d, t, x, gaps, rest)| {
            let mut lines = vec![format!("{d}{t}{x}")];
            lines.extend(gaps);
            lines.extend(rest);
            lines
        })
}

fn head_text() -> impl Strategy<Value = String> {
    (
        proptest::option::weighted(0.15, select(&FIRSTS[..])),
        prop_oneof![
            3 => proptest::collection::vec(body_line(), 0..9),
            2 => labelled_answer(),
            2 => headline_answer(),
        ],
        select(&EOLS[..]),
        0usize..3,
    )
        .prop_map(|(first, body, eol, ends)| {
            let mut lines: Vec<String> = first.map(str::to_owned).into_iter().collect();
            lines.extend(body);
            join(&lines, eol, ends)
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Text,
    Empty,
    Missing,
    Dir,
    Unreadable,
    NoArg,
}

#[derive(Debug, Clone)]
struct FileCase {
    mode: Mode,
    text: String,
}

fn file_case(text: impl Strategy<Value = String>) -> impl Strategy<Value = FileCase> {
    prop_oneof![
        40 => text.prop_map(|text| FileCase { mode: Mode::Text, text }),
        1 => Just(FileCase { mode: Mode::Empty, text: String::new() }),
        1 => Just(FileCase { mode: Mode::Missing, text: String::new() }),
        1 => Just(FileCase { mode: Mode::Dir, text: String::new() }),
        1 => Just(FileCase { mode: Mode::Unreadable, text: "VERDICT: REFUTED — x\n".into() }),
        1 => Just(FileCase { mode: Mode::NoArg, text: String::new() }),
    ]
}

/// Puts the case at `path`, unless it already exists there.
fn materialize(path: &Path, c: &FileCase) {
    if path.symlink_metadata().is_ok() {
        return;
    }
    match c.mode {
        Mode::Text | Mode::Unreadable => std::fs::write(path, &c.text).unwrap(),
        Mode::Empty => std::fs::write(path, "").unwrap(),
        Mode::Dir => std::fs::create_dir(path).unwrap(),
        Mode::Missing | Mode::NoArg => {}
    }
    if c.mode == Mode::Unreadable {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
}

const STDERR_LINES: [&str; 46] = [
    "OpenAI Codex v0.46.0 (research preview)",
    "--------",
    "workdir: /tmp/x",
    "user",
    "Review the billing invoice code; a 401 means login failed.",
    "ERROR: Your workspace is out of credits",
    "Error: insufficient_quota",
    "error: You exceeded your current quota, please check your plan and billing details.",
    "[API Error: 401 Unauthorized]",
    "[API Error] Request failed",
    "ERROR: stream disconnected before completion",
    "  Error: Not logged in. Run codex login",
    "\tERROR: invalid api key",
    "ERROR: Invalid API-key provided",
    "Error: 4013 tokens exceeded",
    "Error:403",
    "error 401",
    "Errors: plural is not a marker",
    "ERROR:catalogIndex failed",
    "error: Billing hard limit reached",
    "ERROR: credit balance is too low",
    "ERROR: Quota exceeded for model",
    "ERROR: authentication failed",
    "ERROR: unauthorised",
    "ERROR: UNAUTHORIZED",
    "ERROR: unauthori\u{17f}ed",
    "\u{a0}ERROR: nbsp lead",
    "\u{3000}Error: ideographic lead",
    "\u{85}ERROR: nel lead",
    "ERROR\tx",
    "ERROR]",
    "[API Error",
    "ERROR",
    "error:",
    "ERROR: 認証エラー 401",
    "ERROR: e\u{301} combining",
    "ERROR: 1401 bad",
    "ERROR: 401",
    "ERROR: x401y",
    "ERROR: login required",
    "ERROR: log in",
    "ERROR: blogging",
    "warning: insufficient quota",
    "Error: credits out of credits",
    "ERROR: \"quoted\" \\ back",
    "",
];

fn stderr_text() -> impl Strategy<Value = String> {
    (
        proptest::collection::vec(
            prop_oneof![
                8 => select(&STDERR_LINES[..]).prop_map(str::to_owned),
                1 => (select(&["ERROR: ", "  Error: ", "[API Error: "][..]), 6usize..20)
                    .prop_map(|(p, n)| long_text(p, n)),
            ],
            0..7,
        ),
        select(&EOLS[..]),
        0usize..3,
    )
        .prop_map(|(lines, eol, ends)| join(&lines, eol, ends))
}

const NAMES: [&str; 16] = [
    "0-codex",
    "1-gemini",
    "2r1-codex",
    "2r2-gemini",
    "x-codex",
    "x-gemini",
    "10r1-codex",
    "3r12-codex",
    "1r1-gemini",
    "é-codex",
    "a.out",
    "0-codex-sweep",
    "_u",
    "-dash",
    "~t",
    "Zz",
];
const FILTERS: [Option<&str>; 10] = [
    None,
    Some(""),
    Some("r1-"),
    Some("r2-"),
    Some("r12-"),
    Some("codex"),
    Some("-"),
    Some("zzz"),
    Some("é"),
    Some("x"),
];

#[derive(Debug, Clone)]
enum Kind {
    Out(FileCase),
    Meta,
    Prompt,
    Dangling,
    Symlink(FileCase),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirMode {
    Normal,
    Missing,
    File,
}

#[derive(Debug, Clone)]
struct VoteDir {
    mode: DirMode,
    files: Vec<(&'static str, Kind)>,
    filter: Option<&'static str>,
}

fn kind() -> impl Strategy<Value = Kind> {
    prop_oneof![
        10 => file_case(head_text()).prop_map(Kind::Out),
        3 => Just(Kind::Meta),
        1 => Just(Kind::Prompt),
        1 => Just(Kind::Dangling),
        1 => file_case(head_text()).prop_map(Kind::Symlink),
    ]
}

fn vote_dir() -> impl Strategy<Value = VoteDir> {
    (
        prop_oneof![
            20 => Just(DirMode::Normal),
            1 => Just(DirMode::Missing),
            1 => Just(DirMode::File),
        ],
        proptest::collection::vec((select(&NAMES[..]), kind()), 0..7),
        select(&FILTERS[..]),
    )
        .prop_map(|(mode, files, filter)| VoteDir {
            mode,
            files,
            filter,
        })
}

fn build_vote_dir(root: &Path, v: &VoteDir) -> PathBuf {
    let dir = root.join("votes");
    match v.mode {
        DirMode::Missing => return dir,
        DirMode::File => {
            std::fs::write(&dir, "not a directory").unwrap();
            return dir;
        }
        DirMode::Normal => std::fs::create_dir_all(&dir).unwrap(),
    }
    for (i, (name, kind)) in v.files.iter().enumerate() {
        let out = dir.join(format!("{name}.out"));
        match kind {
            Kind::Out(c) => materialize(&out, c),
            Kind::Meta => {
                let _ = std::fs::write(
                    dir.join(format!("{name}.meta")),
                    "HEAD-FAILURE: quota: x\nRUNS: 1\n",
                );
            }
            Kind::Prompt => {
                let _ = std::fs::write(dir.join(format!("{name}.prompt")), "VERDICT: REFUTED\n");
            }
            Kind::Dangling => {
                if out.symlink_metadata().is_err() {
                    std::os::unix::fs::symlink(root.join("nowhere"), &out).unwrap();
                }
            }
            Kind::Symlink(c) => {
                let target = root.join(format!("target-{i}"));
                materialize(&target, c);
                if out.symlink_metadata().is_err() {
                    std::os::unix::fs::symlink(&target, &out).unwrap();
                }
            }
        }
    }
    dir
}

// ---------------------------------------------------------------- outcomes

struct VerdictOutcome {
    case: FileCase,
    input: Value,
    js: Value,
    rust: Value,
    result: Classification,
}

struct HeadFailureOutcome {
    case: FileCase,
    input: Value,
    js: Value,
    rust: Value,
    result: HeadFailure,
}

struct ArtifactsOutcome {
    case: VoteDir,
    input: Value,
    js: Value,
    rust: Value,
    result: ArtifactScan,
    reordered: bool,
}

fn printed(code: Option<i32>, stdout: &str) -> Value {
    json!({"code": code, "stdout": stdout})
}

fn file_input(c: &FileCase) -> Value {
    json!({"mode": format!("{:?}", c.mode), "text": c.text})
}

/// Each case written under `root` as `<i>.<ext>`.
fn materialize_all(root: &Path, cases: &[FileCase], ext: &str) -> Vec<PathBuf> {
    cases
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let path = root.join(format!("{i}.{ext}"));
            materialize(&path, c);
            path
        })
        .collect()
}

fn verdict_outcomes() -> &'static [VerdictOutcome] {
    static CELL: OnceLock<Vec<VerdictOutcome>> = OnceLock::new();
    CELL.get_or_init(|| {
        let cases = sample(file_case(head_text()));
        let root = scratch("classify-verdict-gen");
        let paths = materialize_all(&root, &cases, "out");
        let inputs: Vec<Value> = cases.iter().map(file_input).collect();
        let js = oracle("classify-verdict", &[VERDICT_SH], &root, &inputs, |i| {
            let r = bash(VERDICT_SH, &[paths[i].to_str().unwrap()]);
            printed(r.code, &r.stdout)
        });
        par_map(&cases, |i, c| {
            let result = classify_verdict::run(paths[i].to_str().unwrap()).unwrap();
            VerdictOutcome {
                case: c.clone(),
                input: inputs[i].clone(),
                js: js[i].clone(),
                rust: printed(Some(0), &result.to_lines()),
                result,
            }
        })
    })
}

fn head_failure_outcomes() -> &'static [HeadFailureOutcome] {
    static CELL: OnceLock<Vec<HeadFailureOutcome>> = OnceLock::new();
    CELL.get_or_init(|| {
        let cases = sample(file_case(stderr_text()));
        let root = scratch("classify-head-failure-gen");
        let paths = materialize_all(&root, &cases, "err");
        let inputs: Vec<Value> = cases.iter().map(file_input).collect();
        let arg = |i: usize| (cases[i].mode != Mode::NoArg).then(|| paths[i].to_str().unwrap());
        let js = oracle(
            "classify-head-failure",
            &[HEAD_FAILURE_SH],
            &root,
            &inputs,
            |i| {
                let r = bash(HEAD_FAILURE_SH, &arg(i).into_iter().collect::<Vec<_>>());
                printed(r.code, &r.stdout)
            },
        );
        par_map(&cases, |i, c| {
            let result = classify_head_failure::run(arg(i));
            HeadFailureOutcome {
                case: c.clone(),
                input: inputs[i].clone(),
                js: js[i].clone(),
                rust: printed(Some(0), &result.to_lines()),
                result,
            }
        })
    })
}

/// The port's line with its entries in `order`, when `order` holds exactly
/// the port's keys.
fn line_in_order(scan: &ArtifactScan, order: &[String]) -> (String, bool) {
    let ArtifactScan::Scanned(a) = scan else {
        return (scan.to_line(), false);
    };
    let mut ours: Vec<&String> = a.entries.iter().map(|(k, _)| k).collect();
    let mut theirs: Vec<&String> = order.iter().collect();
    ours.sort();
    theirs.sort();
    if ours != theirs {
        return (scan.to_line(), false);
    }
    let mut a = a.clone();
    let before: Vec<String> = a.entries.iter().map(|(k, _)| k.clone()).collect();
    a.entries
        .sort_by_key(|(k, _)| order.iter().position(|o| o == k));
    let moved = before != order;
    (ArtifactScan::Scanned(a).to_line(), moved)
}

fn script_key_order(stdout: &str) -> Vec<String> {
    serde_json::from_str::<Value>(stdout.trim_end())
        .ok()
        .and_then(|v| v.get("entries").and_then(Value::as_object).cloned())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

fn artifacts_outcomes() -> &'static [ArtifactsOutcome] {
    static CELL: OnceLock<Vec<ArtifactsOutcome>> = OnceLock::new();
    CELL.get_or_init(|| {
        let cases = sample(vote_dir());
        let root = scratch("classify-artifacts-gen");
        let dirs: Vec<PathBuf> = cases
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let case_root = root.join(i.to_string());
                std::fs::create_dir_all(&case_root).unwrap();
                build_vote_dir(&case_root, v)
            })
            .collect();
        let inputs: Vec<Value> = cases
            .iter()
            .map(|v| json!({"case": format!("{v:?}")}))
            .collect();
        let js = oracle(
            "classify-artifacts",
            &[ARTIFACTS_SH, VERDICT_SH],
            &root,
            &inputs,
            |i| {
                let mut args = vec![dirs[i].to_str().unwrap()];
                args.extend(cases[i].filter);
                let r = bash(ARTIFACTS_SH, &args);
                printed(r.code, &r.stdout)
            },
        );
        par_map(&cases, |i, v| {
            let arg = dirs[i].to_str().unwrap();
            let result = classify_artifacts::run(arg, v.filter).unwrap();
            let stdout = js[i]["stdout"].as_str().unwrap_or("");
            let (line, reordered) = line_in_order(&result, &script_key_order(stdout));
            ArtifactsOutcome {
                case: v.clone(),
                input: json!({"dir": arg, "case": format!("{v:?}")}),
                js: js[i].clone(),
                rust: printed(Some(0), &format!("{line}\n")),
                result,
                reordered,
            }
        })
    })
}

fn split<'a, T>(
    out: &'a [T],
    f: impl Fn(&'a T) -> (&'a Value, &'a Value, &'a Value),
) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let mut inputs = Vec::new();
    let mut rust = Vec::new();
    let mut js = Vec::new();
    for o in out {
        let (i, r, j) = f(o);
        inputs.push(i.clone());
        rust.push(r.clone());
        js.push(j.clone());
    }
    (inputs, rust, js)
}

#[test]
fn classify_verdict_matches_the_script_on_generated_outputs() {
    let (inputs, rust, js) = split(verdict_outcomes(), |o| (&o.input, &o.rust, &o.js));
    assert_same("classify-verdict", &inputs, &rust, &js);
}

#[test]
fn classify_head_failure_matches_the_script_on_generated_logs() {
    let (inputs, rust, js) = split(head_failure_outcomes(), |o| (&o.input, &o.rust, &o.js));
    assert_same("classify-head-failure", &inputs, &rust, &js);
}

#[test]
fn classify_artifacts_matches_the_script_on_generated_vote_dirs() {
    let (inputs, rust, js) = split(artifacts_outcomes(), |o| (&o.input, &o.rust, &o.js));
    assert_same("classify-artifacts", &inputs, &rust, &js);
}

// ---------------------------------------------------------------- coverage

fn strip_md(line: &str) -> String {
    line.chars()
        .filter(|c| !matches!(c, '*' | '`' | '_'))
        .collect()
}

fn tokenish(line: &str) -> bool {
    let l = line.to_lowercase();
    l.contains("refuted") || l.contains("confirmed")
}

fn labelled_lines(text: &str) -> usize {
    text.split('\n')
        .filter(|l| {
            let l = strip_md(l).to_lowercase();
            let l = l.trim_start_matches(|c: char| c.is_whitespace() || "#->+".contains(c));
            l.starts_with("verdict") && tokenish(l)
        })
        .count()
}

const META: [&str; 9] = [
    "confidence",
    "confianza",
    "refutationtype",
    "decisionquestioned",
    "killreason",
    "coresstands",
    "corestands",
    "source",
    "slotstatus",
];

/// For a reason the classifier took from a later line: which kinds of line
/// it passed over to get there.
fn skipped_kinds(text: &str, reason: &str) -> Vec<&'static str> {
    let lines: Vec<String> = text.split('\n').map(strip_md).collect();
    let Some(at) = lines
        .iter()
        .position(|l| !tokenish(l) && l.contains(reason))
    else {
        return vec![];
    };
    let Some(from) = lines[..at].iter().rposition(|l| tokenish(l)) else {
        return vec![];
    };
    lines[from + 1..at]
        .iter()
        .map(|l| {
            let low = l
                .to_lowercase()
                .replace('\u{212a}', "k")
                .replace("i\u{307}", "i");
            let t = low.trim_start();
            if l.chars().all(|c| c == ' ' || c == '\t') {
                "blank"
            } else if META.iter().any(|m| {
                t.strip_prefix(m)
                    .is_some_and(|r| r.trim_start().starts_with(':'))
            }) {
                "meta"
            } else if l.trim_end().ends_with(':') {
                "heading"
            } else {
                "other"
            }
        })
        .collect()
}

#[test]
fn the_generated_cases_reach_every_branch() {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut hit = |k: &str| *seen.entry(k.to_owned()).or_default() += 1;

    for o in verdict_outcomes() {
        let r = &o.result;
        hit(&format!("verdict:failclass:{}", r.failclass.as_str()));
        hit(&format!(
            "verdict:mode:{:?}:{}",
            o.case.mode,
            r.failclass.as_str()
        ));
        let first = o.case.text.split('\n').next().unwrap_or("");
        if r.failclass == Failclass::Harness {
            hit(if first.contains("TRUNCATED") {
                "verdict:harness:truncated"
            } else {
                "verdict:harness:fault"
            });
        }
        if r.failclass != Failclass::None {
            continue;
        }
        hit(match r.verdict {
            Some(Verdict::Refuted) => "verdict:REFUTED",
            Some(Verdict::NotRefuted) => "verdict:NOT_REFUTED",
            None => "verdict:none-without-verdict",
        });
        if o.case.text.contains("\r\n") {
            hit("verdict:crlf");
        }
        let labelled = labelled_lines(&o.case.text);
        hit(match labelled {
            0 => "verdict:pick:bare",
            1 => "verdict:pick:one-labelled",
            _ => "verdict:pick:last-of-several-labelled",
        });
        let conf = r.confidence.as_deref().unwrap();
        hit(match (r.confidence_declared, conf) {
            (Some(false), _) => "conf:undeclared",
            (Some(true), "0") => "conf:0",
            (Some(true), "1") => "conf:1",
            (Some(true), c) if c.starts_with("1.0") => "conf:1.0x",
            (Some(true), c) if c.starts_with("0.") => "conf:0.x",
            _ => "conf:unexpected",
        });
        if conf.matches('.').count() > 1 {
            hit("conf:not-a-float");
        }
        let reason = r.reason.as_deref().unwrap();
        if reason == NO_REASON {
            hit("reason:none");
        } else if reason.chars().count() == 400 {
            hit("reason:cut-at-400");
        }
        if reason.starts_with(' ') {
            hit("reason:leading-spaces");
        }
        if reason != NO_REASON {
            let lines: Vec<String> = o.case.text.split('\n').map(strip_md).collect();
            let from_verdict_line = lines.iter().any(|l| tokenish(l) && l.contains(reason));
            if !from_verdict_line {
                hit("reason:next-line");
                for k in skipped_kinds(&o.case.text, reason) {
                    hit(&format!("reason:skipped:{k}"));
                }
            }
        }
        let lower = o.case.text.to_lowercase();
        if (lower.contains("reason:") || lower.contains("razón:"))
            && !reason.to_lowercase().contains("reason:")
            && !reason.to_lowercase().contains("razón:")
            && reason != NO_REASON
        {
            hit("reason:label-stripped-maybe");
        }
        if o.case.text.contains("(FALSE POSITIVE)") {
            hit("reason:parenthetical-present");
        }
    }

    for o in head_failure_outcomes() {
        let r = &o.result;
        hit(&format!("hf:class:{}", r.class.as_str()));
        hit(&format!("hf:mode:{:?}", o.case.mode));
        if r.reason == "no error line in the output" {
            hit("hf:no-error-line");
        } else if r.reason.chars().count() == 200 {
            hit("hf:reason-cut-at-200");
        }
        if r.class == Failclass::Auth
            && !["unauthori", "logged", "log", "invalid", "authentication"]
                .iter()
                .any(|w| o.case.text.to_lowercase().contains(w))
        {
            hit("hf:auth-by-status-code");
        }
        if o.case.text.contains("\r\n") && r.reason.ends_with('\r') {
            hit("hf:crlf-reason");
        }
    }

    for o in artifacts_outcomes() {
        let v = &o.case;
        match &o.result {
            ArtifactScan::Skipped => hit(&format!("art:skipped:{:?}", v.mode)),
            ArtifactScan::Scanned(a) => {
                hit(match a.total {
                    0 => "art:total:0",
                    1 | 2 => "art:total:1-2",
                    _ => "art:total:3+",
                });
                let outs: Vec<&str> = v
                    .files
                    .iter()
                    .filter(|(_, k)| matches!(k, Kind::Out(c) | Kind::Symlink(c) if c.mode != Mode::Missing))
                    .map(|(n, _)| *n)
                    .collect();
                if outs.iter().any(|n| !n.contains(v.filter.unwrap_or(""))) {
                    hit("art:filter-excluded");
                }
                for (_, k) in &v.files {
                    hit(match k {
                        Kind::Out(c) => match c.mode {
                            Mode::Dir => "art:file:dir",
                            Mode::Unreadable => "art:file:unreadable",
                            Mode::Empty => "art:file:empty",
                            _ => "art:file:out",
                        },
                        Kind::Meta => "art:file:meta",
                        Kind::Prompt => "art:file:prompt",
                        Kind::Dangling => "art:file:dangling",
                        Kind::Symlink(_) => "art:file:symlink",
                    });
                }
                for (_, e) in &a.entries {
                    hit(&format!("art:entry:{}", e.failclass));
                    if e.confidence.is_none() && e.confidence_declared == Some(true) {
                        hit("art:confidence-null-but-declared");
                    }
                    if e.reason.chars().any(|c| c == '\u{7f}' || c < ' ') {
                        hit("art:reason-has-del-or-control");
                    }
                    if e.confidence.is_some_and(|c| c > 0.0 && c < 1e-4) {
                        hit("art:confidence-in-exponent-form");
                    }
                    if e.reason.chars().count() == 100 {
                        hit("art:reason-cut-at-100");
                    }
                }
                if o.reordered {
                    hit("art:collation-order-differs");
                }
            }
        }
    }
    eprintln!("{seen:#?}");

    let need = |k: &str, n: usize| {
        assert!(
            seen.get(k).copied().unwrap_or(0) >= n,
            "{k}: {} (< {n})\n{seen:#?}",
            seen.get(k).copied().unwrap_or(0)
        );
    };
    for f in [
        "none",
        "unparseable",
        "refused",
        "confined",
        "harness",
        "quota",
        "auth",
        "error",
    ] {
        need(&format!("verdict:failclass:{f}"), 5);
        need(&format!("art:entry:{f}"), 2);
    }
    for k in [
        "verdict:harness:truncated",
        "verdict:harness:fault",
        "verdict:mode:Empty:refused",
        "verdict:mode:Missing:refused",
        "verdict:mode:Dir:unparseable",
        "verdict:mode:Unreadable:unparseable",
        "verdict:crlf",
        "verdict:pick:bare",
        "verdict:pick:last-of-several-labelled",
        "conf:0",
        "conf:1",
        "conf:1.0x",
        "conf:not-a-float",
        "reason:none",
        "reason:cut-at-400",
        "reason:leading-spaces",
        "reason:skipped:blank",
        "reason:skipped:meta",
        "reason:skipped:heading",
        "reason:label-stripped-maybe",
        "reason:parenthetical-present",
        "hf:mode:Empty",
        "hf:mode:Missing",
        "hf:mode:Dir",
        "hf:mode:NoArg",
        "hf:no-error-line",
        "hf:reason-cut-at-200",
        "hf:auth-by-status-code",
        "hf:crlf-reason",
        "art:skipped:Missing",
        "art:skipped:File",
        "art:total:0",
        "art:filter-excluded",
        "art:file:dir",
        "art:file:unreadable",
        "art:file:empty",
        "art:file:meta",
        "art:file:dangling",
        "art:file:symlink",
        "art:confidence-null-but-declared",
        "art:reason-cut-at-100",
        "art:reason-has-del-or-control",
        "art:confidence-in-exponent-form",
    ] {
        need(k, 2);
    }
    for k in [
        "verdict:REFUTED",
        "verdict:NOT_REFUTED",
        "verdict:pick:one-labelled",
        "conf:undeclared",
        "conf:0.x",
        "reason:next-line",
        "hf:class:quota",
        "hf:class:auth",
        "hf:class:error",
        "art:total:3+",
    ] {
        need(k, 20);
    }
    assert!(!seen.contains_key("conf:unexpected"), "{seen:#?}");
    assert!(
        !seen.contains_key("verdict:none-without-verdict"),
        "{seen:#?}"
    );
}

// ---------------------------------------------------------------- edges

/// A script's run as the edge goldens keep it, with the plugin, which bash
/// names in a usage error, as `<PLUGIN>`.
fn edge_run(r: Run) -> Value {
    let given = plugin_root().to_string_lossy().into_owned();
    let real = plugin_root()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let stderr = r
        .stderr
        .replace(&given, "<PLUGIN>")
        .replace(&real, "<PLUGIN>");
    json!({"code": r.code, "stdout": r.stdout, "stderr": stderr})
}

fn edge_code(v: &Value) -> Option<i32> {
    v["code"].as_i64().map(|c| i32::try_from(c).unwrap())
}

fn edge_text<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap()
}

/// The verdict classifier's edge cases: the files they read, kept for the
/// tests, and what the script printed for each.
struct VerdictEdges {
    round_trip: PathBuf,
    invalid: PathBuf,
    nul: PathBuf,
    usage: Value,
    round_trip_js: Value,
    invalid_js: Value,
    nul_js: Value,
}

const INVALID_UTF8: &[u8] = b"VERDICT: REFUTED bad \xff byte\nCONFIDENCE: 0.7\n";
const WITH_NUL: &[u8] = b"VERDICT: REFUTED nul\x00here\n";

fn verdict_edges() -> &'static VerdictEdges {
    static CELL: OnceLock<VerdictEdges> = OnceLock::new();
    CELL.get_or_init(|| {
        let dir = scratch("classify-verdict-edges");
        let log = dir.join("err");
        std::fs::write(
            &log,
            "user\nprompt about billing\nERROR: Your workspace is out of credits\n",
        )
        .unwrap();
        let hf = classify_head_failure::run(log.to_str());
        let round_trip_text = format!("{}\nlater text\n", hf.artifact_line());
        let round_trip = dir.join("0-codex.out");
        std::fs::write(&round_trip, &round_trip_text).unwrap();
        let invalid = dir.join("invalid.out");
        std::fs::write(&invalid, INVALID_UTF8).unwrap();
        let nul = dir.join("nul.out");
        std::fs::write(&nul, WITH_NUL).unwrap();
        let path = |p: &Path| p.to_str().unwrap().to_owned();
        let args = [String::new(), path(&round_trip), path(&invalid), path(&nul)];
        let inputs = [
            json!({"args": [&args[0]]}),
            json!({"args": [&args[1]], "text": round_trip_text}),
            json!({"args": [&args[2]], "bytes": INVALID_UTF8.escape_ascii().to_string()}),
            json!({"args": [&args[3]], "bytes": WITH_NUL.escape_ascii().to_string()}),
        ];
        let js = oracle("classify-edge-verdict", &[VERDICT_SH], &dir, &inputs, |i| {
            edge_run(bash(VERDICT_SH, &[args[i].as_str()]))
        });
        VerdictEdges {
            round_trip,
            invalid,
            nul,
            usage: js[0].clone(),
            round_trip_js: js[1].clone(),
            invalid_js: js[2].clone(),
            nul_js: js[3].clone(),
        }
    })
}

/// `classify-head-failure.sh ""`, as the script printed it.
fn head_failure_usage() -> &'static Value {
    static CELL: OnceLock<Value> = OnceLock::new();
    CELL.get_or_init(|| {
        let dir = scratch("classify-head-failure-edges");
        let js = oracle(
            "classify-edge-head-failure",
            &[HEAD_FAILURE_SH],
            &dir,
            &[json!({"args": [""]})],
            |_| edge_run(bash(HEAD_FAILURE_SH, &[""])),
        );
        js[0].clone()
    })
}

/// classify-artifacts's edge cases: the vote directory the second reads, and
/// what the script printed for a call with no arguments and for one whose
/// `tribunal-root` names a tree without the classifier.
struct ArtifactsEdges {
    votes: PathBuf,
    usage: Value,
    no_classifier: Value,
}

fn artifacts_edges() -> &'static ArtifactsEdges {
    static CELL: OnceLock<ArtifactsEdges> = OnceLock::new();
    CELL.get_or_init(|| {
        let dir = scratch("classify-artifacts-edges");
        let votes = dir.join("votes");
        std::fs::create_dir_all(&votes).unwrap();
        std::fs::write(votes.join("0-codex.out"), "VERDICT: REFUTED\n").unwrap();
        let nowhere = dir.join("nowhere");
        let inputs = [
            json!({"args": []}),
            json!({
                "args": [votes.to_str().unwrap()],
                "tribunalRoot": nowhere.to_str().unwrap(),
                "files": {"0-codex.out": "VERDICT: REFUTED\n"},
            }),
        ];
        let js = oracle(
            "classify-edge-artifacts",
            &[ARTIFACTS_SH],
            &dir,
            &inputs,
            |i| {
                if i == 0 {
                    return edge_run(bash(ARTIFACTS_SH, &[]));
                }
                let bin = stub_bin("classify-empty-root", &nowhere);
                edge_run(bash_with(&bin, ARTIFACTS_SH, &[votes.to_str().unwrap()]))
            },
        );
        ArtifactsEdges {
            votes,
            usage: js[0].clone(),
            no_classifier: js[1].clone(),
        }
    })
}

#[test]
fn usage_errors_match_the_scripts() {
    let r = &verdict_edges().usage;
    assert_eq!(edge_code(r), Some(1));
    assert!(
        edge_text(r, "stdout").is_empty()
            && edge_text(r, "stderr").contains("usage: classify-verdict.sh <file>")
    );
    assert_eq!(
        classify_verdict::run(""),
        Err("usage: classify-verdict.sh <file>".to_owned())
    );

    let r = &artifacts_edges().usage;
    assert_eq!(edge_code(r), Some(1));
    assert!(edge_text(r, "stderr")
        .contains("usage: classify-artifacts.sh <artifact-dir> [suffix-filter]"));
    assert!(classify_artifacts::run("", None).is_err());

    let r = head_failure_usage();
    assert_eq!(edge_code(r), Some(0));
    assert_eq!(
        edge_text(r, "stdout"),
        classify_head_failure::run(Some("")).to_lines()
    );
}

/// A failed head's artifact line, read back by the verdict classifier, keeps
/// its class and reason: the round trip HEAD-FAILURE makes.
#[test]
fn a_head_failure_line_round_trips_through_the_verdict_classifier() {
    let edges = verdict_edges();
    let rust = classify_verdict::run(edges.round_trip.to_str().unwrap()).unwrap();
    assert_eq!(edge_text(&edges.round_trip_js, "stdout"), rust.to_lines());
    assert_eq!(rust.failclass, Failclass::Quota);
    assert_eq!(
        rust.reason.as_deref(),
        Some("ERROR: Your workspace is out of credits")
    );
}

/// Declared difference: the script's bash sorts `*.out` by the locale's
/// collation, the port by bytes. The entries are the same.
#[test]
fn entry_order_is_a_declared_difference() {
    let dir = scratch("classify-order-witness");
    for name in ["a-c", "a_c"] {
        std::fs::write(dir.join(format!("{name}.out")), "VERDICT: REFUTED — x\n").unwrap();
    }
    let js = oracle(
        "classify-entry-order",
        &[ARTIFACTS_SH, VERDICT_SH],
        &dir,
        &[json!(["a-c.out", "a_c.out"])],
        |_| {
            let r = bash(ARTIFACTS_SH, &[dir.to_str().unwrap()]);
            printed(r.code, &r.stdout)
        },
    );
    let js_stdout = js[0]["stdout"].as_str().unwrap();
    let rust = classify_artifacts::run(dir.to_str().unwrap(), None).unwrap();
    assert_eq!(script_key_order(js_stdout), ["a_c", "a-c"]);
    let ArtifactScan::Scanned(a) = &rust else {
        panic!("scanned")
    };
    assert_eq!(a.entries[0].0, "a-c");
    let (line, moved) = line_in_order(&rust, &script_key_order(js_stdout));
    assert!(moved);
    assert_eq!(format!("{line}\n"), js_stdout);
}

/// Declared difference: `tribunal-root` naming a tree without the classifier
/// makes the script print `{}`; the port carries its classifier.
#[test]
fn a_missing_classifier_is_a_declared_difference() {
    let edges = artifacts_edges();
    assert_eq!(edge_text(&edges.no_classifier, "stdout"), "{}\n");
    let rust = classify_artifacts::run(edges.votes.to_str().unwrap(), None).unwrap();
    assert!(matches!(rust, ArtifactScan::Scanned(ref a) if a.total == 1));
}

/// Declared difference: bytes that are not UTF-8 and NUL bytes. BSD sed stops
/// at the first line holding an invalid sequence and bash drops NUL from every
/// `$(…)`; the port reads the file lossily and keeps NUL.
#[test]
fn invalid_utf8_and_nul_are_a_declared_difference() {
    let edges = verdict_edges();
    assert_eq!(
        edge_text(&edges.invalid_js, "stdout"),
        "FAILCLASS: unparseable\n"
    );
    let rust = classify_verdict::run(edges.invalid.to_str().unwrap()).unwrap();
    assert_eq!(rust.failclass, Failclass::None);
    assert_eq!(rust.reason.as_deref(), Some("bad \u{FFFD} byte"));

    let js = edge_text(&edges.nul_js, "stdout");
    assert!(js.ends_with("REASON: nulhere\n"), "{js}");
    let rust = classify_verdict::run(edges.nul.to_str().unwrap()).unwrap();
    assert_eq!(rust.reason.as_deref(), Some("nul\0here"));
}

/// The staleness check on a tree of its own: a script edited since the
/// recording is stale, one deleted (retired with the Workflow) is not, and
/// with the script the cases run deleted nothing is.
#[test]
fn a_retired_script_is_not_stale() {
    let root = scratch("classify-retired");
    std::fs::create_dir_all(root.join("lib")).unwrap();
    let write = |name: &str, text: &str| std::fs::write(root.join(name), text).unwrap();
    write("lib/run.sh", "run");
    write("lib/dep.sh", "dep edited");
    let recorded = json!({"lib/run.sh": "run", "lib/dep.sh": "dep", "lib/gone.sh": "gone"});
    let scripts = ["lib/run.sh", "lib/dep.sh", "lib/gone.sh"];
    assert_eq!(stale_scripts(&recorded, &scripts, &root), ["lib/dep.sh"]);
    write("lib/dep.sh", "dep");
    assert!(stale_scripts(&recorded, &scripts, &root).is_empty());
    write("lib/new.sh", "new");
    assert_eq!(
        stale_scripts(&recorded, &["lib/run.sh", "lib/new.sh"], &root),
        ["lib/new.sh"]
    );
    std::fs::remove_file(root.join("lib/run.sh")).unwrap();
    write("lib/dep.sh", "dep edited again");
    assert!(stale_scripts(&recorded, &scripts, &root).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}
