mod common;
use common::*;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use tribunal_context::incremental_scope::{
    locale_compare, scope_files, scope_incremental, split_diff_by_file,
};
use tribunal_context::resolve_baseline::{
    is_bot_author, metrics_path_from, parse_json, read_metrics_rows, resolve_baseline, run, Run,
};
use tribunal_core::evidence::js_ext::property_order_deep;
use tribunal_core::profile::js_ext::json_stringify;

const RESOLVE: &str = "skills/review-pr/scripts/resolve-baseline.mjs";
const SCOPE: &str = "skills/review-pr/scripts/incremental-scope.mjs";

// ---------------------------------------------------------------------------
// Harness

/// `run_script` with extra environment: resolve-baseline finds its ledger
/// through `TRIBUNAL_DATA_DIR` (or `HOME`), and `localeCompare` follows the
/// process locale, pinned here to the one Node uses by default. The recorded
/// `gh` is first on PATH with no recordings and a call log, so a script that
/// reached for `gh` would fail and leave the log behind.
fn node(
    script: &Path,
    args: &[&str],
    cwd: &Path,
    stdin: &str,
    env: &[(&str, &OsStr)],
    remove: &[&str],
) -> ScriptRun {
    use std::io::Write;
    let rec = recordings(cwd, vec![]);
    let log = cwd.join("gh-calls.jsonl");
    let path = std::env::join_paths(std::iter::once(shim_dir()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let mut cmd = Command::new("node");
    cmd.arg(plugin_root().join(script))
        .args(args)
        .current_dir(cwd)
        .env("PATH", path)
        .env("GH_RECORDINGS", &rec)
        .env("GH_CALL_LOG", &log)
        .env("LC_ALL", "en_US.UTF-8")
        .env_remove("CLAUDE_PLUGIN_ROOT")
        .env_remove("TRIBUNAL_DATA_DIR");
    for k in remove {
        cmd.env_remove(k);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("node must be on PATH");
    let mut input = child.stdin.take().unwrap();
    input.write_all(stdin.as_bytes()).unwrap();
    drop(input);
    let out = child.wait_with_output().unwrap();
    assert!(
        !log.exists(),
        "the script called gh: {:?}",
        std::fs::read_to_string(&log)
    );
    ScriptRun {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// A small deterministic generator, so a failing case can be replayed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, pct: usize) -> bool {
        self.below(100) < pct
    }
    fn pick<T: Clone>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len())].clone()
    }
}

/// `None` stands for a missing key.
fn obj(entries: Vec<(&str, Option<Value>)>) -> Value {
    let mut m = serde_json::Map::new();
    for (k, v) in entries {
        if let Some(v) = v {
            m.insert(k.to_owned(), v);
        }
    }
    Value::Object(m)
}

/// Parses JSON text the way the port's callers do.
fn parse(text: &str) -> Value {
    property_order_deep(serde_json::from_str(text).unwrap())
}

fn module_url(script: &str) -> String {
    format!(
        "file://{}",
        plugin_root().join(script).canonicalize().unwrap().display()
    )
}

/// Runs `body` (a module that may use `cases`, `out` and the named imports
/// from `script`) over a JSON array of cases in one Node process.
fn driver(name: &str, script: &str, imports: &str, body: &str, cases: &str) -> Vec<Value> {
    let dir = scratch(name);
    let src = format!(
        "import {{ {imports} }} from {url};\n\
         import {{ readFileSync }} from \"node:fs\";\n\
         const cases = JSON.parse(readFileSync(0, \"utf8\"));\n\
         const out = [];\n\
         const attempt = (f) => {{ try {{ return f(); }} catch (e) {{ return {{ error: e.constructor.name + \": \" + e.message }}; }} }};\n\
         {body}\n\
         process.stdout.write(JSON.stringify(out) + \"\\n\");\n",
        url = serde_json::to_string(&module_url(script)).unwrap(),
    );
    let file = dir.join("driver.mjs");
    std::fs::write(&file, src).unwrap();
    let out = node(&file, &[], &dir, cases, &[], &[]);
    assert_eq!(out.code, Some(0), "driver failed: {}", out.stderr);
    match last_json(&out.stdout) {
        Value::Array(a) => a,
        other => panic!("driver returned {other}"),
    }
}

// ---------------------------------------------------------------------------
// resolve-baseline: generated inputs

fn sha_value(r: &mut Rng) -> Option<Value> {
    r.pick(&[
        Some(json!("aaaa1111cafe")),
        Some(json!("aaaa2222beef")),
        Some(json!("bbbb3333f00d")),
        Some(json!("head0000head")),
        Some(json!("")),
        Some(Value::Null),
        Some(json!(5)),
        Some(json!(true)),
        Some(json!(["aaaa"])),
        Some(json!({"a": 1})),
        Some(json!("é😀sha-with-astral-chars")),
        None,
    ])
}

fn weird(r: &mut Rng) -> Option<Value> {
    r.pick(&[
        Some(json!("src/a.ts")),
        Some(json!("")),
        Some(Value::Null),
        Some(json!(0)),
        Some(json!(1)),
        Some(json!(true)),
        Some(json!(false)),
        Some(json!(1.5)),
        Some(json!([])),
        Some(json!(["x", null, 2])),
        Some(json!({})),
        Some(json!({"10": 1, "2": 2, "b": {"1": "z", "0": "y"}, "a": 3})),
        Some(serde_json::from_str("12345678901234567890").unwrap()),
        Some(serde_json::from_str("1e21").unwrap()),
        Some(serde_json::from_str("-0.0").unwrap()),
        Some(serde_json::from_str("2.0").unwrap()),
        None,
    ])
}

fn finding(r: &mut Rng) -> Value {
    match r.below(60) {
        0 => json!(5),
        1 => json!("finding as a string"),
        2 => json!([1]),
        3 => Value::Null,
        _ => obj(vec![
            ("file", weird(r)),
            (
                "title",
                r.pick(&[
                    Some(json!("Off by one")),
                    Some(json!("")),
                    None,
                    Some(json!(7)),
                ]),
            ),
            ("severity", weird(r)),
            ("blocking", weird(r)),
            ("survived", weird(r)),
            ("posted", weird(r)),
            ("operatorOverride", weird(r)),
            ("extra", r.pick(&[None, Some(json!("dropped"))])),
        ]),
    }
}

fn findings(r: &mut Rng) -> Option<Value> {
    match r.below(12) {
        0 => Some(json!([])),
        1 => Some(json!("abc")),
        2 => Some(json!({})),
        3 => Some(json!({"length": 1})),
        4 => Some(Value::Null),
        5 => None,
        6 => Some(json!(5)),
        _ => {
            let n = 1 + r.below(3);
            Some(Value::Array((0..n).map(|_| finding(r)).collect()))
        }
    }
}

