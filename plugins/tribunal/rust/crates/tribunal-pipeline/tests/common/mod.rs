//! The span oracle: runs a slice of the JavaScript workflow that sits between
//! marked regions, with its free variables passed in, and returns what the
//! `ret` expression evaluates to. Shares `canon` and `sample` with
//! tribunal-core's differential tests.
//!
//! The workflow is being retired, so a binary with a golden at
//! `tests/goldens/span/<binary>.json` never starts node: each span call is
//! looked up by a SHA-256 of the span, its `ret` and its params, and one the
//! file lacks fails with the regeneration command. The file keeps a SHA-256
//! of the workflow code each span came from and of the span oracle's own
//! files, so editing either without regenerating fails; a source gone from
//! disk is retired and is not checked. A sampled call (more than one params
//! object) keeps each result over [`DIGEST_OVER`] bytes as a SHA-256 of its
//! `canon` rendering, which [`assert_same`] compares; a call of one keeps its
//! result whole, since its test reads into it. [`recorded_value`] keeps, in
//! the same file, what a test read from a retired source other than a span.
//! A binary without the file runs the oracle live and needs `node` on PATH.
//!
//! Regenerate over the whole binary, so that every call is recorded:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-pipeline --test <binary>`.
//! It rewrites the file only when it already exists; to start recording a
//! binary, create the file as `{}` first.
#![allow(dead_code)]

#[path = "../../../tribunal-core/tests/common/mod.rs"]
mod core_common;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};

use serde_json::{json, Map, Value};
use tribunal_context::harness_version::sha256_hex;

#[allow(unused_imports)]
pub use core_common::{canon, sample, CASES};

/// A `log` that records, read back with [`LOGS`] in `ret`.
pub const LOG_FN: &str = "(m) => (globalThis.__logs ||= []).push(m)";
pub const LOGS: &str = "(globalThis.__logs || []).splice(0)";

/// A sampled result whose `canon` rendering is longer is kept as its SHA-256.
pub const DIGEST_OVER: usize = 1024;

const BINARY: &str = env!("CARGO_CRATE_NAME");
const WORKFLOW: &str = "workflow-src/tribunal.js";
/// The span oracle's own files, by path in the plugin.
const ORACLE_FILES: &[&str] = &[
    "rust/oracle/span-oracle.mjs",
    "skills/review-pr/eval/extract-pure-region.mjs",
];

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
    span_raw_at(span, params, &[])
}

pub fn span(span: &Span, params: &[Value]) -> Vec<Value> {
    span_at(span, params, &[])
}

/// [`span`] with machine-specific strings in the params and results, such as
/// a scratch directory, kept in the golden as the placeholder paired with
/// each, e.g. `(dir, "<ROOT>")`.
pub fn span_at(span: &Span, params: &[Value], places: &[(&str, &str)]) -> Vec<Value> {
    span_raw_at(span, params, places)
        .into_iter()
        .map(|r| r.unwrap_or_else(|e| panic!("span {} threw: {e}", span.start)))
        .collect()
}

