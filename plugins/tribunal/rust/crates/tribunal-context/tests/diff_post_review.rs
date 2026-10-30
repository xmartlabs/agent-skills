//! `post-review.mjs` against `post_review`, with a fake `gh` on both sides.
//! Nothing here can reach GitHub: the script runs with `PATH` holding only
//! the fake, a `node` symlink, `/usr/bin` and `/bin`, an empty environment
//! and an empty `GH_CONFIG_DIR`, and the port runs the fake by absolute path.
//!
//! The fake records each call's arguments (the body file's temp path replaced
//! by `<BODY>`), its stdin and the body file's contents, and answers from a
//! script of exact argument lists. `{{BODY}}` in an answer is the last body
//! it was handed, JSON-escaped, so a re-fetch returns what was "posted".
//! Each case compares stdout, stderr (unless the script crashed), exit
//! status, the recorded calls, and the body files left in the temp dir.
//!
//! Nothing here runs the script outside a regeneration; the port still runs
//! the fake `gh`, which is a node script. The script's side of the generated
//! review lists is read from `tests/goldens/post-review.json`, which keeps the
//! text of the scripts and of the fake `gh` it was recorded with. The fixed
//! cases (the fixtures, the posted body, the declared difference) are read
//! from `tests/goldens/post-review-fixed.json`, one entry per test, with a
//! SHA-256 of each module the script imports and, per case, of its input and
//! of the fake `gh`; each case keeps the calls the fake recorded and the body
//! files left behind. Editing a script, the fake or a case without a
//! regeneration fails here; a script no longer on disk was retired with the
//! Workflow, so its check is skipped and the golden is final. Regenerate
//! after changing them, over the whole binary:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_post_review`.

mod posting;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use posting::*;
use proptest::prelude::*;
use serde_json::{json, Value};
use tribunal_context::exec::Tools;
use tribunal_context::operator::CliOutput;
use tribunal_context::post_review::{self, PostEnv};

const FAKE_GH: &str = r#"#!/usr/bin/env node
const fs = require("fs");
const path = require("path");
const args = process.argv.slice(2);
const stdin = fs.readFileSync(0, "utf8");
let bodyPath = null;
const norm = args.map((a, i) => {
  if (args[i - 1] === "--body-file") { bodyPath = a; return "<BODY>"; }
  if (a.startsWith("body=@")) { bodyPath = a.slice(6); return "body=@<BODY>"; }
  return a;
});
const body = bodyPath === null ? null : fs.readFileSync(bodyPath, "utf8");
const inTmp = bodyPath === null ? null
  : path.dirname(bodyPath) === process.env.FAKE_GH_TMP && /^post-review-body-\d+\.md$/.test(path.basename(bodyPath));
fs.appendFileSync(process.env.FAKE_GH_LOG, JSON.stringify({ args: norm, stdin, body, inTmp }) + "\n");
if (body !== null) fs.writeFileSync(process.env.FAKE_GH_STATE, body);
const rules = JSON.parse(fs.readFileSync(process.env.FAKE_GH_SCRIPT, "utf8"));
const key = JSON.stringify(norm);
const rule = rules.find((r) => JSON.stringify(r.args) === key);
if (!rule) {
  process.stderr.write(`fake gh: unscripted call ${key}\n`);
  process.exit(99);
}
let out = rule.stdout;
if (out.includes("{{BODY}}")) {
  const posted = fs.existsSync(process.env.FAKE_GH_STATE) ? fs.readFileSync(process.env.FAKE_GH_STATE, "utf8") : "";
  out = out.split("{{BODY}}").join(JSON.stringify(posted).slice(1, -1));
}
process.stdout.write(out);
process.stderr.write(rule.stderr || "");
process.exitCode = rule.code ?? 0;
"#;

const REPO: &str = "acme/widgets";
const PR: &str = "42";

fn rule(args: &[&str], stdout: &str, code: i32) -> Value {
    json!({"args": args, "stdout": stdout, "stderr": "", "code": code})
}

fn reviews_call() -> Vec<&'static str> {
    vec!["api", "repos/acme/widgets/pulls/42/reviews"]
}

fn post_call(flag: &'static str) -> Vec<&'static str> {
    vec![
        "pr",
        "review",
        PR,
        "--repo",
        REPO,
        "--body-file",
        "<BODY>",
        flag,
    ]
}

fn view_call() -> Vec<&'static str> {
    vec![
        "pr", "view", PR, "--repo", REPO, "--json", "url", "--jq", ".url",
    ]
}

const URL: &str = "https://github.com/acme/widgets/pull/42\n";