fn metrics_row(r: &mut Rng) -> Value {
    obj(vec![
        ("pr", Some(json!(42))),
        (
            "schemaVersion",
            r.pick(&[
                Some(json!(3)),
                Some(json!(4)),
                Some(json!(3)),
                Some(json!(2)),
                Some(json!(0)),
                Some(Value::Null),
                Some(json!("3")),
                Some(json!("2")),
                Some(json!("abc")),
                Some(json!(true)),
                Some(json!([5])),
                Some(json!([])),
                Some(json!({})),
                Some(json!({"toString": 1})),
                Some(json!(3.5)),
                None,
            ]),
        ),
        ("headSha", sha_value(r)),
        (
            "posted",
            r.pick(&[
                Some(json!(true)),
                Some(json!(true)),
                Some(json!(false)),
                Some(json!(1)),
                Some(json!(0)),
                Some(json!("yes")),
                Some(json!("")),
                None,
            ]),
        ),
        ("findings", findings(r)),
        ("ts", weird(r)),
    ])
}

fn review(r: &mut Rng) -> Value {
    match r.below(40) {
        0 => return Value::Null,
        1 => return json!(5),
        2 => return json!("APPROVED"),
        3 => return json!([]),
        _ => {}
    }
    obj(vec![
        (
            "state",
            r.pick(&[
                Some(json!("APPROVED")),
                Some(json!("CHANGES_REQUESTED")),
                Some(json!("APPROVED")),
                Some(json!("COMMENTED")),
                Some(json!("approved")),
                Some(Value::Null),
                None,
            ]),
        ),
        (
            "author",
            if r.chance(80) {
                r.pick(&[
                    Some(json!("alice")),
                    Some(json!("bob")),
                    Some(json!("dependabot[bot]")),
                    Some(json!("coderabbitai")),
                ])
            } else {
                r.pick(&[
                    Some(json!("alice")),
                    Some(json!("bob")),
                    Some(json!("dependabot[bot]")),
                    Some(json!("renovate[BOT]")),
                    Some(json!("coderabbitai")),
                    Some(json!("CodeRabbitAI")),
                    Some(json!("[bot]")),
                    Some(json!("bot]")),
                    Some(json!("x[bot] ")),
                    Some(json!("")),
                    Some(Value::Null),
                    Some(json!(5)),
                    Some(json!(["a", "b"])),
                    Some(json!({"a": 1})),
                    Some(json!({"toString": "x"})),
                    Some(json!([{"toString": 1}])),
                    None,
                ])
            },
        ),
        ("commit", sha_value(r)),
        (
            "at",
            r.pick(&[
                Some(json!("2026-01-01T00:00:00Z")),
                Some(json!("2026-02-01T00:00:00Z")),
                Some(json!("2026-01-15T10:00:00Z")),
                Some(json!("2026-02-01T00:00:00Z")),
                Some(json!("")),
                Some(Value::Null),
                Some(json!(5)),
                Some(json!("10")),
                Some(json!("9")),
                Some(json!(["2026"])),
                Some(json!({})),
                Some(json!(true)),
                None,
            ]),
        ),
    ])
}

fn head_or_since(r: &mut Rng, since: bool) -> Option<Value> {
    if since {
        r.pick(&[
            Some(json!("")),
            Some(json!("")),
            Some(json!("")),
            Some(json!("aaaa")),
            Some(json!("bbbb3333")),
            Some(json!("zz")),
            Some(json!(["aaaa"])),
            Some(json!(0)),
            Some(json!(5)),
            Some(Value::Null),
            None,
        ])
    } else {
        r.pick(&[
            Some(json!("aaaa1111cafe")),
            Some(json!("head0000head")),
            Some(json!("")),
            Some(json!(5)),
            None,
        ])
    }
}

fn baseline_case(r: &mut Rng) -> Value {
    let rows = (0..r.below(5)).map(|_| metrics_row(r)).collect::<Vec<_>>();
    let reviews = (0..r.below(5)).map(|_| review(r)).collect::<Vec<_>>();
    obj(vec![
        ("rows", Some(Value::Array(rows))),
        (
            "reviews",
            Some(if r.chance(5) {
                json!({"not": "an array"})
            } else {
                Value::Array(reviews)
            }),
        ),
        ("headSha", head_or_since(r, false)),
        ("since", head_or_since(r, true)),
    ])
}

fn empty() -> Value {
    json!("")
}

fn or_empty(v: Option<&Value>) -> Value {
    match v {
        Some(v) if tribunal_core::js::truthy(Some(v)) => v.clone(),
        _ => empty(),
    }
}

/// The port's answer in the driver's shape: the decision's JSON, or the
/// thrown error.
fn rust_resolve(case: &Value) -> Value {
    let rows = case["rows"].as_array().cloned().unwrap_or_default();
    let reviews = case
        .get("reviews")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    match resolve_baseline(
        &rows,
        &reviews,
        &or_empty(case.get("headSha")),
        &or_empty(case.get("since")),
    ) {
        Ok(d) => Value::String(json_stringify(Some(&d))),
        Err(msg) => json!({ "error": format!("TypeError: {msg}") }),
    }
}

#[test]
fn resolve_baseline_matches_js_on_generated_inputs() {
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    let cases: Vec<Value> = (0..6000).map(|_| baseline_case(&mut r)).collect();
    let text = Value::Array(cases).to_string();
    let js = driver(
        "resolve-gen",
        RESOLVE,
        "resolveBaseline",
        "for (const c of cases) out.push(attempt(() => JSON.stringify(resolveBaseline({\n\
           rows: c.rows,\n\
           reviews: Array.isArray(c.reviews) ? c.reviews : [],\n\
           headSha: c.headSha || \"\",\n\
           since: c.since || \"\",\n\
         }))));",
        &text,
    );
    let cases = match parse(&text) {
        Value::Array(a) => a,
        _ => unreachable!(),
    };
    assert_eq!(js.len(), cases.len());
    let (mut found_rows, mut found_human, mut errors) = (0, 0, 0);
    for (i, (case, js)) in cases.iter().zip(&js).enumerate() {
        let rust = rust_resolve(case);
        assert_eq!(&rust, js, "case {i}: {case}");
        match js.as_str() {
            Some(s) if s.contains("\"tribunal-row\"") => found_rows += 1,
            Some(s) if s.contains("\"human-review\"") => found_human += 1,
            _ if js.get("error").is_some() => errors += 1,
            _ => {}
        }
    }
    // The generator has to reach every branch, or the comparison proves little.
    assert!(
        found_rows > 350 && found_human > 700 && errors > 100,
        "{found_rows} {found_human} {errors}"
    );
}

