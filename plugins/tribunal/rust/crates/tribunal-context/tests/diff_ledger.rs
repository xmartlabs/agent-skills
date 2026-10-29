//! `locate-last-row.mjs` and `ledger-lock.mjs` against `ledger`: the same
//! generated `metrics.jsonl` and arguments on both sides, compared on stdout,
//! stderr and exit status; the lock in each state it can find on disk.
//!
//! The script's side of the generated ledgers is read from
//! `tests/goldens/locate-last-row.json`, which keeps the text of the script
//! and of every module it imports, and a hash of each case's input. Regenerate
//! after changing them:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_ledger`.

mod posting;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use posting::*;
use proptest::prelude::*;
use serde_json::{json, Value};
use tribunal_context::harness_version::sha256_hex;
use tribunal_context::ledger::{self, LedgerEnv};

const GOLDEN: &str = "locate-last-row.json";
const LOCATE: &str = "skills/review-pr/scripts/locate-last-row.mjs";
const REGENERATE: &str =
    "regenerate: TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_ledger";

#[derive(Debug, Clone)]
enum FileState {
    Lines(Vec<String>, bool),
    Missing,
    Empty,
    Directory,
}

#[derive(Debug, Clone)]
struct Case {
    file: FileState,
    argv: Vec<String>,
}

fn field(key: &'static str, values: Vec<&'static str>) -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        1 => Just(None),
        3 => proptest::sample::select(values).prop_map(move |v| Some(format!("\"{key}\":{v}"))),
    ]
}

fn object_row() -> impl Strategy<Value = String> {
    (
        field(
            "pr",
            vec![
                "12",
                "\"12\"",
                "13",
                "null",
                "\"undefined\"",
                "[12]",
                "true",
                "12.0",
            ],
        ),
        field("headSha", vec!["\"abc\"", "\"def\"", "true", "5", "null"]),
        field("repo", vec!["\"o/r\"", "\"x/y\"", "\"\"", "5"]),
        proptest::sample::subsequence(
            vec![
                "\"1\":\"i\"",
                "\"b\":1.0",
                "\"n\":1e21",
                "\"s\":\"\\u00e9\\u0001\\n\"",
                "\"z\":[1,{\"2\":0,\"a\":-0}]",
                "\"pr\":99",
                "\"__proto__\":{\"x\":1}",
            ],
            0..3,
        ),
        any::<bool>(),
    )
        .prop_map(|(pr, sha, repo, extra, dup_first)| {
            let mut parts: Vec<String> = Vec::new();
            if dup_first {
                parts.extend(extra.iter().map(|s| (*s).to_owned()));
            }
            parts.extend([pr, sha, repo].into_iter().flatten());
            if !dup_first {
                parts.extend(extra.iter().map(|s| (*s).to_owned()));
            }
            format!("{{{}}}", parts.join(","))
        })
}

fn line() -> impl Strategy<Value = String> {
    prop_oneof![
        8 => object_row(),
        1 => proptest::sample::select(vec![
            "null", "5", "\"str\"", "[1,2]", "{bad", "", "  ", "{\"pr\":12}\r", "{\"pr\":12,}", "true",
        ])
        .prop_map(str::to_owned),
    ]
}

fn arg(name: &'static str, values: Vec<&'static str>) -> impl Strategy<Value = Vec<String>> {
    prop_oneof![
        2 => Just(Vec::new()),
        1 => Just(vec![format!("--{name}")]),
        4 => proptest::sample::select(values).prop_map(move |v| vec![format!("--{name}"), v.to_owned()]),
    ]
}

