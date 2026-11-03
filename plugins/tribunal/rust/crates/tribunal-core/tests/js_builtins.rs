//! The JavaScript number conversions and `JSON.stringify` the ports depend
//! on, checked against Node's own `String(x)`, `Number(s)` and
//! `JSON.stringify`. Doubles cross the process boundary as their bit
//! patterns, so NaN, -0 and the last ulp survive the trip.
//!
//! Node's answers are recorded in `tests/goldens/js_builtins.json`, one
//! section per test, so a run starts no node. The inputs are generated here,
//! deterministically, and each section keeps a SHA-256 of them. An answer or
//! input is rendered as its JSON encoding followed by `\n`; a number's answer
//! is the 16 hex digits of its bits, and any NaN is `"NaN"`. A section keeps
//! the SHA-256 of each run of `CHUNK` rendered answers, in case order, and the
//! first `SAMPLE` cases and the hand-picked ones verbatim. Regenerate, with
//! node on PATH:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-core --test js_builtins`.

mod common;

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;

use common::sha256_hex;
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use serde_json::{json, Value};

const DOUBLE_CASES: usize = 200_000;
const CHUNK: usize = 1000;
const SAMPLE: usize = 200;

const REGENERATE: &str = "regenerate with node on PATH: `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-core --test js_builtins`";

const EDGE_DOUBLES: [f64; 11] = [
    0.0,
    -0.0,
    f64::NAN,
    f64::INFINITY,
    f64::NEG_INFINITY,
    1e21,
    1e-7,
    123e-20,
    5e-324,
    f64::MAX,
    1699999.9000000001,
];

const EDGE_STRINGS: [&str; 14] = [
    "",
    " ",
    "-0",
    "+0",
    ".5",
    "5.",
    "+.5",
    "1e400",
    "-1e400",
    "0x",
    "0b2",
    "1_000",
    "0x1fffffffffffff",
    "0x20000000000001",
];

const NODE_DRIVER: &str = r#"
const input = JSON.parse(require("fs").readFileSync(0, "utf8"));
const buf = new DataView(new ArrayBuffer(8));
const toDouble = (h) => { buf.setBigUint64(0, BigInt("0x" + h)); return buf.getFloat64(0); };
const toBits = (x) => { buf.setFloat64(0, x); return buf.getBigUint64(0).toString(16).padStart(16, "0"); };
process.stdout.write(JSON.stringify({
  strings: input.doubles.map((h) => String(toDouble(h))),
  numbers: input.strings.map((s) => toBits(Number(s))),
}));
"#;

const STRINGIFY_DRIVER: &str = r#"const {values, indents} = JSON.parse(require("fs").readFileSync(0, "utf8"));
process.stdout.write(JSON.stringify(values.flatMap((v) => indents.map((i) => JSON.stringify(v, null, i)))));"#;

fn run_node(driver: &str, payload: &Value) -> Vec<u8> {
    let mut child = Command::new("node")
        .arg("-e")
        .arg(driver)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("node must be on PATH");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    out.stdout
}

struct NodeAnswers {
    strings: Vec<String>,
    numbers: Vec<u64>,
}