#[test]
fn bot_authors_match_js() {
    let logins = json!([
        "alice", "dependabot[bot]", "x[BOT]", "x[Bot]", "[bot]", "bot]", "[bot] ", "x[bot]\n",
        "coderabbitai", "CodeRabbitAI", "coderabbitai ", "", null, 0, 5, true, false, [],
        ["a[bot]"], ["coderabbitai"], {}, {"toString": 1}, "ſ[bot]", "x[\u{212A}ot]", "x[bOt]"
    ]);
    let js = driver(
        "bot-gen",
        RESOLVE,
        "isBotAuthor",
        "for (const l of cases) out.push(attempt(() => isBotAuthor(l)));",
        &logins.to_string(),
    );
    for (login, js) in logins.as_array().unwrap().iter().zip(&js) {
        let rust = match is_bot_author(Some(login)) {
            Ok(b) => json!(b),
            Err(msg) => json!({ "error": format!("TypeError: {msg}") }),
        };
        assert_eq!(&rust, js, "login {login}");
    }
    assert_eq!(is_bot_author(None), Ok(false));
}

#[test]
fn metrics_row_selection_matches_js() {
    let rows = json!([
        {"pr": 42, "repo": "acme/widgets", "id": 1},
        {"pr": 42, "repo": "acme/gadgets", "id": 2},
        {"pr": 42, "id": 3},
        {"pr": 42, "repo": "", "id": 4},
        {"pr": 42, "repo": null, "id": 5},
        {"pr": 42.0, "repo": "acme/widgets", "id": 6},
        {"pr": "42", "repo": "acme/widgets", "id": 7},
        {"pr": 43, "repo": "acme/widgets", "id": 8},
        {"pr": 42, "repo": 0, "id": 9},
        {"pr": 42, "repo": 7, "id": 10},
        {"pr": 42, "repo": ["acme/widgets"], "id": 11},
        [42], "pr", 42, true
    ]);
    let cases = json!([
        {"pr": 42, "repo": "acme/widgets"},
        {"pr": 42, "repo": null},
        {"pr": 43, "repo": "acme/widgets"},
        {"pr": -0.0, "repo": null},
        {"pr": 42.5, "repo": null},
    ]);
    let input = json!({"rows": rows, "cases": cases}).to_string();
    let js = driver(
        "rows-gen",
        RESOLVE,
        "readMetricsRows",
        "for (const c of cases.cases) out.push(readMetricsRows(c.pr, c.repo, cases.rows).map((r) => r.id ?? JSON.stringify(r)));",
        &input,
    );
    let parsed = parse(&input);
    for (c, js) in parsed["cases"].as_array().unwrap().iter().zip(&js) {
        let rows = read_metrics_rows(
            c["pr"].as_f64().unwrap(),
            c["repo"].as_str(),
            parsed["rows"].as_array().unwrap().clone(),
        );
        let ids: Vec<Value> = rows.iter().map(|r| r["id"].clone()).collect();
        assert_eq!(&Value::Array(ids), js, "case {c}");
    }
}

// ---------------------------------------------------------------------------
// resolve-baseline: the script's main path

/// Compares a script run with the port's. A thrown `TypeError` is compared on
/// exit code, stdout, the lines before the error, and V8's message: Node also
/// prints the source line and a stack trace, the port does not.
fn assert_same_run(script: &ScriptRun, rust: &Run, what: &str) {
    assert_eq!(
        script.code,
        Some(rust.code),
        "{what}: exit code\nscript stderr: {}",
        script.stderr
    );
    assert_eq!(script.stdout, rust.stdout, "{what}: stdout");
    if let Some(at) = rust.stderr.find("TypeError: ") {
        let (before, error) = rust.stderr.split_at(at);
        assert!(
            script.stderr.starts_with(before),
            "{what}: stderr before the error\n{}",
            script.stderr
        );
        assert!(
            script.stderr.contains(&format!("\n{error}")),
            "{what}: error {error:?} not in\n{}",
            script.stderr
        );
    } else {
        assert_eq!(script.stderr, rust.stderr, "{what}: stderr");
    }
}

fn run_both(dir: &Path, ledger: &str, stdin: &str) -> (ScriptRun, Run) {
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("metrics.jsonl"), ledger).unwrap();
    let script = node(
        Path::new(RESOLVE),
        &[],
        dir,
        stdin,
        &[("TRIBUNAL_DATA_DIR", data.as_os_str())],
        &[],
    );
    let rust = run(
        stdin,
        &metrics_path_from(Some(data.as_os_str()), Path::new("/nonexistent")),
    );
    (script, rust)
}

fn ledger_line(r: &mut Rng, pr: i64) -> String {
    match r.below(16) {
        0 => String::new(),
        1 => "   ".into(),
        2 => "{not json".into(),
        3 => r
            .pick(&["0", "false", "null", "\"\"", "\"str\"", "[1]", "7", "true"])
            .into(),
        4 => format!("\u{feff}{}", metrics_row(r)),
        5 => format!("{}\r", metrics_row(r)),
        6 => {
            let mut row = metrics_row(r);
            row["pr"] = json!(pr + 1);
            row.to_string()
        }
        7 => {
            let mut row = metrics_row(r);
            row["repo"] = json!(r.pick(&["acme/widgets", "acme/gadgets", ""]));
            row.to_string()
        }
        8 => {
            let mut row = metrics_row(r);
            row["pr"] = json!(r.pick(&[json!("42"), json!(42.0)]));
            row.to_string()
        }
        _ => metrics_row(r).to_string(),
    }
}

#[test]
fn resolve_baseline_script_matches_on_generated_ledgers() {
    let dir = scratch("resolve-main-gen");
    let mut r = Rng(0x0123_4567_89ab_cdef);
    let mut found = 0;
    for i in 0..120 {
        let ledger: Vec<String> = (0..r.below(8)).map(|_| ledger_line(&mut r, 42)).collect();
        let ledger = ledger.join(if r.chance(20) { "\r\n" } else { "\n" });
        let reviews: Vec<Value> = (0..r.below(4)).map(|_| review(&mut r)).collect();
        let stdin = obj(vec![
            ("pr", Some(json!(42))),
            (
                "repo",
                r.pick(&[
                    Some(json!("acme/widgets")),
                    Some(json!("")),
                    None,
                    Some(json!(5)),
                ]),
            ),
            ("headSha", head_or_since(&mut r, false)),
            ("since", head_or_since(&mut r, true)),
            ("reviews", Some(Value::Array(reviews))),
        ])
        .to_string();
        let (script, rust) = run_both(&dir, &ledger, &stdin);
        assert_same_run(
            &script,
            &rust,
            &format!("case {i}: {stdin}\nledger:\n{ledger}"),
        );
        if rust.stdout.contains("\"found\":true") {
            found += 1;
        }
    }
    assert!(found > 15, "only {found} found");
}

