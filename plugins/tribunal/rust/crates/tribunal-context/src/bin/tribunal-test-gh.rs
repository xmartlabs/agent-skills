//! `tests/shim/gh` for the Rust side of the differential tests, without
//! starting node for every call. It answers the common case itself: an exact
//! match on the argument list, the recorded stdout, stderr and exit code, and
//! the call appended to `GH_CALL_LOG` as `JSON.stringify` writes it. Anything
//! it cannot reproduce byte for byte (recording mode, recordings serde cannot
//! read, an answer that is not a string, an argument that is not UTF-8) is
//! handed to the node shim, so the two never answer differently.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

use serde_json::Value;
use tribunal_core::profile::js_ext::json_stringify;

fn node_shim() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/shim/gh")
}

/// Runs the node shim in this process's place.
fn delegate(args: &[OsString]) -> ! {
    let mut cmd = std::process::Command::new("node");
    cmd.arg(node_shim()).args(args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let e = cmd.exec();
        eprintln!("tribunal-test-gh: cannot run the node shim: {e}");
        std::process::exit(97);
    }
    #[cfg(not(unix))]
    {
        let status = cmd.status().unwrap_or_else(|e| {
            eprintln!("tribunal-test-gh: cannot run the node shim: {e}");
            std::process::exit(97);
        });
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// `hit.stdout || ""`: a string, or nothing for what JavaScript reads as false.
/// `None` for anything `process.stdout.write` would throw on.
fn output(v: Option<&Value>) -> Option<&str> {
    match v {
        None | Some(Value::Null | Value::Bool(false)) => Some(""),
        Some(Value::String(s)) => Some(s),
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => Some(""),
        _ => None,
    }
}

/// What the node shim would do, decided before anything is written, so a
/// call handed to it is not logged twice.
enum Answer<'a> {
    NoRecordings,
    Unrecorded,
    Hit {
        stdout: &'a str,
        stderr: &'a str,
        code: i32,
    },
}

/// The node shim's answer for `args`, or `None` where only node can say.
fn answer<'a>(recordings: Option<&'a Value>, set: bool, args: &[String]) -> Option<Answer<'a>> {
    if !set {
        return Some(Answer::NoRecordings);
    }
    let Some(recordings) = recordings else {
        return Some(Answer::Unrecorded);
    };
    for call in recordings.get("calls")?.as_array()? {
        // Node reads `.length` and indexes whatever `args` is, so anything
        // but an array is node's to answer.
        let recorded = call.get("args")?.as_array()?;
        if recorded.len() != args.len() || !recorded.iter().zip(args).all(|(x, y)| x == y) {
            continue;
        }
        let code = match call.get("code") {
            None | Some(Value::Null) => 0,
            Some(v) => i32::try_from(v.as_i64()?).ok()?,
        };
        return Some(Answer::Hit {
            stdout: output(call.get("stdout"))?,
            stderr: output(call.get("stderr"))?,
            code,
        });
    }
    Some(Answer::Unrecorded)
}

fn main() {
    let raw: Vec<OsString> = std::env::args_os().skip(1).collect();
    let Some(args) = raw
        .iter()
        .map(|a| a.to_str().map(str::to_owned))
        .collect::<Option<Vec<String>>>()
    else {
        delegate(&raw)
    };
    if std::env::var_os("GH_RECORD").is_some_and(|v| v == "1") {
        delegate(&raw);
    }
    let file = std::env::var_os("GH_RECORDINGS");
    let recordings: Option<Value> = match &file {
        Some(f) if std::path::Path::new(f).exists() => {
            let parsed = std::fs::read_to_string(f)
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok());
            let Some(v) = parsed else { delegate(&raw) };
            Some(v)
        }
        _ => None,
    };
    let Some(answer) = answer(recordings.as_ref(), file.is_some(), &args) else {
        delegate(&raw)
    };
    let args_json = json_stringify(Some(&Value::from(args)));
    if let Some(log) = std::env::var_os("GH_CALL_LOG") {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .expect("GH_CALL_LOG");
        f.write_all(format!("{args_json}\n").as_bytes()).unwrap();
    }
    match answer {
        Answer::NoRecordings => {
            eprintln!("gh shim: GH_RECORDINGS is not set");
            std::process::exit(98);
        }
        Answer::Unrecorded => {
            eprintln!("gh shim: unrecorded call: {args_json}");
            std::process::exit(99);
        }
        Answer::Hit {
            stdout,
            stderr,
            code,
        } => {
            std::io::stdout().write_all(stdout.as_bytes()).unwrap();
            std::io::stderr().write_all(stderr.as_bytes()).unwrap();
            std::process::exit(code);
        }
    }
}