pub fn span_raw_at(
    span: &Span,
    params: &[Value],
    places: &[(&str, &str)],
) -> Vec<Result<Value, String>> {
    let calls: Vec<Value> = params
        .iter()
        .map(|p| {
            json!({
                "start": span.start, "end": span.end, "deps": span.deps, "pre": span.pre,
                "fns": span.fns, "params": p, "ret": span.ret,
            })
        })
        .collect();
    let mut guard = golden();
    if guard.as_ref().unwrap().is_none() {
        drop(guard);
        return run_oracle(&calls).into_iter().map(result).collect();
    }
    let (golden, path) = guard.as_mut().unwrap().as_mut().unwrap();
    let span_key = sha256_hex(
        serde_json::to_string(&json!([span.start, span.end, span.deps, span.pre]))
            .unwrap()
            .as_bytes(),
    );
    let key = sha256_hex(swap(&serde_json::to_string(&calls).unwrap(), places, true).as_bytes());
    let oracle_now: BTreeMap<&str, Option<String>> = ORACLE_FILES
        .iter()
        .map(|f| (*f, plugin_file_sha(f)))
        .collect();
    if regenerating() {
        let with: Map<String, Value> = oracle_now
            .iter()
            .map(|(k, sha)| ((*k).to_owned(), json!(present(k, sha))))
            .collect();
        golden["recordedWith"] = Value::Object(with);
        let workflow = read_workflow()
            .unwrap_or_else(|| panic!("{WORKFLOW} is gone: nothing to regenerate from"));
        let source = span_source(&workflow, span)
            .unwrap_or_else(|| panic!("span {} is no longer in the workflow", span.start));
        golden["spans"][&span_key] =
            json!({"start": span.start, "source": sha256_hex(source.as_bytes())});
        let out = run_oracle(&calls);
        let kept: Vec<Value> = out
            .iter()
            .map(|r| {
                let r = digest(r, calls.len() > 1);
                serde_json::from_str(&swap(&serde_json::to_string(&r).unwrap(), places, true))
                    .unwrap()
            })
            .collect();
        golden["calls"][&key] = json!({"span": span_key, "results": kept});
        write_golden(golden, path);
        return out.into_iter().map(result).collect();
    }
    let stale = stale(&golden["recordedWith"], &oracle_now);
    assert!(
        stale.is_empty(),
        "{} changed since {} was recorded; {}",
        stale.join(", "),
        path.display(),
        regenerate()
    );
    let recorded = &golden["spans"][&span_key];
    assert!(
        !recorded.is_null(),
        "span {} has no entry in {}; {}",
        span.start,
        path.display(),
        regenerate()
    );
    if let Some(src) = read_workflow() {
        let now = span_source(&src, span).map(|s| sha256_hex(s.as_bytes()));
        assert!(
            now.is_some_and(|s| recorded["source"] == json!(s)),
            "the workflow code of span {} changed since {} was recorded; {}",
            span.start,
            path.display(),
            regenerate()
        );
    }
    let Some(results) = golden["calls"][&key]["results"].as_array() else {
        panic!(
            "a call of span {} is not in {}: its params or `ret` changed; {}",
            span.start,
            path.display(),
            regenerate()
        );
    };
    assert_eq!(results.len(), calls.len(), "{}", regenerate());
    results
        .iter()
        .map(|r| {
            let text = swap(&serde_json::to_string(r).unwrap(), places, false);
            result(serde_json::from_str(&text).unwrap())
        })
        .collect()
}

/// What a test read from an oracle other than a span, kept as `name` in the
/// binary's golden: `live` runs only when regenerating, or always in a binary
/// without a golden. `sources` are the SHA-256s of what `live` reads, `None`
/// for one gone from disk (retired, not checked); `input` is checked always,
/// as a SHA-256 of its `canon` rendering.
pub fn recorded_value(
    name: &str,
    sources: &[(&str, Option<String>)],
    input: &Value,
    live: impl FnOnce() -> Value,
) -> Value {
    let input_sha = sha256_hex(canon(input).as_bytes());
    let mut guard = golden();
    if guard.as_ref().unwrap().is_none() {
        drop(guard);
        return live();
    }
    let (golden, path) = guard.as_mut().unwrap().as_mut().unwrap();
    if regenerating() {
        let with: Map<String, Value> = sources
            .iter()
            .map(|(k, sha)| ((*k).to_owned(), json!(present(k, sha))))
            .collect();
        let value = live();
        golden["values"][name] = json!({"recordedWith": with, "input": input_sha, "value": value});
        write_golden(golden, path);
        return value;
    }
    let entry = &golden["values"][name];
    assert!(
        !entry.is_null(),
        "{name} has no entry in {}; {}",
        path.display(),
        regenerate()
    );
    let now: BTreeMap<&str, Option<String>> =
        sources.iter().map(|(k, sha)| (*k, sha.clone())).collect();
    let stale = stale(&entry["recordedWith"], &now);
    assert!(
        stale.is_empty(),
        "{} changed since {name} in {} was recorded; {}",
        stale.join(", "),
        path.display(),
        regenerate()
    );
    assert_eq!(
        entry["input"],
        json!(input_sha),
        "the input of {name} changed; {}",
        regenerate()
    );
    entry["value"].clone()
}