#[test]
fn resolve_baseline_script_matches_on_named_cases() {
    let dir = scratch("resolve-main-named");
    let row = |sha: &str, posted: bool, extra: Value| {
        let mut v = json!({
            "schemaVersion": 3, "pr": 42, "repo": "acme/widgets", "headSha": sha,
            "posted": posted, "ts": "2026-09-01T00:00:00Z",
            "findings": [{"file": "src/a.ts", "title": "Null deref", "severity": "high",
                          "blocking": true, "survived": true, "posted": true,
                          "operatorOverride": {"verdict": "dismissed", "1": "x"}}]
        });
        for (k, val) in extra.as_object().unwrap() {
            v[k] = val.clone();
        }
        v.to_string()
    };
    let approved = json!([{"author": "alice", "state": "APPROVED", "at": "2026-09-02T00:00:00Z", "commit": "c0ffee11"}]);
    let stdin = |extra: Value| {
        let mut v = json!({"pr": 42, "repo": "acme/widgets", "headSha": "head0000", "since": "", "reviews": approved});
        for (k, val) in extra.as_object().unwrap() {
            v[k] = val.clone();
        }
        v.to_string()
    };
    let two_rows = format!(
        "{}\n{}\n",
        row("aaaa1111", true, json!({})),
        row("bbbb2222", true, json!({}))
    );
    let cases: Vec<(&str, String, String)> = vec![
        ("last posted row wins", two_rows.clone(), stdin(json!({}))),
        (
            "since picks a prefix, posted or not",
            format!(
                "{}\n{}\n",
                row("aaaa1111", false, json!({})),
                row("bbbb2222", true, json!({}))
            ),
            stdin(json!({"since": "aaaa"})),
        ),
        (
            "since matching nothing falls to the human review",
            two_rows.clone(),
            stdin(json!({"since": "ffff"})),
        ),
        (
            "head commit row is skipped",
            row("head0000", true, json!({})),
            stdin(json!({})),
        ),
        (
            "unposted row is skipped",
            row("aaaa1111", false, json!({})),
            stdin(json!({})),
        ),
        (
            "old schema row is skipped",
            row("aaaa1111", true, json!({"schemaVersion": 2})),
            stdin(json!({})),
        ),
        (
            "no findings row is skipped",
            row("aaaa1111", true, json!({"findings": []})),
            stdin(json!({})),
        ),
        (
            "other repo row is skipped",
            row("aaaa1111", true, json!({"repo": "acme/gadgets"})),
            stdin(json!({})),
        ),
        (
            "row without repo matches",
            row("aaaa1111", true, json!({"repo": null})),
            stdin(json!({})),
        ),
        (
            "no repo given matches any",
            row("aaaa1111", true, json!({"repo": "acme/gadgets"})),
            stdin(json!({"repo": ""})),
        ),
        (
            "other PR row is skipped",
            row("aaaa1111", true, json!({"pr": 7})),
            stdin(json!({})),
        ),
        (
            "no reviews and no rows",
            String::new(),
            stdin(json!({"reviews": []})),
        ),
        (
            "reviews not an array",
            String::new(),
            stdin(json!({"reviews": "nope"})),
        ),
        (
            "bot review ignored",
            String::new(),
            stdin(
                json!({"reviews": [{"author": "ci[bot]", "state": "APPROVED", "commit": "c1", "at": "2026"}]}),
            ),
        ),
        (
            "coderabbit ignored",
            String::new(),
            stdin(
                json!({"reviews": [{"author": "coderabbitai", "state": "CHANGES_REQUESTED", "commit": "c1", "at": "2026"}]}),
            ),
        ),
        (
            "commented ignored",
            String::new(),
            stdin(
                json!({"reviews": [{"author": "bob", "state": "COMMENTED", "commit": "c1", "at": "2026"}]}),
            ),
        ),
        (
            "latest human review wins",
            String::new(),
            stdin(json!({"reviews": [
            {"author": "bob", "state": "CHANGES_REQUESTED", "commit": "c1", "at": "2026-09-03T00:00:00Z"},
            {"author": "carol", "state": "APPROVED", "commit": "c2", "at": "2026-09-01T00:00:00Z"},
            {"author": "dave", "state": "APPROVED", "commit": "c3", "at": "2026-09-03T00:00:00Z"}]})),
        ),
        (
            "author missing",
            String::new(),
            stdin(json!({"reviews": [{"state": "APPROVED", "commit": "c1"}]})),
        ),
        (
            "astral sha cut at 8 units",
            String::new(),
            stdin(
                json!({"reviews": [{"author": "eve", "state": "APPROVED", "commit": "abcdefg\u{1F600}xyz", "at": "1"}]}),
            ),
        ),
        (
            "sha as an object that throws after stdout",
            String::new(),
            stdin(
                json!({"reviews": [{"author": "eve", "state": "APPROVED", "commit": {"toString": 1}, "at": "1"}]}),
            ),
        ),
        (
            "number sha",
            row("x", true, json!({"headSha": 12345})),
            stdin(json!({})),
        ),
        (
            "number sha with since throws",
            row("x", true, json!({"headSha": 12345})),
            stdin(json!({"since": "1"})),
        ),
        (
            "null finding throws",
            row("aaaa1111", true, json!({"findings": [null]})),
            stdin(json!({})),
        ),
        (
            "string findings throw",
            row("aaaa1111", true, json!({"findings": "abc"})),
            stdin(json!({})),
        ),
        (
            "null review throws",
            String::new(),
            stdin(json!({"reviews": [null]})),
        ),
        (
            "pr as 42.0",
            row("aaaa1111", true, json!({})),
            stdin(json!({"pr": 42.0})),
        ),
        (
            "garbage and blank ledger lines",
            format!(
                "\n  \n{{oops\n0\n\u{feff}{}\r\n",
                row("aaaa1111", true, json!({}))
            ),
            stdin(json!({})),
        ),
        (
            "big numbers echo as JavaScript prints them",
            row(
                "aaaa1111",
                true,
                json!({"ts": 1e21, "findings": [{"severity": 2.0, "operatorOverride": [1.5e-7, -0.0]}]}),
            ),
            stdin(json!({})),
        ),
    ];
    for (what, ledger, input) in cases {
        let (script, rust) = run_both(&dir, &ledger, &input);
        assert_same_run(&script, &rust, what);
    }
}

