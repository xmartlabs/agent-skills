//! Shared harness for the differential tests: the JavaScript oracle, a
//! comparison that keeps key order, and a deterministic case generator.
//!
//! These tests need `node` on PATH. Without it they fail; they do not skip.
#![allow(dead_code)]

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use proptest::strategy::Strategy;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use serde_json::Value;

pub const CASES: usize = 600;

pub fn oracle_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../oracle/region-oracle.mjs")
}

/// Runs every call through the JavaScript region in one process. A call that
/// throws in JavaScript comes back as `Err` with its message.
pub fn oracle_raw(
    region: &str,
    exports: &[&str],
    deps: &[&str],
    calls: &[Value],
) -> Vec<Result<Value, String>> {
    let results = run_oracle(region, exports, deps, calls);
    results
        .into_iter()
        .map(|r| match r.get("ok") {
            Some(v) => Ok(v.clone()),
            None => Err(r["err"].as_str().unwrap_or_default().to_owned()),
        })
        .collect()
}

/// `oracle_raw` for regions that must not throw on the inputs given.
pub fn oracle(region: &str, exports: &[&str], deps: &[&str], calls: &[Value]) -> Vec<Value> {
    oracle_raw(region, exports, deps, calls)
        .into_iter()
        .map(|r| r.unwrap_or_else(|e| panic!("{region} threw: {e}")))
        .collect()
}

fn run_oracle(region: &str, exports: &[&str], deps: &[&str], calls: &[Value]) -> Vec<Value> {
    let mut cmd = Command::new("node");
    cmd.arg(oracle_path()).arg(region).arg(exports.join(","));
    if !deps.is_empty() {
        cmd.arg(deps.join(","));
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("node must be on PATH to run the differential tests");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(calls).unwrap().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "oracle failed for {region}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let results: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        results.len(),
        calls.len(),
        "oracle returned the wrong number of results"
    );
    results
}

/// A rendering that keeps object key order and writes numbers the way
/// `JSON.stringify` does, so `3.0` and `3` compare equal but a moved key does
/// not.
pub fn canon(v: &Value) -> String {
    match v {
        Value::Number(n) => {
            let x = n.as_f64().unwrap();
            if x.fract() == 0.0 && x.abs() < 1e21 {
                format!("{}", x as i128)
            } else {
                format!("{x}")
            }
        }
        Value::Array(a) => format!("[{}]", a.iter().map(canon).collect::<Vec<_>>().join(",")),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}:{}", serde_json::to_string(k).unwrap(), canon(v)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        other => other.to_string(),
    }
}

pub fn sample<S: Strategy>(strategy: S) -> Vec<S::Value> {
    let mut runner = TestRunner::deterministic();
    (0..CASES)
        .map(|_| strategy.new_tree(&mut runner).unwrap().current())
        .collect()
}

pub fn assert_same(what: &str, inputs: &[Value], rust: &[Value], js: &[Value]) {
    let mut diffs = Vec::new();
    for ((input, r), j) in inputs.iter().zip(rust).zip(js) {
        if canon(r) != canon(j) {
            diffs.push(format!(
                "input {}\n  rust {}\n  js   {}",
                input,
                canon(r),
                canon(j)
            ));
        }
    }
    assert!(
        diffs.is_empty(),
        "{what}: {} of {} cases differ. First:\n{}",
        diffs.len(),
        inputs.len(),
        diffs[0]
    );
}
