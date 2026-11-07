//! Runtime confirmation (`--runtime-confirm`, `docs/runtime-confirmation-v1.md`):
//! a lens's `runtimeClaim` about what an exported function returns or throws
//! becomes a vitest case the binary writes, runs at the pinned head with the
//! reviewed repo's own vitest, and classifies. Only an assertion that fails
//! confirms; everything else leaves the finding as it was.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use oxc_allocator::Allocator;
use oxc_ast::ast::{Expression, Statement};
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

pub const RUNNER: &str = "vitest";
pub const MAX_CASES: usize = 8;
pub const CASE_TIMEOUT: Duration = Duration::from_secs(60);
pub const OUTPUT_CAP: usize = 16 * 1024;
/// The candidate field a confirmed finding carries to the panel.
pub const RUNTIME_CASE: &str = "runtimeCase";
pub const CLAIM_FIELD: &str = "runtimeClaim";

const EXTENSIONS: [&str; 7] = ["ts", "tsx", "mts", "cts", "js", "mjs", "cjs"];
const SCRIPTS: [&str; 2] = ["test:unit", "test"];
const FAILURE_CHARS: usize = 300;

/// What the lens is told when the flag is on. Appended to each lens prompt,
/// never to the shared system prompt, so the cache prefix is the same.
pub const LENS_PARAGRAPH: &str = "\n\nRUNTIME CLAIMS (this run confirms some claims by running them): on a finding whose core claim is what an EXPORTED function of this PR returns or throws for a given input, you may add runtimeClaim = {\"function\": <its exported name>, \"args\": [<arguments as JSON>], \"expect\": {\"returns\": <the JSON value a correct function returns>} or {\"throws\": true}}. \"expect\" is the CORRECT behaviour, the one your finding says the code fails: the binary turns it into a test, and a failing test is evidence for your finding. Only when the function is in the finding's file, every argument and the expected value can be written as JSON, and the function touches no database, network, clock, randomness, environment variable or shared state. Otherwise omit it; most findings have none.";

#[derive(Debug, Clone, PartialEq)]
pub enum Expect {
    Returns(Value),
    Throws,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Claim {
    pub function: String,
    pub args: Vec<Value>,
    pub expect: Expect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Confirmed,
    Passed,
    Inconclusive,
    Skipped,
}

impl Outcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Passed => "passed",
            Self::Inconclusive => "inconclusive",
            Self::Skipped => "skipped",
        }
    }
}

/// `None` when the finding carries no claim; `Err` when it carries one the
/// binary cannot turn into a case.
#[must_use]
pub fn parse_claim(v: Option<&Value>) -> Option<Result<Claim, &'static str>> {
    let o = match v {
        None | Some(Value::Null) => return None,
        Some(Value::Object(o)) => o,
        Some(_) => return Some(Err("malformed-claim")),
    };
    let parsed = (|| {
        let function = o.get("function")?.as_str()?;
        if !is_identifier(function) {
            return None;
        }
        let args = o.get("args")?.as_array()?.clone();
        let e = o.get("expect")?.as_object()?;
        let expect = match (e.get("returns"), e.get("throws")) {
            (Some(r), None) => Expect::Returns(r.clone()),
            (None, Some(Value::Bool(true))) => Expect::Throws,
            _ => return None,
        };
        Some(Claim {
            function: function.to_owned(),
            args,
            expect,
        })
    })();
    Some(parsed.ok_or("malformed-claim"))
}

#[must_use]
pub fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

#[must_use]
pub fn runnable_file(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && !stem.ends_with('/') && EXTENSIONS.contains(&ext)
    }) && !path.ends_with(".d.ts")
}

fn source_type(path: &str) -> SourceType {
    match path.rsplit_once('.').map(|(_, e)| e) {
        Some("tsx") => SourceType::tsx(),
        Some("ts" | "mts" | "cts") => SourceType::ts(),
        _ => SourceType::unambiguous().with_jsx(true),
    }
}

