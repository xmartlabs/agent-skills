//! `lib/check-report-fidelity.mjs` against
//! `tribunal_context::check_report_fidelity`: the same report file and the
//! same `{"findings": [...]}` on stdin, run as SKILL.md Step 4 runs it,
//! compared on the exit code, stdout byte for byte, and stderr (for an
//! uncaught throw, its `TypeError:` line, since Node prints a stack around
//! it).
mod common;
#[path = "../../tribunal-core/tests/common/mod.rs"]
mod core_common;

use common::{plugin_root, scratch};
use core_common::{assert_same, sample};

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use proptest::collection::vec;
use proptest::prelude::*;
use proptest::sample::select;
use serde_json::{json, Map, Value};
use tribunal_context::check_report_fidelity::{apply_fidelity_checks, run};
use tribunal_context::operator::CliOutput;
use tribunal_core::js::stringify;

const SCRIPT: &str = "lib/check-report-fidelity.mjs";

fn node(script: &Path, args: &[String], stdin: &str) -> CliOutput {
    let mut child = Command::new("node")
        .arg(script)
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
    CliOutput {
        stdout: String::from_utf8(out.stdout).expect("UTF-8 stdout"),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        code: out.status.code().unwrap_or(-1),
    }
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

const LENSES: [&str; 3] = ["lens-correctness", "lens-security", "lens-tests"];
const KILL_REASONS: [&str; 3] = ["wrong-premise", "not-reachable", "uncategorized"];
const DIAGNOSTIC: &str =
    "TS2345: Argument of type 'string' is not assignable to parameter of type 'number'.";

fn files() -> Vec<Value> {
    vec![
        json!("a.ts"),
        json!("src/b.ts"),
        json!("c d.ts"),
        json!("ü.ts"),
        Value::Null,
        json!(5),
        json!(["x", "y"]),
        to_string_obj(),
        json!({}),
    ]
}

fn lines() -> Vec<Value> {
    vec![
        json!(12),
        json!(3),
        json!(3.0),
        json!(0),
        json!(""),
        json!("7"),
        Value::Null,
        json!(1.5),
        json!(true),
        json!([4]),
        to_string_obj(),
        serde_json::from_str("1e21").unwrap(),
    ]
}

fn location() -> BoxedStrategy<Value> {
    prop_oneof![
        8 => (
            proptest::option::of(select(files())),
            proptest::option::of(select(lines())),
        )
            .prop_map(|(file, line)| {
                let mut m = Map::new();
                if let Some(f) = file {
                    m.insert("file".into(), f);
                }
                if let Some(l) = line {
                    m.insert("line".into(), l);
                }
                Value::Object(m)
            }),
        1 => select(vec![Value::Null, json!(5), json!("s"), json!([])]),
    ]
    .boxed()
}

fn merged_locations() -> BoxedStrategy<Value> {
    prop_oneof![
        6 => vec(location(), 0..5).prop_map(Value::Array),
        1 => select(vec![Value::Null, json!(""), json!("abc"), json!("a"), json!(5), json!({}), json!(true), json!(0)]),
    ]
    .boxed()
}

fn kill_reason() -> BoxedStrategy<Option<Value>> {
    proptest::option::of(select(vec![
        json!(KILL_REASONS[0]),
        json!(KILL_REASONS[1]),
        json!(""),
        Value::Null,
        json!(5),
        to_string_obj(),
        json!(["a", "b"]),
        json!({}),
        json!({"b": 1, "0": 2}),
    ]))
    .boxed()
}

fn vote() -> BoxedStrategy<Value> {
    prop_oneof![
        6 => (
            select(vec!["REFUTED", "REFUTED", "CONFIRMED", "refuted"]),
            kill_reason(),
        )
            .prop_map(|(verdict, reason)| {
                let mut m = Map::new();
                m.insert("verdict".into(), json!(verdict));
                if let Some(r) = reason {
                    m.insert("killReason".into(), r);
                }
                Value::Object(m)
            }),
        1 => select(vec![Value::Null, json!("REFUTED"), json!(3)]),
    ]
    .boxed()
}

fn merged_from() -> BoxedStrategy<Value> {
    let lens = select(vec![
        json!(LENSES[0]),
        json!(LENSES[1]),
        json!(LENSES[2]),
        json!(5),
        Value::Null,
        to_string_obj(),
    ]);
    prop_oneof![
        5 => vec(lens, 0..4).prop_map(Value::Array),
        1 => select(vec![json!("abc"), Value::Null, json!({})]),
    ]
    .boxed()
}

fn grounded() -> BoxedStrategy<Value> {
    prop_oneof![
        5 => (
            proptest::option::of(select(vec![json!("confirmed"), json!("confirmed"), json!("refuted"), Value::Null])),
            proptest::option::of(select(vec![json!(DIAGNOSTIC), json!(""), json!(5), to_string_obj(), json!("TS2322: elsewhere"), json!("TS9999: never printed")])),
        )
            .prop_map(|(verdict, diagnostic)| {
                let mut m = Map::new();
                if let Some(v) = verdict {
                    m.insert("verdict".into(), v);
                }
                if let Some(d) = diagnostic {
                    m.insert("diagnostic".into(), d);
                }
                Value::Object(m)
            }),
        1 => select(vec![Value::Null, json!("confirmed"), json!(0), json!([])]),
    ]
    .boxed()
}

fn finding() -> BoxedStrategy<Value> {
    (
        proptest::option::of(merged_locations()),
        proptest::option::of(prop_oneof![
            5 => vec(vote(), 0..4).prop_map(Value::Array),
            1 => select(vec![json!("x"), Value::Null]),
        ]),
        proptest::option::of(merged_from()),
        proptest::option::of(grounded()),
        any::<bool>(),
        0..4usize,
        prop_oneof![
            2 => Just(None),
            2 => Just(Some((json!("a.ts"), json!(12), json!("Downgraded from Blocking")))),
            1 => (
                select(vec![json!("a.ts"), json!(""), json!(5)]),
                select(vec![json!(12.0), json!("12"), Value::Null]),
                select(vec![json!("Downgraded from Blocking"), json!(""), Value::Null]),
            )
                .prop_map(Some),
        ],
    )
        .prop_map(
            |(locs, votes, from, grounded, index_key, existing, gated)| {
                let mut m = Map::new();
                if index_key {
                    m.insert("2".into(), json!(1));
                }
                m.insert("title".into(), json!("Missing guard"));
                if existing == 1 {
                    m.insert("killReasonMissingFromReport".into(), json!(["stale"]));
                }
                for (k, v) in [
                    ("mergedLocations", locs),
                    ("votes", votes),
                    ("mergedFrom", from),
                    ("grounded", grounded),
                ] {
                    if let Some(v) = v {
                        m.insert(k.into(), v);
                    }
                }
                if existing == 2 {
                    m.insert("groundedDiagnosticMissingFromReport".into(), Value::Null);
                }
                if let Some((file, line, note)) = gated {
                    m.insert("file".into(), file);
                    m.insert("line".into(), line);
                    m.insert("verifiedByGateNote".into(), note);
                }
                m.insert("severity".into(), json!(2.0));
                Value::Object(m)
            },
        )
        .boxed()
}

fn findings() -> BoxedStrategy<Value> {
    let odd = select(vec![
        Value::Null,
        json!(5),
        json!("s"),
        json!([]),
        json!([1]),
    ]);
    prop_oneof![
        8 => vec(prop_oneof![9 => finding(), 1 => odd], 0..5).prop_map(Value::Array),
        1 => select(vec![Value::Null, json!(5), json!({}), json!("abc")]),
    ]
    .boxed()
}

const TEXTS: [&str; 12] = [
    "",
    "not json",
    "null",
    "5",
    "\"s\u{2028}\"",
    "[]",
    "{}",
    "{\"findings\": 5}",
    "{\"b\":1.0,\"a\":-0.0,\"c\":1e21,\"d\":12345678901234567890,\"2\":[1e-7]}",
    "\u{FEFF}{}",
    "{\"findings\": [], \"findings\": [{}]}",
    "{\"x\": 1,}",
];

#[derive(Debug, Clone)]
enum Stdin {
    Metrics(Value),
    Text(&'static str),
}

/// Report pieces: annotations the checks look for, and the draft section's
/// heading in its look-alike forms.
fn pieces() -> Vec<String> {
    let mut p: Vec<String> = vec![
        "### Findings".into(),
        "### Summary".into(),
        "### Draft comments".into(),
        "### Draft comments".into(),
        "### Draft commentsé".into(),
        "### Draft commentary".into(),
        "### Draft comments_x".into(),
        "#### Draft comments".into(),
        String::new(),
        String::new(),
        "  \t".into(),
        "also at: a.ts:12".into(),
        "also at: src/b.ts:3, c d.ts:7".into(),
        "also at: ü.ts".into(),
        "also at: null:1.5".into(),
        "x,y:4 and 5 and [object Object]".into(),
        "(found by: lens-correctness, lens-security)".into(),
        "(found by:".into(),
        "lens-tests".into(),
        "killed: wrong-premise".into(),
        "killed: not-reachable".into(),
        "uncategorized".into(),
        "a,b".into(),
        format!("(confirmed by typecheck: {DIAGNOSTIC})"),
        "TS2322: elsewhere".into(),
        "line with\r".into(),
        "5 and undefined".into(),
        "**Demoted**".into(),
        "`a.ts:12`".into(),
        "1. Demoted — a.ts:12 — why".into(),
        "NOT ADJUDICATED: no refuter examined this claim".into(),
        "No Refuter Examined".into(),
        "**Demoted**\n`a.ts:12`\nNOT ADJUDICATED: no refuter examined this claim".into(),
        "**Demoted**\n`a.ts:12`\nThe panel ran and the gate demoted it.".into(),
    ];
    p.push("trailing".into());
    p
}

#[derive(Debug, Clone)]
enum Report {
    Pieces(Vec<usize>, bool),
    /// Bytes that are not UTF-8, around a location.
    Invalid,
    Missing,
    Directory,
    NoArg,
    EmptyArg,
}

#[derive(Debug, Clone)]
struct Case {
    report: Report,
    stdin: Stdin,
}

fn case() -> impl Strategy<Value = Case> {
    let n = pieces().len();
    let report = prop_oneof![
        24 => (vec(0..n, 0..14), any::<bool>()).prop_map(|(p, bom)| Report::Pieces(p, bom)),
        1 => Just(Report::Invalid),
        1 => Just(Report::Missing),
        1 => Just(Report::Directory),
        1 => Just(Report::NoArg),
        1 => Just(Report::EmptyArg),
    ];
    let stdin = prop_oneof![
        9 => (findings(), any::<bool>()).prop_map(|(f, extra)| {
            let mut m = Map::new();
            if extra {
                m.insert("10".into(), json!("x"));
                m.insert("runId".into(), json!(3.0));
            }
            m.insert("findings".into(), f);
            Stdin::Metrics(Value::Object(m))
        }),
        1 => select(TEXTS.to_vec()).prop_map(Stdin::Text),
    ];
    (report, stdin).prop_map(|(report, stdin)| Case { report, stdin })
}

fn report_args(root: &Path, report: &Report) -> Vec<String> {
    let path = root.join("body.md");
    let p = path.to_string_lossy().into_owned();
    match report {
        Report::Pieces(idx, bom) => {
            let all = pieces();
            let mut text = if *bom {
                "\u{FEFF}".to_owned()
            } else {
                String::new()
            };
            text.push_str(
                &idx.iter()
                    .map(|&i| all[i].as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            std::fs::write(&path, text).unwrap();
            vec![p]
        }
        Report::Invalid => {
            std::fs::write(&path, b"also at: a.ts:12 \xff\xc3( \xe2\x82 lens-tests\n").unwrap();
            vec![p]
        }
        Report::Missing => vec![p],
        Report::Directory => {
            std::fs::create_dir(&path).unwrap();
            vec![p]
        }
        Report::NoArg => vec![],
        Report::EmptyArg => vec![String::new()],
    }
}

fn stdin_text(s: &Stdin) -> String {
    match s {
        Stdin::Metrics(v) => serde_json::to_string(v).unwrap(),
        Stdin::Text(t) => (*t).to_owned(),
    }
}

fn normalized(out: &CliOutput, crash_from_stack: bool) -> Value {
    let crashed = out.code != 0
        && out.stderr.lines().any(|l| l.starts_with("TypeError: "))
        && (crash_from_stack || !out.stderr.contains('\n'));
    let stderr = if crashed {
        out.stderr
            .lines()
            .find(|l| l.starts_with("TypeError: "))
            .unwrap()
            .to_owned()
    } else {
        out.stderr.clone()
    };
    json!({"code": out.code, "stdout": out.stdout, "stderr": stderr, "crashed": crashed})
}

struct Outcome {
    input: Value,
    js: Value,
    rust: Value,
    case: Case,
    args: Vec<String>,
}

fn check(root: &Path, case: &Case) -> Outcome {
    std::fs::create_dir_all(root).unwrap();
    let args = report_args(root, &case.report);
    let stdin = stdin_text(&case.stdin);
    let js = node(&plugin_root().join(SCRIPT), &args, &stdin);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let rust = run(&refs, &stdin);
    Outcome {
        input: json!({"args": args, "stdin": stdin, "case": format!("{case:?}")}),
        js: normalized(&js, true),
        rust: normalized(&rust, false),
        case: case.clone(),
        args,
    }
}

fn outcomes() -> &'static [Outcome] {
    static CELL: OnceLock<Vec<Outcome>> = OnceLock::new();
    CELL.get_or_init(|| {
        let cases = sample(case());
        let root = scratch("report-fidelity-gen");
        par_map(&cases, |i, case| check(&root.join(i.to_string()), case))
    })
}

#[test]
fn check_report_fidelity_matches_js_on_generated_cases() {
    let out = outcomes();
    let inputs: Vec<Value> = out.iter().map(|o| o.input.clone()).collect();
    let rust: Vec<Value> = out.iter().map(|o| o.rust.clone()).collect();
    let js: Vec<Value> = out.iter().map(|o| o.js.clone()).collect();
    assert_same("check-report-fidelity", &inputs, &rust, &js);
}

#[test]
fn the_generated_cases_reach_every_branch() {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut bump = |k: String| *seen.entry(k).or_default() += 1;
    for o in outcomes() {
        let r = &o.rust;
        if r["code"] != 0 {
            let line = r["stderr"].as_str().unwrap();
            let key = line
                .split(" — ")
                .next()
                .unwrap()
                .split(": no such")
                .next()
                .unwrap()
                .split(" /")
                .next()
                .unwrap()
                .trim_end()
                .to_owned();
            bump(format!("fail {key}"));
            if line.contains("EISDIR") {
                bump("fail EISDIR".into());
            }
            if line.contains("ENOENT") {
                bump("fail ENOENT".into());
            }
            continue;
        }
        let out: Value = serde_json::from_str(r["stdout"].as_str().unwrap()).unwrap();
        let Some(list) = out.get("findings").and_then(Value::as_array) else {
            bump("no findings array".into());
            continue;
        };
        for f in list {
            let Some(obj) = f.as_object() else {
                bump(format!(
                    "left as is: {}",
                    if f.is_array() { "array" } else { "other" }
                ));
                continue;
            };
            for key in [
                "mergedLocationsMissingFromReport",
                "killReasonMissingFromReport",
                "mergedFromAnnotationMissingFromReport",
                "groundedDiagnosticMissingFromReport",
                "unexaminedWordingInReport",
            ] {
                let empty = obj[key].as_array().unwrap().is_empty();
                bump(format!("{key} {}", if empty { "empty" } else { "missing" }));
            }
            if obj.get("verifiedByGateNote") == Some(&json!("Downgraded from Blocking"))
                && obj.get("file") == Some(&json!("a.ts"))
                && obj.get("line").is_some_and(|l| l.as_f64() == Some(12.0))
                && obj
                    .get("votes")
                    .and_then(Value::as_array)
                    .is_some_and(|v| !v.is_empty())
            {
                bump("gated finding".into());
            }
            if obj["killReasonMissingFromReport"] == json!(["uncategorized"]) {
                bump("uncategorized missing".into());
            }
            if obj["mergedFromAnnotationMissingFromReport"]
                .as_array()
                .unwrap()
                .contains(&json!("(found by:"))
            {
                bump("found-by missing".into());
            }
            let keys: Vec<&String> = obj.keys().collect();
            if keys.first().map(|k| k.as_str()) == Some("2") {
                bump("index key first".into());
            }
            if obj.contains_key("mergedLocations")
                && obj["mergedLocationsMissingFromReport"] != json!([])
            {
                bump("location missing".into());
            }
        }
        // The draft strip decided the answer: the same findings against the
        // unstripped body come out different.
        if let (Report::Pieces(..), Stdin::Metrics(m)) = (&o.case.report, &o.case.stdin) {
            let raw = std::fs::read_to_string(&o.args[0]).unwrap();
            let mut f = m["findings"].clone();
            if apply_fidelity_checks(&mut f, &raw).is_ok()
                && stringify(&f, "") != stringify(&out["findings"], "")
            {
                bump("drafts stripped mattered".into());
            }
        }
    }
    eprintln!("{seen:#?}");
    for k in [
        "fail usage: check-report-fidelity.mjs <report-file> < metrics.json > metrics.json",
        "fail ENOENT",
        "fail EISDIR",
        "fail check-report-fidelity: metrics on stdin were not valid JSON",
        "fail TypeError: Cannot read properties of null (reading 'findings')",
        "fail TypeError: Cannot read properties of null (reading 'line')",
        "fail TypeError: Cannot convert object to primitive value",
        "fail TypeError: (mergedLocations || []).slice is not a function",
        "fail TypeError: (mergedLocations || []).slice(...).map is not a function",
        "no findings array",
        "left as is: array",
        "left as is: other",
        "mergedLocationsMissingFromReport empty",
        "mergedLocationsMissingFromReport missing",
        "killReasonMissingFromReport empty",
        "killReasonMissingFromReport missing",
        "mergedFromAnnotationMissingFromReport empty",
        "mergedFromAnnotationMissingFromReport missing",
        "groundedDiagnosticMissingFromReport empty",
        "groundedDiagnosticMissingFromReport missing",
        "unexaminedWordingInReport empty",
        "unexaminedWordingInReport missing",
        "gated finding",
        "uncategorized missing",
        "found-by missing",
        "index key first",
        "drafts stripped mattered",
    ] {
        assert!(seen.get(k).copied().unwrap_or(0) >= 3, "{k}: {seen:#?}");
    }
}

/// Declared difference: a number literal beyond the double range is Infinity
/// to the script and null to the port, so a location's line prints
/// differently.
#[test]
fn witness_number_beyond_double_range() {
    let root = scratch("report-fidelity-witness-inf");
    let body = root.join("body.md");
    std::fs::write(&body, "also at: a.ts").unwrap();
    let args = vec![body.to_string_lossy().into_owned()];
    let stdin = r#"{"findings":[{"mergedLocations":[{},{"file":"a.ts","line":1e400}]}]}"#;
    let js = node(&plugin_root().join(SCRIPT), &args, stdin);
    let rust = run(&[args[0].as_str()], stdin);
    assert!(
        js.stdout
            .contains(r#""mergedLocationsMissingFromReport":["a.ts:Infinity"]"#),
        "{}",
        js.stdout
    );
    assert!(
        rust.stdout
            .contains(r#""mergedLocationsMissingFromReport":[]"#),
        "{}",
        rust.stdout
    );
}

/// Declared difference: the script loads its checks from the workflow at
/// run time and exits 1 when a region is missing; the port carries them.
#[test]
fn witness_regions_missing_from_the_workflow() {
    let root = scratch("report-fidelity-witness-regions");
    std::fs::create_dir_all(root.join("lib")).unwrap();
    std::fs::create_dir_all(root.join("workflows")).unwrap();
    let script = root.join("lib/check-report-fidelity.mjs");
    std::fs::copy(plugin_root().join(SCRIPT), &script).unwrap();
    std::fs::write(root.join("workflows/tribunal.js"), "// no regions\n").unwrap();
    let body = root.join("body.md");
    std::fs::write(&body, "x").unwrap();
    let args = vec![body.to_string_lossy().into_owned()];
    let js = node(&script, &args, "{}");
    assert_eq!(js.code, 1);
    assert!(
        js.stderr.contains("could not load the shared checks"),
        "{}",
        js.stderr
    );
    assert_eq!(run(&[args[0].as_str()], "{}").code, 0);
}