/// `canon(rust) == canon(js)` for each case, where a `js` result kept as a
/// digest compares by the SHA-256 of `canon(rust)`.
pub fn assert_same(what: &str, inputs: &[Value], rust: &[Value], js: &[Value]) {
    assert_eq!(rust.len(), js.len(), "{what}: the number of results");
    let mut diffs = Vec::new();
    for ((input, r), j) in inputs.iter().zip(rust).zip(js) {
        if !same(r, j) {
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

/// Whether `rust` renders as `js` does, `js` possibly holding digests.
pub fn same(rust: &Value, js: &Value) -> bool {
    if let Some(d) = js
        .get("$digest")
        .filter(|_| js.as_object().is_some_and(|o| o.len() == 1))
    {
        return d["sha256"] == json!(sha256_hex(canon(rust).as_bytes()));
    }
    match (rust, js) {
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(x, y))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|((ka, x), (kb, y))| ka == kb && same(x, y))
        }
        _ => canon(rust) == canon(js),
    }
}

/// The SHA-256 of the file at `rel` in the plugin, `None` when it is gone.
pub fn plugin_file_sha(rel: &str) -> Option<String> {
    std::fs::read(plugin_root().join(rel))
        .ok()
        .map(|b| sha256_hex(&b))
}

pub fn plugin_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

pub fn regenerating() -> bool {
    std::env::var("TRIBUNAL_REGEN_GOLDENS").is_ok_and(|v| v == "1")
}

fn regenerate() -> String {
    format!(
        "regenerate with `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-pipeline --test {BINARY}`"
    )
}

fn present(name: &str, sha: &Option<String>) -> String {
    sha.clone()
        .unwrap_or_else(|| panic!("{name} is gone: nothing to regenerate from"))
}

fn read_workflow() -> Option<String> {
    std::fs::read_to_string(plugin_root().join(WORKFLOW)).ok()
}

/// The workflow code the span oracle runs for `s`, cut out the way it cuts it;
/// `None` when an anchor is no longer in `src`.
pub fn span_source(src: &str, s: &Span) -> Option<String> {
    let cut = |start: &str, end: &str| -> Option<String> {
        let a = src.find(start)?;
        let e = a + src[a..].find(end)?;
        Some(src[a..e + end.len()].to_owned())
    };
    let mut parts = Vec::new();
    for d in s.deps {
        let (begin, end) = (
            format!("// ===== BEGIN {d} ====="),
            format!("// ===== END {d} ====="),
        );
        let mut from = 0;
        while let Some(b) = src[from..].find(&begin).map(|b| b + from) {
            let e = b + src[b..].find(&end)?;
            parts.push(src[b + begin.len()..e].to_owned());
            from = e + end.len();
        }
    }
    for (a, b) in s.pre {
        parts.push(cut(a, b)?);
    }
    parts.push(cut(s.start, s.end)?);
    Some(parts.join("\n"))
}

/// A result as the golden keeps it: an `ok` value of a sampled call over
/// [`DIGEST_OVER`] bytes becomes `{"$digest": {"sha256", "bytes"}}`.
fn digest(r: &Value, sampled: bool) -> Value {
    match r.get("ok") {
        Some(v) if sampled => {
            let text = canon(v);
            if text.len() <= DIGEST_OVER {
                return r.clone();
            }
            json!({"ok": {"$digest": {"sha256": sha256_hex(text.as_bytes()), "bytes": text.len()}}})
        }
        _ => r.clone(),
    }
}