#[test]
fn resolve_baseline_failure_paths_match_js() {
    let dir = scratch("resolve-fail");
    let valid_ledger =
        "{\"schemaVersion\":3,\"pr\":1,\"headSha\":\"a\",\"posted\":true,\"findings\":[{}]}\n";
    for stdin in [
        "{}",
        "{\"pr\":\"42\"}",
        "{\"pr\":null}",
        "[]",
        "5",
        "\"x\"",
        "false",
        "null",
        "{\"pr\":1}",
        "{\"pr\":1,\"repo\":\"acme/widgets\"}",
    ] {
        let (script, rust) = run_both(&dir, valid_ledger, stdin);
        assert_same_run(&script, &rust, stdin);
    }
    // Invalid stdin: same exit code and nothing on stdout; the message after
    // the prefix is V8's on one side and the port's parser's on the other.
    for stdin in ["", "{", "{\"pr\": 1,}", "\u{feff}{\"pr\":1}", "NaN"] {
        let (script, rust) = run_both(&dir, valid_ledger, stdin);
        let prefix = "resolve-baseline: stdin is not valid JSON: ";
        assert_eq!(
            (script.code, script.stdout.as_str()),
            (Some(1), ""),
            "{stdin:?}"
        );
        assert_eq!((rust.code, rust.stdout.as_str()), (1, ""), "{stdin:?}");
        assert!(
            script.stderr.starts_with(prefix) && rust.stderr.starts_with(prefix),
            "{stdin:?}"
        );
    }
    // A missing ledger and a ledger that is a directory both read as empty.
    let input = "{\"pr\":1,\"repo\":\"acme/widgets\",\"reviews\":[{\"author\":\"a\",\"state\":\"APPROVED\",\"commit\":\"c\"}]}";
    let missing = dir.join("no-such-dir");
    let as_dir = dir.join("ledger-dir");
    std::fs::create_dir_all(as_dir.join("metrics.jsonl")).unwrap();
    for data in [missing, as_dir] {
        let script = node(
            Path::new(RESOLVE),
            &[],
            &dir,
            input,
            &[("TRIBUNAL_DATA_DIR", data.as_os_str())],
            &[],
        );
        let rust = run(
            input,
            &metrics_path_from(Some(data.as_os_str()), Path::new("/nonexistent")),
        );
        assert_same_run(&script, &rust, &data.display().to_string());
    }
}

#[test]
fn resolve_baseline_default_ledger_is_under_home() {
    let home = scratch("resolve-home");
    let data = home.join(".claude").join("tribunal-data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("metrics.jsonl"),
        "{\"schemaVersion\":3,\"pr\":9,\"headSha\":\"feed1234\",\"posted\":true,\"findings\":[{\"file\":\"x.ts\"}]}\n",
    )
    .unwrap();
    let input = "{\"pr\":9,\"repo\":\"acme/widgets\",\"headSha\":\"head\"}";
    let empty = OsStr::new("");
    for env in [
        vec![("HOME", home.as_os_str())],
        vec![("HOME", home.as_os_str()), ("TRIBUNAL_DATA_DIR", empty)],
    ] {
        let script = node(Path::new(RESOLVE), &[], &home, input, &env, &[]);
        let data_dir = env
            .iter()
            .find(|(k, _)| *k == "TRIBUNAL_DATA_DIR")
            .map(|(_, v)| *v);
        let rust = run(input, &metrics_path_from(data_dir, &home));
        assert_same_run(&script, &rust, "default ledger");
        assert!(rust.stdout.contains("feed1234"));
    }
}

/// Runs `f` on a thread with a large stack: printing or dropping a deeply
/// nested `Value` recurses.
fn on_big_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(1 << 30)
            .spawn_scoped(s, f)
            .unwrap()
            .join()
            .unwrap()
    })
}

fn nested(depth: usize, open: &str, inner: &str, close: &str) -> String {
    format!("{}{inner}{}", open.repeat(depth), close.repeat(depth))
}

/// The JSON texts `JSON.parse` sees, mutated one character at a time so that
/// both the texts it accepts and the ones it refuses are compared. Numbers
/// that overflow a double and lone surrogate escapes are left out: those are
/// declared differences.
fn json_text_case(r: &mut Rng) -> String {
    let bases = [
        r#"{"schemaVersion":3,"pr":42,"headSha":"aaaa1111","posted":true,"findings":[{"file":"src/a.ts","operatorOverride":{"10":1,"2":2,"b":[true,false,null],"a":-0.5e-3}}]}"#,
        r#"{"a":1,"b":2,"a":{"z":0,"1":1,"y":"é\n\t\"\\\/"}}"#,
        r#"[ 0 , -0 , 1E5 , 1e-7 , 12345678901234567890 , 0.1 , [ ] , { } , "" ]"#,
        "{\"\u{e9}\u{1F600}\":\"\u{2028}\u{2029}\",\"__proto__\":{\"x\":1},\"4294967295\":1,\"4294967294\":2,\"01\":3}",
        r#""just a string""#,
        "  \t\r\n {\"k\" : [ null , true , false ] } \n\r\t ",
    ];
    let mut text: Vec<char> = r.pick(&bases).chars().collect();
    let pieces = [
        "", ",", ":", "[", "]", "{", "}", "\"", "\\", "0", "1", "9", "-", "+", ".", "e", "E", " ",
        "\t", "\n", "\r", "\u{c}", "\u{b}", "\u{a0}", "\u{feff}", "\u{0}", "\u{1f}", "x", "t", "n",
        "u", "/", "'", "true", "null", "NaN", "Infinity", "\\u", "\\x", "\\uzzzz", "\\u00e9",
    ];
    for _ in 0..r.below(3) {
        let at = r.below(text.len() + 1);
        let piece: Vec<char> = r.pick(&pieces).chars().collect();
        match r.below(3) {
            0 if at < text.len() => {
                text.remove(at);
            }
            1 if at < text.len() => {
                text.splice(at..=at, piece);
            }
            _ => {
                text.splice(at..at, piece);
            }
        }
    }
    text.into_iter().collect()
}

#[test]
fn json_parse_matches_js_on_mutated_texts() {
    let mut r = Rng(0x1357_9bdf_2468_ace0);
    let mut cases: Vec<String> = (0..6000).map(|_| json_text_case(&mut r)).collect();
    // JSON.stringify itself gives up somewhere past a thousand levels.
    for depth in [127, 128, 129, 1000] {
        cases.push(nested(depth, "[", "1", "]"));
        cases.push(nested(depth, "{\"a\":", "{\"2\":0,\"1\":1}", "}"));
        cases.push(format!("{} 2", nested(depth, "[", "1", "]")));
        cases.push(nested(depth, "[", "1,", "]"));
    }
    let text = serde_json::to_string(&cases).unwrap();
    let js = driver(
        "json-parse",
        RESOLVE,
        "resolveBaseline",
        "void resolveBaseline;\n\
         const inf = (k, x) => (x === Infinity || x === -Infinity ? \"__INF__\" : x);\n\
         for (const t of cases) out.push(attempt(() => {\n\
           const v = JSON.parse(t);\n\
           return JSON.stringify(v, inf).includes(\"__INF__\") ? { overflow: true } : JSON.stringify(v);\n\
         }));",
        &text,
    );
    let rust: Vec<Option<String>> = on_big_stack(|| {
        cases
            .iter()
            .map(|t| parse_json(t).ok().map(|v| json_stringify(Some(&v))))
            .collect()
    });
    let (mut ok, mut refused) = (0, 0);
    for ((case, js), rust) in cases.iter().zip(&js).zip(&rust) {
        let case: String = case.chars().take(300).collect();
        match (js, rust) {
            (Value::String(js), Some(rust)) => {
                assert_eq!(js, rust, "{case:?}");
                ok += 1;
            }
            (js, None) if js.get("error").is_some() => refused += 1,
            // A number that overflows a double: declared, the port refuses it.
            (js, None) if js.get("overflow").is_some() => {}
            (js, rust) => panic!(
                "{case:?}: script {js}, port {:?}",
                rust.as_deref()
                    .map(|r| r.chars().take(300).collect::<String>())
            ),
        }
    }
    assert!(
        ok > 1500 && refused > 1500,
        "{ok} accepted, {refused} refused"
    );
}

