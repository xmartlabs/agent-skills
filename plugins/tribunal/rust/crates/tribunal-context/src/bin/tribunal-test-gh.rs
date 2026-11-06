//! The stand-in `gh` of the tests, compiled, so no test starts node to fake
//! `gh`. `TRIBUNAL_TEST_GH` picks what it does:
//!
//! - unset: answers from recordings. `GH_RECORDINGS` names a JSON file,
//!   `{"calls": [{"args": [...], "stdout": "...", "stderr": "", "code": 0}]}`.
//!   An exact match on the argument list answers with the recorded stdout,
//!   stderr and exit code; a call nobody recorded exits 99 naming it, and an
//!   unset `GH_RECORDINGS` exits 98. `GH_CALL_LOG`, when set, gets each call's
//!   arguments as one `JSON.stringify` line. The node shim that recorded the
//!   context goldens answered the same way, byte for byte. Its `GH_RECORD=1`
//!   ran the real `gh` and recorded the answer; the scripts those recordings
//!   were compared against are gone, so `GH_RECORD=1` now exits 97.
//! - `bytes`: answers from `GH_BYTES`, `[{"args", "hex", "cut", "code"}]`,
//!   writing the hex-decoded bytes (invalid UTF-8 included), in two writes
//!   150 ms apart when `cut` splits them.
//! - `clone`: `gh repo clone <repo> <dir> -- <flags>` runs `git clone <flags>
//!   file://$GH_BARE <dir>`.
//! - `post-review`: logs each call to `FAKE_GH_LOG` as `{args, stdin, body,
//!   inTmp}` with the body file's path as `<BODY>`, keeps the last body in
//!   `FAKE_GH_STATE`, and answers from the rules in `FAKE_GH_SCRIPT`, where
//!   `{{BODY}}` is the last body, JSON-escaped.
//!
//! In every mode, `GH_TOKEN_LOG`, when set, gets each call as one line of
//! `{"token": <GH_TOKEN or null>, "args": [...]}`, so a test can see which
//! credential a call ran with.
//!
//! Each mode writes what the node stand-in it replaced wrote, which is what
//! the goldens hold. Where node threw (an answer that is not a string, an
//! exit code that is not an integer, unreadable JSON) this exits 1, as node's
//! uncaught exception did, with a `gh shim:` line instead of node's stack.

use std::io::{Read, Write};
use std::process::exit;

use serde_json::Value;
use tribunal_core::profile::js_ext::json_stringify;

fn main() {
    // Node decoded its argv as UTF-8, an invalid sequence as U+FFFD.
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if let Some(log) = env("GH_TOKEN_LOG") {
        let token = env("GH_TOKEN").map_or_else(|| "null".to_owned(), |t| stringify(&t));
        append(
            &log,
            &format!("{{\"token\":{token},\"args\":{}}}\n", stringify_args(&args)),
        );
    }
    match std::env::var("TRIBUNAL_TEST_GH")
        .unwrap_or_default()
        .as_str()
    {
        "" => recordings(&args),
        "bytes" => bytes(&args),
        "clone" => clone(&args),
        "post-review" => post_review(&args),
        other => fail(2, &format!("unknown TRIBUNAL_TEST_GH mode {other:?}")),
    }
}

fn fail(code: i32, message: &str) -> ! {
    eprintln!("gh shim: {message}");
    exit(code)
}

/// An environment variable as JavaScript tests one: unset and empty are both
/// false.
fn env(name: &str) -> Option<String> {
    std::env::var_os(name)
        .map(|v| v.to_string_lossy().into_owned())
        .filter(|v| !v.is_empty())
}

fn stringify(s: &str) -> String {
    json_stringify(Some(&Value::from(s)))
}

fn stringify_args(args: &[String]) -> String {
    json_stringify(Some(&Value::from(args.to_vec())))
}

fn append(path: &str, line: &str) {
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = written {
        fail(1, &format!("cannot append to {path}: {e}"));
    }
}