fn case() -> impl Strategy<Value = Case> {
    let lines = (
        proptest::collection::vec(line(), 0..8),
        0usize..4,
        47usize..52,
        any::<bool>(),
    )
        .prop_map(|(mut lines, pad, fill, trailing)| {
            if pad == 0 {
                let filler: Vec<String> = (0..fill)
                    .map(|i| format!("{{\"pr\":{}}}", 1000 + i))
                    .collect();
                lines.splice(lines.len().min(1)..lines.len().min(1), filler);
            }
            FileState::Lines(lines, trailing)
        });
    (
        prop_oneof![
            12 => lines,
            1 => Just(FileState::Missing),
            1 => Just(FileState::Empty),
            1 => Just(FileState::Directory),
        ],
        arg("pr", vec!["12", "undefined", "13", "true"]),
        arg("head-sha", vec!["abc", "def", ""]),
        any::<bool>(),
    )
        .prop_map(|(file, pr, sha, sha_first)| {
            let argv = if sha_first {
                [sha, pr].concat()
            } else {
                [pr, sha].concat()
            };
            Case { file, argv }
        })
}

fn write_state(data: &Path, state: &FileState) {
    let metrics = data.join("metrics.jsonl");
    match state {
        FileState::Lines(lines, trailing) => {
            let mut text = lines.join("\n");
            if *trailing {
                text.push('\n');
            }
            std::fs::write(metrics, text).unwrap();
        }
        FileState::Missing => {}
        FileState::Empty => std::fs::write(metrics, "\n\n").unwrap(),
        FileState::Directory => std::fs::create_dir(metrics).unwrap(),
    }
}

fn env_for(root: &Path) -> (LedgerEnv, Vec<(&'static str, String)>) {
    let home = root.join("home");
    let data = root.join("data");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    let env = LedgerEnv {
        home: home.to_string_lossy().into_owned(),
        cwd: root.to_string_lossy().into_owned(),
        data_dir: Some(data.to_string_lossy().into_owned()),
        project_dir: None,
        lock_wait: ledger::WAIT,
    };
    let vars = vec![
        ("HOME", env.home.clone()),
        ("TRIBUNAL_DATA_DIR", env.data_dir.clone().unwrap()),
    ];
    (env, vars)
}

fn outcome(js: &Run) -> &'static str {
    let e = &js.stderr;
    if js.code == 0 {
        "found"
    } else if e.contains("last row is pr") {
        "refused-pr"
    } else if e.contains("headSha") && e.contains("does not match") {
        "refused-sha"
    } else if e.contains("is empty") {
        "empty"
    } else if e.contains("could not read") {
        "unreadable"
    } else if e.contains("does not parse") {
        "unparseable"
    } else if e.contains("Cannot read properties of null") {
        "null-row"
    } else {
        "other"
    }
}

