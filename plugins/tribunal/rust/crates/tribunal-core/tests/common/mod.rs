//! Shared harness for the differential tests: the JavaScript oracle, a
//! comparison that keeps key order, and a deterministic case generator.
//!
//! The oracle's results are recorded per test binary in
//! `tests/goldens/oracle/<test>.json` under the including crate, because the
//! Workflow it evaluates is being retired. A binary with that file never
//! starts node: each `oracle_raw` call is looked up by a SHA-256 of its
//! region, exports, deps and calls, and one the file lacks fails with the
//! regeneration command. The file also keeps a SHA-256 of each region's text
//! in `workflow-src/tribunal.js` and of the oracle's own files, so editing
//! either without regenerating fails; once the workflow is gone from disk
//! those checks are skipped, since nothing can regenerate from it. A binary
//! without the file runs the oracle live and needs `node` on PATH; without it
//! those tests fail, they do not skip.
//!
//! Regenerate over the whole binary, so that every call is recorded:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p <crate> --test <test>`. It
//! rewrites the file only when it already exists; to start recording a
//! binary, create the file as `{}` first.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

use proptest::strategy::Strategy;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use serde_json::{json, Value};

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
    let results = recorded_oracle(region, exports, deps, calls);
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

// ---------------------------------------------------------------------------
// Golden: the oracle's results recorded once per test binary.

/// The workflow the oracle slices regions from, by path in the plugin.
pub const ORACLE_WORKFLOW: &str = "workflow-src/tribunal.js";
/// The oracle's own files, by path in the plugin.
pub const ORACLE_FILES: &[&str] = &[
    "rust/oracle/region-oracle.mjs",
    "skills/review-pr/eval/extract-pure-region.mjs",
];

pub fn oracle_golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens/oracle")
        .join(format!("{}.json", env!("CARGO_CRATE_NAME")))
}

fn oracle_regenerate() -> String {
    format!(
        "regenerate with `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p {} --test {}`",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_CRATE_NAME")
    )
}

fn oracle_plugin_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

enum Mode {
    Live,
    Record(Mutex<Recording>),
    Replay(Value),
}

#[derive(Default)]
struct Recording {
    sources: BTreeMap<String, String>,
    calls: BTreeMap<String, (String, Vec<Value>)>,
}

fn mode() -> &'static Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    MODE.get_or_init(|| {
        let path = oracle_golden_path();
        if !path.exists() {
            return Mode::Live;
        }
        if std::env::var("TRIBUNAL_REGEN_GOLDENS").is_ok_and(|v| v == "1") {
            return Mode::Record(Mutex::new(Recording::default()));
        }
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e}; {}", path.display(), oracle_regenerate()));
        let golden: Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{}: {e}; {}", path.display(), oracle_regenerate()));
        let stale = oracle_stale(&golden["sources"], &oracle_plugin_root());
        assert!(
            stale.is_empty(),
            "{} changed since {} was recorded; {}",
            stale.join(", "),
            path.display(),
            oracle_regenerate()
        );
        Mode::Replay(golden)
    })
}

fn recorded_oracle(region: &str, exports: &[&str], deps: &[&str], calls: &[Value]) -> Vec<Value> {
    match mode() {
        Mode::Live => run_oracle(region, exports, deps, calls),
        Mode::Replay(golden) => oracle_replay(golden, region, exports, deps, calls),
        Mode::Record(rec) => {
            let mut names = vec![region];
            names.extend_from_slice(deps);
            let sources: BTreeMap<String, String> = oracle_sources(&oracle_plugin_root(), &names)
                .into_iter()
                .map(|(k, sha)| {
                    let sha =
                        sha.unwrap_or_else(|| panic!("{k} is gone: nothing to regenerate from"));
                    (k, sha)
                })
                .collect();
            let results = run_oracle(region, exports, deps, calls);
            let mut rec = rec.lock().unwrap_or_else(|e| e.into_inner());
            rec.sources.extend(sources);
            rec.calls.insert(
                oracle_call_key(region, exports, deps, calls),
                (region.to_owned(), results.clone()),
            );
            std::fs::write(oracle_golden_path(), render_golden(&rec)).unwrap();
            results
        }
    }
}

/// The SHA-256 that names one oracle call in the golden.
pub fn oracle_call_key(region: &str, exports: &[&str], deps: &[&str], calls: &[Value]) -> String {
    sha256_hex(
        serde_json::to_string(&json!([region, exports, deps, calls]))
            .unwrap()
            .as_bytes(),
    )
}

