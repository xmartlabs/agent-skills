//! The JavaScript number conversions the ports depend on, checked against
//! Node's own `String(x)` and `Number(s)`. Doubles cross the process boundary
//! as their bit patterns, so NaN, -0 and the last ulp survive the trip.

use std::io::Write;
use std::process::{Command, Stdio};

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;

const DOUBLE_CASES: usize = 200_000;

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

struct NodeAnswers {
    strings: Vec<String>,
    numbers: Vec<u64>,
}

fn node(doubles: &[f64], strings: &[String]) -> NodeAnswers {
    let payload = serde_json::json!({
        "doubles": doubles.iter().map(|x| format!("{:016x}", x.to_bits())).collect::<Vec<_>>(),
        "strings": strings,
    });
    let mut child = Command::new("node")
        .arg("-e")
        .arg(NODE_DRIVER)
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
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
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
    v.extend([
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
    ]);
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
    v.extend(
        [
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
        ]
        .map(String::from),
    );
    v
}

fn check_number_to_string(name: &str, f: fn(f64) -> String) -> usize {
    let ds = doubles();
    let js = node(&ds, &[]);
    let bad: Vec<String> = ds
        .iter()
        .zip(&js.strings)
        .filter(|(x, s)| f(**x) != **s)
        .map(|(x, s)| format!("{:016x}: rust {} js {}", x.to_bits(), f(*x), s))
        .collect();
    if !bad.is_empty() {
        eprintln!(
            "{name}: {} of {} differ, first {}",
            bad.len(),
            ds.len(),
            bad[0]
        );
    }
    bad.len()
}

fn check_string_to_number(name: &str, f: fn(&str) -> f64) -> usize {
    let ss = numeric_strings();
    let js = node(&[], &ss);
    let bad: Vec<String> = ss
        .iter()
        .zip(&js.numbers)
        .filter(|(s, bits)| {
            let r = f(s);
            !(r.to_bits() == **bits || (r.is_nan() && f64::from_bits(**bits).is_nan()))
        })
        .map(|(s, bits)| format!("{s:?}: rust {:e} js {:e}", f(s), f64::from_bits(*bits)))
        .collect();
    if !bad.is_empty() {
        eprintln!(
            "{name}: {} of {} differ, first {}",
            bad.len(),
            ss.len(),
            bad[0]
        );
    }
    bad.len()
}

#[test]
fn number_to_string_matches_node() {
    assert_eq!(
        check_number_to_string("js::number_to_string", tribunal_core::js::number_to_string),
        0
    );
}

#[test]
fn string_to_number_matches_node() {
    assert_eq!(
        check_string_to_number("js::string_to_number", tribunal_core::js::string_to_number),
        0
    );
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
    let mut child = Command::new("node")
        .arg("-e")
        .arg(r#"const {values, indents} = JSON.parse(require("fs").readFileSync(0, "utf8"));
process.stdout.write(JSON.stringify(values.flatMap((v) => indents.map((i) => JSON.stringify(v, null, i)))));"#)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("node must be on PATH");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            serde_json::json!({"values": values, "indents": indents})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let node: Vec<String> = serde_json::from_slice(&out.stdout).unwrap();
    let rust: Vec<String> = values
        .iter()
        .flat_map(|v| indents.iter().map(|i| tribunal_core::js::stringify(v, i)))
        .collect();
    for (i, (r, n)) in rust.iter().zip(&node).enumerate() {
        assert_eq!(r, n, "value {}", values[i / indents.len()]);
    }
    assert_eq!(rust.len(), node.len());
}