#[test]
fn locate_last_row_matches_js_on_generated_ledgers() {
    let root = scratch("locate");
    let cases = sample(CASES, case());
    let scratch_text = root.to_string_lossy().into_owned();
    let plugin_text = plugin_root().to_string_lossy().into_owned();
    let to_golden = [
        (scratch_text.as_str(), "<SCRATCH>"),
        (plugin_text.as_str(), "<PLUGIN>"),
    ];
    let from_golden = [
        ("<SCRATCH>", scratch_text.as_str()),
        ("<PLUGIN>", plugin_text.as_str()),
    ];
    let with = json!(module_texts(LOCATE));
    let golden = recorded(GOLDEN, &with, REGENERATE);
    let replay: Option<&Vec<Value>> = golden.as_ref().map(|g| {
        let recorded = g["cases"].as_array().unwrap();
        assert_eq!(
            recorded.len(),
            cases.len(),
            "the generator changed; {REGENERATE}"
        );
        recorded
    });
    let seen = Mutex::new(BTreeMap::<String, usize>::new());
    let runs = parallel(&cases, |i, c| {
        let dir = root.join(i.to_string());
        let (env, vars) = env_for(&dir);
        write_state(&dir.join("data"), &c.file);
        // A hash: the long ledgers would make most of the file.
        let input = json!(sha256_hex(format!("{c:?}").as_bytes()));
        let js = match replay {
            Some(r) => {
                assert_eq!(r[i]["input"], input, "the generator changed; {REGENERATE}");
                run_from(&swapped(&r[i]["js"], &from_golden))
            }
            None => node(
                &script("locate-last-row.mjs"),
                &c.argv,
                &dir,
                &vars,
                &safe_path(None),
            ),
        };
        let kept = json!({"input": input, "js": swapped(&run_json(&js), &to_golden)});
        let rs = ledger::run(&c.argv, &env);
        let mut seen = seen.lock().unwrap();
        *seen.entry(outcome(&js).into()).or_default() += 1;
        if js.code == 0 {
            if let FileState::Lines(lines, _) = &c.file {
                let last = lines.iter().rev().find(|l| !l.is_empty()).unwrap();
                if js.stdout.trim_end() != ledger_row(last) {
                    *seen.entry("found-older-row".into()).or_default() += 1;
                }
                if lines.len() > 50 {
                    *seen.entry("found-in-long-ledger".into()).or_default() += 1;
                }
            }
        } else if matches!(&c.file, FileState::Lines(l, _) if l.len() > 50) {
            *seen.entry("refused-long-ledger".into()).or_default() += 1;
        }
        (diff_cli(&js, &rs).map(|d| format!("{c:?}\n{d}")), kept)
    });
    let (diffs, kept): (Vec<Option<String>>, Vec<Value>) = runs.into_iter().unzip();
    if replay.is_none() {
        write_golden(GOLDEN, with, kept);
    }
    eprintln!("{:?}", seen.lock().unwrap());
    assert_same("locate-last-row", &diffs);
    let seen = seen.into_inner().unwrap();
    for k in [
        "found",
        "refused-pr",
        "refused-sha",
        "empty",
        "unreadable",
        "unparseable",
        "null-row",
        "found-older-row",
        "found-in-long-ledger",
        "refused-long-ledger",
    ] {
        assert!(
            seen.get(k).copied().unwrap_or(0) >= 3,
            "{k} reached too rarely: {seen:?}"
        );
    }
}

/// The row as the script prints it, for telling a newest-row hit from an
/// older one.
fn ledger_row(line: &str) -> String {
    tribunal_context::operator::json_parse(line)
        .map(|v| tribunal_core::js::stringify(&v, ""))
        .unwrap_or_default()
}

/// A row beyond the 50 the scan reads is not found, even when it matches.
#[test]
fn locate_last_row_scans_only_the_last_fifty_rows() {
    let dir = scratch("locate-window");
    let (env, vars) = env_for(&dir);
    let argv = vec!["--pr".to_owned(), "7".to_owned()];
    // The run's row 50th from the end is found, 51st is not: either side of
    // the window, so neither a smaller nor a larger one passes.
    for (after, found) in [(49, true), (50, false)] {
        let mut lines = vec!["{\"pr\":7}".to_owned()];
        lines.extend((0..after).map(|i| format!("{{\"pr\":{}}}", 100 + i)));
        std::fs::write(dir.join("data/metrics.jsonl"), lines.join("\n")).unwrap();
        let js = node(
            &script("locate-last-row.mjs"),
            &argv,
            &dir,
            &vars,
            &safe_path(None),
        );
        let rs = ledger::run(&argv, &env);
        if found {
            assert_eq!((js.code, js.stdout.as_str()), (0, "{\"pr\":7}\n"), "{js:?}");
        } else {
            assert_eq!(js.code, 1, "{js:?}");
            assert!(
                js.stderr
                    .contains(&format!("last row is pr {}, expected 7", 99 + after)),
                "{js:?}"
            );
        }
        assert_eq!(diff_cli(&js, &rs), None, "{after} rows after the run's");
    }
}

// ---------------------------------------------------------------------------
// ledger-lock

const LOCK_HARNESS: &str = r#"
import { writeFileSync } from "node:fs";
const { withLedgerLock } = await import(process.argv[2]);
const [path, marker] = process.argv.slice(3);
try {
  const r = withLedgerLock(path, () => {
    writeFileSync(marker, "ran");
    if (process.env.LOCK_THROW) throw new Error("boom");
    return "ok";
  });
  process.stdout.write(`${r}\n`);
} catch (e) {
  process.stdout.write(`ERR ${e.message}\n`);
}
"#;