const POST_REVIEW: &str = "skills/review-pr/scripts/post-review.mjs";
const FIXED: &str = "post-review-fixed.json";
const REGENERATE: &str =
    "regenerate: TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_post_review";

#[derive(Debug, Clone)]
struct Case {
    name: String,
    argv: Vec<String>,
    rules: Vec<Value>,
    report: Option<String>,
    footer: Option<String>,
}

impl Case {
    fn new(name: &str, extra: &[&str], rules: Vec<Value>) -> Self {
        let mut argv: Vec<String> = [
            "--pr",
            PR,
            "--repo",
            REPO,
            "--report-file",
            "report.md",
            "--footer-file",
            "footer.txt",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        argv.extend(extra.iter().map(|s| (*s).to_owned()));
        Self {
            name: name.into(),
            argv,
            rules,
            report: Some(REPORT.into()),
            footer: Some("<sub>tribunal · tribunal-v105 · full review · 3 agents · 2025-01-02T03:04:05Z</sub>\n".into()),
        }
    }
}

const REPORT: &str = "## Review of #42\n\nOne blocker.\n\n### Draft comments (ready to paste)\n\n- src/a.ts:3 drop this\n\n### Verdict\n\nRequest changes.\n\n  \n";

/// What one side did: its output, the calls the fake recorded, and the body
/// files it left in its temp dir.
#[derive(Debug, PartialEq)]
struct Side {
    out: CliOutput,
    calls: Vec<Value>,
    left: usize,
}

struct Dirs {
    dir: PathBuf,
    fake: PathBuf,
}

/// The directory holding the fake `gh`, written once for the whole binary.
/// A case that wrote its own and ran it at once could fail to start it with
/// `ETXTBSY`: a child another thread forks meanwhile keeps the write
/// descriptor until it execs. Running it once here, retrying while busy,
/// leaves no write descriptor for a case to meet.
fn fake_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = scratch("post-review-fake");
        let gh = dir.join("gh");
        std::fs::write(&gh, FAKE_GH).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        for _ in 0..200 {
            match std::process::Command::new(&gh).output() {
                Ok(_) => return dir,
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(e) => panic!("{}: {e}", gh.display()),
            }
        }
        panic!("{} stayed busy for a second", gh.display());
    })
    .clone()
}

fn setup(root: &Path, name: &str, case: &Case) -> Dirs {
    let dir = root.join(name);
    let _ = std::fs::remove_dir_all(&dir);
    let fake = fake_dir();
    // `footer-is-a-directory` points --footer-file at this directory.
    std::fs::create_dir_all(dir.join("fake")).unwrap();
    std::fs::create_dir_all(dir.join("home")).unwrap();
    std::fs::create_dir_all(dir.join("ghconfig")).unwrap();
    std::fs::write(
        dir.join("rules.json"),
        Value::Array(case.rules.clone()).to_string(),
    )
    .unwrap();
    if let Some(r) = &case.report {
        std::fs::write(dir.join("report.md"), r).unwrap();
    }
    if let Some(f) = &case.footer {
        std::fs::write(dir.join("footer.txt"), f).unwrap();
    }
    Dirs { dir, fake }
}

fn fake_env(d: &Dirs, side: &str) -> Vec<(&'static str, String)> {
    let s = |p: PathBuf| p.to_string_lossy().into_owned();
    vec![
        ("FAKE_GH_SCRIPT", s(d.dir.join("rules.json"))),
        ("FAKE_GH_LOG", s(d.dir.join(format!("calls-{side}.jsonl")))),
        ("FAKE_GH_STATE", s(d.dir.join(format!("state-{side}")))),
        ("FAKE_GH_TMP", s(d.dir.join(format!("tmp-{side}")))),
        ("GH_CONFIG_DIR", s(d.dir.join("ghconfig"))),
        ("HOME", s(d.dir.join("home"))),
        ("TMPDIR", s(d.dir.join(format!("tmp-{side}")))),
    ]
}

fn calls(d: &Dirs, side: &str) -> Vec<Value> {
    read_opt(&d.dir.join(format!("calls-{side}.jsonl")))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn left(d: &Dirs, side: &str) -> usize {
    std::fs::read_dir(d.dir.join(format!("tmp-{side}")))
        .map(|r| {
            r.filter_map(Result::ok)
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("post-review-body-")
                })
                .count()
        })
        .unwrap_or(0)
}

fn run_js(d: &Dirs, case: &Case) -> (Side, Run) {
    std::fs::create_dir_all(d.dir.join("tmp-js")).unwrap();
    let js = node(
        &script("post-review.mjs"),
        &case.argv,
        &d.dir,
        &fake_env(d, "js"),
        &safe_path(Some(&d.fake)),
    );
    let side = js_side(&js, calls(d, "js"), left(d, "js"));
    (side, js)
}

