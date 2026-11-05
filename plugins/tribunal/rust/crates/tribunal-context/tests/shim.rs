//! The recorded `gh`, `tribunal-test-gh`: it answers what was recorded and
//! refuses a call nobody recorded. The expected answers are the node shim's,
//! which recorded the goldens, taken byte for byte while it still existed.
//! Where node threw, the binary exits 1 as node did, and says why on stderr
//! instead of printing a stack.

mod common;

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use common::*;

#[tokio::test]
async fn the_recorded_answer_comes_back() {
    let dir = scratch("shim-hit");
    let rec = recordings(
        &dir,
        vec![call(
            &["api", "repos/o/r", "--jq", ".default_branch"],
            "main\n",
            0,
        )],
    );
    let out = tools(&rec)
        .gh(&["api", "repos/o/r", "--jq", ".default_branch"], None)
        .await
        .unwrap();
    assert_eq!((out.code, out.stdout.as_str()), (Some(0), "main\n"));
}

#[tokio::test]
async fn an_unrecorded_call_fails_loudly() {
    let dir = scratch("shim-miss");
    let rec = recordings(&dir, vec![]);
    let out = tools(&rec).gh(&["pr", "view", "1"], None).await.unwrap();
    assert_eq!(out.code, Some(99));
    assert!(out.stderr.contains("unrecorded call"));
}

/// What the shim did with a call: exit code, stdout, stderr, and what it
/// appended to the call log.
struct Ran {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: String,
    log: String,
}

fn run(dir: &Path, args: &[&OsStr], env: &[(&str, &OsStr)]) -> Ran {
    let log = dir.join("log");
    let _ = std::fs::remove_file(&log);
    let out = std::process::Command::new(test_gh())
        .args(args)
        .env_remove("GH_RECORDINGS")
        .env_remove("GH_RECORD")
        .env_remove("TRIBUNAL_TEST_GH")
        .env("GH_CALL_LOG", &log)
        .envs(env.iter().copied())
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    Ran {
        code: out.status.code(),
        stdout: out.stdout,
        stderr: String::from_utf8(out.stderr).unwrap(),
        log: std::fs::read_to_string(&log).unwrap_or_default(),
    }
}