fn calls(e: &Expression) -> bool {
    match e {
        Expression::CallExpression(_)
        | Expression::NewExpression(_)
        | Expression::AwaitExpression(_)
        | Expression::TaggedTemplateExpression(_)
        | Expression::ImportExpression(_)
        | Expression::ChainExpression(_) => true,
        Expression::ParenthesizedExpression(p) => calls(&p.expression),
        Expression::UnaryExpression(u) => calls(&u.argument),
        Expression::SequenceExpression(s) => s.expressions.iter().any(calls),
        Expression::AssignmentExpression(a) => calls(&a.right),
        Expression::LogicalExpression(l) => calls(&l.left) || calls(&l.right),
        Expression::ConditionalExpression(c) => {
            calls(&c.test) || calls(&c.consequent) || calls(&c.alternate)
        }
        _ => false,
    }
}

/// Whether importing the file runs a call it makes at the top level, such as
/// `main();`, `app.listen(3000)` or `export default build()`. A call guarded
/// by an `if` (`if (require.main === module) main();`) and a call inside a
/// declaration's initializer (`const schema = z.object(...)`) do not count.
/// `None` when the file does not parse.
#[must_use]
pub fn top_level_call(path: &str, source: &str) -> Option<bool> {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, source, source_type(path))
        .with_options(ParseOptions::default())
        .parse();
    if ret.fatal_error || ret.diagnostics.iter().next().is_some() {
        return None;
    }
    Some(ret.program.body.iter().any(|s| match s {
        Statement::ExpressionStatement(es) => calls(&es.expression),
        Statement::ExportDefaultDeclaration(d) => d.declaration.as_expression().is_some_and(calls),
        _ => false,
    }))
}

/// The vitest script of a `package.json`: `Some(config)` for the first of
/// `test:unit` and `test` that runs vitest, with its `--config` when it names
/// one; `None` when neither does.
#[must_use]
pub fn vitest_script(package_json: &str) -> Option<Option<String>> {
    let pkg: Value = serde_json::from_str(package_json).ok()?;
    let scripts = pkg.get("scripts")?.as_object()?;
    for name in SCRIPTS {
        let Some(script) = scripts.get(name).and_then(Value::as_str) else {
            continue;
        };
        for segment in script.split(['&', ';', '|']) {
            let words: Vec<&str> = segment.split_whitespace().collect();
            if !words
                .iter()
                .any(|w| *w == "vitest" || w.ends_with("/vitest"))
            {
                continue;
            }
            let mut config = None;
            for (i, w) in words.iter().enumerate() {
                if let Some(v) = w.strip_prefix("--config=") {
                    config = Some(v.to_owned());
                } else if (*w == "--config" || *w == "-c") && i + 1 < words.len() {
                    config = Some(words[i + 1].to_owned());
                }
            }
            return Some(config);
        }
    }
    None
}

/// The test file for `claim`, importing the finding's file by `module`
/// (`./name.ts`). The name and the values go in as JSON literals.
#[must_use]
pub fn render_case(module: &str, claim: &Claim) -> String {
    let q = |v: &Value| serde_json::to_string(v).unwrap_or_else(|_| "null".into());
    let check = match &claim.expect {
        Expect::Returns(v) => format!(
            "  if (threw) throw new Error('TRIBUNAL_THREW ' + String(error));\n  expect(value).toEqual({});\n",
            q(v)
        ),
        Expect::Throws => "  expect(threw, 'TRIBUNAL_EXPECTED_THROW').toBe(true);\n".to_owned(),
    };
    format!(
        "import {{ test, expect }} from 'vitest';\nimport * as m from {module};\ntest('tribunal runtime case', async () => {{\n  const f = (m as Record<string, unknown>)[{name}];\n  if (typeof f !== 'function') throw new Error('TRIBUNAL_NOT_A_FUNCTION');\n  let threw = false;\n  let value: unknown;\n  let error: unknown;\n  try {{ value = await (f as (...a: unknown[]) => unknown)(...{args}); }}\n  catch (e) {{ threw = true; error = e; }}\n{check}}});\n",
        module = q(&json!(module)),
        name = q(&json!(claim.function)),
        args = q(&Value::Array(claim.args.clone())),
    )
}

