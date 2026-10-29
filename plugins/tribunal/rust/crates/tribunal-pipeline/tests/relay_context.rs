//! `context::relay_context` against fetch-pr-context.mjs's own exported
//! `relayContext`: the same context back, and the same bytes on disk.

mod common;

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use common::{assert_same, sample};
use proptest::prelude::*;
use serde_json::{json, Value};
use tribunal_pipeline::context::relay_context;

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../skills/review-pr/scripts/fetch-pr-context.mjs")
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

#[test]
fn relay_context_matches_the_script() {
    let cases = sample(case());
    let js_root = scratch("js");
    let rs_root = scratch("rs");
    let js = js_relay(&cases, &js_root);
    let mut rust = Vec::new();
    for (i, c) in cases.iter().enumerate() {
        let dir = rs_root.join(i.to_string());
        let mut v = relay_context(c, &dir.to_string_lossy()).unwrap();
        for key in ["rawPath", "botCommentsPath"] {
            let p = v[key].as_str().unwrap().replace(
                &rs_root.to_string_lossy().into_owned(),
                &js_root.to_string_lossy(),
            );
            v[key] = json!(p);
        }
        rust.push(v);
        for f in ["raw.json", "bot-comments.json"] {
            let a = std::fs::read(js_root.join(i.to_string()).join(f)).unwrap();
            let b = std::fs::read(dir.join(f)).unwrap();
            assert_eq!(a, b, "case {i}: {f} differs");
        }
    }
    assert_same("relayContext", &cases, &rust, &js);
    let _ = std::fs::remove_dir_all(js_root);
    let _ = std::fs::remove_dir_all(rs_root);
}