/// What the node shim answered: exit code, stdout, and stderr, which is
/// compared whole, or by its start where node threw.
enum Stderr<'a> {
    Is(&'a str),
    StartsWith(&'a str),
}

/// Arguments, environment, and the exit code, stdout, stderr and call log
/// line expected.
type Case<'a> = (
    Vec<&'a OsStr>,
    Vec<(&'a str, OsString)>,
    i32,
    &'a [u8],
    Stderr<'a>,
    String,
);

#[test]
fn the_compiled_shim_answers_as_the_node_shim_did() {
    let dir = scratch("shim-answers");
    let big = "x".repeat(200_000);
    let odd = "tab\tnl\nquote\"bs\\ctl\u{1}\u{1f}del\u{7f} é 😀 \u{2028}";
    let odd_json =
        r#""tab\tnl\nquote\"bs\\ctl\u0001\u001fdel"#.to_owned() + "\u{7f} é 😀 \u{2028}\"";
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
        {"args": ["api", "str-code"], "stdout": "s", "code": "2"},
        {"args": ["api", "c300"], "stdout": "s", "code": 300},
        {"args": ["api", "cbig"], "stdout": "s", "code": 1_099_511_627_777_i64},
        {"args": ["api", "nul"], "stdout": "a\u{0}b"},
        {"args": ["api", "fffd\u{fffd}"], "stdout": "lossy hit"},
        {"args": ["api", "num-out"], "stdout": 7},
        {"args": ["api", "float-code"], "stdout": "", "code": 1.5},
        {"args": ["api", "bool-err"], "stdout": "o", "stderr": true},
    ]);
    let write = |name: &str, text: &[u8]| {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        p.into_os_string()
    };
    let rec = write(
        "rec.json",
        serde_json::json!({"calls": calls}).to_string().as_bytes(),
    );
    // Recordings serde alone does not read as `JSON.parse` does, or that
    // made node throw. `%u` is `\u`: lone surrogates, a pair, an escaped
    // backslash before `ud800`, and U+FFFD.
    let surrogate = write(
        "surrogate.json",
        r#"{"calls":[{"args":["api","s"],"stdout":"%ud800 %udc00%ud83d%ude00 \\ud800 %ud83d"},{"args":["api","%ud800"],"stdout":"never"},{"args":["api","%ufffd"],"stdout":"fffd"}]}"#
            .replace("%u", "\\u")
            .as_bytes(),
    );
    let not_array = write(
        "not-array.json",
        br#"{"calls":[{"args":"api"},{"args":{"length":3}},{"args":5},{"args":["api","a"],"stdout":"ok"}]}"#,
    );
    let string_args = write("string-args.json", br#"{"calls":[{"args":"ab"}]}"#);
    let object_args = write("object-args.json", br#"{"calls":[{"args":{"length":2}}]}"#);
    let no_args = write("no-args.json", br#"{"calls":[{"stdout":"x"}]}"#);
    let null_call = write("null-call.json", br#"{"calls":[null]}"#);
    let no_calls = write("no-calls.json", b"{}");
    let not_json = write("not-json.json", b"{nope");
    let bad_utf8 = write(
        "bad-utf8.json",
        b"{\"calls\":[{\"args\":[\"api\",\"u\"],\"stdout\":\"a\xff\xe2\x82b\"}]}",
    );
    let missing = dir.join("missing.json").into_os_string();
    let a_dir = dir.clone().into_os_string();
    let empty = OsString::new();

    let s = |x: &'static str| OsStr::new(x);
    let set = |v: &OsString| vec![("GH_RECORDINGS", v.clone())];
    let unrecorded = format!("gh shim: unrecorded call: [\"api\",\"unrecorded\",{odd_json}]\n");
    #[rustfmt::skip]
    let cases: Vec<Case> = vec![
        (vec![s("api"), s("a")], set(&rec), 0, b"main\n", Stderr::Is(""), r#"["api","a"]"#.into()),
        (vec![s("api"), s("warn")], set(&rec), 1, b"x", Stderr::Is("! careful\n"), r#"["api","warn"]"#.into()),
        (vec![s("api"), s("neg")], set(&rec), 255, b"", Stderr::Is(""), r#"["api","neg"]"#.into()),
        (vec![s("api"), s("null-code")], set(&rec), 0, b"y", Stderr::Is(""), r#"["api","null-code"]"#.into()),
        (vec![s("api"), s("no-code")], set(&rec), 0, b"z", Stderr::Is(""), r#"["api","no-code"]"#.into()),
        (vec![s("api"), s("null-out")], set(&rec), 0, b"", Stderr::Is(""), r#"["api","null-out"]"#.into()),
        (vec![s("api"), s("zero-out")], set(&rec), 0, b"", Stderr::Is(""), r#"["api","zero-out"]"#.into()),
        (vec![s("api"), OsStr::new(odd)], set(&rec), 0, odd.as_bytes(), Stderr::Is(""), format!("[\"api\",{odd_json}]")),
        (vec![s("api"), s("5")], set(&rec), 0, b"string five", Stderr::Is(""), r#"["api","5"]"#.into()),
        (vec![s("api"), s("dup")], set(&rec), 0, b"first", Stderr::Is(""), r#"["api","dup"]"#.into()),
        (vec![s("api"), s("str-code")], set(&rec), 2, b"s", Stderr::Is(""), r#"["api","str-code"]"#.into()),
        (vec![s("api"), s("c300")], set(&rec), 44, b"s", Stderr::Is(""), r#"["api","c300"]"#.into()),
        (vec![s("api"), s("cbig")], set(&rec), 1, b"s", Stderr::Is(""), r#"["api","cbig"]"#.into()),
        (vec![s("api"), s("nul")], set(&rec), 0, b"a\0b", Stderr::Is(""), r#"["api","nul"]"#.into()),
        // Node read an argument that is not UTF-8 with U+FFFD in its place.
        (vec![s("api"), OsStr::from_bytes(b"fffd\xff")], set(&rec), 0, b"lossy hit", Stderr::Is(""), "[\"api\",\"fffd\u{fffd}\"]".into()),
        (vec![s("api"), s("unrecorded"), OsStr::new(odd)], set(&rec), 99, b"", Stderr::Is(&unrecorded), format!("[\"api\",\"unrecorded\",{odd_json}]")),
        (vec![], set(&rec), 99, b"", Stderr::Is("gh shim: unrecorded call: []\n"), "[]".into()),
        (vec![s("api"), s("a")], vec![], 98, b"", Stderr::Is("gh shim: GH_RECORDINGS is not set\n"), r#"["api","a"]"#.into()),
        (vec![s("api"), s("a")], set(&empty), 98, b"", Stderr::Is("gh shim: GH_RECORDINGS is not set\n"), r#"["api","a"]"#.into()),
        (vec![s("api"), s("a")], set(&missing), 99, b"", Stderr::Is("gh shim: unrecorded call: [\"api\",\"a\"]\n"), r#"["api","a"]"#.into()),
        (vec![s("api"), s("s")], set(&surrogate), 0, "\u{fffd} \u{fffd}😀 \\ud800 \u{fffd}".as_bytes(), Stderr::Is(""), r#"["api","s"]"#.into()),
        (vec![s("api"), s("\u{fffd}")], set(&surrogate), 0, b"fffd", Stderr::Is(""), "[\"api\",\"\u{fffd}\"]".into()),
        (vec![s("api"), s("a")], set(&not_array), 0, b"ok", Stderr::Is(""), r#"["api","a"]"#.into()),
        (vec![s("api"), s("u")], set(&bad_utf8), 0, "a\u{fffd}\u{fffd}b".as_bytes(), Stderr::Is(""), r#"["api","u"]"#.into()),
        // Node threw on these.
        (vec![s("api"), s("num-out")], set(&rec), 1, b"", Stderr::StartsWith("gh shim: the recorded stdout is not a string"), r#"["api","num-out"]"#.into()),
        (vec![s("api"), s("float-code")], set(&rec), 1, b"", Stderr::StartsWith("gh shim: the recorded code is not a safe integer"), r#"["api","float-code"]"#.into()),
        (vec![s("api"), s("bool-err")], set(&rec), 1, b"o", Stderr::StartsWith("gh shim: the recorded stderr is not a string"), r#"["api","bool-err"]"#.into()),
        (vec![s("a"), s("b")], set(&string_args), 1, b"", Stderr::StartsWith("gh shim: recorded args \"ab\" are a string"), r#"["a","b"]"#.into()),
        (vec![s("a"), s("b")], set(&object_args), 1, b"", Stderr::StartsWith("gh shim: recorded args are an object"), r#"["a","b"]"#.into()),
        (vec![s("a")], set(&no_args), 1, b"", Stderr::StartsWith("gh shim: a recorded call has no args"), r#"["a"]"#.into()),
        (vec![s("a")], set(&null_call), 1, b"", Stderr::StartsWith("gh shim: recorded call 0 is not an object"), r#"["a"]"#.into()),
        (vec![s("api"), s("a")], set(&no_calls), 1, b"", Stderr::StartsWith("gh shim: "), r#"["api","a"]"#.into()),
        (vec![s("api"), s("a")], set(&not_json), 1, b"", Stderr::StartsWith("gh shim: "), r#"["api","a"]"#.into()),
        (vec![s("api"), s("a")], set(&a_dir), 1, b"", Stderr::StartsWith("gh shim: cannot read "), r#"["api","a"]"#.into()),
    ];
    let big_out = run(
        &dir,
        &[s("api"), s("big")],
        &[("GH_RECORDINGS", rec.as_os_str())],
    );
    assert_eq!(big_out.code, Some(0));
    assert!(
        big_out.stdout == big.as_bytes(),
        "the 200 KB answer was cut"
    );
    for (args, env, code, stdout, stderr, log) in &cases {
        let env: Vec<(&str, &OsStr)> = env.iter().map(|(k, v)| (*k, v.as_os_str())).collect();
        let got = run(&dir, args, &env);
        let what = format!("{args:?} with {env:?}");
        assert_eq!(got.code, Some(*code), "{what}: {}", got.stderr);
        assert!(
            got.stdout == *stdout,
            "{what}: stdout {:?}",
            String::from_utf8_lossy(&got.stdout)
        );
        match stderr {
            Stderr::Is(e) => assert_eq!(got.stderr, *e, "{what}"),
            Stderr::StartsWith(e) => assert!(got.stderr.starts_with(e), "{what}: {}", got.stderr),
        }
        assert_eq!(got.log, format!("{log}\n"), "{what}");
    }
}

/// `GH_RECORD=1` ran the real `gh` and recorded it; nothing is left to
/// record against, so it is refused rather than reaching GitHub.
#[test]
fn recording_mode_is_refused() {
    let dir = scratch("shim-record");
    let rec = recordings(&dir, vec![call(&["api", "a"], "main\n", 0)]);
    let got = run(
        &dir,
        &[OsStr::new("api"), OsStr::new("a")],
        &[
            ("GH_RECORDINGS", rec.as_os_str()),
            ("GH_RECORD", OsStr::new("1")),
        ],
    );
    assert_eq!(got.code, Some(97), "{}", got.stderr);
    assert!(got.stdout.is_empty());
    assert!(got.stderr.contains("GH_RECORD=1 is no longer supported"));
}
