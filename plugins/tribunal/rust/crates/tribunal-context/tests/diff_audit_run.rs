//! `lib/audit-run.mjs` against `tribunal_context::audit_run`: the same
//! metrics on stdin and the same vote directory and arguments on both sides,
//! run as `persist-run.sh` runs it, compared on the exit code and on the
//! printed line, byte for byte.
mod common;
#[path = "../../tribunal-core/tests/common/mod.rs"]
mod core_common;

use common::scratch;
use core_common::{assert_same, sample};

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use proptest::collection::vec;
use proptest::prelude::*;
use proptest::sample::select;
use serde_json::{json, Map, Value};
use tribunal_context::audit_run::run;

const SCRIPT: &str = "lib/audit-run.mjs";

fn node(args: &[String], stdin: &str) -> (Option<i32>, String, String) {
    let mut child = Command::new("node")
        .arg(common::plugin_root().join(SCRIPT))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("node must be on PATH");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

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

// ---------------------------------------------------------------- pools

fn to_string_obj() -> Value {
    json!({"toString": 1})
}

fn flags() -> Vec<Value> {
    vec![
        json!(true),
        json!(false),
        json!(1),
        json!(0),
        json!(""),
        json!("yes"),
        Value::Null,
        json!([]),
        json!({}),
    ]
}

/// Values a `> 0` comparison sees, including the ones `ToPrimitive` throws on.
fn counts() -> Vec<Value> {
    vec![
        json!(0),
        json!(3),
        json!(-2),
        json!(1.5),
        json!("5"),
        json!("abc"),
        json!(""),
        json!(true),
        json!(false),
        json!([3]),
        json!([1, 2]),
        json!([]),
        json!({}),
        to_string_obj(),
        json!([[to_string_obj()]]),
        Value::Null,
        json!("0x10"),
        json!(" 7 "),
        serde_json::from_str("1e21").unwrap(),
        json!("Infinity"),
    ]
}

fn titles() -> Vec<Value> {
    vec![
        json!("Null deref in payments"),
        json!("Ünïcode — title 😀"),
        json!(""),
        Value::Null,
        json!(5),
        to_string_obj(),
        json!(["a", null, "b"]),
        json!({}),
        json!("quote \" and \\ back"),
        json!("line\nbreak\u{2028}"),
        json!("Missing guard"),
        json!("Missing guard"),
    ]
}

fn slot_numbers() -> Vec<Value> {
    vec![
        json!(0),
        json!(1),
        json!(2),
        json!(18),
        json!(37),
        json!(38),
        json!(39),
        json!(-1),
        json!(1.5),
        json!("18"),
        Value::Null,
        json!(true),
    ]
}

fn agent_numbers() -> Vec<Value> {
    vec![
        json!(0),
        json!(10),
        json!(12),
        json!(8),
        json!(7),
        json!(100),
        json!(79),
        json!(121),
        json!(0.3),
        json!(3),
        json!(-5),
        json!(1.7e308),
        json!(-1.7e308),
        json!(5e-324),
        json!("10"),
        Value::Null,
        json!(83.25),
        json!(1e-7),
    ]
}

fn slot_details() -> Vec<Value> {
    vec![
        Value::Null,
        json!("x"),
        json!(5),
        json!({"source": "codex", "reason": "out of credits"}),
        json!({"source": "codex", "reason": "out of credits"}),
        json!({"source": "gemini", "reason": "timeout"}),
        json!({"source": "claude", "reason": "x"}),
        json!({"source": "", "reason": "y"}),
        json!({"source": "codex"}),
        json!({"source": 5, "reason": null}),
        json!({"source": to_string_obj(), "reason": "r"}),
        json!({"source": ["a", "b"], "reason": {}}),
        json!({"source": "gemini", "reason": [to_string_obj()]}),
    ]
}

fn missing_arrays() -> Vec<Value> {
    vec![
        json!([]),
        json!([]),
        json!(["b.ts:12"]),
        json!(["a.ts:3", "c.ts"]),
        json!([null, 5]),
        json!([to_string_obj()]),
        json!("str"),
        json!([["x", "y"]]),
        json!({}),
    ]
}

fn reasons() -> Vec<Value> {
    vec![
        json!(""),
        json!("checksum mismatch"),
        json!("r".repeat(300)),
        // The cut at 120 units lands right after the pair, never inside it.
        json!(format!("{}😀tail", "x".repeat(118))),
        json!("é".repeat(130)),
        json!(5),
        Value::Null,
        json!(["a", "b"]),
        to_string_obj(),
    ]
}

// ---------------------------------------------------------------- generators

fn pick(pool: Vec<Value>) -> BoxedStrategy<Value> {
    select(pool).boxed()
}

/// A field that is absent about half the time.
fn field(s: BoxedStrategy<Value>) -> BoxedStrategy<Option<Value>> {
    prop_oneof![1 => Just(None), 1 => s.prop_map(Some)].boxed()
}

fn object(fields: Vec<(&'static str, BoxedStrategy<Option<Value>>)>) -> BoxedStrategy<Value> {
    let (names, strategies): (Vec<_>, Vec<_>) = fields.into_iter().unzip();
    strategies
        .prop_map(move |vals| {
            let mut m = Map::new();
            for (k, v) in names.iter().zip(vals) {
                if let Some(v) = v {
                    m.insert((*k).to_owned(), v);
                }
            }
            Value::Object(m)
        })
        .boxed()
}

fn finding() -> BoxedStrategy<Value> {
    let detail = prop_oneof![
        1 => vec(pick(slot_details()), 0..4).prop_map(Value::Array),
        1 => pick(vec![json!(5), json!(1.5), json!(true), json!(true), json!({}), json!({}), json!("abc"), Value::Null, json!(0)]),
    ]
    .boxed();
    let debate = pick(vec![
        json!({"outcome": "unadjudicated"}),
        json!({"outcome": "unadjudicated"}),
        json!({"outcome": "settled"}),
        json!("unadjudicated"),
        Value::Null,
        json!({}),
    ]);
    object(vec![
        ("title", field(pick(titles()))),
        (
            "panelRan",
            prop_oneof![1 => field(pick(flags())), 1 => Just(Some(json!(true)))].boxed(),
        ),
        (
            "hadExternalVoter",
            field(pick(vec![
                json!(false),
                json!(true),
                json!(0),
                Value::Null,
                json!(false),
            ])),
        ),
        ("failedSlotsDetail", field(detail)),
        ("blocking", field(pick(flags()))),
        ("adjudicated", field(pick(flags()))),
        (
            "slotId",
            field(pick(vec![
                json!("slot-3"),
                json!(""),
                json!(7),
                to_string_obj(),
            ])),
        ),
        ("debate", field(debate)),
        (
            "mergedLocationsMissingFromReport",
            field(pick(missing_arrays())),
        ),
        ("killReasonMissingFromReport", field(pick(missing_arrays()))),
        (
            "mergedFromAnnotationMissingFromReport",
            field(pick(missing_arrays())),
        ),
        (
            "groundedDiagnosticMissingFromReport",
            field(pick(missing_arrays())),
        ),
        ("unexaminedWordingInReport", field(pick(missing_arrays()))),
    ])
}

fn findings() -> BoxedStrategy<Value> {
    let odd = pick(vec![
        Value::Null,
        json!(5),
        json!("s"),
        json!([]),
        json!(true),
    ]);
    prop_oneof![
        6 => vec(finding(), 0..6).prop_map(Value::Array),
        1 => vec(prop_oneof![5 => finding(), 1 => odd], 1..5).prop_map(Value::Array),
        1 => pick(vec![Value::Null, json!("abc"), json!({}), json!(5)]),
    ]
    .boxed()
}

fn vote_reconciliation() -> BoxedStrategy<Value> {
    let orphan_counts = prop_oneof![
        3 => pick(counts()).prop_map(|c| json!({"orphan-artifact": c})),
        1 => pick(vec![json!({}), json!("str"), json!([]), Value::Null, json!({"orphan-artifact": 0})]),
    ]
    .boxed();
    prop_oneof![
        6 => object(vec![
            ("counts", field(orphan_counts)),
            ("checked", field(pick(slot_numbers()))),
            ("classifierFailed", field(pick(counts()))),
        ]),
        1 => pick(vec![Value::Null, json!(5), json!("s")]),
    ]
    .boxed()
}

fn fidelity_critic() -> BoxedStrategy<Value> {
    let entry = pick(vec![
        json!({"rule": "vote-reasoning", "findingTitle": "T"}),
        json!({"rule": "prior-review", "findingTitle": "Missing guard"}),
        json!({"rule": "vote-reasoning"}),
        Value::Null,
        json!(5),
        json!({"rule": to_string_obj()}),
        json!({"findingTitle": ["a"]}),
        json!("s"),
    ]);
    let list = prop_oneof![
        4 => vec(entry, 0..5).prop_map(Value::Array),
        2 => pick(vec![
            json!("abc"),
            json!(""),
            json!({"length": 1}),
            json!({"length": 0}),
            json!({"length": "0"}),
            json!({}),
            json!(5),
            json!(true),
            Value::Null,
        ]),
    ]
    .boxed();
    prop_oneof![
        5 => list.prop_map(|l| json!({"violations": l})),
        1 => pick(vec![Value::Null, json!("x"), json!(0), json!({})]),
    ]
    .boxed()
}

fn relays() -> BoxedStrategy<Value> {
    let relay = prop_oneof![
        4 => (
            pick(vec![json!("section-diff"), json!("fetch-pr"), Value::Null, to_string_obj()]),
            pick(vec![json!(false), json!(false), json!(true), json!(0)]),
            field(pick(vec![json!(true), json!(false), json!("x")])),
            field(pick(reasons())),
        )
            .prop_map(|(label, verified, script_error, reason)| {
                let mut m = Map::new();
                m.insert("label".into(), label);
                m.insert("verified".into(), verified);
                if let Some(e) = script_error {
                    m.insert("scriptError".into(), e);
                }
                if let Some(r) = reason {
                    m.insert("reason".into(), r);
                }
                Value::Object(m)
            }),
        1 => pick(vec![Value::Null, json!(5), json!({"verified": 0}), json!({"verified": false})]),
    ]
    .boxed();
    prop_oneof![
        5 => vec(relay, 0..5).prop_map(Value::Array),
        1 => pick(vec![json!("x"), json!({})]),
    ]
    .boxed()
}

fn bash_over() -> BoxedStrategy<Value> {
    let entry = pick(vec![
        json!({"label": "persist", "chars": 7264}),
        json!({"label": "fetch-pr", "chars": 6001.5}),
        json!({"label": "x"}),
        Value::Null,
        json!(5),
        json!({"label": to_string_obj(), "chars": 1}),
        json!({"chars": null}),
    ]);
    prop_oneof![
        5 => vec(entry, 0..8).prop_map(Value::Array),
        1 => pick(vec![json!("x"), json!({})]),
    ]
    .boxed()
}

fn verdict_corrected() -> BoxedStrategy<Value> {
    pick(vec![
        Value::Null,
        json!("x"),
        json!(0),
        json!([]),
        json!([1]),
        json!({"from": "COMMENT", "reason": "prose argued it", "to": "APPROVE"}),
        json!({"reason": "unreadable", "to": "REQUEST_CHANGES"}),
        json!({"from": "", "reason": null}),
        json!({"from": to_string_obj(), "reason": "r", "to": "x"}),
        json!({"from": 5, "to": ["a", "b"]}),
    ])
}

/// `(checked, externalSlots)` around the `expected - 1` boundary.
const SLOT_PAIRS: [(f64, f64); 8] = [
    (38.0, 39.0),
    (37.0, 39.0),
    (39.0, 39.0),
    (0.0, 1.0),
    (1.5, 2.0),
    (1.0, 2.0),
    (0.0, 0.0),
    (2.0, 1.0),
];

fn metrics() -> BoxedStrategy<Value> {
    (
        fields(),
        prop_oneof![2 => Just(None), 1 => select(SLOT_PAIRS.to_vec()).prop_map(Some)],
    )
        .prop_map(|(mut m, pair)| {
            if let Some((checked, slots)) = pair {
                m["externalSlots"] = json!(slots);
                if !m.get("voteReconciliation").is_some_and(Value::is_object) {
                    m["voteReconciliation"] = json!({});
                }
                m["voteReconciliation"]["checked"] = json!(checked);
            }
            m
        })
        .boxed()
}

fn fields() -> BoxedStrategy<Value> {
    object(vec![
        ("findings", field(findings())),
        (
            "codexEnabled",
            prop_oneof![1 => field(pick(flags())), 1 => Just(Some(json!(true)))].boxed(),
        ),
        ("geminiEnabled", field(pick(flags()))),
        ("voteReconciliation", field(vote_reconciliation())),
        ("externalSlots", field(pick(slot_numbers()))),
        ("agentsProjected", field(pick(agent_numbers()))),
        ("agentsActual", field(pick(agent_numbers()))),
        ("fidelityCritic", field(fidelity_critic())),
        (
            "externalProbeAnswered",
            field(pick(vec![
                Value::Null,
                json!(false),
                json!(true),
                json!(0),
                json!("false"),
            ])),
        ),
        ("reportVerdictCorrected", field(verdict_corrected())),
        ("relays", field(relays())),
        ("bashOverLimit", field(bash_over())),
        (
            "platform",
            field(pick(vec![
                json!("win32"),
                json!("darwin"),
                json!("linux"),
                Value::Null,
            ])),
        ),
    ])
}

const TEXTS: [&str; 11] = [
    "",
    "not json",
    "null",
    "5",
    "\"s\"",
    "[]",
    "{}",
    "  ",
    "\u{FEFF}{}",
    "{\"findings\": null}",
    "[{\"findings\": [null]}]",
];

#[derive(Debug, Clone)]
enum Stdin {
    Metrics(Value, bool),
    Text(&'static str),
}

impl Stdin {
    fn text(&self) -> String {
        match self {
            Self::Metrics(v, true) => serde_json::to_string_pretty(v).unwrap(),
            Self::Metrics(v, false) => serde_json::to_string(v).unwrap(),
            Self::Text(t) => (*t).to_owned(),
        }
    }
}

const NAMES: [&str; 9] = [
    "a.out",
    "b.meta",
    "c.txt",
    ".out",
    "x.meta.bak",
    "d.out",
    "e.meta",
    "f.OUT",
    "g.out",
];
const OFFSETS: [i64; 7] = [-400_000, -3600, -1, 0, 0, 1, 60];

#[derive(Debug, Clone, Copy)]
enum Kind {
    File,
    Dir,
    Dangling,
    /// A symlink to a file outside the directory, whose mtime is the one set.
    Link,
}

#[derive(Debug, Clone)]
struct Entry {
    name: usize,
    kind: Kind,
    offset: usize,
    nsec: u32,
}

#[derive(Debug, Clone)]
enum DirArg {
    None,
    Empty,
    Missing,
    File,
    /// The vote dir, written with this suffix (`""`, `"/"`, `"/."`).
    Dir(&'static str),
}

#[derive(Debug, Clone)]
enum StartArg {
    Absent,
    Fixed(&'static str),
    /// Seconds from the directory's birth second, printed with this format.
    Birth(i64, usize),
}

#[derive(Debug, Clone)]
struct Case {
    stdin: Stdin,
    dir: DirArg,
    entries: Vec<Entry>,
    start: StartArg,
}

fn entry() -> impl Strategy<Value = Entry> {
    (
        0..NAMES.len(),
        prop_oneof![8 => Just(Kind::File), 1 => Just(Kind::Dir), 1 => Just(Kind::Dangling), 1 => Just(Kind::Link)],
        0..OFFSETS.len(),
        prop_oneof![Just(0u32), 1u32..1_000_000_000],
    )
        .prop_map(|(name, kind, offset, nsec)| Entry {
            name,
            kind,
            offset,
            nsec,
        })
}

fn case() -> impl Strategy<Value = Case> {
    let stdin = prop_oneof![
        9 => (metrics(), any::<bool>()).prop_map(|(m, pretty)| Stdin::Metrics(m, pretty)),
        1 => select(TEXTS.to_vec()).prop_map(Stdin::Text),
    ];
    let dir = prop_oneof![
        1 => Just(DirArg::None),
        1 => Just(DirArg::Empty),
        1 => Just(DirArg::Missing),
        1 => Just(DirArg::File),
        8 => select(vec!["", "", "/", "/."]).prop_map(DirArg::Dir),
    ];
    let start = prop_oneof![
        6 => Just(StartArg::Absent),
        2 => select(vec!["", "0", "abc", "NaN", "Infinity", "-5", "0x10"]).prop_map(StartArg::Fixed),
        3 => (select(vec![-50_000i64, -2, 0, 1, 10, 100]), 0..3usize).prop_map(|(o, f)| StartArg::Birth(o, f)),
    ];
    (stdin, dir, vec(entry(), 0..6), start).prop_map(|(stdin, dir, entries, start)| Case {
        stdin,
        dir,
        entries,
        start,
    })
}

// ---------------------------------------------------------------- running

fn birth_second(dir: &Path) -> i64 {
    let t = std::fs::metadata(dir).unwrap().created().unwrap();
    t.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

fn set_mtime(path: &Path, secs: i64, nsec: u32) {
    let t = UNIX_EPOCH + Duration::new(secs as u64, nsec);
    File::open(path).unwrap().set_modified(t).unwrap();
}

/// Builds the case's directory and returns the script's arguments.
fn build(root: &Path, case: &Case) -> Vec<String> {
    let votes = root.join("votes");
    let mut args = Vec::new();
    match &case.dir {
        DirArg::None => return args,
        DirArg::Empty => args.push(String::new()),
        DirArg::Missing => args.push(root.join("absent").to_string_lossy().into_owned()),
        DirArg::File => {
            let f = root.join("plain-file");
            std::fs::write(&f, "x").unwrap();
            args.push(f.to_string_lossy().into_owned());
        }
        DirArg::Dir(suffix) => {
            std::fs::create_dir(&votes).unwrap();
            let birth = birth_second(&votes);
            let mut used = Vec::new();
            for e in &case.entries {
                if used.contains(&e.name) {
                    continue;
                }
                used.push(e.name);
                let path = votes.join(NAMES[e.name]);
                let secs = birth + OFFSETS[e.offset];
                match e.kind {
                    Kind::File => {
                        std::fs::write(&path, "x").unwrap();
                        set_mtime(&path, secs, e.nsec);
                    }
                    Kind::Dir => {
                        std::fs::create_dir(&path).unwrap();
                        set_mtime(&path, secs, e.nsec);
                    }
                    Kind::Dangling => {
                        std::os::unix::fs::symlink(root.join("nowhere"), &path).unwrap();
                    }
                    Kind::Link => {
                        let target = root.join(format!("target-{}", e.name));
                        std::fs::write(&target, "t").unwrap();
                        set_mtime(&target, secs, e.nsec);
                        std::os::unix::fs::symlink(&target, &path).unwrap();
                    }
                }
            }
            args.push(format!("{}{suffix}", votes.to_string_lossy()));
        }
    }
    match &case.start {
        StartArg::Absent => {}
        StartArg::Fixed(s) => args.push((*s).to_owned()),
        StartArg::Birth(offset, format) => {
            let base = if votes.exists() {
                birth_second(&votes)
            } else {
                1_760_000_000
            };
            let n = base + offset;
            args.push(match format {
                0 => n.to_string(),
                1 => format!(" {n}.5 "),
                _ => format!("{n}e0"),
            });
        }
    }
    args
}

struct Outcome {
    input: Value,
    js: Value,
    rust: Value,
}

fn js_value(code: Option<i32>, stdout: &str, stderr: &str) -> Value {
    if code == Some(0) {
        let line = stdout
            .strip_suffix('\n')
            .unwrap_or_else(|| panic!("no newline: {stdout:?}"));
        assert!(!line.contains('\n'), "{stdout:?}");
        let v: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}"));
        return json!({"code": 0, "line": line, "value": v});
    }
    let thrown = stderr
        .lines()
        .find(|l| l.starts_with("TypeError: "))
        .map_or_else(|| format!("exit {code:?}: {stderr}"), str::to_owned);
    json!({"code": code, "stdout": stdout, "threw": thrown})
}

fn rust_value(out: Result<tribunal_context::audit_run::Audit, String>) -> Value {
    match out {
        Ok(a) => json!({"code": 0, "line": a.to_line(), "value": a.to_value()}),
        Err(e) => json!({"code": 1, "stdout": "", "threw": e}),
    }
}

fn check(root: &Path, case: &Case) -> Outcome {
    std::fs::create_dir_all(root).unwrap();
    let args = build(root, case);
    let stdin = case.stdin.text();
    let (code, stdout, stderr) = node(&args, &stdin);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    Outcome {
        input: json!({"args": args, "stdin": stdin, "case": format!("{case:?}")}),
        js: js_value(code, &stdout, &stderr),
        rust: rust_value(run(&refs, &stdin)),
    }
}

fn outcomes() -> &'static [Outcome] {
    static CELL: OnceLock<Vec<Outcome>> = OnceLock::new();
    CELL.get_or_init(|| {
        let cases = sample(case());
        let root = scratch("audit-run-gen");
        par_map(&cases, |i, case| check(&root.join(i.to_string()), case))
    })
}

#[test]
fn audit_run_matches_js_on_generated_cases() {
    let out = outcomes();
    let inputs: Vec<Value> = out.iter().map(|o| o.input.clone()).collect();
    let rust: Vec<Value> = out.iter().map(|o| o.rust.clone()).collect();
    let js: Vec<Value> = out.iter().map(|o| o.js.clone()).collect();
    assert_same("audit-run", &inputs, &rust, &js);
}

#[test]
fn the_generated_cases_reach_every_rule() {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut bump = |k: String| *seen.entry(k).or_default() += 1;
    for o in outcomes() {
        let v = &o.rust;
        if let Some(t) = v.get("threw") {
            bump(format!("threw {}", t.as_str().unwrap()));
            continue;
        }
        let value = &v["value"];
        bump(format!("passed {}", value["passed"]));
        if value.get("notes").is_none() {
            bump("no notes".into());
        }
        for (list, tag) in [("violations", "v"), ("notes", "n")] {
            for i in value[list].as_array().into_iter().flatten() {
                let id = i["id"].as_str().unwrap();
                let detail = i["detail"].as_str().unwrap();
                bump(format!("{tag} {id}"));
                let sub = match id {
                    "invariant-crashed" => detail
                        .split(" (reading")
                        .next()
                        .unwrap()
                        .split(" is not iterable")
                        .next()
                        .unwrap()
                        .trim_end_matches(|c: char| c.is_ascii_digit() || c == '.')
                        .trim_end()
                        .split(", stat '")
                        .next()
                        .unwrap()
                        .split(", scandir '")
                        .next()
                        .unwrap()
                        .to_owned(),
                    "no-external-voter" if detail.ends_with(')') => "with causes".into(),
                    "no-external-voter" => "without causes".into(),
                    "relay-unverified" if detail.contains("the script failed") => {
                        "script failed".into()
                    }
                    "cost-model-drift" if detail.contains("Infinity%") => "infinite".into(),
                    "debate-unadjudicated" if detail.contains("(untitled)") => "untitled".into(),
                    "report-verdict-corrected" if detail.contains("nothing readable") => {
                        "nothing readable".into()
                    }
                    _ => continue,
                };
                bump(format!("{tag} {id}: {sub}"));
            }
        }
    }
    eprintln!("{seen:#?}");
    let mut need: Vec<String> = [
        "no-external-voter",
        "orphan-artifacts",
        "reconciliation-incomplete",
        "classifier-unavailable",
        "blocking-without-adjudication",
        "stale-artifacts",
        "cost-model-drift",
        "merged-locations-dropped",
        "kill-reason-not-cited",
        "merged-from-annotation-dropped",
        "grounded-diagnostic-not-cited",
        "gate-demoted-called-unexamined",
        "fidelity-critic-flagged",
        "external-probe-unreadable",
        "debate-unadjudicated",
        "report-verdict-corrected",
        "relay-unverified",
        "bash-over-windows-limit",
        "invariant-crashed",
        "metrics-unreadable",
        "no-external-voter: with causes",
        "no-external-voter: without causes",
        "relay-unverified: script failed",
        "debate-unadjudicated: untitled",
        "report-verdict-corrected: nothing readable",
        "invariant-crashed: TypeError: Cannot read properties of null",
        "invariant-crashed: TypeError: Cannot convert object to primitive value",
        "invariant-crashed: TypeError: number",
        "invariant-crashed: TypeError: boolean true",
        "invariant-crashed: TypeError: object",
        "invariant-crashed: TypeError: violations.map is not a function",
        "invariant-crashed: Error: ENOENT: no such file or directory",
        "invariant-crashed: Error: ENOTDIR: not a directory",
    ]
    .iter()
    .map(|id| format!("v {id}"))
    .collect();
    need.extend(
        [
            "n bash-over-windows-limit",
            "passed true",
            "passed false",
            "no notes",
            "threw TypeError: Cannot read properties of null (reading 'findings')",
        ]
        .map(String::from),
    );
    for k in need {
        assert!(seen.get(&k).copied().unwrap_or(0) >= 3, "{k}: {seen:#?}");
    }
}

/// Stale artifacts are found from the directory's birth time and from an
/// explicit run start alike.
#[test]
fn the_generated_cases_reach_both_run_starts() {
    let (mut by_birth, mut by_arg) = (0, 0);
    for o in outcomes() {
        let stale = o.rust["value"]["violations"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|i| i["id"] == "stale-artifacts");
        if stale {
            if o.input["case"].as_str().unwrap().contains("start: Absent") {
                by_birth += 1;
            } else {
                by_arg += 1;
            }
        }
    }
    assert!(by_birth >= 5 && by_arg >= 5, "{by_birth} {by_arg}");
}

fn both(args: &[String], stdin: &str) -> (Value, Value) {
    let (code, stdout, stderr) = node(args, stdin);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    (
        js_value(code, &stdout, &stderr),
        rust_value(run(&refs, stdin)),
    )
}

/// Declared difference: a relay reason cut at 120 units inside a surrogate
/// pair keeps the lone half in the script's JSON and ends in U+FFFD in the
/// port's.
#[test]
fn witness_relay_reason_cut_inside_a_pair() {
    let reason = format!("{}😀", "x".repeat(119));
    let stdin =
        json!({"relays": [{"label": "l", "verified": false, "reason": reason}]}).to_string();
    let (code, stdout, _) = node(&[], &stdin);
    assert_eq!(code, Some(0));
    assert!(stdout.contains("x\\ud83d)"), "{stdout}");
    let line = run(&[], &stdin).unwrap().to_line();
    assert!(line.contains("x\u{FFFD})"), "{line}");
}

/// Declared difference: a number literal beyond the double range is Infinity
/// to the script and null to the port.
#[test]
fn witness_number_beyond_double_range() {
    let stdin = r#"{"voteReconciliation": {"classifierFailed": 1e400}}"#;
    let (js, rust) = both(&[], stdin);
    assert_eq!(js["value"]["violations"][0]["id"], "classifier-unavailable");
    assert_eq!(rust["value"]["violations"], json!([]));
}

#[test]
fn a_vote_dir_older_than_its_files_is_clean() {
    let root = scratch("audit-run-clean");
    let votes = root.join("votes");
    std::fs::create_dir(&votes).unwrap();
    let file = votes.join("a.out");
    std::fs::write(&file, "x").unwrap();
    set_mtime(
        &file,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 5,
        0,
    );
    let args = vec![votes.to_string_lossy().into_owned()];
    let (js, rust) = both(&args, "{}");
    assert_eq!(js, rust);
    assert_eq!(rust["value"]["passed"], true);
}