fn log_call(args: &[String]) {
    if let Some(log) = env("GH_CALL_LOG") {
        append(&log, &format!("{}\n", stringify_args(args)));
    }
}

/// `readFileSync(path, "utf8")`: invalid UTF-8 reads as U+FFFD.
fn read_text(path: &str) -> String {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => fail(1, &format!("cannot read {path}: {e}")),
    }
}

fn write_stdout(bytes: &[u8]) {
    let mut out = std::io::stdout().lock();
    if out.write_all(bytes).and_then(|()| out.flush()).is_err() {
        exit(1);
    }
}

fn write_stderr(bytes: &[u8]) {
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(bytes).and_then(|()| err.flush());
}

/// `x || ""` handed to `process.stdout.write`: a string, nothing for what
/// JavaScript reads as false, and an error where the write threw.
fn output<'a>(v: Option<&'a Value>, what: &str) -> &'a str {
    match v {
        None | Some(Value::Null | Value::Bool(false)) => "",
        Some(Value::String(s)) => s,
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => "",
        Some(other) => fail(1, &format!("the recorded {what} is not a string: {other}")),
    }
}

/// The status node exits with after `process.exitCode = x ?? 0`: an integer,
/// or a string that converts to one, in the safe range, modulo 256. Anything
/// else made the assignment throw.
fn exit_status(v: Option<&Value>) -> i32 {
    let n = match v {
        None | Some(Value::Null) => return 0,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) if !s.is_empty() => tribunal_core::js::string_to_number(s),
        Some(other) => fail(1, &format!("the recorded code is not a number: {other}")),
    };
    if !n.is_finite() || n.fract() != 0.0 || n.abs() > 9_007_199_254_740_991.0 {
        fail(1, &format!("the recorded code is not a safe integer: {n}"));
    }
    i32::try_from((n as i64) & 0xff).unwrap_or(1)
}

/// `text` with each `\uXXXX` escape of a lone surrogate replaced by `with`,
/// so serde reads it where `JSON.parse` did. A surrogate pair stays.
fn replace_lone_surrogates(text: &str, with: &str) -> String {
    fn unit(b: &[u8], at: usize) -> Option<u16> {
        if b.get(at..at + 2)? != b"\\u" {
            return None;
        }
        u16::from_str_radix(std::str::from_utf8(b.get(at + 2..at + 6)?).ok()?, 16).ok()
    }
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut in_string, mut i, mut copied) = (false, 0, 0);
    while i < b.len() {
        match b[i] {
            b'"' => {
                in_string = !in_string;
                i += 1;
            }
            b'\\' if in_string => match unit(b, i) {
                Some(0xD800..=0xDBFF)
                    if unit(b, i + 6).is_some_and(|lo| (0xDC00..=0xDFFF).contains(&lo)) =>
                {
                    i += 12;
                }
                Some(0xD800..=0xDFFF) => {
                    out.push_str(&text[copied..i]);
                    out.push_str(with);
                    i += 6;
                    copied = i;
                }
                Some(_) => i += 6,
                None => i += 2,
            },
            _ => i += 1,
        }
    }
    out.push_str(&text[copied.min(text.len())..]);
    out
}

/// The node shim's `same(c.args, args)`, `a.length === b.length && a.every((x,
/// i) => x === b[i])`, exiting where it threw.
fn same(recorded: Option<&Value>, args: &[String]) -> bool {
    match recorded {
        None | Some(Value::Null) => fail(1, "a recorded call has no args"),
        Some(Value::Array(a)) => a.len() == args.len() && a.iter().zip(args).all(|(x, y)| x == y),
        // A string, or an object whose length matches, reaches `.every`,
        // which neither has.
        Some(Value::String(s)) if s.encode_utf16().count() == args.len() => {
            fail(1, &format!("recorded args {s:?} are a string"))
        }
        Some(Value::Object(o))
            if o.get("length").and_then(Value::as_f64) == Some(args.len() as f64) =>
        {
            fail(1, "recorded args are an object")
        }
        Some(_) => false,
    }
}

