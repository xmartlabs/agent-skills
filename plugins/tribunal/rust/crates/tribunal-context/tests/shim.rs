//! The recorded `gh` answers a script and the Rust side identically, and
//! refuses a call nobody recorded.

mod common;

use common::*;

const PROBE: &str = r#"
const { execFileSync } = require("node:child_process");
try {
  process.stdout.write(execFileSync("gh", ["api", "repos/o/r", "--jq", ".default_branch"], { encoding: "utf8" }));
} catch (e) {
  process.stdout.write("FAILED " + e.status);
}
"#;

#[tokio::test]
async fn both_sides_get_the_recorded_answer() {
    let dir = scratch("shim-hit");
    std::fs::write(dir.join("probe.cjs"), PROBE).unwrap();
    let rec = recordings(
        &dir,
        vec![call(
            &["api", "repos/o/r", "--jq", ".default_branch"],
            "main\n",
            0,
        )],
    );

    let rust = tools(&rec)
        .gh(&["api", "repos/o/r", "--jq", ".default_branch"], None)
        .await
        .unwrap();
    assert_eq!((rust.code, rust.stdout.as_str()), (Some(0), "main\n"));

    let script = run_script(
        dir.join("probe.cjs").to_str().unwrap(),
        &[],
        &dir,
        &rec,
        None,
    );
    assert_eq!(script.stdout, "main\n", "{}", script.stderr);
}

#[tokio::test]
async fn an_unrecorded_call_fails_loudly() {
    let dir = scratch("shim-miss");
    let rec = recordings(&dir, vec![]);
    let out = tools(&rec).gh(&["pr", "view", "1"], None).await.unwrap();
    assert_eq!(out.code, Some(99));
    assert!(out.stderr.contains("unrecorded call"));
}

/// What one shim did with a call: exit code, stdout, stderr, and the line it
/// appended to the call log.
fn run_shim(
    program: &std::path::Path,
    node_first: bool,
    dir: &std::path::Path,
    args: &[&str],
    recordings: Option<&std::path::Path>,
) -> (Option<i32>, Vec<u8>, Vec<u8>, Vec<u8>) {
    let log = dir.join(format!("log-{}", u8::from(node_first)));
    let _ = std::fs::remove_file(&log);
    let mut cmd = if node_first {
        let mut c = std::process::Command::new("node");
        c.arg(program);
        c
    } else {
        std::process::Command::new(program)
    };
    cmd.args(args)
        .env_remove("GH_RECORDINGS")
        .env_remove("GH_RECORD")
        .env("GH_CALL_LOG", &log)
        .stdin(std::process::Stdio::null());
    if let Some(r) = recordings {
        cmd.env("GH_RECORDINGS", r);
    }
    let out = cmd.output().unwrap();
    (
        out.status.code(),
        out.stdout,
        out.stderr,
        std::fs::read(&log).unwrap_or_default(),
    )
}

/// The compiled shim the Rust side runs answers every call as the node shim
/// does, byte for byte, including the calls it hands to node.
#[test]
fn the_compiled_shim_answers_as_the_node_shim() {
    let dir = scratch("shim-parity");
    let big = "x".repeat(200_000);
    let odd = "tab\tnl\nquote\"bs\\ctl\u{1}\u{1f}del\u{7f} é 😀 \u{2028}";
    let calls = serde_json::json!([
        {"args": ["api", "a"], "stdout": "main\n", "stderr": "", "code": 0},
        {"args": ["api", "warn"], "stdout": "x", "stderr": "! careful\n", "code": 1},
        {"args": ["api", "neg"], "stdout": "", "code": -1},
        {"args": ["api", "null-code"], "stdout": "y", "code": null},
        {"args": ["api", "no-code"], "stdout": "z"},
        {"args": ["api", "null-out"], "stdout": null, "stderr": false},
        {"args": ["api", "zero-out"], "stdout": 0},
        {"args": ["api", "big"], "stdout": big},
        {"args": ["api", odd], "stdout": odd},
        {"args": ["api", 5], "stdout": "never: 5 is not the string \"5\""},
        {"args": ["api", "5"], "stdout": "string five"},
        {"args": ["api", "dup"], "stdout": "first"},
        {"args": ["api", "dup"], "stdout": "second"},
        {"args": ["api", "num-out"], "stdout": 7},
        {"args": ["api", "float-code"], "stdout": "", "code": 1.5},
    ]);
    let rec = dir.join("rec.json");
    std::fs::write(&rec, serde_json::json!({"calls": calls}).to_string()).unwrap();
    // Recordings only node reads the same way.
    let surrogate = dir.join("surrogate.json");
    std::fs::write(
        &surrogate,
        r#"{"calls":[{"args":["api","s"],"stdout":"\ud800"}]}"#,
    )
    .unwrap();
    let not_array = dir.join("not-array.json");
    std::fs::write(
        &not_array,
        r#"{"calls":[{"args":"api"},{"args":["api","a"],"stdout":"ok"}]}"#,
    )
    .unwrap();
    let no_calls = dir.join("no-calls.json");
    std::fs::write(&no_calls, "{}").unwrap();
    let missing = dir.join("missing.json");

    let node_shim = shim_dir().join("gh");
    let compiled = std::path::PathBuf::from(env!("CARGO_BIN_EXE_tribunal-test-gh"));
    let cases: Vec<(Vec<&str>, Option<&std::path::Path>)> = vec![
        (vec!["api", "a"], Some(&rec)),
        (vec!["api", "warn"], Some(&rec)),
        (vec!["api", "neg"], Some(&rec)),
        (vec!["api", "null-code"], Some(&rec)),
        (vec!["api", "no-code"], Some(&rec)),
        (vec!["api", "null-out"], Some(&rec)),
        (vec!["api", "zero-out"], Some(&rec)),
        (vec!["api", "big"], Some(&rec)),
        (vec!["api", odd], Some(&rec)),
        (vec!["api", "5"], Some(&rec)),
        (vec!["api", "dup"], Some(&rec)),
        (vec!["api", "num-out"], Some(&rec)),
        (vec!["api", "float-code"], Some(&rec)),
        (vec!["api", "unrecorded", odd], Some(&rec)),
        (vec![], Some(&rec)),
        (vec!["api", "a"], None),
        (vec!["api", "a"], Some(&missing)),
        (vec!["api", "s"], Some(&surrogate)),
        (vec!["api", "a"], Some(&not_array)),
        (vec!["api", "a"], Some(&no_calls)),
    ];
    for (args, recordings) in cases {
        let want = run_shim(&node_shim, true, &dir, &args, recordings);
        let got = run_shim(&compiled, false, &dir, &args, recordings);
        assert!(
            want == got,
            "{args:?} with {recordings:?}\n node: {:?} {:?} {:?} {:?}\n rust: {:?} {:?} {:?} {:?}",
            want.0,
            String::from_utf8_lossy(&want.1)
                .chars()
                .take(200)
                .collect::<String>(),
            String::from_utf8_lossy(&want.2)
                .chars()
                .take(400)
                .collect::<String>(),
            String::from_utf8_lossy(&want.3),
            got.0,
            String::from_utf8_lossy(&got.1)
                .chars()
                .take(200)
                .collect::<String>(),
            String::from_utf8_lossy(&got.2)
                .chars()
                .take(400)
                .collect::<String>(),
            String::from_utf8_lossy(&got.3),
        );
    }
}