#[test]
fn resolve_baseline_takes_any_nesting_depth() {
    let dir = scratch("resolve-deep");
    let row = |extra: String| {
        format!("{{\"schemaVersion\":3,\"pr\":1,\"headSha\":\"aaaa1111\",\"posted\":true,\"findings\":[{{\"file\":\"x\"}}]{extra}}}\n")
    };
    let input = "{\"pr\":1,\"repo\":\"acme/widgets\",\"headSha\":\"h\"}".to_owned();
    let cases = [
        // serde_json's own limit is 128 levels.
        (row(format!(",\"deep\":{}", nested(127, "[", "1", "]"))), input.clone()),
        (row(format!(",\"deep\":{}", nested(200, "{\"a\":", "1", "}"))), input.clone()),
        (row(format!(",\"deep\":{}", nested(100_000, "[", "1", "]"))), input.clone()),
        (
            format!("{{\"schemaVersion\":3,\"pr\":1,\"headSha\":\"aaaa1111\",\"posted\":true,\"findings\":[{{\"operatorOverride\":{}}}]}}\n", nested(300, "[", "1", "]")),
            input.clone(),
        ),
        (
            String::new(),
            format!(
                "{{\"pr\":1,\"x\":{},\"reviews\":[{{\"author\":\"a\",\"state\":\"APPROVED\",\"commit\":\"c1\",\"at\":{}}}]}}",
                nested(100_000, "[", "1", "]"),
                nested(150, "[", "\"2026\"", "]")
            ),
        ),
        (String::new(), format!("{{\"pr\":1,\"x\":{}", nested(300, "[", "1", "]"))),
    ];
    for (i, (ledger, stdin)) in cases.iter().enumerate() {
        let (script, rust) = run_both(&dir, ledger, stdin);
        if script.code == Some(1) {
            // Invalid stdin: the message after the prefix differs by design.
            assert_eq!((rust.code, rust.stdout.as_str()), (1, ""), "case {i}");
            assert!(rust
                .stderr
                .starts_with("resolve-baseline: stdin is not valid JSON: "));
            continue;
        }
        assert_same_run(&script, &rust, &format!("case {i}"));
        assert!(
            rust.stdout.contains("\"found\":true"),
            "case {i}: {}",
            rust.stdout
        );
    }
}

#[test]
fn resolve_baseline_data_dir_resolves_like_path_resolve() {
    let dir = scratch("resolve-lexical");
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("metrics.jsonl"),
        "{\"schemaVersion\":3,\"pr\":5,\"headSha\":\"feed1234\",\"posted\":true,\"findings\":[{\"file\":\"x.ts\"}]}\n",
    )
    .unwrap();
    // `link` points somewhere else, so `link/..` on disk is not `dir`.
    let elsewhere = dir.join("elsewhere").join("deeper");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, dir.join("link")).unwrap();
    let input = "{\"pr\":5,\"repo\":\"acme/widgets\",\"headSha\":\"head\"}";
    let d = dir.display();
    for data_dir in [
        format!("{d}/missing/../data"),
        format!("{d}/link/../data"),
        format!("{d}/./data/"),
        format!("{d}//data"),
        format!("{d}/data/sub/.."),
    ] {
        let script = node(
            Path::new(RESOLVE),
            &[],
            &dir,
            input,
            &[("TRIBUNAL_DATA_DIR", OsStr::new(&data_dir))],
            &[],
        );
        let rust = run(
            input,
            &metrics_path_from(Some(OsStr::new(&data_dir)), Path::new("/nonexistent")),
        );
        assert_same_run(&script, &rust, &data_dir);
        assert!(rust.stdout.contains("feed1234"), "{data_dir}");
    }
    // The same for HOME, when TRIBUNAL_DATA_DIR is unset.
    let home = dir.join("home");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::os::unix::fs::symlink(&data, home.join(".claude").join("tribunal-data")).unwrap();
    let odd_home = format!("{d}/missing/../home");
    let script = node(
        Path::new(RESOLVE),
        &[],
        &dir,
        input,
        &[("HOME", OsStr::new(&odd_home))],
        &[],
    );
    let rust = run(input, &metrics_path_from(None, Path::new(&odd_home)));
    assert_same_run(&script, &rust, &odd_home);
    assert!(rust.stdout.contains("feed1234"));
}

// ---------------------------------------------------------------------------
// incremental-scope

const PATH_CHARS: &[char] = &[
    'a', 'b', 'B', 'z', 'Z', '0', '9', '_', '-', '.', '/', ' ', '"', '\'', '$', '~', '(', '[', '{',
    '@', '+', '#', '\\', '\t', '\u{1}', '\u{7f}', '^', '`', '|', '=', '<', ',', ';', ':', '!', '?',
    '&', '%', '*', '}', ']', ')', '>',
];

fn random_path(r: &mut Rng) -> String {
    let n = 1 + r.below(6);
    (0..n).map(|_| r.pick(PATH_CHARS)).collect()
}

fn path_pool(r: &mut Rng) -> Vec<String> {
    let mut pool: Vec<String> = vec![
        "src/a.ts".into(),
        "src/A.ts".into(),
        "src/a-b.ts".into(),
        "src/a_b.ts".into(),
        "src/a b.ts".into(),
        "src/x b/y.ts".into(),
        "Readme.md".into(),
        "readme.md".into(),
        "lib/10.ts".into(),
        "lib/9.ts".into(),
    ];
    for _ in 0..6 {
        pool.push(random_path(r));
    }
    pool
}

fn body_line(r: &mut Rng) -> String {
    r.pick(&[
        "+added",
        "-removed",
        " context",
        "+++ b/x",
        "--- a/x",
        "++plus",
        "--minus",
        "",
        "@@ -1,3 +1,4 @@",
        "\\ No newline at end of file",
        "+",
        "-",
        "index 1234..5678 100644",
        "+\r",
        "+crlf\r",
    ])
    .to_owned()
}

fn section(r: &mut Rng, path: &str) -> String {
    let header = match r.below(40) {
        0 => format!("diff --git \"a/{path}\" \"b/{path}\""),
        1 => format!("diff --git a/{path} b/{path}\r"),
        2 => "diff --git a/ b/".to_owned(),
        3 => format!("diff --git a/{path} b/{path}\u{2028}"),
        _ => format!("diff --git a/{path} b/{path}"),
    };
    let mut lines = vec![header];
    for _ in 0..r.below(6) {
        lines.push(body_line(r));
    }
    lines.join("\n")
}