fn node(doubles: &[f64], strings: &[String]) -> NodeAnswers {
    let payload = json!({
        "doubles": doubles.iter().map(|x| format!("{:016x}", x.to_bits())).collect::<Vec<_>>(),
        "strings": strings,
    });
    let v: Value = serde_json::from_slice(&run_node(NODE_DRIVER, &payload)).unwrap();
    NodeAnswers {
        strings: v["strings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_owned())
            .collect(),
        numbers: v["numbers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| u64::from_str_radix(s.as_str().unwrap(), 16).unwrap())
            .collect(),
    }
}

fn node_version() -> String {
    let out = Command::new("node")
        .arg("--version")
        .output()
        .expect("node must be on PATH");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn sample<S: Strategy>(n: usize, strategy: S) -> Vec<S::Value> {
    let mut runner = TestRunner::deterministic();
    (0..n)
        .map(|_| strategy.new_tree(&mut runner).unwrap().current())
        .collect()
}

fn doubles() -> Vec<f64> {
    let mut v = sample(
        DOUBLE_CASES,
        prop_oneof![
            any::<u64>().prop_map(f64::from_bits),
            (-1e6f64..1e6f64),
            (0u64..1_000_000).prop_map(|n| n as f64 / 1000.0),
            (-30i32..30).prop_map(|e| 10f64.powi(e)),
            (0u64..100).prop_map(|n| 1e21 * (1.0 + n as f64 / 7.0)),
        ],
    );
    v.extend(EDGE_DOUBLES);
    v
}

fn numeric_strings() -> Vec<String> {
    let mut v: Vec<String> = sample(
        2000,
        prop_oneof![
            "[ \t\n\u{FEFF}\u{00A0}\u{0085}]{0,2}[+-]?[0-9]{0,6}(\\.[0-9]{0,4})?([eE][+-]?[0-9]{1,3})?[ \u{3000}]{0,2}",
            "0[xXoObB][0-9a-fA-F]{0,40}",
            "0x[0-9a-f]{30,90}",
            "[+-]?(Infinity|infinity|NaN)",
            "[0-9a-z ._-]{0,6}",
        ],
    );
    v.extend(EDGE_STRINGS.map(String::from));
    v
}

/// A number as the golden keeps it: its bits, or `NaN` for every NaN.
fn number_answer(x: f64) -> String {
    if x.is_nan() {
        "NaN".to_owned()
    } else {
        format!("{:016x}", x.to_bits())
    }
}

/// One test's cases, inputs and Rust answers as the golden renders them.
struct Cases {
    section: &'static str,
    driver: &'static str,
    /// How many cases at the end are hand-picked, kept verbatim.
    edges: usize,
    inputs: Vec<Value>,
    rust: Vec<String>,
}

fn regenerating() -> bool {
    std::env::var("TRIBUNAL_REGEN_GOLDENS").is_ok_and(|v| v == "1")
}

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/js_builtins.json")
}

fn digest<T: serde::Serialize>(items: &[T]) -> String {
    let mut text = String::new();
    for item in items {
        text.push_str(&serde_json::to_string(item).unwrap());
        text.push('\n');
    }
    sha256_hex(text.as_bytes())
}

fn chunk_hashes(answers: &[String]) -> Vec<String> {
    answers.chunks(CHUNK).map(digest).collect()
}

fn sample_cases(n: usize, edges: usize) -> Vec<usize> {
    let mut v: Vec<usize> = (0..SAMPLE.min(n)).collect();
    v.extend((n - edges).max(SAMPLE.min(n))..n);
    v
}

/// Writes `cases`' section with node's answers `js`; the sections sort by
/// name, so the three tests writing at once leave the same file.
fn record(cases: &Cases, js: &[String]) {
    static WRITING: Mutex<()> = Mutex::new(());
    let section = json!({
        "recordedWith": {"node": node_version(), "driver": sha256_hex(cases.driver.as_bytes())},
        "cases": js.len(),
        "inputsSha256": digest(&cases.inputs),
        "sample": sample_cases(js.len(), cases.edges)
            .into_iter()
            .map(|i| json!({"case": i, "input": cases.inputs[i], "js": js[i]}))
            .collect::<Vec<_>>(),
        "chunkSize": CHUNK,
        "chunks": chunk_hashes(js),
    });
    let _w = WRITING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = golden_path();
    let mut golden: serde_json::Map<String, Value> = std::fs::read_to_string(&path)
        .ok()
        .map(|t| serde_json::from_str(&t).unwrap())
        .unwrap_or_default();
    golden.insert(cases.section.to_owned(), section);
    golden.sort_keys();
    let text = serde_json::to_string_pretty(&golden).unwrap();
    std::fs::write(&path, text + "\n").unwrap();
}

/// Compares the Rust answers with node's: live, recording them, when
/// regenerating, else with the golden.
fn check(cases: Cases, node: impl FnOnce() -> Vec<String>) {
    let Cases {
        section,
        inputs,
        rust,
        ..
    } = &cases;
    if regenerating() {
        let js = node();
        assert_eq!(js.len(), rust.len(), "{section}: one answer per case");
        record(&cases, &js);
        let bad: Vec<String> = (0..js.len())
            .filter(|&i| rust[i] != js[i])
            .map(|i| format!("{}: rust {} js {}", inputs[i], rust[i], js[i]))
            .collect();
        assert!(
            bad.is_empty(),
            "{section}: {} of {} differ, first {}",
            bad.len(),
            js.len(),
            bad[0]
        );
        return;
    }
    let path = golden_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; {REGENERATE}", path.display()));
    let golden: Value = serde_json::from_str(&text).unwrap();
    let s = &golden[*section];
    assert!(
        s.is_object(),
        "{section} is not in {}; {REGENERATE}",
        path.display()
    );
    assert_eq!(
        s["recordedWith"]["driver"],
        json!(sha256_hex(cases.driver.as_bytes())),
        "{section}: the node driver changed since it was recorded; {REGENERATE}"
    );
    assert_eq!(
        s["inputsSha256"],
        json!(digest(inputs)),
        "{section}: the generated inputs changed since they were recorded; {REGENERATE}"
    );
    let bad: Vec<String> = s["sample"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| {
            let i = usize::try_from(e["case"].as_u64().unwrap()).unwrap();
            assert_eq!(e["input"], inputs[i], "{section}: case {i}; {REGENERATE}");
            (e["js"] != json!(rust[i]))
                .then(|| format!("case {i} {}: rust {} js {}", inputs[i], rust[i], e["js"]))
        })
        .collect();
    assert!(
        bad.is_empty(),
        "{section}: {} sampled cases differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
    let want = s["chunks"].as_array().unwrap();
    let got = chunk_hashes(rust);
    assert_eq!(
        want.len(),
        got.len(),
        "{section}: chunk count; {REGENERATE}"
    );
    let differ: Vec<usize> = (0..got.len()).filter(|&k| want[k] != got[k]).collect();
    if let Some(&k) = differ.first() {
        let lines: Vec<String> = (k * CHUNK..((k + 1) * CHUNK).min(rust.len()))
            .map(|i| format!("{i} {} -> {}", inputs[i], rust[i]))
            .collect();
        panic!(
            "{section}: {} of {} chunks differ from node's answers, first chunk {k} (cases {} to {}); its Rust answers:\n{}",
            differ.len(),
            got.len(),
            k * CHUNK,
            k * CHUNK + lines.len() - 1,
            lines.join("\n")
        );
    }
}

#[test]
fn number_to_string_matches_node() {
    let ds = doubles();
    let cases = Cases {
        section: "numberToString",
        driver: NODE_DRIVER,
        edges: EDGE_DOUBLES.len(),
        inputs: ds
            .iter()
            .map(|x| json!(format!("{:016x}", x.to_bits())))
            .collect(),
        rust: ds
            .iter()
            .map(|x| tribunal_core::js::number_to_string(*x))
            .collect(),
    };
    check(cases, || node(&ds, &[]).strings);
}

#[test]
fn string_to_number_matches_node() {
    let ss = numeric_strings();
    let cases = Cases {
        section: "stringToNumber",
        driver: NODE_DRIVER,
        edges: EDGE_STRINGS.len(),
        inputs: ss.iter().map(|s| json!(s)).collect(),
        rust: ss
            .iter()
            .map(|s| number_answer(tribunal_core::js::string_to_number(s)))
            .collect(),
    };
    check(cases, || {
        node(&[], &ss)
            .numbers
            .into_iter()
            .map(|bits| number_answer(f64::from_bits(bits)))
            .collect()
    });
}

fn json_value() -> impl Strategy<Value = serde_json::Value> {
    use serde_json::{json, Value};
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        prop_oneof![
            Just(json!(0)),
            Just(json!(-0.0)),
            Just(json!(1e21)),
            Just(json!(0.1)),
            Just(json!(1.5e-7)),
            Just(json!(-42)),
            Just(json!(u64::MAX))
        ],
        prop_oneof![
            "[a-z\"\\\\ ]{0,6}",
            Just("\u{0}\u{8}\u{c}\u{1f}\u{7f}\t\n\r".to_owned()),
            Just("\u{2028}\u{2029} ü 🎉".to_owned()),
        ]
        .prop_map(Value::String),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::vec(
                (
                    prop_oneof![
                        "[a-z]{1,3}",
                        Just("2".to_owned()),
                        Just("10".to_owned()),
                        Just("01".to_owned())
                    ],
                    inner
                ),
                0..4
            )
            .prop_map(|kv| Value::Object(kv.into_iter().collect())),
        ]
    })
}

#[test]
fn stringify_matches_node() {
    let values = sample(2000, json_value());
    let indents = ["", "  ", " "];
    let cases = Cases {
        section: "stringify",
        driver: STRINGIFY_DRIVER,
        edges: 0,
        inputs: values
            .iter()
            .flat_map(|v| indents.iter().map(move |i| json!([v, i])))
            .collect(),
        rust: values
            .iter()
            .flat_map(|v| indents.iter().map(|i| tribunal_core::js::stringify(v, i)))
            .collect(),
    };
    check(cases, || {
        let out = run_node(
            STRINGIFY_DRIVER,
            &json!({"values": values, "indents": indents}),
        );
        serde_json::from_slice(&out).unwrap()
    });
}