/// The message without its stack, on one line: a reporter message can be a
/// pretty-printed object, whose first line alone is `[`.
fn summary(s: &str) -> String {
    let body = s.split("\n    at ").next().unwrap_or("");
    let one = body.split_whitespace().collect::<Vec<_>>().join(" ");
    one.chars().take(FAILURE_CHARS).collect()
}

/// Reads vitest's JSON reporter output. Only a failed test whose first
/// failure message is an `AssertionError` confirms.
#[must_use]
pub fn classify(report: Option<&str>) -> (Outcome, String) {
    let Some(v) = report.and_then(|r| serde_json::from_str::<Value>(r).ok()) else {
        return (Outcome::Inconclusive, "no reporter output".into());
    };
    let suites = v
        .get("testResults")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let tests: Vec<&Value> = suites
        .iter()
        .flat_map(|s| {
            s.get("assertionResults")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .collect();
    if let Some(failed) = tests
        .iter()
        .find(|t| t.get("status").and_then(Value::as_str) == Some("failed"))
    {
        let msg = failed
            .get("failureMessages")
            .and_then(Value::as_array)
            .and_then(|m| m.first())
            .and_then(Value::as_str)
            .unwrap_or("");
        let line = summary(msg);
        return if msg.starts_with("AssertionError") {
            (Outcome::Confirmed, line)
        } else {
            (Outcome::Inconclusive, line)
        };
    }
    if tests
        .iter()
        .any(|t| t.get("status").and_then(Value::as_str) == Some("passed"))
    {
        return (Outcome::Passed, String::new());
    }
    let msg = suites
        .iter()
        .find_map(|s| {
            s.get("message")
                .and_then(Value::as_str)
                .filter(|m| !m.is_empty())
        })
        .map_or_else(|| "no test result".to_owned(), summary);
    (Outcome::Inconclusive, msg)
}

/// What one child process left behind.
#[derive(Debug, Default)]
pub struct Ran {
    pub report: Option<String>,
    pub timed_out: bool,
    pub output: String,
}

/// Reads `r` to its end and keeps the first [`OUTPUT_CAP`] bytes, so a
/// child that floods its output cannot fill memory.
pub async fn capped<R: tokio::io::AsyncRead + Unpin>(mut r: R) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buf = [0u8; 8192];
    while let Ok(n) = r.read(&mut buf).await {
        if n == 0 {
            break;
        }
        let room = OUTPUT_CAP.saturating_sub(kept.len());
        kept.extend_from_slice(&buf[..n.min(room)]);
    }
    kept
}

/// The environment the case runs in: `PATH` from the parent and nothing
/// else of it, a `HOME` that is an empty directory of the run's, and the
/// variables a test runner expects.
#[must_use]
pub fn child_env(
    path: Option<&std::ffi::OsStr>,
    home: &Path,
    tmp: &Path,
) -> Vec<(String, std::ffi::OsString)> {
    let mut env = vec![
        ("HOME".to_owned(), home.as_os_str().to_owned()),
        ("TMPDIR".to_owned(), tmp.as_os_str().to_owned()),
        ("CI".to_owned(), "1".into()),
        ("NODE_ENV".to_owned(), "test".into()),
    ];
    if let Some(p) = path {
        env.push(("PATH".to_owned(), p.to_owned()));
    }
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        env.push(("SystemRoot".to_owned(), root));
    }
    env
}

/// Runs `program args` in `cwd` with only `env`, its own process group, a
/// timeout and capped output, then reads `report` if the child wrote it.
pub async fn run_child(
    program: &Path,
    args: &[String],
    cwd: &Path,
    env: &[(String, std::ffi::OsString)],
    report: &Path,
    timeout: Duration,
) -> Ran {
    let mut cmd = crate::proc::group_command(&program.to_string_lossy());
    cmd.args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let Ok(mut child) = cmd.spawn() else {
        return Ran::default();
    };
    let out = tokio::spawn(capped(child.stdout.take().expect("piped")));
    let err = tokio::spawn(capped(child.stderr.take().expect("piped")));
    let timed_out = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(_) => false,
        Err(_) => {
            if let Some(pid) = child.id() {
                crate::proc::kill_tree(pid).await;
            }
            let _ = child.kill().await;
            true
        }
    };
    let mut output = out.await.unwrap_or_default();
    output.extend(err.await.unwrap_or_default());
    output.truncate(OUTPUT_CAP);
    Ran {
        report: (!timed_out)
            .then(|| std::fs::read_to_string(report).ok())
            .flatten(),
        timed_out,
        output: String::from_utf8_lossy(&output).into_owned(),
    }
}