fn recordings(args: &[String]) {
    // A `repo clone` is not recorded: its destination is a temp directory the
    // recording cannot name. With `GH_BARE` set it is answered by cloning from
    // that repo, as in `clone` mode, and everything else is still recorded.
    if env("GH_BARE").is_some() && args.len() >= 2 && args[0] == "repo" && args[1] == "clone" {
        return clone(args);
    }
    log_call(args);
    let Some(file) = env("GH_RECORDINGS") else {
        fail(98, "GH_RECORDINGS is not set")
    };
    if std::env::var_os("GH_RECORD").is_some_and(|v| v == "1") {
        fail(
            97,
            "GH_RECORD=1 is no longer supported: the scripts the recordings were \
             compared against were removed, so the recordings are final",
        );
    }
    let mut hit = None;
    if std::path::Path::new(&file).exists() {
        let text = read_text(&file);
        // Matching reads a lone surrogate as NUL, which no argument can hold,
        // so it never matches, as in node; the output writes it as U+FFFD.
        let calls = |with: &str| -> Vec<Value> {
            let v: Value = serde_json::from_str(&replace_lone_surrogates(&text, with))
                .unwrap_or_else(|e| fail(1, &format!("{file} is not JSON: {e}")));
            match v.get("calls") {
                Some(Value::Array(calls)) => calls.clone(),
                _ => fail(1, &format!("{file} has no calls array")),
            }
        };
        let (for_args, for_output) = (calls("\\u0000"), calls("\\ufffd"));
        for (i, call) in for_args.iter().enumerate() {
            if !call.is_object() {
                fail(1, &format!("recorded call {i} is not an object: {call}"));
            }
            if same(call.get("args"), args) {
                hit = Some(for_output[i].clone());
                break;
            }
        }
    }
    let Some(hit) = hit else {
        fail(99, &format!("unrecorded call: {}", stringify_args(args)))
    };
    write_stdout(output(hit.get("stdout"), "stdout").as_bytes());
    write_stderr(output(hit.get("stderr"), "stderr").as_bytes());
    exit(exit_status(hit.get("code")))
}

/// `Buffer.from(hex, "hex")`, which stops at the first pair that is not hex.
fn unhex(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map_while(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok())
        .collect()
}

fn bytes(args: &[String]) {
    log_call(args);
    let file = env("GH_BYTES").unwrap_or_else(|| fail(1, "GH_BYTES is not set"));
    let entries: Value = serde_json::from_str(&read_text(&file))
        .unwrap_or_else(|e| fail(1, &format!("{file} is not JSON: {e}")));
    let want = Value::from(args.to_vec());
    let Some(hit) = entries
        .as_array()
        .and_then(|a| a.iter().find(|c| c.get("args") == Some(&want)))
    else {
        exit(99)
    };
    let out = unhex(hit.get("hex").and_then(Value::as_str).unwrap_or(""));
    let cut = hit
        .get("cut")
        .and_then(Value::as_u64)
        .map_or(out.len(), |c| {
            usize::try_from(c).unwrap_or(usize::MAX).min(out.len())
        });
    write_stdout(&out[..cut]);
    std::thread::sleep(std::time::Duration::from_millis(150));
    write_stdout(&out[cut..]);
    exit(exit_status(hit.get("code")))
}

fn clone(args: &[String]) {
    log_call(args);
    if args.len() < 5 || args[0] != "repo" || args[1] != "clone" || args[4] != "--" {
        exit(99);
    }
    let bare = std::env::var("GH_BARE").unwrap_or_else(|_| "undefined".into());
    let status = std::process::Command::new("git")
        .arg("clone")
        .args(&args[5..])
        .arg(format!("file://{bare}"))
        .arg(&args[3])
        .status();
    exit(status.ok().and_then(|s| s.code()).unwrap_or(1))
}