fn diff(r: &mut Rng, pool: &[String]) -> (Vec<(String, String)>, String) {
    let n = r.below(6);
    let parts: Vec<(String, String)> = (0..n)
        .map(|_| {
            let p = r.pick(pool);
            let s = section(r, &p);
            (p, s)
        })
        .collect();
    let mut text = parts
        .iter()
        .map(|(_, s)| s.clone())
        .collect::<Vec<_>>()
        .join("\n");
    if r.chance(10) {
        text = format!("preamble line\n{text}");
    }
    if r.chance(30) {
        text.push('\n');
    }
    (parts, text)
}

fn scope_case(r: &mut Rng) -> (String, String) {
    let pool = path_pool(r);
    let (parts, baseline) = diff(r, &pool);
    if r.chance(40) && !parts.is_empty() {
        // Keep some sections byte-identical, so "unchanged" is reached.
        let mut kept: Vec<String> = parts
            .iter()
            .filter(|_| r.chance(60))
            .map(|(_, s)| s.clone())
            .collect();
        let (_, extra) = diff(r, &pool);
        if !extra.is_empty() {
            kept.push(extra);
        }
        return (baseline, kept.join("\n"));
    }
    let (_, current) = diff(r, &pool);
    (baseline, current)
}

#[test]
fn incremental_scope_matches_js_on_generated_diffs() {
    let mut r = Rng(0xdead_beef_cafe_f00d);
    let cases: Vec<Value> = (0..4000)
        .map(|_| {
            let (b, c) = scope_case(&mut r);
            json!([b, c])
        })
        .collect();
    let text = Value::Array(cases.clone()).to_string();
    let js = driver(
        "scope-gen",
        SCOPE,
        "scopeIncremental",
        "for (const [b, c] of cases) out.push(JSON.stringify(scopeIncremental(b, c)));",
        &text,
    );
    let (mut usable, mut unchanged, mut unparsed) = (0, 0, 0);
    for (i, (case, js)) in cases.iter().zip(&js).enumerate() {
        let scope = scope_incremental(case[0].as_str().unwrap(), case[1].as_str().unwrap());
        assert_eq!(Value::String(scope.to_json()), *js, "case {i}: {case}");
        usable += usize::from(scope.usable);
        unchanged += usize::from(!scope.unchanged.is_empty());
        unparsed += usize::from(
            scope
                .reason
                .as_deref()
                .is_some_and(|r| r.starts_with("unparsed")),
        );
    }
    assert!(
        usable > 1000 && unchanged > 200 && unparsed > 200,
        "{usable} {unchanged} {unparsed}"
    );
}

#[test]
fn locale_compare_matches_js_on_ascii() {
    let mut r = Rng(0x5151_aaaa_0000_0001);
    let mut pairs: Vec<Value> = Vec::new();
    let all: Vec<char> = (0u8..128).map(char::from).collect();
    for a in &all {
        for b in &all {
            pairs.push(json!([a.to_string(), b.to_string()]));
        }
    }
    for _ in 0..20000 {
        let mut s = || -> String { (0..r.below(5)).map(|_| r.pick(&all)).collect() };
        let (a, b) = (s(), s());
        pairs.push(json!([a, b]));
    }
    let text = Value::Array(pairs.clone()).to_string();
    let js = driver(
        "collate-gen",
        SCOPE,
        "scopeIncremental",
        "void scopeIncremental; for (const [a, b] of cases) out.push(Math.sign(a.localeCompare(b)));",
        &text,
    );
    for (pair, js) in pairs.iter().zip(&js) {
        let ord = locale_compare(pair[0].as_str().unwrap(), pair[1].as_str().unwrap()) as i64;
        assert_eq!(json!(ord), *js, "pair {pair}");
    }
}

fn write(dir: &Path, name: &str, text: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    p
}

#[test]
fn incremental_scope_script_matches_on_files() {
    let dir = scratch("scope-main");
    let base = b"diff --git a/src/b.ts b/src/b.ts\n--- a/src/b.ts\n+++ b/src/b.ts\n@@ -1 +1 @@\n-x\n+y\ndiff --git a/src/same.ts b/src/same.ts\n+same\ndiff --git a/src/old.ts b/src/old.ts\n+old\n";
    let cur = b"diff --git a/src/same.ts b/src/same.ts\n+same\ndiff --git a/src/b.ts b/src/b.ts\n--- a/src/b.ts\n+++ b/src/b.ts\n@@ -1 +1,2 @@\n-x\n+y\n+z\ndiff --git a/src/A_new.ts b/src/A_new.ts\n+++ b/src/A_new.ts\n+n\n";
    write(&dir, "base.patch", base);
    write(&dir, "cur.patch", cur);
    write(&dir, "empty.patch", b"");
    write(&dir, "nosections.patch", b"just text\nno headers\n");
    write(
        &dir,
        "quoted.patch",
        b"diff --git \"a/x y\" \"b/x y\"\n+q\n",
    );
    write(&dir, "crlf.patch", b"diff --git a/w.ts b/w.ts\r\n+w\r\n");
    write(
        &dir,
        "latin1.patch",
        b"diff --git a/src/b.ts b/src/b.ts\n-x\n+caf\xe9\n",
    );
    std::fs::create_dir_all(dir.join("adir")).unwrap();
    let abs = dir.join("base.patch");
    let abs = abs.to_str().unwrap();
    let cases: Vec<Vec<&str>> = vec![
        vec!["base.patch", "cur.patch"],
        vec![abs, "cur.patch"],
        vec!["cur.patch", "base.patch"],
        vec!["base.patch", "base.patch"],
        vec!["base.patch", "missing.patch"],
        vec!["missing.patch", "cur.patch"],
        vec!["base.patch", "empty.patch"],
        vec!["base.patch", "adir"],
        vec!["base.patch", "nosections.patch"],
        vec!["quoted.patch", "cur.patch"],
        vec!["base.patch", "crlf.patch"],
        vec!["base.patch", "latin1.patch"],
        vec!["base.patch"],
        vec![],
        vec!["", "cur.patch"],
        vec!["base.patch", ""],
        vec!["base.patch", "cur.patch", "extra"],
    ];
    for args in cases {
        let script = node(Path::new(SCOPE), &args, &dir, "", &[], &[]);
        assert_eq!(script.code, Some(0), "{args:?}: {}", script.stderr);
        assert_eq!(script.stderr, "", "{args:?}");
        // The port reads relative paths against the process cwd; the script
        // ran in `dir`, so the port is handed the same files by absolute path.
        let abs: Vec<String> = args
            .iter()
            .map(|a| {
                if a.is_empty() {
                    String::new()
                } else {
                    dir.join(a).to_string_lossy().into_owned()
                }
            })
            .collect();
        let rust = scope_files(
            abs.first().map(String::as_str),
            abs.get(1).map(String::as_str),
        );
        assert_eq!(script.stdout, format!("{}\n", rust.to_json()), "{args:?}");
    }
}