/// Where the runner lives and how to call it, or why there is none.
#[derive(Debug, Clone)]
pub struct Runner {
    pub program: PathBuf,
    pub config: Option<String>,
}

fn vitest_bin(checkout: &Path) -> PathBuf {
    let bin = checkout.join("node_modules").join(".bin");
    if cfg!(windows) {
        bin.join("vitest.cmd")
    } else {
        bin.join("vitest")
    }
}

/// The reviewed repo's vitest: the binary in the checkout's `node_modules`
/// and the config its `package.json` script names, read from the pinned tree.
///
/// # Errors
/// The reason, as recorded, when there is none.
pub fn locate_runner(checkout: &Path, tree: &Path) -> Result<Runner, &'static str> {
    let pkg = std::fs::read_to_string(tree.join("package.json")).map_err(|_| "no-package-json")?;
    let config = vitest_script(&pkg).ok_or("no-vitest-script")?;
    let program = vitest_bin(checkout);
    if !program.is_file() {
        return Err("vitest-not-installed");
    }
    Ok(Runner { program, config })
}

/// Everything one run needs to place and run cases.
pub struct Setup<'a> {
    pub pr_files: &'a [String],
    /// The pinned head tree, `None` when the run has none.
    pub tree: Option<&'a Path>,
    /// The checkout whose `node_modules` the typecheck uses.
    pub checkout: &'a Path,
    /// Where copies of the cases and their reports stay after the run.
    pub data_dir: &'a Path,
    pub timeout: Duration,
}

fn record(
    file: &str,
    claim: Option<&Claim>,
    outcome: Outcome,
    reason: &str,
    seconds: f64,
) -> Value {
    json!({
        "file": file,
        "function": claim.map(|c| c.function.clone()),
        "outcome": outcome.as_str(),
        "reason": reason,
        "seconds": (seconds * 10.0).round() / 10.0,
    })
}

fn module_spec(file: &str) -> (String, String) {
    let name = file.rsplit('/').next().unwrap_or(file).to_owned();
    let stem = name
        .rsplit_once('.')
        .map_or(name.as_str(), |(s, _)| s)
        .to_owned();
    (format!("./{name}"), stem)
}