/// `path.posix.dirname`.
fn dirname(path: &str) -> &str {
    let b = path.as_bytes();
    if b.is_empty() {
        return ".";
    }
    let has_root = b[0] == b'/';
    let mut end = None;
    let mut matched_slash = true;
    for i in (1..b.len()).rev() {
        if b[i] == b'/' {
            if !matched_slash {
                end = Some(i);
                break;
            }
        } else {
            matched_slash = false;
        }
    }
    match end {
        None if has_root => "/",
        None => ".",
        Some(1) if has_root => "//",
        Some(e) => &path[..e],
    }
}

/// `path.posix.basename`, with no extension to strip.
fn basename(path: &str) -> &str {
    let b = path.as_bytes();
    let mut start = 0;
    let mut end = None;
    for i in (0..b.len()).rev() {
        if b[i] == b'/' {
            if end.is_some() {
                start = i + 1;
                break;
            }
        } else if end.is_none() {
            end = Some(i + 1);
        }
    }
    end.map_or("", |e| &path[start..e])
}

/// `/^post-review-body-\d+\.md$/`.
fn is_body_name(name: &str) -> bool {
    name.strip_prefix("post-review-body-")
        .and_then(|r| r.strip_suffix(".md"))
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit()))
}

fn post_review(args: &[String]) {
    let mut stdin = Vec::new();
    if let Err(e) = std::io::stdin().read_to_end(&mut stdin) {
        fail(1, &format!("cannot read stdin: {e}"));
    }
    let stdin = String::from_utf8_lossy(&stdin).into_owned();
    let mut body_path: Option<&str> = None;
    let norm: Vec<String> = args
        .iter()
        .enumerate()
        .map(|(i, a)| {
            if i > 0 && args[i - 1] == "--body-file" {
                body_path = Some(a);
                "<BODY>".to_owned()
            } else if let Some(p) = a.strip_prefix("body=@") {
                body_path = Some(p);
                "body=@<BODY>".to_owned()
            } else {
                a.clone()
            }
        })
        .collect();
    let body = body_path.map(read_text);
    let in_tmp = body_path.map(|p| {
        std::env::var("FAKE_GH_TMP").is_ok_and(|t| dirname(p) == t) && is_body_name(basename(p))
    });
    let log = env("FAKE_GH_LOG").unwrap_or_else(|| fail(1, "FAKE_GH_LOG is not set"));
    append(
        &log,
        &format!(
            "{{\"args\":{},\"stdin\":{},\"body\":{},\"inTmp\":{}}}\n",
            stringify_args(&norm),
            stringify(&stdin),
            body.as_deref().map_or("null".to_owned(), stringify),
            in_tmp.map_or("null", |t| if t { "true" } else { "false" }),
        ),
    );
    let state = env("FAKE_GH_STATE").unwrap_or_else(|| fail(1, "FAKE_GH_STATE is not set"));
    if let Some(b) = &body {
        if let Err(e) = std::fs::write(&state, b) {
            fail(1, &format!("cannot write {state}: {e}"));
        }
    }
    let script = env("FAKE_GH_SCRIPT").unwrap_or_else(|| fail(1, "FAKE_GH_SCRIPT is not set"));
    let rules: Value = serde_json::from_str(&read_text(&script))
        .unwrap_or_else(|e| fail(1, &format!("{script} is not JSON: {e}")));
    let want = Value::from(norm.clone());
    let Some(rule) = rules
        .as_array()
        .and_then(|a| a.iter().find(|r| r.get("args") == Some(&want)))
    else {
        write_stderr(format!("fake gh: unscripted call {}\n", stringify_args(&norm)).as_bytes());
        exit(99)
    };
    let Some(Value::String(stdout)) = rule.get("stdout") else {
        fail(1, &format!("the scripted stdout is not a string: {rule}"))
    };
    let stdout = if stdout.contains("{{BODY}}") {
        let posted = std::fs::read(&state)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let quoted = stringify(&posted);
        stdout.replace("{{BODY}}", &quoted[1..quoted.len() - 1])
    } else {
        stdout.clone()
    };
    write_stdout(stdout.as_bytes());
    write_stderr(output(rule.get("stderr"), "stderr").as_bytes());
    exit(exit_status(rule.get("code")))
}