fn rust_lock(path: &str, marker: &Path, throw: bool, wait: Duration) -> String {
    match ledger::with_ledger_lock(path, wait, || {
        std::fs::write(marker, "ran").unwrap();
        if throw {
            Err("boom".to_owned())
        } else {
            Ok("ok".to_owned())
        }
    }) {
        Ok(Ok(r)) => format!("{r}\n"),
        Ok(Err(e)) | Err(e) => format!("ERR {e}\n"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum LockState {
    Free,
    Stale,
    Held,
    ReleasedSoon,
    NoParent,
}

/// What one side leaves behind: its output, whether `fn` ran, whether the
/// lock directory is still there.
#[derive(Debug, PartialEq)]
struct LockResult {
    out: String,
    ran: bool,
    lock_left: bool,
}

fn lock_setup(dir: &Path, state: LockState) -> String {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    let path = if state == LockState::NoParent {
        dir.join("missing/metrics.jsonl")
    } else {
        dir.join("metrics.jsonl")
    };
    let lock = format!("{}.lock", path.display());
    match state {
        LockState::Free | LockState::NoParent => {}
        LockState::Stale => {
            std::fs::create_dir(&lock).unwrap();
            let ok = std::process::Command::new("touch")
                .args(["-t", "202001010000", &lock])
                .status()
                .unwrap()
                .success();
            assert!(ok);
        }
        LockState::Held | LockState::ReleasedSoon => std::fs::create_dir(&lock).unwrap(),
    }
    if state == LockState::ReleasedSoon {
        let lock = lock.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            let _ = std::fs::remove_dir(lock);
        });
    }
    path.to_string_lossy().into_owned()
}

/// One lock state on both sides. A held lock gets the short wait on both, so
/// the test does not sit out the real 15 s; the other states never reach it.
fn lock_case(root: &Path, state: LockState, throw: bool) -> (LockResult, LockResult, Duration) {
    let short = state == LockState::Held;
    let wait = if short {
        Duration::from_millis(LOCK_WAIT_MS)
    } else {
        ledger::WAIT
    };
    let harness = root.join("harness.mjs");
    std::fs::write(&harness, LOCK_HARNESS).unwrap();
    let module = script("ledger-lock.mjs").to_string_lossy().into_owned();
    let tag = format!("{state:?}-{throw}");
    let js_dir = root.join(format!("js-{tag}"));
    let rs_dir = root.join(format!("rs-{tag}"));
    let js_path = lock_setup(&js_dir, state);
    let rs_path = lock_setup(&rs_dir, state);
    let started = Instant::now();
    let (js, rs) = std::thread::scope(|s| {
        let js = s.spawn(|| {
            let mut vars = vec![("HOME", root.to_string_lossy().into_owned())];
            if throw {
                vars.push(("LOCK_THROW", "1".into()));
            }
            if short {
                vars.push(("TRIBUNAL_LEDGER_LOCK_WAIT_MS", LOCK_WAIT_MS.to_string()));
            }
            node(
                &harness,
                &[
                    module.clone(),
                    js_path.clone(),
                    js_dir.join("marker").to_string_lossy().into_owned(),
                ],
                root,
                &vars,
                &safe_path(None),
            )
        });
        let rs = s.spawn(|| rust_lock(&rs_path, &rs_dir.join("marker"), throw, wait));
        (js.join().unwrap(), rs.join().unwrap())
    });
    let elapsed = started.elapsed();
    assert_eq!(js.code, 0, "{js:?}");
    let result = |out: String, dir: &Path, path: &str| LockResult {
        out: out.replace(&dir.to_string_lossy().into_owned(), "<dir>"),
        ran: dir.join("marker").exists(),
        lock_left: Path::new(&format!("{path}.lock")).exists(),
    };
    (
        result(js.stdout, &js_dir, &js_path),
        result(rs, &rs_dir, &rs_path),
        elapsed,
    )
}

#[test]
fn ledger_lock_matches_js_in_every_state() {
    let root = scratch("lock");
    let mut held_waited = Duration::ZERO;
    for (state, throw) in [
        (LockState::Free, false),
        (LockState::Free, true),
        (LockState::Stale, false),
        (LockState::ReleasedSoon, false),
        (LockState::NoParent, false),
        (LockState::Held, false),
    ] {
        let (js, rs, elapsed) = lock_case(&root, state, throw);
        assert_eq!(js, rs, "{state:?} throw={throw}");
        match state {
            LockState::Free | LockState::Stale | LockState::ReleasedSoon => {
                assert!(js.ran && !js.lock_left, "{state:?}: {js:?}");
                assert_eq!(js.out, if throw { "ERR boom\n" } else { "ok\n" });
            }
            LockState::NoParent => {
                assert!(
                    js.out
                        .starts_with("ERR ENOENT: no such file or directory, mkdir '"),
                    "{js:?}"
                );
            }
            LockState::Held => {
                held_waited = elapsed;
                assert!(!js.ran && js.lock_left, "{js:?}");
                assert!(
                    js.out.contains(&format!("within {LOCK_WAIT_MS} ms")),
                    "{js:?}"
                );
            }
        }
    }
    assert!(
        held_waited >= Duration::from_millis(LOCK_WAIT_MS),
        "{held_waited:?}"
    );
}

/// The wait the script reads from `TRIBUNAL_LEDGER_LOCK_WAIT_MS` against the
/// port's `lock_wait_from`, the 15 s default included, so shortening it for
/// the tests above cannot hide a change to the real wait.
#[test]
fn the_lock_wait_reads_the_variable_as_the_script_does() {
    let module = script("ledger-lock.mjs").to_string_lossy().into_owned();
    let probe = format!(
        "const m = await import({module:?}); process.stdout.write(`${{m.DEFAULT_WAIT_MS}} ${{m.WAIT_MS}}`);"
    );
    let root = scratch("lock-wait");
    let probe_path = root.join("probe.mjs");
    std::fs::write(&probe_path, probe).unwrap();
    for v in [
        None,
        Some(""),
        Some("300"),
        Some("1"),
        Some("0"),
        Some("007"),
        Some("-5"),
        Some("1.5"),
        Some("1e3"),
        Some(" 300"),
        Some("abc"),
    ] {
        let vars: Vec<(&str, String)> = v
            .map(|v| ("TRIBUNAL_LEDGER_LOCK_WAIT_MS", v.to_owned()))
            .into_iter()
            .collect();
        let js = node(&probe_path, &[], &root, &vars, &safe_path(None));
        let rust = format!(
            "{} {}",
            ledger::WAIT.as_millis(),
            ledger::lock_wait_from(v).as_millis()
        );
        assert_eq!(js.stdout, rust, "{v:?}: {}", js.stderr);
    }
}

/// Declared difference: an escaped lone surrogate in a row comes back as
/// U+FFFD in the port, where the script prints the escape.
#[test]
fn a_lone_surrogate_row_is_a_declared_difference() {
    let dir = scratch("locate-witness");
    let (env, vars) = env_for(&dir);
    std::fs::write(
        dir.join("data/metrics.jsonl"),
        "{\"pr\":7,\"x\":\"\\ud800\"}\n",
    )
    .unwrap();
    let argv = vec!["--pr".to_owned(), "7".to_owned()];
    let js = node(
        &script("locate-last-row.mjs"),
        &argv,
        &dir,
        &vars,
        &safe_path(None),
    );
    let rs = ledger::run(&argv, &env);
    assert_eq!(js.stdout, "{\"pr\":7,\"x\":\"\\ud800\"}\n");
    assert_eq!(rs.stdout, "{\"pr\":7,\"x\":\"\u{FFFD}\"}\n");
}
