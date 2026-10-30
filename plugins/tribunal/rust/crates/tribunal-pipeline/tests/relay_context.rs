//! `context::relay_context` against fetch-pr-context.mjs's own exported
//! `relayContext`: the same context back, and the same bytes on disk.
//!
//! The script's side is read from `tests/goldens/span/relay_context.json`
//! (see `common`): each case's context, with the scratch directory as
//! `<ROOT>`, and a SHA-256 of each file it wrote, together with a SHA-256 of
//! the script and of the generated cases. The context names the platform it
//! ran on; replayed on another one, the platform node would name there is
//! expected instead. Regenerate after changing the script or the cases:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-pipeline --test relay_context`.

mod common;

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use common::{assert_same, plugin_file_sha, recorded_value, sample};
use proptest::prelude::*;
use serde_json::{json, Value};
use tribunal_context::harness_version::sha256_hex;
use tribunal_pipeline::context::relay_context;

const SCRIPT: &str = "skills/review-pr/scripts/fetch-pr-context.mjs";
const FILES: [&str; 2] = ["raw.json", "bot-comments.json"];

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join(SCRIPT)
}

/// `process.platform` on this machine.
fn node_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

fn text() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-zA-Z .]{0,20}",
        Just("tab\there\u{1f}\u{7f} ü 🎉 \u{2028}".to_owned()),
        Just(String::new()),
    ]
}

fn bot_comment() -> impl Strategy<Value = Value> {
    (
        text(),
        prop::option::of(prop_oneof![Just(json!("src/a.ts")), Just(json!(""))]),
        prop::option::of(prop_oneof![
            Just(json!(3)),
            Just(Value::Null),
            Just(json!(0))
        ]),
        prop::option::of(text()),
    )
        .prop_map(|(user, path, line, body)| {
            let mut c = json!({"user": user});
            if let Some(p) = path {
                c["path"] = p;
            }
            if let Some(l) = line {
                c["line"] = l;
            }
            if let Some(b) = body {
                c["body"] = json!(b);
            }
            c
        })
}

fn case() -> impl Strategy<Value = Value> {
    (
        prop::collection::vec(bot_comment(), 0..3),
        prop::collection::vec(text(), 0..2),
        prop::collection::vec(text(), 0..2),
        any::<bool>(),
    )
        .prop_map(|(bots, threads, reviews, errors)| {
            json!({
                "context": {"title": "t", "files": ["a.ts"], "diffPath": "/tmp/x.patch", "botComments": bots},
                "raw": {
                    "description": [],
                    "changesRequested": reviews.iter().enumerate().map(|(i, b)| json!({"index": i, "author": "a", "at": "", "body": b})).collect::<Vec<_>>(),
                    "humanReviews": [],
                    "threadComments": threads.iter().map(|b| json!({"user": "u", "at": "", "body": b})).collect::<Vec<_>>(),
                    "inlineComments": [],
                },
                "errors": if errors { json!([{"step": "thread", "detail": "x"}]) } else { json!([]) },
            })
        })
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("tribunal-relay-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn js_relay(cases: &[Value], root: &std::path::Path) -> Vec<Value> {
    let program = format!(
        r#"import {{ relayContext }} from {script};
import {{ readFileSync }} from "node:fs";
const cases = JSON.parse(readFileSync(0, "utf8"));
const out = cases.map((c, i) => relayContext(c, {root} + "/" + i));
process.stdout.write(JSON.stringify(out));"#,
        script = serde_json::to_string(&script().to_string_lossy()).unwrap(),
        root = serde_json::to_string(&root.to_string_lossy()).unwrap(),
    );
    let mut child = Command::new("node")
        .args(["--input-type=module", "-e", &program])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("node");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(cases).unwrap().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// The script's side of each case: its context, with `js_root` as `<ROOT>`,
/// and a SHA-256 of each file it wrote.
fn js_side(cases: &[Value], js_root: &std::path::Path) -> Value {
    let root = js_root.to_string_lossy().into_owned();
    let out = js_relay(cases, js_root);
    let sides: Vec<Value> = out
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let mut v = v.clone();
            for key in ["rawPath", "botCommentsPath"] {
                v[key] = json!(v[key].as_str().unwrap().replace(&root, "<ROOT>"));
            }
            let files: Vec<String> = FILES
                .iter()
                .map(|f| sha256_hex(&std::fs::read(js_root.join(i.to_string()).join(f)).unwrap()))
                .collect();
            json!({"context": v, "files": files})
        })
        .collect();
    json!({"platform": node_platform(), "cases": sides})
}

#[test]
fn relay_context_matches_the_script() {
    let cases = sample(case());
    let js_root = scratch("js");
    let rs_root = scratch("rs");
    let recorded = recorded_value(
        "relayContext",
        &[(SCRIPT, plugin_file_sha(SCRIPT))],
        &json!(cases),
        || js_side(&cases, &js_root),
    );
    let sides = recorded["cases"].as_array().unwrap();
    assert_eq!(sides.len(), cases.len());
    let mut js = Vec::new();
    let mut rust = Vec::new();
    for (i, (c, side)) in cases.iter().zip(sides).enumerate() {
        let mut j = side["context"].clone();
        if recorded["platform"] != node_platform() {
            j["platform"] = json!(node_platform());
        }
        js.push(j);
        let dir = rs_root.join(i.to_string());
        let mut v = relay_context(c, &dir.to_string_lossy()).unwrap();
        for key in ["rawPath", "botCommentsPath"] {
            let p = v[key]
                .as_str()
                .unwrap()
                .replace(&rs_root.to_string_lossy().into_owned(), "<ROOT>");
            v[key] = json!(p);
        }
        rust.push(v);
        for (f, sha) in FILES.iter().zip(side["files"].as_array().unwrap()) {
            let b = std::fs::read(dir.join(f)).unwrap();
            assert_eq!(&json!(sha256_hex(&b)), sha, "case {i}: {f} differs");
        }
    }
    assert_same("relayContext", &cases, &rust, &js);
    let _ = std::fs::remove_dir_all(js_root);
    let _ = std::fs::remove_dir_all(rs_root);
}