/// Runs the case of every candidate that carries a `runtimeClaim`, in order,
/// and puts `runtimeCase` on the ones that confirmed. Returns the
/// `runtimeChecks` record.
pub async fn confirm(candidates: &mut [Value], setup: &Setup<'_>) -> Value {
    let started = Instant::now();
    let runner = setup
        .tree
        .ok_or("no-pinned-tree")
        .and_then(|t| locate_runner(setup.checkout, t));
    let mut cases = Vec::new();
    let mut ran = 0usize;
    let mut without_claim = 0usize;
    for (n, cand) in candidates.iter_mut().enumerate() {
        let file = cand
            .get("file")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let claim = match parse_claim(cand.get(CLAIM_FIELD)) {
            None => {
                without_claim += 1;
                continue;
            }
            Some(Err(reason)) => {
                cases.push(record(&file, None, Outcome::Skipped, reason, 0.0));
                continue;
            }
            Some(Ok(c)) => c,
        };
        let skip = |reason: &str| record(&file, Some(&claim), Outcome::Skipped, reason, 0.0);
        if !runnable_file(&file) {
            cases.push(skip("not-a-js-or-ts-file"));
            continue;
        }
        if !setup.pr_files.iter().any(|f| f == &file) {
            cases.push(skip("file-not-in-pr"));
            continue;
        }
        let (tree, runner) = match (setup.tree, &runner) {
            (Some(t), Ok(r)) => (t, r),
            (_, Err(reason)) => {
                cases.push(skip(reason));
                continue;
            }
            (None, Ok(_)) => {
                cases.push(skip("no-pinned-tree"));
                continue;
            }
        };
        let Ok(source) = std::fs::read_to_string(tree.join(&file)) else {
            cases.push(skip("file-not-at-head"));
            continue;
        };
        match top_level_call(&file, &source) {
            None => {
                cases.push(skip("unparsed"));
                continue;
            }
            Some(true) => {
                cases.push(skip("top-level-call"));
                continue;
            }
            Some(false) => {}
        }
        if ran >= MAX_CASES {
            cases.push(skip("case-limit"));
            continue;
        }
        ran += 1;
        let (outcome, detail, seconds) = run_one(tree, runner, &file, &claim, n, setup).await;
        if outcome == Outcome::Confirmed {
            if let Some(m) = cand.as_object_mut() {
                m.insert(
                    RUNTIME_CASE.into(),
                    json!({
                        "function": claim.function,
                        "args": claim.args,
                        "expect": match &claim.expect {
                            Expect::Returns(v) => json!({"returns": v}),
                            Expect::Throws => json!({"throws": true}),
                        },
                        "failure": detail,
                    }),
                );
            }
        }
        cases.push(record(&file, Some(&claim), outcome, &detail, seconds));
    }
    let count = |o: Outcome| cases.iter().filter(|c| c["outcome"] == o.as_str()).count();
    json!({
        "runner": RUNNER,
        "config": runner.as_ref().ok().and_then(|r| r.config.clone()),
        "withoutClaim": without_claim,
        "confirmed": count(Outcome::Confirmed),
        "passed": count(Outcome::Passed),
        "inconclusive": count(Outcome::Inconclusive),
        "skipped": count(Outcome::Skipped),
        "seconds": (started.elapsed().as_secs_f64() * 10.0).round() / 10.0,
        "dir": setup.data_dir.to_string_lossy(),
        "cases": cases,
    })
}

async fn run_one(
    tree: &Path,
    runner: &Runner,
    file: &str,
    claim: &Claim,
    n: usize,
    setup: &Setup<'_>,
) -> (Outcome, String, f64) {
    let started = Instant::now();
    let (module, stem) = module_spec(file);
    let case_name = format!("{stem}.tribunal-case-{}.test.ts", n + 1);
    let dir = Path::new(file).parent().unwrap_or(Path::new(""));
    let case_rel = dir.join(&case_name);
    let case_path = tree.join(&case_rel);
    let body = render_case(&module, claim);
    let work = setup.data_dir.join(format!("case-{}", n + 1));
    let home = work.join("home");
    let tmp = work.join("tmp");
    if std::fs::create_dir_all(&home).is_err()
        || std::fs::create_dir_all(&tmp).is_err()
        || std::fs::write(&case_path, &body).is_err()
    {
        return (
            Outcome::Inconclusive,
            "could not write the case".into(),
            0.0,
        );
    }
    let _ = std::fs::write(work.join(&case_name), &body);
    let nm = tree.join("node_modules");
    if !nm.exists() {
        let _ = crate::proc::link_dir(&setup.checkout.join("node_modules"), &nm);
    }
    let report = work.join("report.json");
    let mut args = vec![
        "run".to_owned(),
        case_rel.to_string_lossy().replace('\\', "/"),
    ];
    if let Some(c) = &runner.config {
        args.push("--config".into());
        args.push(c.clone());
    }
    args.push("--reporter=json".into());
    args.push(format!("--outputFile={}", report.to_string_lossy()));
    let env = child_env(std::env::var_os("PATH").as_deref(), &home, &tmp);
    let ran = run_child(&runner.program, &args, tree, &env, &report, setup.timeout).await;
    let _ = std::fs::remove_file(&case_path);
    let _ = std::fs::write(work.join("output.txt"), &ran.output);
    let (outcome, detail) = if ran.timed_out {
        (Outcome::Inconclusive, "timeout".to_owned())
    } else {
        classify(ran.report.as_deref())
    };
    (outcome, detail, started.elapsed().as_secs_f64())
}