fn js_side(js: &Run, calls: Vec<Value>, left: usize) -> Side {
    Side {
        out: CliOutput {
            stdout: js.stdout.clone(),
            stderr: js.stderr.clone(),
            code: js.code,
        },
        calls,
        left,
    }
}

fn run_rs(d: &Dirs, case: &Case) -> Side {
    std::fs::create_dir_all(d.dir.join("tmp-rs")).unwrap();
    let mut env: Vec<(OsString, OsString)> = fake_env(d, "rs")
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
    env.push(("PATH".into(), safe_path(Some(&d.fake)).into()));
    let tools = Tools {
        gh: d.fake.join("gh").to_string_lossy().into_owned(),
        git: "git".into(),
        env,
    };
    let post_env = PostEnv {
        cwd: d.dir.clone(),
        tmp_dir: d.dir.join("tmp-rs"),
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let rs = rt.block_on(post_review::run(&case.argv, &tools, &post_env));
    Side {
        out: rs,
        calls: calls(d, "rs"),
        left: left(d, "rs"),
    }
}

fn compare(case: &Case, js: &Side, rs: &Side, raw: &Run) -> Option<String> {
    let mut d = Vec::new();
    if let Some(x) = diff_cli(raw, &rs.out) {
        d.push(x);
    }
    if js.calls != rs.calls {
        d.push(format!(
            "calls\n  js {}\n  rs {}",
            Value::Array(js.calls.clone()),
            Value::Array(rs.calls.clone())
        ));
    }
    if js.left != rs.left {
        d.push(format!("body files left: js {} rs {}", js.left, rs.left));
    }
    if js.calls.iter().any(|c| c["inTmp"] == json!(false)) {
        d.push("the body file was not in TMPDIR".into());
    }
    (!d.is_empty()).then(|| {
        let answers: Vec<String> = case
            .rules
            .iter()
            .map(|r| {
                r["stdout"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(2000)
                    .collect()
            })
            .collect();
        format!(
            "{}: {:?}\nanswers {:?}\n{}",
            case.name,
            case.argv,
            answers,
            d.join("\n")
        )
    })
}

fn reviews(entries: &[Value]) -> String {
    Value::Array(entries.to_vec()).to_string()
}

fn fixtures() -> Vec<Case> {
    let ok_list = reviews(&[
        json!({"id": 7, "body": "old Draft comments"}),
        json!({"id": 99, "body": "{{BODY}}"}),
        json!({"id": 12, "body": "x"}),
    ]);
    let happy = |flag: &'static str| {
        vec![
            rule(&post_call(flag), "", 0),
            rule(&reviews_call(), &ok_list, 0),
            rule(&view_call(), &format!("  {URL}  "), 0),
        ]
    };
    let ow = |id: &'static str| {
        vec![
            "api".to_owned(),
            format!("repos/acme/widgets/pulls/42/reviews/{id}"),
        ]
    };
    let ow_rules = |id: &'static str, state: &str| {
        let get: Vec<String> = ow(id);
        let put: Vec<String> = [
            ow(id),
            vec![
                "-X".into(),
                "PUT".into(),
                "-F".into(),
                "body=@<BODY>".into(),
            ],
        ]
        .concat();
        vec![
            json!({"args": get, "stdout": state, "code": 0}),
            json!({"args": put, "stdout": "{}", "code": 0}),
            rule(
                &reviews_call(),
                &reviews(&[
                    json!({"id": 5, "body": "Draft comments"}),
                    json!({"id": id.parse::<i64>().map_or(json!(id), |n| json!(n)), "body": "{{BODY}}"}),
                ]),
                0,
            ),
            rule(&view_call(), URL, 0),
        ]
    };
    let mut cases = vec![
        Case::new("approve", &["--approve"], happy("--approve")),
        Case::new(
            "request-changes",
            &["--request-changes"],
            happy("--request-changes"),
        ),
        Case::new("comment", &["--comment"], happy("--comment")),
        Case::new(
            "approve-wins-over-comment",
            &["--comment", "--approve"],
            happy("--approve"),
        ),
        Case::new(
            "overwrite-approved",
            &["--overwrite", "77"],
            ow_rules("77", r#"{"state":"APPROVED"}"#),
        ),
        Case::new(
            "overwrite-changes",
            &["--overwrite", "77"],
            ow_rules("77", r#"{"state":"CHANGES_REQUESTED"}"#),
        ),
        Case::new(
            "overwrite-commented",
            &["--overwrite", "77", "--approve"],
            ow_rules("77", r#"{"state":"COMMENTED"}"#),
        ),
        Case::new(
            "overwrite-array-state",
            &["--overwrite", "77"],
            ow_rules("77", r#"{"state":["APPROVED"]}"#),
        ),
        Case::new(
            "overwrite-dismissed",
            &["--overwrite", "77"],
            ow_rules("77", r#"{"state":"DISMISSED"}"#),
        ),
        Case::new(
            "overwrite-no-state",
            &["--overwrite", "77"],
            ow_rules("77", "[1]"),
        ),
        Case::new(
            "overwrite-prototype-state",
            &["--overwrite", "77"],
            ow_rules("77", r#"{"state":"toString"}"#),
        ),
        Case::new(
            "overwrite-null",
            &["--overwrite", "77"],
            ow_rules("77", "null"),
        ),
        Case::new(
            "overwrite-bad-json",
            &["--overwrite", "77"],
            ow_rules("77", "{nope"),
        ),
        Case::new(
            "overwrite-flag",
            &["--approve", "--overwrite"],
            ow_rules("true", r#"{"state":"APPROVED"}"#),
        ),
        Case::new(
            "overwrite-target-missing",
            &["--overwrite", "77"],
            vec![
                json!({"args": ow("77"), "stdout": r#"{"state":"APPROVED"}"#}),
                json!({"args": ([ow("77"), vec!["-X".into(), "PUT".into(), "-F".into(), "body=@<BODY>".into()]].concat()), "stdout": ""}),
                rule(
                    &reviews_call(),
                    &reviews(&[json!({"id": 1, "body": "Draft comments"})]),
                    0,
                ),
                rule(&view_call(), URL, 0),
            ],
        ),
        Case::new(
            "leak",
            &["--approve"],
            vec![
                rule(&post_call("--approve"), "", 0),
                rule(
                    &reviews_call(),
                    &reviews(&[
                        json!({"id": 3, "body": "x"}),
                        json!({"id": 100, "body": "## Draft comments leaked"}),
                    ]),
                    0,
                ),
                rule(&view_call(), URL, 0),
            ],
        ),
        Case::new(
            "leak-string-ids",
            &["--comment"],
            vec![
                rule(&post_call("--comment"), "", 0),
                rule(
                    &reviews_call(),
                    &reviews(&[
                        json!({"id": "9", "body": "Draft comments"}),
                        json!({"id": "10", "body": "clean"}),
                    ]),
                    0,
                ),
                rule(&view_call(), URL, 0),
            ],
        ),
        Case::new(
            "reduce-skips-leading-null",
            &["--comment"],
            vec![
                rule(&post_call("--comment"), "", 0),
                rule(
                    &reviews_call(),
                    &reviews(&[Value::Null, json!({"id": 1, "body": "Draft comments"})]),
                    0,
                ),
                rule(&view_call(), URL, 0),
            ],
        ),
        Case::new(
            "reduce-hits-null",
            &["--comment"],
            vec![
                rule(&post_call("--comment"), "", 0),
                rule(
                    &reviews_call(),
                    &reviews(&[json!({"id": 1}), Value::Null]),
                    0,
                ),
                rule(&view_call(), URL, 0),
            ],
        ),
        Case::new(
            "empty-review-list",
            &["--comment"],
            vec![
                rule(&post_call("--comment"), "", 0),
                rule(&reviews_call(), "[]", 0),
                rule(&view_call(), URL, 0),
            ],
        ),
        Case::new(
            "reviews-not-an-array",
            &["--comment"],
            vec![
                rule(&post_call("--comment"), "", 0),
                rule(&reviews_call(), "{}", 0),
                rule(&view_call(), URL, 0),
            ],
        ),
        Case::new(
            "post-fails",
            &["--approve"],
            vec![
                json!({"args": post_call("--approve"), "stdout": "", "stderr": "HTTP 422\n", "code": 1}),
            ],
        ),
        Case::new(
            "refetch-unparseable",
            &["--approve"],
            vec![
                rule(&post_call("--approve"), "", 0),
                rule(&reviews_call(), "<html>", 0),
            ],
        ),
        Case::new(
            "view-fails",
            &["--approve"],
            vec![
                rule(&post_call("--approve"), "", 0),
                rule(&reviews_call(), &ok_list, 0),
                rule(&view_call(), "", 4),
            ],
        ),
        Case::new(
            "refetch-over-max-buffer",
            &["--approve"],
            vec![
                rule(&post_call("--approve"), "", 0),
                rule(&reviews_call(), &" ".repeat(1024 * 1024 + 8), 0),
            ],
        ),
        Case::new(
            "gh-stderr-passes-through",
            &["--approve"],
            vec![
                json!({"args": post_call("--approve"), "stdout": "", "stderr": "! note from gh\n", "code": 0}),
                rule(&reviews_call(), &ok_list, 0),
                rule(&view_call(), URL, 0),
            ],
        ),
    ];

    let mut usage = |name: &str, argv: &[&str], f: &dyn Fn(&mut Case)| {
        let mut c = Case::new(name, &[], vec![]);
        c.argv = argv.iter().map(|s| (*s).to_owned()).collect();
        f(&mut c);
        cases.push(c);
    };
    let keep = |_: &mut Case| {};
    usage("no-pr", &["--repo", REPO, "--approve"], &keep);
    usage("no-repo", &["--pr", PR, "--repo"], &keep);
    usage(
        "no-report",
        &["--pr", PR, "--repo", REPO, "--approve"],
        &keep,
    );
    usage(
        "no-mode",
        &["--pr", PR, "--repo", REPO, "--report-file", "report.md"],
        &keep,
    );
    usage(
        "no-footer",
        &[
            "--pr",
            PR,
            "--repo",
            REPO,
            "--report-file",
            "report.md",
            "--comment",
        ],
        &keep,
    );
    usage(
        "footer-flag",
        &[
            "--pr",
            PR,
            "--repo",
            REPO,
            "--report-file",
            "report.md",
            "--comment",
            "--footer-file",
        ],
        &keep,
    );
    usage(
        "footer-missing",
        &[
            "--pr",
            PR,
            "--repo",
            REPO,
            "--report-file",
            "report.md",
            "--comment",
            "--footer-file",
            "nope.txt",
        ],
        &keep,
    );
    usage(
        "footer-is-a-directory",
        &[
            "--pr",
            PR,
            "--repo",
            REPO,
            "--report-file",
            "report.md",
            "--comment",
            "--footer-file",
            "fake",
        ],
        &keep,
    );
    usage(
        "footer-empty",
        &[
            "--pr",
            PR,
            "--repo",
            REPO,
            "--report-file",
            "report.md",
            "--comment",
            "--footer-file",
            "footer.txt",
        ],
        &|c: &mut Case| c.footer = Some(" \n\t\u{FEFF}\n".into()),
    );
    usage(
        "report-missing",
        &[
            "--pr",
            PR,
            "--repo",
            REPO,
            "--report-file",
            "gone.md",
            "--comment",
            "--footer-file",
            "footer.txt",
        ],
        &keep,
    );
    usage(
        "report-flag",
        &[
            "--pr",
            PR,
            "--repo",
            REPO,
            "--comment",
            "--footer-file",
            "footer.txt",
            "--report-file",
        ],
        &keep,
    );

    let find = |name: &str, login: &[&str], answer: Value| {
        let mut c = Case::new(
            name,
            &[],
            vec![
                json!({"args": reviews_call(), "stdout": answer.as_str().map_or_else(|| answer.to_string(), str::to_owned)}),
            ],
        );
        c.argv = [
            vec!["--pr", PR, "--repo", REPO, "--find-review-id"],
            login.to_vec(),
        ]
        .concat()
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        c
    };
    let list = json!([
        {"id": 1, "user": {"login": "bot"}, "submitted_at": "2026-10-01T10:00:00Z"},
        {"id": 2, "user": {"login": "bot"}, "submitted_at": "2026-10-03T10:00:00Z"},
        {"id": 3, "user": {"login": "human"}, "submitted_at": "2026-10-09T10:00:00Z"},
        {"id": 4, "user": null},
        5,
    ]);
    cases.push(find("find-newest", &["bot"], list.clone()));
    cases.push(find("find-none", &["ghost"], list));
    cases.push(find(
        "find-null-id",
        &["bot"],
        json!([{"id": null, "user": {"login": "bot"}}]),
    ));
    cases.push(find(
        "find-missing-id",
        &["bot"],
        json!([{"user": {"login": "bot"}}]),
    ));
    cases.push(find(
        "find-object-id",
        &["bot"],
        json!([{"id": {"a": 1}, "user": {"login": "bot"}}]),
    ));
    cases.push(find(
        "find-flag-login",
        &[],
        json!([{"id": 8, "user": {"login": true}}]),
    ));
    cases.push(find("find-null-review", &["bot"], json!([null])));
    cases.push(find(
        "find-not-an-array",
        &["bot"],
        json!({"message": "Not Found"}),
    ));
    cases.push(find(
        "find-bad-json",
        &["bot"],
        Value::String("nope".into()),
    ));
    let mut failing = find("find-gh-fails", &["bot"], json!([]));
    failing.rules[0]["code"] = json!(1);
    cases.push(failing);
    cases
}

/// The script's side of `cases`, read from `post-review-fixed.json` under
/// `test`; regenerating runs the script on each in `root`.
fn fixed_sides(test: &str, root: &Path, cases: &[Case]) -> Vec<(Side, Run)> {
    let inputs: Vec<Value> = cases
        .iter()
        .map(|c| json!({"name": c.name, "case": sha(&format!("{c:?}")), "fakeGh": sha(FAKE_GH)}))
        .collect();
    let recorded = fixed(
        FIXED,
        test,
        REGENERATE,
        &inputs,
        || module_shas(POST_REVIEW),
        |w| stale_module_shas(w, POST_REVIEW),
        || {
            parallel(cases, |_, c| {
                let d = setup(root, &c.name, c);
                let (side, mut js) = run_js(&d, c);
                // A crash's stderr, which is not compared, names pids; masked
                // so a regeneration rewrites the same file.
                if crashed(&js) {
                    let pid = regex::Regex::new(r"(post-review-body-|pid: )\d+").unwrap();
                    js.stderr = pid.replace_all(&js.stderr, "${1}<PID>").into_owned();
                }
                portable(&js, &side, root)
            })
        },
    );
    recorded.iter().map(|v| local(v, root)).collect()
}

/// The script's recorded side of `case`, the port's run now, and the
/// script's raw run.
fn replay_both(root: &Path, case: &Case, recorded: &(Side, Run)) -> (Side, Side, Run) {
    let d = setup(root, &case.name, case);
    let (js, raw) = recorded;
    let js = Side {
        out: js.out.clone(),
        calls: js.calls.clone(),
        left: js.left,
    };
    (js, run_rs(&d, case), raw.clone())
}

#[test]
fn post_review_matches_js_on_fixtures() {
    let root = scratch("post-review-fixtures");
    let cases = fixtures();
    let sides = fixed_sides("post_review_matches_js_on_fixtures", &root, &cases);
    let seen = Mutex::new(BTreeMap::<String, usize>::new());
    let diffs = parallel(&cases, |i, c| {
        let (js, rs, raw) = replay_both(&root, c, &sides[i]);
        let kind = if raw.code == 0 && c.argv.iter().any(|a| a == "--find-review-id") {
            "found-id"
        } else if raw.code == 0 {
            "posted"
        } else if raw
            .stderr
            .contains("FATAL: posted review body still contains")
        {
            "leak"
        } else if raw.code == 2 {
            "usage"
        } else if crashed(&raw) {
            "crash"
        } else if raw.stderr.contains("cannot determine verdict banner") {
            "no-banner"
        } else {
            "refused"
        };
        let mut s = seen.lock().unwrap();
        *s.entry(kind.into()).or_default() += 1;
        if c.name == "refetch-over-max-buffer"
            && raw.stderr.contains("ENOBUFS")
            && rs.out.stderr.contains("ENOBUFS")
        {
            *s.entry("max-buffer".into()).or_default() += 1;
        }
        if js.left > 0 {
            *s.entry("body-left".into()).or_default() += 1;
        }
        if js
            .calls
            .iter()
            .any(|c| c["args"][0] == json!("api") && c["args"].as_array().unwrap().len() == 6)
        {
            *s.entry("overwrite-put".into()).or_default() += 1;
        }
        compare(c, &js, &rs, &raw)
    });
    eprintln!("{:?}", seen.lock().unwrap());
    assert_same("post-review fixtures", &diffs);
    let seen = seen.into_inner().unwrap();
    for (k, n) in [
        ("posted", 5),
        ("found-id", 3),
        ("leak", 3),
        ("usage", 8),
        ("crash", 8),
        ("no-banner", 2),
        ("refused", 2),
        ("body-left", 3),
        ("overwrite-put", 4),
        ("max-buffer", 1),
    ] {
        assert!(
            seen.get(k).copied().unwrap_or(0) >= n,
            "{k} reached too rarely: {seen:?}"
        );
    }
}

/// The posted body is exactly the banner, the stripped report and the
/// footer: checked on the call the fake recorded, not on the port's word.
#[test]
fn the_posted_body_is_banner_report_and_footer() {
    let root = scratch("post-review-body");
    let case = fixtures()
        .into_iter()
        .find(|c| c.name == "request-changes")
        .unwrap();
    let cases = [case];
    let sides = fixed_sides("the_posted_body_is_banner_report_and_footer", &root, &cases);
    let (js, rs, _) = replay_both(&root, &cases[0], &sides[0]);
    let body = js.calls[0]["body"].as_str().unwrap();
    assert_eq!(
        body,
        "### 🔴 Changes requested\n\n## Review of #42\n\nOne blocker.\n### Verdict\n\nRequest changes.\n\n<sub>tribunal · tribunal-v105 · full review · 3 agents · 2025-01-02T03:04:05Z</sub>\n"
    );
    assert_eq!(js.calls, rs.calls);
    assert_eq!(js.left, 0);
}

// ---------------------------------------------------------------------------
// generated review lists: the newest-by-author sort and the leak check's pick

fn submitted_at() -> impl Strategy<Value = Option<Value>> {
    prop_oneof![
        8 => (1u32..13, 1u32..29, 0u32..24, 0u32..60).prop_map(|(m, d, h, mi)| {
            Some(json!(format!("2026-{m:02}-{d:02}T{h:02}:{mi:02}:00Z")))
        }),
        1 => Just(None),
        1 => proptest::sample::select(vec![
            json!(null), json!(0), json!(1_800_000_000_000_i64), json!("garbage"), json!(""), json!(true),
            json!({"a": 1}), json!(9e15),
        ])
        .prop_map(Some),
    ]
}

fn review() -> impl Strategy<Value = Value> {
    prop_oneof![
        12 => (
            prop_oneof![
                6 => (1i64..60).prop_map(|n| json!(n)),
                1 => (1i64..60).prop_map(|n| json!(n.to_string())),
                1 => Just(json!(null)),
            ],
            proptest::sample::select(vec!["bot", "bot", "bot", "human"]),
            submitted_at(),
            proptest::sample::select(vec!["clean", "has Draft comments", "", "{{BODY}}"]),
            any::<bool>(),
        )
            .prop_map(|(id, login, at, body, with_id)| {
                let mut m = serde_json::Map::new();
                if with_id {
                    m.insert("id".into(), id);
                }
                m.insert("user".into(), json!({"login": login}));
                if let Some(at) = at {
                    m.insert("submitted_at".into(), at);
                }
                m.insert("body".into(), json!(body));
                Value::Object(m)
            }),
        1 => proptest::sample::select(vec![json!(5), json!("x"), json!({"user": null}), json!([1])]),
        1 => Just(Value::Null),
    ]
}

/// What the script reads: post-review.mjs and what it imports.
const SCRIPTS: [&str; 2] = ["post-review.mjs", "strip-draft-comments.mjs"];
const GOLDEN: &str = "post-review.json";

/// The scripts and the fake `gh`, which decides what a call records.
fn recorded_with() -> Value {
    let texts: BTreeMap<&str, String> = SCRIPTS
        .iter()
        .map(|s| (*s, std::fs::read_to_string(script(s)).unwrap()))
        .collect();
    json!({"scripts": texts, "fakeGh": FAKE_GH})
}

/// What changed since `with` was recorded: a script still on disk whose text
/// differs (one that is gone was retired and is skipped), or the fake `gh`,
/// which this file holds and so is always checked.
fn stale_with(with: &Value) -> Vec<String> {
    let mut stale: Vec<String> = SCRIPTS
        .iter()
        .filter(|s| {
            on_disk(&format!("skills/review-pr/scripts/{s}"))
                .is_some_and(|text| with["scripts"][**s] != json!(text))
        })
        .map(|s| (*s).to_owned())
        .collect();
    if with["fakeGh"] != json!(FAKE_GH) {
        stale.push("the fake gh".into());
    }
    stale
}

/// The script's run and calls with the machine's paths replaced by
/// placeholders, so the golden reads the same wherever it was recorded.
fn portable(js: &Run, side: &Side, scratch: &Path) -> Value {
    let text = json!({
        "code": js.code,
        "stdout": js.stdout,
        "stderr": js.stderr,
        "calls": side.calls,
        "left": side.left,
    })
    .to_string()
    .replace(&*scratch.to_string_lossy(), "<SCRATCH>")
    .replace(&*plugin_root().to_string_lossy(), "<PLUGIN>");
    serde_json::from_str(&text).unwrap()
}

fn local(recorded: &Value, scratch: &Path) -> (Side, Run) {
    let text = recorded
        .to_string()
        .replace("<SCRATCH>", &scratch.to_string_lossy())
        .replace("<PLUGIN>", &plugin_root().to_string_lossy());
    let v: Value = serde_json::from_str(&text).unwrap();
    let js = Run {
        code: i32::try_from(v["code"].as_i64().unwrap()).unwrap(),
        stdout: v["stdout"].as_str().unwrap().to_owned(),
        stderr: v["stderr"].as_str().unwrap().to_owned(),
    };
    let calls = v["calls"].as_array().unwrap().clone();
    let left = usize::try_from(v["left"].as_u64().unwrap()).unwrap();
    (js_side(&js, calls, left), js)
}

#[test]
fn post_review_matches_js_on_generated_review_lists() {
    let root = scratch("post-review-gen");
    let lists = sample(
        CASES,
        (proptest::collection::vec(review(), 0..12), any::<bool>()),
    );
    let inputs: Vec<Value> = lists
        .iter()
        .map(|(list, find)| json!({"list": list, "find": find}))
        .collect();
    let golden = recorded(GOLDEN, REGENERATE, stale_with);
    let replay: Option<&Vec<Value>> = golden.as_ref().map(|g| {
        let cases = g["cases"].as_array().unwrap();
        assert_eq!(
            cases.len(),
            inputs.len(),
            "the generator changed; {REGENERATE}"
        );
        for (c, input) in cases.iter().zip(&inputs) {
            assert_eq!(&c["input"], input, "the generator changed; {REGENERATE}");
        }
        cases
    });
    let seen = Mutex::new(BTreeMap::<String, usize>::new());
    let runs = parallel(&lists, |i, (list, find)| {
        let answer = reviews(list);
        let c = if *find {
            let mut c = Case::new(
                &format!("gen-{i}"),
                &[],
                vec![rule(&reviews_call(), &answer, 0)],
            );
            c.argv = ["--pr", PR, "--repo", REPO, "--find-review-id", "bot"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect();
            c
        } else {
            Case::new(
                &format!("gen-{i}"),
                &["--comment"],
                vec![
                    rule(&post_call("--comment"), "", 0),
                    rule(&reviews_call(), &answer, 0),
                    rule(&view_call(), URL, 0),
                ],
            )
        };
        let d = setup(&root, &c.name, &c);
        let (js, raw) = match replay {
            Some(cases) => local(&cases[i]["js"], &root),
            None => run_js(&d, &c),
        };
        let rs = run_rs(&d, &c);
        let undated = list
            .iter()
            .filter(|r| {
                r.get("submitted_at")
                    .is_none_or(|v| !v.is_string() || v == "garbage")
            })
            .count();
        let kind = match (find, raw.code) {
            (true, 0) if undated > 0 && list.len() > 2 => "found-with-undated",
            (true, 0) => "found",
            (true, 1) if crashed(&raw) => "find-crash",
            (true, _) => "not-found",
            (false, 0) => "posted",
            (false, 1) if crashed(&raw) => "post-crash",
            (false, _) => "leak",
        };
        *seen.lock().unwrap().entry(kind.into()).or_default() += 1;
        let recorded = replay.is_none().then(|| portable(&raw, &js, &root));
        (compare(&c, &js, &rs, &raw), recorded)
    });
    if replay.is_none() {
        let cases: Vec<Value> = inputs
            .iter()
            .zip(&runs)
            .map(|(input, (_, js))| json!({"input": input, "js": js}))
            .collect();
        write_golden(GOLDEN, recorded_with(), cases);
    }
    let diffs: Vec<Option<String>> = runs.into_iter().map(|(d, _)| d).collect();
    eprintln!("{:?}", seen.lock().unwrap());
    assert_same("post-review generated", &diffs);
    let seen = seen.into_inner().unwrap();
    for k in [
        "found",
        "found-with-undated",
        "find-crash",
        "not-found",
        "posted",
        "post-crash",
        "leak",
    ] {
        assert!(
            seen.get(k).copied().unwrap_or(0) >= 10,
            "{k} reached too rarely: {seen:?}"
        );
    }
}

/// Declared difference: a date-only `submitted_at` parses in V8 and is NaN
/// in the port, so the newest review differs.
#[test]
fn a_date_only_submitted_at_is_a_declared_difference() {
    let root = scratch("post-review-witness");
    let list = json!([
        {"id": 2, "user": {"login": "bot"}, "submitted_at": "2026-10-03T00:00:00Z"},
        {"id": 1, "user": {"login": "bot"}, "submitted_at": "2026-12-01"},
    ]);
    let mut c = Case::new(
        "witness",
        &[],
        vec![rule(&reviews_call(), &list.to_string(), 0)],
    );
    c.argv = ["--pr", PR, "--repo", REPO, "--find-review-id", "bot"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let cases = [c];
    let sides = fixed_sides(
        "a_date_only_submitted_at_is_a_declared_difference",
        &root,
        &cases,
    );
    let (js, rs, _) = replay_both(&root, &cases[0], &sides[0]);
    assert_eq!(js.out.stdout, "1\n");
    assert_eq!(rs.out.stdout, "2\n");
}
