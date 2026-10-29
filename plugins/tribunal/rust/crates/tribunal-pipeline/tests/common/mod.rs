//! The span oracle: runs a slice of the JavaScript workflow that sits between
//! marked regions, with its free variables passed in, and returns what the
//! `ret` expression evaluates to. Shares `canon`, `sample` and `assert_same`
//! with tribunal-core's differential tests.
#![allow(dead_code)]

#[path = "../../../tribunal-core/tests/common/mod.rs"]
mod core_common;

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

#[allow(unused_imports)]
pub use core_common::{assert_same, canon, sample, CASES};

/// A `log` that records, read back with [`LOGS`] in `ret`.
pub const LOG_FN: &str = "(m) => (globalThis.__logs ||= []).push(m)";
pub const LOGS: &str = "(globalThis.__logs || []).splice(0)";

pub fn span_oracle_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../oracle/span-oracle.mjs")
}

pub struct Span<'a> {
    pub start: &'a str,
    pub end: &'a str,
    pub deps: &'a [&'a str],
    pub pre: &'a [(&'a str, &'a str)],
    pub fns: &'a [&'a str],
    pub ret: &'a str,
}

/// One result per params object; a span that throws comes back as `Err`.
pub fn span_raw(span: &Span, params: &[Value]) -> Vec<Result<Value, String>> {
    let calls: Vec<Value> = params
        .iter()
        .map(|p| {
            json!({
                "start": span.start, "end": span.end, "deps": span.deps, "pre": span.pre,
                "fns": span.fns, "params": p, "ret": span.ret,
            })
        })
        .collect();
    let mut child = Command::new("node")
        .arg(span_oracle_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("node must be on PATH to run the differential tests");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(&calls).unwrap().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "span oracle failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let results: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(results.len(), calls.len());
    results
        .into_iter()
        .map(|r| match r.get("ok") {
            Some(v) => Ok(v.clone()),
            None => Err(r["err"].as_str().unwrap_or_default().to_owned()),
        })
        .collect()
}

pub fn span(span: &Span, params: &[Value]) -> Vec<Value> {
    span_raw(span, params)
        .into_iter()
        .map(|r| r.unwrap_or_else(|e| panic!("span {} threw: {e}", span.start)))
        .collect()
}