/// V8's longest string, in UTF-16 units. `readFileSync(p, "utf8")` throws for
/// a file of this many bytes or more, whatever the bytes decode to.
const NODE_STRING_LIMIT: u64 = 0x1fff_ffe8;

/// A file of `len` bytes that starts with `head` and continues with NULs, kept
/// sparse so it costs no disk.
fn sparse(dir: &Path, name: &str, head: &[u8], len: u64) -> PathBuf {
    let p = write(dir, name, head);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&p)
        .unwrap()
        .set_len(len)
        .unwrap();
    p
}

#[test]
fn incremental_scope_reads_a_patch_as_node_does_at_the_string_limit() {
    let dir = scratch("scope-limit");
    let head = b"diff --git a/x.ts b/x.ts\n+1\n";
    write(&dir, "small.patch", b"diff --git a/y.ts b/y.ts\n+2\n");
    sparse(&dir, "at.patch", head, NODE_STRING_LIMIT);
    sparse(&dir, "below.patch", head, NODE_STRING_LIMIT - 1);
    for name in ["at.patch", "below.patch"] {
        let script = node(Path::new(SCOPE), &["small.patch", name], &dir, "", &[], &[]);
        assert_eq!(
            (script.code, script.stderr.as_str()),
            (Some(0), ""),
            "{name}"
        );
        let small = dir.join("small.patch");
        let big = dir.join(name);
        let rust = scope_files(small.to_str(), big.to_str());
        assert_eq!(script.stdout, format!("{}\n", rust.to_json()), "{name}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn resolve_baseline_reads_the_ledger_as_node_does_at_the_string_limit() {
    let dir = scratch("resolve-limit");
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let row = b"{\"schemaVersion\":3,\"pr\":5,\"headSha\":\"feed1234\",\"posted\":true,\"findings\":[{\"file\":\"x.ts\"}]}\n";
    sparse(&data, "metrics.jsonl", row, NODE_STRING_LIMIT);
    let input = "{\"pr\":5,\"repo\":\"acme/widgets\",\"headSha\":\"head\",\"reviews\":[{\"author\":\"a\",\"state\":\"APPROVED\",\"commit\":\"c1\"}]}";
    let script = node(
        Path::new(RESOLVE),
        &[],
        &dir,
        input,
        &[("TRIBUNAL_DATA_DIR", data.as_os_str())],
        &[],
    );
    let rust = run(input, &data.join("metrics.jsonl"));
    assert_same_run(&script, &rust, "ledger at the string limit");
    assert!(rust.stdout.contains("\"human-review\""), "{}", rust.stdout);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn split_keeps_quoted_headers_the_workflow_splitter_refuses() {
    // Why the port has its own splitter rather than the workflow's.
    let diff = "diff --git a/x\"y b/x\"y\n+q";
    let js = driver(
        "split-quote",
        SCOPE,
        "splitDiffByFile",
        "for (const d of cases) { const s = splitDiffByFile(d); out.push([[...s.sections.entries()], s.unparsed]); }",
        &json!([diff]).to_string(),
    );
    let ours = split_diff_by_file(diff);
    assert_eq!(
        js[0],
        json!([
            ours.sections
                .iter()
                .map(|(p, s)| json!([p, s]))
                .collect::<Vec<_>>(),
            ours.unparsed
        ])
    );
    assert_eq!(ours.sections.len(), 1);
    let core = tribunal_core::lens::diff_sectioning::split_diff_by_file(Some(diff));
    assert_eq!((core.sections.len(), core.unparsed.len()), (0, 1));
}

// ---------------------------------------------------------------------------
// Declared differences, each with its witness

#[test]
fn declared_non_ascii_paths_collate_by_code_point() {
    let cur = "diff --git a/f.ts b/f.ts\n+f\ndiff --git a/\u{e9}.ts b/\u{e9}.ts\n+e";
    let js = driver(
        "decl-nonascii",
        SCOPE,
        "scopeIncremental",
        "for (const [b, c] of cases) out.push(scopeIncremental(b, c).added);",
        &json!([["diff --git a/z.ts b/z.ts\n+z", cur]]).to_string(),
    );
    assert_eq!(js[0], json!(["\u{e9}.ts", "f.ts"]));
    let rust = scope_incremental("diff --git a/z.ts b/z.ts\n+z", cur);
    assert_eq!(rust.added, ["f.ts", "\u{e9}.ts"]);
}

#[test]
fn declared_locale_tailoring_is_not_followed() {
    let dir = scratch("decl-locale");
    let p = write(&dir, "b.patch", b"diff --git a/q.ts b/q.ts\n+q\n");
    let c = write(
        &dir,
        "c.patch",
        b"diff --git a/z.ts b/z.ts\n+z\ndiff --git a/aa.ts b/aa.ts\n+a\n",
    );
    let (p, c) = (p.to_str().unwrap(), c.to_str().unwrap());
    let script = node(
        Path::new(SCOPE),
        &[p, c],
        &dir,
        "",
        &[("LC_ALL", OsStr::new("da_DK.UTF-8"))],
        &[],
    );
    assert_eq!(last_json(&script.stdout)["added"], json!(["z.ts", "aa.ts"]));
    assert_eq!(scope_files(Some(p), Some(c)).added, ["aa.ts", "z.ts"]);
}

#[test]
fn declared_overflowing_numbers_are_rejected() {
    let dir = scratch("decl-overflow");
    // stdin: JSON.parse reads 1e999 as Infinity and fails the finite check;
    // serde refuses the number. Both exit 1 with nothing on stdout.
    let (script, rust) = run_both(&dir, "", "{\"pr\":1e999}");
    assert_eq!(
        script.stderr,
        "resolve-baseline: input must carry a numeric \"pr\"\n"
    );
    assert!(rust
        .stderr
        .starts_with("resolve-baseline: stdin is not valid JSON: "));
    assert_eq!((script.code, rust.code), (Some(1), 1));
    // Ledger: a row holding such a number is kept by the script, dropped here.
    // JSON.stringify writes Infinity as null, so the harness never writes one.
    let row = "{\"schemaVersion\":3,\"pr\":1,\"headSha\":\"aaaa\",\"posted\":true,\"findings\":[{}],\"cost\":1e999}\n";
    let (script, rust) = run_both(&dir, row, "{\"pr\":1,\"repo\":\"acme/widgets\"}");
    assert!(script.stdout.contains("\"found\":true"));
    assert!(rust.stdout.contains("\"found\":false"));
}

#[test]
fn declared_type_error_stderr_has_no_stack() {
    let dir = scratch("decl-stack");
    let (script, rust) = run_both(&dir, "", "null");
    assert_eq!((script.code, rust.code), (Some(1), 1));
    assert!(script
        .stderr
        .contains("\nTypeError: Cannot read properties of null (reading 'pr')\n    at "));
    assert_eq!(
        rust.stderr,
        "TypeError: Cannot read properties of null (reading 'pr')\n"
    );
}