/// The recorded results of one call; a call the golden lacks fails with the
/// regeneration command.
pub fn oracle_replay(
    golden: &Value,
    region: &str,
    exports: &[&str],
    deps: &[&str],
    calls: &[Value],
) -> Vec<Value> {
    let key = oracle_call_key(region, exports, deps, calls);
    let Some(results) = golden["calls"][&key]["results"].as_array() else {
        panic!(
            "{region}: call {key} is not in {}; {}",
            oracle_golden_path().display(),
            oracle_regenerate()
        );
    };
    assert_eq!(
        results.len(),
        calls.len(),
        "{region}: call {key} has the wrong number of results; {}",
        oracle_regenerate()
    );
    results.clone()
}

/// What the recorded results came from, by path in the plugin under `root`:
/// a SHA-256 of each region's text in the workflow (keyed
/// `<workflow>#<region>`) and of each of the oracle's files. A file that is
/// gone maps to `None`.
pub fn oracle_sources(root: &Path, regions: &[&str]) -> BTreeMap<String, Option<String>> {
    let mut out = BTreeMap::new();
    let workflow = std::fs::read_to_string(root.join(ORACLE_WORKFLOW)).ok();
    for r in regions {
        let sha = workflow
            .as_deref()
            .map(|w| sha256_hex(region_text(w, r).as_bytes()));
        out.insert(format!("{ORACLE_WORKFLOW}#{r}"), sha);
    }
    for f in ORACLE_FILES {
        let sha = std::fs::read(root.join(f)).ok().map(|b| sha256_hex(&b));
        out.insert((*f).to_owned(), sha);
    }
    out
}

/// The recorded sources (`{name: sha}`) that differ under `root`. With the
/// workflow gone the oracle is retired and the golden is final, so nothing
/// is checked; otherwise a file gone from disk is skipped.
pub fn oracle_stale(recorded: &Value, root: &Path) -> Vec<String> {
    if !root.join(ORACLE_WORKFLOW).exists() {
        return Vec::new();
    }
    let recorded = recorded.as_object().cloned().unwrap_or_default();
    if recorded.is_empty() {
        return vec!["the list of sources".to_owned()];
    }
    let regions: Vec<&str> = recorded
        .keys()
        .filter_map(|k| k.strip_prefix(&format!("{ORACLE_WORKFLOW}#")))
        .collect();
    oracle_sources(root, &regions)
        .into_iter()
        .filter(|(k, sha)| {
            sha.as_ref()
                .is_some_and(|s| recorded.get(k) != Some(&json!(s)))
        })
        .map(|(k, _)| k)
        .collect()
}

/// Every `name` region of `src` joined by newlines, cut the way
/// `extract-pure-region.mjs`'s `extractRegions` cuts them.
fn region_text(src: &str, name: &str) -> String {
    let (begin, end) = (
        format!("// ===== BEGIN {name} ====="),
        format!("// ===== END {name} ====="),
    );
    let mut parts = Vec::new();
    let mut from = 0;
    while let Some(b) = src[from..].find(&begin).map(|b| b + from) {
        let Some(e) = src[b..].find(&end).map(|e| e + b) else {
            parts.push(&src[b..]);
            break;
        };
        parts.push(&src[b + begin.len()..e]);
        from = e + end.len();
    }
    parts.join("\n")
}

/// The golden with one result per line, so a regeneration diffs by case.
fn render_golden(rec: &Recording) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap();
    let sources: Vec<String> = rec
        .sources
        .iter()
        .map(|(k, v)| format!("    {}: {}", q(k), q(v)))
        .collect();
    let calls: Vec<String> = rec
        .calls
        .iter()
        .map(|(key, (region, results))| {
            let lines: Vec<String> = results
                .iter()
                .map(|r| format!("      {}", serde_json::to_string(r).unwrap()))
                .collect();
            format!(
                "    {}: {{\"region\": {}, \"results\": [\n{}\n    ]}}",
                q(key),
                q(region),
                lines.join(",\n")
            )
        })
        .collect();
    format!(
        "{{\n  \"sources\": {{\n{}\n  }},\n  \"calls\": {{\n{}\n  }}\n}}\n",
        sources.join(",\n"),
        calls.join(",\n")
    )
}

/// `createHash("sha256").update(bytes).digest("hex")`; tribunal-core has no
/// hashing dependency to borrow.
pub fn sha256_hex(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = bytes.to_vec();
    let bit_len = (msg.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for block in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
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