/// Swaps each machine-specific string for its placeholder (`out`), or back,
/// inside serialized JSON.
fn swap(text: &str, places: &[(&str, &str)], out: bool) -> String {
    let quoted = |s: &str| {
        let q = serde_json::to_string(s).unwrap();
        q[1..q.len() - 1].to_owned()
    };
    places.iter().fold(text.to_owned(), |t, (real, place)| {
        let (real, place) = (quoted(real), quoted(place));
        if out {
            t.replace(&real, &place)
        } else {
            t.replace(&place, &real)
        }
    })
}

fn result(r: Value) -> Result<Value, String> {
    match r.get("ok") {
        Some(v) => Ok(v.clone()),
        None => Err(r["err"].as_str().unwrap_or_default().to_owned()),
    }
}

fn run_oracle(calls: &[Value]) -> Vec<Value> {
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
}

static GOLDEN: Mutex<Golden> = Mutex::new(None);

type Golden = Option<Option<(Value, PathBuf)>>;

/// The binary's golden, read once: always `Some` outside, `None` inside when
/// the binary has none and runs live. Started empty when regenerating, so a
/// call no test makes any more is dropped.
fn golden() -> MutexGuard<'static, Golden> {
    let mut guard = GOLDEN.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/goldens/span")
            .join(format!("{BINARY}.json"));
        let loaded = if !path.exists() {
            None
        } else if regenerating() {
            Some(json!({"recordedWith": {}, "spans": {}, "calls": {}, "values": {}}))
        } else {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}; {}", path.display(), regenerate()));
            Some(
                serde_json::from_str(&text)
                    .unwrap_or_else(|e| panic!("{}: {e}; {}", path.display(), regenerate())),
            )
        };
        *guard = Some(loaded.map(|g| (g, path)));
    }
    guard
}

/// The sources still on disk that differ from `recorded`; a retired one is
/// skipped, since nothing can regenerate from it.
fn stale(recorded: &Value, now: &BTreeMap<&str, Option<String>>) -> Vec<String> {
    let keys: Vec<&str> = recorded
        .as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    if keys != now.keys().copied().collect::<Vec<_>>() {
        return vec!["the list of sources".to_owned()];
    }
    now.iter()
        .filter(|(k, sha)| sha.as_ref().is_some_and(|s| recorded[**k] != json!(s)))
        .map(|(k, _)| (*k).to_owned())
        .collect()
}

/// Pretty at the top, one line per span, per result and per value, so a
/// regeneration diffs by case.
fn write_golden(golden: &mut Value, path: &Path) {
    let line = |v: &Value| serde_json::to_string(v).unwrap();
    let section = |name: &str, each: &dyn Fn(&Value) -> String| -> String {
        let mut m: Vec<(&String, &Value)> = golden[name]
            .as_object()
            .map(|m| m.iter().collect())
            .unwrap_or_default();
        m.sort_by(|a, b| a.0.cmp(b.0));
        let body: Vec<String> = m
            .iter()
            .map(|(k, v)| format!("    {}: {}", line(&json!(k)), each(v)))
            .collect();
        format!("  {}: {{\n{}\n  }}", line(&json!(name)), body.join(",\n"))
    };
    let calls = |c: &Value| {
        let results: Vec<String> = c["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| format!("      {}", line(r)))
            .collect();
        format!(
            "{{\"span\": {}, \"results\": [\n{}\n    ]}}",
            line(&c["span"]),
            results.join(",\n")
        )
    };
    let text = [
        format!(
            "  \"recordedWith\": {}",
            serde_json::to_string_pretty(&golden["recordedWith"])
                .unwrap()
                .replace('\n', "\n  ")
        ),
        section("spans", &|v| line(v)),
        section("calls", &calls),
        section("values", &|v| line(v)),
    ]
    .join(",\n");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("{{\n{text}\n}}\n")).unwrap();
}
