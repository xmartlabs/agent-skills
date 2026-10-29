mod common;
use common::*;

// Differential tests for the loaders group: `load-profile.mjs`,
// `load-conventions.mjs`, `assemble-methodology.mjs` and `lib/operator.mjs`
// against their Rust ports, on the same recorded `gh`, files and git repos.
//
// Each CLI case runs the script and the port, then compares the printed line
// byte for byte, the exit code, every file either side wrote, and the `gh`
// calls each made (the shim's `GH_CALL_LOG`). Every case also names a
// substring its output must contain, so a case that silently took another
// path (an unrecorded call, say) cannot pass by both sides failing alike.
//
// The pure helpers are compared in bulk against the exported JavaScript
// functions (`batch` below): generated inputs, one node process per kind.
//
// No output here embeds a timestamp. Absolute paths in error messages are
// the scratch paths both sides were given, so they need no normalization;
// the template path `assemble-methodology.mjs` reports is the script's real
// path, so the Rust side gets the canonicalized plugin root.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use tribunal_context::assemble_methodology as am;
use tribunal_context::exec::Tools;
use tribunal_context::load_conventions as lc;
use tribunal_context::load_profile as lp;
use tribunal_context::operator::{self as op, CliOutput, OperatorEnv};
use tribunal_core::profile::js_ext::json_stringify;

// ---------------------------------------------------------------------------
// harness

struct Fx {
    dir: PathBuf,
    home: PathBuf,
    rec: PathBuf,
    js_log: PathBuf,
    rs_log: PathBuf,
}

fn fixture(name: &str, calls: Vec<Value>) -> Fx {
    let dir = scratch(&format!("loaders-{name}"));
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let rec = recordings(&dir, calls);
    Fx {
        js_log: dir.join("js-calls.log"),
        rs_log: dir.join("rs-calls.log"),
        dir,
        home,
        rec,
    }
}

fn root() -> PathBuf {
    plugin_root().canonicalize().unwrap()
}

/// `node <script> <args>` with the recorded `gh` first on PATH, `HOME` at the
/// fixture's home and `REVIEW_HARNESS_CONFIG` only when given, so neither side
/// can read the real operator settings.
fn node(
    script: &Path,
    args: &[String],
    cwd: &Path,
    fx: &Fx,
    config_env: Option<&str>,
) -> ScriptRun {
    let path = std::env::join_paths(std::iter::once(shim_dir()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let mut cmd = Command::new("node");
    cmd.arg(script)
        .args(args)
        .current_dir(cwd)
        .env("PATH", path)
        .env("GH_RECORDINGS", &fx.rec)
        .env("GH_CALL_LOG", &fx.js_log)
        .env("HOME", &fx.home)
        .env_remove("CLAUDE_PLUGIN_ROOT")
        .env_remove("REVIEW_HARNESS_CONFIG")
        .stdin(Stdio::null());
    if let Some(c) = config_env {
        cmd.env("REVIEW_HARNESS_CONFIG", c);
    }
    let out = cmd.output().expect("node must be on PATH");
    ScriptRun {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn rs_tools(fx: &Fx) -> Tools {
    let mut t = tools(&fx.rec);
    t.env
        .push((OsString::from("HOME"), fx.home.clone().into_os_string()));
    t.env.push((
        OsString::from("GH_CALL_LOG"),
        fx.rs_log.clone().into_os_string(),
    ));
    t
}

fn rs_env(fx: &Fx, config_env: Option<&str>, cwd: &Path) -> OperatorEnv {
    OperatorEnv {
        review_harness_config: config_env.map(str::to_owned),
        home: fx.home.to_string_lossy().into_owned(),
        cwd: cwd.to_path_buf(),
    }
}

fn read_opt(p: &Path) -> Option<Vec<u8>> {
    std::fs::read(p).ok()
}

/// Removes `paths` and returns what was there.
fn take(paths: &[PathBuf]) -> Vec<Option<Vec<u8>>> {
    paths
        .iter()
        .map(|p| {
            let got = if p.is_file() { read_opt(p) } else { None };
            if p.is_file() {
                std::fs::remove_file(p).unwrap();
            }
            got
        })
        .collect()
}

enum Port<'a> {
    Profile { env: OperatorEnv },
    Conventions,
    Methodology { root: &'a str },
}

/// Runs the script and the port on the same inputs and asserts they agree.
/// Returns the script's stdout.
#[allow(clippy::too_many_arguments)]
async fn same(
    label: &str,
    fx: &Fx,
    script: &Path,
    port: Port<'_>,
    args: &[&str],
    cwd: &Path,
    config_env: Option<&str>,
    written: &[PathBuf],
    expect: &str,
) -> String {
    let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    for p in [&fx.js_log, &fx.rs_log] {
        let _ = std::fs::remove_file(p);
    }
    take(written);
    let js = node(script, &args, cwd, fx, config_env);
    let js_files = take(written);
    let tools = rs_tools(fx);
    let rs: CliOutput = match port {
        Port::Profile { env } => lp::main(&tools, &env, &args).await,
        Port::Conventions => lc::main(&tools, &args).await,
        Port::Methodology { root } => am::main(root, &args),
    };
    let rs_files = take(written);
    assert_eq!(
        rs.stdout, js.stdout,
        "[{label}] stdout differs; js stderr: {}",
        js.stderr
    );
    assert_eq!(Some(rs.code), js.code, "[{label}] exit code differs");
    assert_eq!(rs_files, js_files, "[{label}] written files differ");
    let log = |p: &Path| std::fs::read_to_string(p).unwrap_or_default();
    assert_eq!(
        log(&fx.rs_log),
        log(&fx.js_log),
        "[{label}] gh calls differ"
    );
    assert!(
        js.stdout.contains(expect),
        "[{label}] the case did not reach its path: expected {expect:?} in {}{}",
        js.stdout,
        js.stderr
    );
    if !expect.contains("unrecorded") {
        assert!(
            !js.stdout.contains("unrecorded call"),
            "[{label}] {}",
            js.stdout
        );
    }
    js.stdout
}

fn script(rel: &str) -> PathBuf {
    root().join(rel)
}

const LOAD_PROFILE: &str = "skills/review-pr/scripts/load-profile.mjs";
const LOAD_CONVENTIONS: &str = "skills/review-pr/scripts/load-conventions.mjs";
const ASSEMBLE: &str = "skills/review-pr/scripts/assemble-methodology.mjs";

fn git(dir: &Path, home: &Path, args: &[&str]) {
    let st = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("HOME", home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?}");
}

/// A git checkout in `dir` whose origin is `url` (none when `None`).
fn checkout(fx: &Fx, name: &str, url: Option<&str>) -> PathBuf {
    let d = fx.dir.join(name);
    std::fs::create_dir_all(&d).unwrap();
    git(&d, &fx.home, &["init", "-q"]);
    if let Some(u) = url {
        git(&d, &fx.home, &["remote", "add", "origin", u]);
    }
    d
}

// ---------------------------------------------------------------------------
// recordings

const REPO: &str = "acme/widgets";

fn base_profile() -> Value {
    json!({
        "schemaVersion": 1,
        "project": "Widgets",
        "repo": REPO,
        "reviewer": "backend",
        "baseBranch": "main",
        "deployBranches": ["staging", "production"],
        "foci": {
            "correctness": "c sentence", "dataContract": "d sentence", "security": "s sentence",
            "performance": "p sentence", "errorHandling": "e sentence"
        },
        "highRiskSurface": "payments and auth",
        "crossRepoAnchors": [{"repo": "acme/gadgets", "ref": "main"}],
        "riskPaths": [{"focus": "security", "pattern": "auth|login"}, {"focus": "money", "pattern": "pay"}],
        "projectRules": ".claude/review-rules.md",
        "conventionFiles": ["CLAUDE.md", ".claude/rules/*.md", "api/CLAUDE.md", "docs/MISSING.md"]
    })
}

fn text_of(v: &Value) -> String {
    format!("{}\n", serde_json::to_string_pretty(v).unwrap())
}

fn without(mut v: Value, key: &str) -> Value {
    v.as_object_mut().unwrap().shift_remove(key);
    v
}

const RULES: &str = "Notes for people.\n<!-- slot: tribunal/intro -->\nYou review the widgets API.\n<!-- /slot -->\n<!-- slot: other/intro -->\nnot ours\n<!-- /slot -->\n<!-- slot: tribunal/no-such-slot -->\nx\n<!-- /slot -->\n";

fn contents_args(path: &str, ref_enc: &str) -> Vec<String> {
    vec![
        "api".into(),
        format!("repos/{REPO}/contents/{path}?ref={ref_enc}"),
        "-H".into(),
        "Accept: application/vnd.github.raw".into(),
    ]
}

fn file_at(path: &str, ref_enc: &str, text: &str) -> Value {
    json!({"args": contents_args(path, ref_enc), "stdout": text, "stderr": "", "code": 0})
}

fn file_err(path: &str, ref_enc: &str, stderr: &str, code: i32) -> Value {
    json!({"args": contents_args(path, ref_enc), "stdout": "", "stderr": stderr, "code": code})
}

fn profile_at(ref_enc: &str, text: &str) -> Value {
    file_at(".claude/review-profile.json", ref_enc, text)
}

fn profile_404(ref_enc: &str) -> Value {
    file_err(
        ".claude/review-profile.json",
        ref_enc,
        "gh: Not Found (HTTP 404)\n",
        1,
    )
}

fn rules_at(ref_enc: &str, text: &str) -> Value {
    file_at(".claude/review-rules.md", ref_enc, text)
}

fn base_ref(pr: &str, stdout: &str, stderr: &str, code: i32) -> Value {
    json!({"args": ["pr", "view", pr, "--repo", REPO, "--json", "baseRefName", "--jq", ".baseRefName"],
           "stdout": stdout, "stderr": stderr, "code": code})
}

fn default_branch(stdout: &str, stderr: &str, code: i32) -> Value {
    json!({"args": ["api", format!("repos/{REPO}"), "--jq", ".default_branch"],
           "stdout": stdout, "stderr": stderr, "code": code})
}

// ---------------------------------------------------------------------------
// load-profile.mjs

async fn profile_case(
    label: &str,
    fx: &Fx,
    args: &[&str],
    cwd: &Path,
    config_env: Option<&str>,
    written: &[PathBuf],
    expect: &str,
) -> String {
    let env = rs_env(fx, config_env, cwd);
    same(
        label,
        fx,
        &script(LOAD_PROFILE),
        Port::Profile { env },
        args,
        cwd,
        config_env,
        written,
        expect,
    )
    .await
}

#[tokio::test]
async fn load_profile_matches_js_on_the_skill_path() {
    let ptext = text_of(&base_profile());
    let bare = text_of(&without(base_profile(), "projectRules"));
    let fx = fixture(
        "lp-skill",
        vec![
            base_ref("7", "main\n", "", 0),
            base_ref("8", "feat/x y\n", "", 0),
            profile_at("main", &ptext),
            rules_at("main", RULES),
            profile_at("feat%2Fx%20y", &bare),
        ],
    );
    let save = fx.dir.join("profile-7.json");
    let save_rules = fx.dir.join("rules-7.md");
    let s = save.to_str().unwrap();
    let sr = save_rules.to_str().unwrap();
    let written = [save.clone(), save_rules.clone()];
    let plain = fx.dir.join("plain");
    std::fs::create_dir_all(&plain).unwrap();

    let out = profile_case(
        "flag repo",
        &fx,
        &["--pr", "7", "--save", s, "--save-rules", sr, "--repo", REPO],
        &plain,
        None,
        &written,
        "\"ok\":true",
    )
    .await;
    assert!(out.contains("\"repoSource\":\"flag\""));
    assert!(
        out.contains("tribunal does not force"),
        "warnings reach the line"
    );

    std::fs::create_dir_all(fx.home.join(".claude")).unwrap();
    std::fs::write(
        fx.home.join(".claude/review-harness.json"),
        "{\"defaultRepo\": \"acme/widgets\"}\n",
    )
    .unwrap();
    profile_case(
        "operator default",
        &fx,
        &["--pr", "7", "--save", s, "--save-rules", sr],
        &plain,
        None,
        &written,
        "\"repoSource\":\"operator:",
    )
    .await;

    let elsewhere = fx.dir.join("elsewhere.json");
    std::fs::write(
        &elsewhere,
        "{\"defaultRepo\": \"acme/widgets\", \"10\": 1, \"other\": [1.0]}",
    )
    .unwrap();
    profile_case(
        "REVIEW_HARNESS_CONFIG",
        &fx,
        &["--pr", "7", "--save", s],
        &plain,
        Some(elsewhere.to_str().unwrap()),
        &written,
        "elsewhere.json",
    )
    .await;
    std::fs::remove_file(fx.home.join(".claude/review-harness.json")).unwrap();

    for (i, url) in [
        "git@github.com:acme/widgets.git",
        "https://github.com/acme/widgets",
        "ssh://git@github.com/acme/widgets.git/",
    ]
    .iter()
    .enumerate()
    {
        let co = checkout(&fx, &format!("co{i}"), Some(url));
        profile_case(
            &format!("cwd origin {url}"),
            &fx,
            &["--pr", "7"],
            &co,
            None,
            &written,
            "\"repoSource\":\"cwd-origin\"",
        )
        .await;
    }

    profile_case(
        "explicit ref",
        &fx,
        &["--ref", "main", "--repo", REPO],
        &plain,
        None,
        &written,
        "\"baseRef\":\"main\"",
    )
    .await;
    let out = profile_case(
        "no projectRules, encoded ref",
        &fx,
        &["--pr", "8", "--repo", REPO, "--save", s, "--save-rules", sr],
        &plain,
        None,
        &written,
        "\"projectRules\":null",
    )
    .await;
    assert!(out.contains("no projectRules: reviewers get"));
    // A flag whose value starts with -- is no value.
    profile_case(
        "flag value --x",
        &fx,
        &["--repo", "--pr", "7"],
        &plain,
        None,
        &written,
        "no repo to review",
    )
    .await;
}

#[tokio::test]
async fn load_profile_matches_js_on_repo_resolution_failures() {
    let ptext = text_of(&base_profile());
    let fx = fixture(
        "lp-repo",
        vec![
            base_ref("7", "main\n", "", 0),
            profile_at("main", &ptext),
            rules_at("main", RULES),
        ],
    );
    let plain = fx.dir.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let cfg = fx.dir.join("cfg.json");
    let c = cfg.to_str().unwrap();

    profile_case(
        "bad --repo",
        &fx,
        &["--repo", "acme", "--pr", "7"],
        &plain,
        None,
        &[],
        "--repo must be owner/name",
    )
    .await;
    profile_case(
        "bad --repo quotes",
        &fx,
        &["--repo", "a\"b/c\\", "--pr", "7"],
        &plain,
        None,
        &[],
        "--repo must be owner/name",
    )
    .await;
    profile_case(
        "nothing",
        &fx,
        &["--pr", "7"],
        &plain,
        None,
        &[],
        "no repo to review",
    )
    .await;
    let no_origin = checkout(&fx, "no-origin", None);
    profile_case(
        "git without origin",
        &fx,
        &["--pr", "7"],
        &no_origin,
        None,
        &[],
        "no repo to review",
    )
    .await;
    let gitlab = checkout(&fx, "gitlab", Some("https://gitlab.com/acme/widgets.git"));
    profile_case(
        "origin elsewhere",
        &fx,
        &["--pr", "7"],
        &gitlab,
        None,
        &[],
        "no repo to review",
    )
    .await;

    for (label, body, expect) in [
        (
            "empty",
            "",
            "is not valid JSON: Unexpected end of JSON input",
        ),
        (
            "trailing comma",
            "{\"defaultRepo\": \"acme/widgets\",}",
            "Expected double-quoted property name",
        ),
        ("array", "[]", "must hold a JSON object"),
        ("null", "null", "must hold a JSON object"),
        (
            "bad default",
            "{\"defaultRepo\": \"widgets\"}",
            "defaultRepo must be owner/name, got \\\"widgets\\\"",
        ),
        (
            "empty default",
            "{\"defaultRepo\": \"\"}",
            "defaultRepo must be owner/name",
        ),
        ("null default", "{\"defaultRepo\": null}", "got null"),
        (
            "object default",
            "{\"defaultRepo\": {\"a\": 1}}",
            "got {\\\"a\\\":1}",
        ),
        ("no default", "{\"other\": 1}", "no repo to review"),
        ("BOM", "\u{feff}{}", "Unexpected token"),
        // String(["acme/widgets"]) passes the check, so the array is the repo.
        (
            "array default",
            "{\"defaultRepo\": [\"acme/widgets\"]}",
            "but this run reviews acme/widgets",
        ),
    ] {
        std::fs::write(&cfg, body).unwrap();
        profile_case(label, &fx, &["--pr", "7"], &plain, Some(c), &[], expect).await;
    }
    // A flag wins over a broken settings file.
    std::fs::write(&cfg, "{").unwrap();
    profile_case(
        "flag over broken config",
        &fx,
        &["--pr", "7", "--repo", REPO],
        &plain,
        Some(c),
        &[],
        "\"ok\":true",
    )
    .await;
    let as_dir = fx.dir.join("cfg-dir");
    std::fs::create_dir_all(&as_dir).unwrap();
    profile_case(
        "config is a directory",
        &fx,
        &["--pr", "7"],
        &plain,
        Some(as_dir.to_str().unwrap()),
        &[],
        "EISDIR: illegal operation on a directory, read",
    )
    .await;
    // An empty REVIEW_HARNESS_CONFIG falls back to the home path.
    profile_case(
        "empty env",
        &fx,
        &["--pr", "7"],
        &plain,
        Some(""),
        &[],
        ".claude/review-harness.json, or run",
    )
    .await;
}

#[tokio::test]
async fn load_profile_matches_js_on_fallbacks_and_gh_failures() {
    let ptext = text_of(&base_profile());
    let plain_dir = |fx: &Fx| {
        let p = fx.dir.join("plain");
        std::fs::create_dir_all(&p).unwrap();
        p
    };
    let cases: Vec<(&str, Vec<Value>, Vec<&str>, &str)> = vec![
        ("default branch has it", vec![base_ref("7", "release/1\n", "", 0), profile_404("release%2F1"), default_branch("main\n", "", 0), profile_at("main", &ptext), rules_at("main", RULES)], vec!["--pr", "7"], "repo:acme/widgets@main"),
        ("default is the base", vec![base_ref("7", "main\n", "", 0), profile_404("main"), default_branch("main\n", "", 0)], vec!["--pr", "7"], "no profile for acme/widgets at main."),
        ("nowhere", vec![base_ref("7", "dev\n", "", 0), profile_404("dev"), default_branch("main\n", "", 0), profile_404("main")], vec!["--pr", "7"], "at dev or on the default branch main"),
        ("not found any case", vec![base_ref("7", "dev\n", "", 0), file_err(".claude/review-profile.json", "dev", "NOT FOUND\n", 1), default_branch("main\n", "", 0), profile_404("main")], vec!["--pr", "7"], "or on the default branch main"),
        ("server error", vec![base_ref("7", "main\n", "", 0), file_err(".claude/review-profile.json", "main", "\n  HTTP 500: Server Error\nmore\n", 1)], vec!["--pr", "7"], "(HTTP 500: Server Error). Refusing to guess"),
        ("silent failure", vec![base_ref("7", "main\n", "", 0), file_err(".claude/review-profile.json", "main", "", 1)], vec!["--pr", "7"], "(Command failed: gh api repos/acme/widgets/contents/.claude/review-profile.json?ref=main -H Accept: application/vnd.github.raw)"),
        ("killed", vec![base_ref("7", "main\n", "", 0), file_err(".claude/review-profile.json", "main", "boom\n", 137)], vec!["--pr", "7"], "(boom)"),
        ("default branch fails", vec![base_ref("7", "dev\n", "", 0), profile_404("dev"), default_branch("", "HTTP 403: rate limited\nx", 1)], vec!["--pr", "7"], "its default branch could not be read (HTTP 403: rate limited)"),
        ("default branch silent", vec![base_ref("7", "dev\n", "", 0), profile_404("dev"), default_branch("", "", 1)], vec!["--pr", "7"], "(Command failed: gh api repos/acme/widgets --jq .default_branch)"),
        ("default branch empty", vec![base_ref("7", "dev\n", "", 0), profile_404("dev"), default_branch("  \n", "", 0)], vec!["--pr", "7"], "(gh returned no default branch)"),
        // Whitespace-only stderr is a falsy error: the script goes on with an
        // undefined default branch and reads the profile at ref=undefined.
        ("default branch blank stderr", vec![base_ref("7", "dev\n", "", 0), profile_404("dev"), default_branch("", "  \n", 1), profile_404("undefined")], vec!["--pr", "7"], "on the default branch undefined"),
        ("default branch blank stderr, found", vec![base_ref("7", "dev\n", "", 0), profile_404("dev"), default_branch("", " ", 1), profile_at("undefined", &text_of(&without(base_profile(), "projectRules")))], vec!["--pr", "7"], "\"ok\":true"),
        ("default blank, rules named", vec![base_ref("7", "dev\n", "", 0), profile_404("dev"), default_branch("", " ", 1), profile_at("undefined", &ptext)], vec!["--pr", "7"], "has no ref to read it at"),
        ("pr view fails", vec![base_ref("7", "", "GraphQL: Could not resolve to a PullRequest\n", 1)], vec!["--pr", "7"], "could not read the base branch of PR #7 in acme/widgets (GraphQL"),
        ("pr view empty", vec![base_ref("7", "\n", "", 0)], vec!["--pr", "7"], "(gh returned no base branch)"),
        ("pr view blank stderr", vec![base_ref("7", "", "\t\n", 1)], vec!["--pr", "7"], "no base branch to read acme/widgets"),
        ("no pr no ref", vec![], vec![], "pass --pr <n> or --ref <branch>"),
        ("rules 404", vec![base_ref("7", "main\n", "", 0), profile_at("main", &ptext), file_err(".claude/review-rules.md", "main", "HTTP 404\n", 1)], vec!["--pr", "7"], "which does not exist at main"),
        ("rules 500", vec![base_ref("7", "main\n", "", 0), profile_at("main", &ptext), file_err(".claude/review-rules.md", "main", "HTTP 502\n", 1)], vec!["--pr", "7"], "Refusing to review without the rules"),
        ("profile not JSON", vec![base_ref("7", "main\n", "", 0), profile_at("main", "<html>Not Found</html>")], vec!["--pr", "7"], "is not valid JSON: Unexpected token '<'"),
        ("unrecorded", vec![], vec!["--pr", "7"], "unrecorded call"),
    ];
    for (i, (label, calls, mut args, expect)) in cases.into_iter().enumerate() {
        let fx = fixture(&format!("lp-gh-{i}"), calls);
        let plain = plain_dir(&fx);
        args.extend(["--repo", REPO]);
        profile_case(label, &fx, &args, &plain, None, &[], expect).await;
    }
}

#[tokio::test]
async fn load_profile_matches_js_on_profile_files() {
    let fx = fixture("lp-files", vec![]);
    let plain = fx.dir.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let w = |name: &str, body: &str| {
        let p = fx.dir.join(name);
        std::fs::write(&p, body).unwrap();
        p.to_string_lossy().into_owned()
    };
    let bare = w(
        "bare.json",
        &text_of(&without(base_profile(), "projectRules")),
    );
    let named = w("named.json", &text_of(&base_profile()));
    let rules = w("rules.md", RULES);
    let dir = fx.dir.to_string_lossy().into_owned();
    let missing = fx.dir.join("missing.json").to_string_lossy().into_owned();
    let save = fx.dir.join("saved.json");
    let save_rules = fx.dir.join("saved-rules.md");
    let written = [save.clone(), save_rules.clone()];
    let (s, sr) = (save.to_str().unwrap(), save_rules.to_str().unwrap());

    let mut weird = base_profile();
    let obj = weird.as_object_mut().unwrap();
    obj.shift_remove("projectRules");
    let numbers = r#"{"10": "ten", "2": "two", "big": 12345678901234567890, "e": 1e21, "small": 1e-7, "neg0": -0, "f": 1.50, "huge": 1e999, "x": 1, "x": 2, "s": "é \"", "__proto__": {"a": 1}}"#;
    let weird_text = text_of(&weird).replacen(
        "\"schemaVersion\": 1",
        &format!("\"schemaVersion\": 1.0, \"extra\": {numbers}"),
        1,
    );
    let weird = w("weird.json", &weird_text);
    let encoded = w(
        "encoded.json",
        &serde_json::to_string(&text_of(&without(base_profile(), "projectRules"))).unwrap(),
    );
    let encoded_bad = w(
        "encoded-bad.json",
        &serde_json::to_string("{\"a\": tru}").unwrap(),
    );
    let encoded_twice = w(
        "encoded-twice.json",
        &serde_json::to_string(&serde_json::to_string("{}").unwrap()).unwrap(),
    );

    let cases: Vec<(&str, Vec<&str>, &str)> = vec![
        (
            "bare file",
            vec!["--profile", &bare, "--save", s, "--save-rules", sr],
            "\"profileRef\":null",
        ),
        (
            "named, no --rules",
            vec!["--profile", &named],
            "has no ref to read it at: pass --rules <path>",
        ),
        (
            "named with --rules",
            vec![
                "--profile",
                &named,
                "--rules",
                &rules,
                "--save",
                s,
                "--save-rules",
                sr,
            ],
            "\"source\":\"file:",
        ),
        (
            "bare with --rules",
            vec!["--profile", &bare, "--rules", &rules, "--save-rules", sr],
            "\"path\":null",
        ),
        (
            "--rules missing",
            vec!["--profile", &bare, "--rules", &missing],
            "could not read --rules",
        ),
        (
            "--profile missing",
            vec!["--profile", &missing],
            "ENOENT: no such file or directory, open",
        ),
        (
            "--profile is a dir",
            vec!["--profile", &dir],
            "EISDIR: illegal operation on a directory, read",
        ),
        (
            "numbers and keys",
            vec!["--profile", &weird, "--save", s],
            "\"ok\":true",
        ),
        (
            "string profile",
            vec!["--profile", &encoded, "--save", s],
            "\"ok\":true",
        ),
        (
            "string profile, bad inner",
            vec!["--profile", &encoded_bad],
            "args.profile is not valid JSON (Unexpected token '}'",
        ),
        (
            "string profile twice",
            vec!["--profile", &encoded_twice],
            "args.profile is missing",
        ),
    ];
    for (label, mut args, expect) in cases {
        args.extend(["--repo", REPO]);
        profile_case(label, &fx, &args, &plain, None, &written, expect).await;
    }

    let bad = |v: Value| text_of(&v);
    let mut other_repo = base_profile();
    other_repo["repo"] = json!("acme/gadgets");
    let mut many = base_profile();
    many["reviewer"] = json!("x");
    many["foci"]["extra"] = json!("e sentence");
    many["conventionFiles"] = json!(["a/*/b.md"]);
    many["tribunal"] = json!([]);
    let invalid: Vec<(&str, String, &str)> = vec![
        ("empty", String::new(), "Unexpected end of JSON input"),
        (
            "truncated",
            text_of(&base_profile())[..40].to_owned(),
            "is not valid JSON",
        ),
        (
            "trailing comma",
            "{\"a\": 1,}".into(),
            "Expected double-quoted property name",
        ),
        ("NaN", "NaN".into(), "\\\"NaN\\\" is not valid JSON"),
        (
            "long junk",
            format!("{{\"a\": 1, \"b\": x{}}}", "y".repeat(40)),
            "is not valid JSON",
        ),
        (
            "lines",
            "{\n  \"a\": 1\n  \"b\": 2\n}".into(),
            "(line 3 column 3)",
        ),
        ("array", "[]".into(), "args.profile is missing"),
        (
            "other repo",
            bad(other_repo),
            "the profile describes acme/gadgets, but this run reviews acme/widgets",
        ),
        (
            "many errors",
            bad(many),
            "reviewer must be one of backend, frontend",
        ),
    ];
    for (i, (label, body, expect)) in invalid.into_iter().enumerate() {
        let p = w(&format!("invalid-{i}.json"), &body);
        profile_case(
            label,
            &fx,
            &["--profile", &p, "--repo", REPO],
            &plain,
            None,
            &[],
            expect,
        )
        .await;
    }
}

#[tokio::test]
async fn load_profile_save_failures_match_js() {
    let fx = fixture(
        "lp-save",
        vec![
            base_ref("7", "main\n", "", 0),
            profile_at("main", &text_of(&base_profile())),
            rules_at("main", RULES),
        ],
    );
    let plain = fx.dir.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let nowhere = fx.dir.join("no/such/dir/p.json");
    let rules_ok = fx.dir.join("rules.md");
    let dir = fx.dir.join("a-dir");
    std::fs::create_dir_all(&dir).unwrap();
    profile_case(
        "save into a missing dir",
        &fx,
        &[
            "--pr",
            "7",
            "--repo",
            REPO,
            "--save",
            nowhere.to_str().unwrap(),
            "--save-rules",
            rules_ok.to_str().unwrap(),
        ],
        &plain,
        None,
        std::slice::from_ref(&rules_ok),
        "loaded the profile but could not write --save",
    )
    .await;
    profile_case(
        "save-rules onto a dir",
        &fx,
        &[
            "--pr",
            "7",
            "--repo",
            REPO,
            "--save-rules",
            dir.to_str().unwrap(),
        ],
        &plain,
        None,
        &[],
        "EISDIR: illegal operation on a directory, open",
    )
    .await;
}

/// `execFileSync`'s 1 MiB `maxBuffer`, at the boundary. The recorded `gh`
/// cannot serve this (a shim that exits right after a large write loses the
/// tail of it on macOS), so both sides run `node -e` printing N bytes and
/// report whether the call came back or failed with ENOBUFS.
#[tokio::test]
async fn exec_buffer_limit_matches_node() {
    let dir = scratch("loaders-enobufs");
    let probe = dir.join("probe.cjs");
    std::fs::write(
        &probe,
        r#"const { execFileSync } = require("node:child_process");
const out = [];
for (const n of [1048576, 1048577]) {
  try {
    execFileSync(process.execPath, ["-e", `process.stdout.write("a".repeat(${n}))`], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
    out.push("ok");
  } catch (e) {
    out.push(e.code || `status ${e.status}`);
  }
}
console.log(JSON.stringify(out));
"#,
    )
    .unwrap();
    let rec = recordings(&dir, vec![]);
    let js = run_script(probe.to_str().unwrap(), &[], &dir, &rec, None);
    let mut rust = Vec::new();
    for n in [1_048_576, 1_048_577] {
        let code = format!("process.stdout.write(\"a\".repeat({n}))");
        let args = ["-e", code.as_str()];
        let res = tribunal_context::exec::run("node", &args, None, &[]).await;
        rust.push(match lp::exec_file_sync(res, &args) {
            Ok(_) => "ok".to_owned(),
            Err(f) => f.message.rsplit(' ').next().unwrap_or_default().to_owned(),
        });
    }
    assert_eq!(json!(rust), last_json(&js.stdout), "{}", js.stderr);
    assert_eq!(rust, ["ok", "ENOBUFS"]);
}

/// Declared difference: a risk-path pattern that does not compile carries
/// regress's reason, not V8's. Everything before the reason agrees.
#[tokio::test]
async fn load_profile_regex_error_wording_is_a_declared_difference() {
    let fx = fixture("lp-regex", vec![]);
    let mut p = base_profile();
    p["riskPaths"] = json!([{"focus": "security", "pattern": "("}]);
    let path = fx.dir.join("p.json");
    std::fs::write(&path, text_of(&p)).unwrap();
    let args: Vec<String> = ["--profile", path.to_str().unwrap(), "--repo", REPO]
        .map(String::from)
        .to_vec();
    let js = node(&script(LOAD_PROFILE), &args, &fx.dir, &fx, None);
    let rs = lp::main(&rs_tools(&fx), &rs_env(&fx, None, &fx.dir), &args).await;
    let prefix = "riskPaths[0].pattern does not compile (Invalid regular expression: /(/i: ";
    assert!(
        js.stdout.contains(prefix) && rs.stdout.contains(prefix),
        "{}\n{}",
        js.stdout,
        rs.stdout
    );
    assert_eq!(Some(rs.code), js.code);
    assert_ne!(
        rs.stdout, js.stdout,
        "if these agree, drop the declared difference"
    );
}

// ---------------------------------------------------------------------------
// load-conventions.mjs

const ROOT_CLAUDE: &str = "# Widgets\nUse the logger.\n";
const RULE_A: &str =
    "---\npaths:\n  - \"src/api/**\"\n  - 'lib/*.ts'\n---\n# API rule\nValidate input.\n\n";
const RULE_B: &str = "---\ndescription: web\npaths: [web/**, \"{a,b}/*.css\"]\n---\nWeb rule.";
const API_CLAUDE: &str = "Scoped by directory.\n";

fn rules_listing() -> String {
    json!([
        {"name": "b.md", "type": "file"},
        {"name": "a.md", "type": "file"},
        {"name": "sub", "type": "dir"},
        {"name": "notes.txt", "type": "file"},
        {"name": "A.md", "type": "symlink"}
    ])
    .to_string()
}

fn dir_call(dir: &str, ref_enc: &str, stdout: &str, stderr: &str, code: i32) -> Value {
    json!({"args": ["api", format!("repos/{REPO}/contents/{dir}?ref={ref_enc}")],
           "stdout": stdout, "stderr": stderr, "code": code})
}

fn conventions_calls(rule_a: &str, listing: &str) -> Vec<Value> {
    vec![
        file_at("CLAUDE.md", "main", ROOT_CLAUDE),
        dir_call(".claude/rules", "main", listing, "", 0),
        file_at(".claude/rules/a.md", "main", rule_a),
        file_at(".claude/rules/b.md", "main", RULE_B),
        file_at("api/CLAUDE.md", "main", API_CLAUDE),
        file_err("docs/MISSING.md", "main", "gh: Not Found (HTTP 404)", 1),
    ]
}

const PATCH: &str = "diff --git a/src/api/x.ts b/src/api/x.ts\n--- a/src/api/x.ts\n+++ b/src/api/x.ts\n@@ -1 +1 @@\n-a\n+b\ndiff --git a/api/y.ts b/api/y.ts\n--- /dev/null\n+++ b/api/y.ts\n";

struct ConvFx {
    fx: Fx,
    profile: String,
    diff: String,
    out: PathBuf,
}

fn conv_fixture(name: &str, calls: Vec<Value>, profile: &Value, patch: &str) -> ConvFx {
    let fx = fixture(name, calls);
    let p = fx.dir.join("profile.json");
    std::fs::write(&p, text_of(profile)).unwrap();
    let d = fx.dir.join("diff.patch");
    std::fs::write(&d, patch).unwrap();
    let out = fx.dir.join("conventions.md");
    ConvFx {
        profile: p.to_string_lossy().into_owned(),
        diff: d.to_string_lossy().into_owned(),
        out,
        fx,
    }
}

async fn conv_case(label: &str, c: &ConvFx, args: &[&str], expect: &str) -> String {
    same(
        label,
        &c.fx,
        &script(LOAD_CONVENTIONS),
        Port::Conventions,
        args,
        &c.fx.dir,
        None,
        std::slice::from_ref(&c.out),
        expect,
    )
    .await
}

#[tokio::test]
async fn load_conventions_matches_js_on_the_skill_path() {
    let c = conv_fixture(
        "lc-skill",
        conventions_calls(RULE_A, &rules_listing()),
        &base_profile(),
        PATCH,
    );
    let o = c.out.to_str().unwrap();
    let full = [
        "--profile",
        c.profile.as_str(),
        "--ref",
        "main",
        "--diff",
        c.diff.as_str(),
        "--out",
        o,
    ];
    let out = conv_case(
        "skill flags",
        &c,
        &full,
        "\"missing\":[\"docs/MISSING.md\"]",
    )
    .await;
    assert!(
        out.contains("\"matched\":\"src/api/x.ts\"") && out.contains("\"matched\":\"api/y.ts\"")
    );
    assert!(out.contains("\"skipped\":[{\"path\":\".claude/rules/b.md\""));
    let mut with_repo = full.to_vec();
    with_repo.extend(["--repo", REPO]);
    conv_case("with --repo", &c, &with_repo, "\"ok\":true").await;

    let c = conv_fixture(
        "lc-skip",
        conventions_calls(RULE_A, &rules_listing()),
        &base_profile(),
        "+++ b/web/app.css\n--- a/docs/z.md\n",
    );
    let o = c.out.to_str().unwrap();
    conv_case(
        "web only",
        &c,
        &[
            "--profile",
            &c.profile,
            "--ref",
            "main",
            "--diff",
            &c.diff,
            "--out",
            o,
        ],
        "\"matched\":\"web/app.css\"",
    )
    .await;

    let c = conv_fixture(
        "lc-empty-diff",
        conventions_calls(RULE_A, &rules_listing()),
        &base_profile(),
        "",
    );
    let o = c.out.to_str().unwrap();
    conv_case(
        "nothing changed",
        &c,
        &[
            "--profile",
            &c.profile,
            "--ref",
            "main",
            "--diff",
            &c.diff,
            "--out",
            o,
        ],
        "\"skipped\":[{\"path\":\".claude/rules/a.md\"",
    )
    .await;

    let none = without(base_profile(), "conventionFiles");
    let c = conv_fixture("lc-none", vec![], &none, PATCH);
    let o = c.out.to_str().unwrap();
    conv_case(
        "no conventionFiles",
        &c,
        &["--profile", &c.profile, "--diff", &c.diff, "--out", o],
        "\"out\":null",
    )
    .await;

    let mut root_glob = base_profile();
    root_glob["conventionFiles"] = json!(["*.md", "CLAUDE.md"]);
    let listing = json!([{"name": "README.md", "type": "file"}, {"name": "CLAUDE.md", "type": "file"}, {"name": "x.txt", "type": "file"}, {"name": "Z.md", "type": "file"}, {"name": "é.md", "type": "file"}]).to_string();
    let calls = vec![
        dir_call("", "v%2F2", &listing, "", 0),
        file_at("CLAUDE.md", "v%2F2", ROOT_CLAUDE),
        file_at("README.md", "v%2F2", "readme"),
        file_at("Z.md", "v%2F2", "---\npaths: \"src/**\"\n---"),
        file_at("é.md", "v%2F2", "e"),
    ];
    let c = conv_fixture("lc-root-glob", calls, &root_glob, PATCH);
    let o = c.out.to_str().unwrap();
    conv_case(
        "glob at the root, deduped",
        &c,
        &[
            "--profile",
            &c.profile,
            "--ref",
            "v/2",
            "--diff",
            &c.diff,
            "--out",
            o,
        ],
        "\"path\":\"Z.md\"",
    )
    .await;

    let calls = vec![
        file_at("CLAUDE.md", "main", ROOT_CLAUDE),
        dir_call(".claude/rules", "main", "", "gh: Not Found (HTTP 404)\n", 1),
        file_at("api/CLAUDE.md", "main", API_CLAUDE),
        file_err("docs/MISSING.md", "main", "Not Found", 1),
    ];
    let c = conv_fixture("lc-no-rules-dir", calls, &base_profile(), PATCH);
    let o = c.out.to_str().unwrap();
    conv_case(
        "rules dir absent",
        &c,
        &[
            "--profile",
            &c.profile,
            "--ref",
            "main",
            "--diff",
            &c.diff,
            "--out",
            o,
        ],
        "\"ok\":true",
    )
    .await;

    let calls = vec![
        file_at("CLAUDE.md", "main", ROOT_CLAUDE),
        dir_call(
            ".claude/rules",
            "main",
            "{\"type\": \"file\", \"name\": \"x\"}",
            "",
            0,
        ),
        file_at("api/CLAUDE.md", "main", API_CLAUDE),
        file_err("docs/MISSING.md", "main", "Not Found", 1),
    ];
    let c = conv_fixture("lc-not-a-list", calls, &base_profile(), PATCH);
    let o = c.out.to_str().unwrap();
    conv_case(
        "listing is an object",
        &c,
        &[
            "--profile",
            &c.profile,
            "--ref",
            "main",
            "--diff",
            &c.diff,
            "--out",
            o,
        ],
        "\"ok\":true",
    )
    .await;
}

#[tokio::test]
async fn load_conventions_matches_js_on_failures() {
    let c = conv_fixture("lc-flags", vec![], &base_profile(), PATCH);
    let o = c.out.to_str().unwrap();
    let missing = c.fx.dir.join("nope").to_string_lossy().into_owned();
    let bad_profile = c.fx.dir.join("bad.json");
    std::fs::write(&bad_profile, "{\"schemaVersion\": 2}").unwrap();
    let b = bad_profile.to_str().unwrap();
    for (label, args, expect) in [
        (
            "no --profile",
            vec!["--ref", "main", "--diff", &c.diff, "--out", o],
            "--profile <file> is required",
        ),
        (
            "no --out",
            vec!["--profile", &c.profile, "--diff", &c.diff],
            "--out <file> is required",
        ),
        (
            "no --diff",
            vec!["--profile", &c.profile, "--out", o],
            "--diff <patch> is required",
        ),
        (
            "profile missing",
            vec!["--profile", &missing, "--diff", &c.diff, "--out", o],
            "could not read the profile at",
        ),
        (
            "profile invalid",
            vec!["--profile", b, "--diff", &c.diff, "--out", o],
            "schemaVersion must be 1, got 2",
        ),
        (
            "repo mismatch",
            vec![
                "--profile",
                &c.profile,
                "--repo",
                "acme/gadgets",
                "--diff",
                &c.diff,
                "--out",
                o,
            ],
            "the profile describes acme/widgets, not acme/gadgets",
        ),
        (
            "diff missing",
            vec!["--profile", &c.profile, "--diff", &missing, "--out", o],
            "could not read --diff",
        ),
        (
            "no --ref",
            vec!["--profile", &c.profile, "--diff", &c.diff, "--out", o],
            "--ref <the ref the profile was read at> is required",
        ),
    ] {
        conv_case(label, &c, &args, expect).await;
    }

    let null_entry = json!([{"name": "a.md", "type": "file"}, null]).to_string();
    let scoped_bad = "---\npaths:\n  - a\n  oops\n---\n";
    let failures: Vec<(&str, Vec<Value>, &str)> = vec![
        (
            "root file error",
            vec![file_err(
                "CLAUDE.md",
                "main",
                "HTTP 401: Bad credentials\n",
                1,
            )],
            "could not read acme/widgets:CLAUDE.md at main (HTTP 401: Bad credentials)",
        ),
        (
            "dir error",
            vec![
                file_at("CLAUDE.md", "main", ROOT_CLAUDE),
                dir_call(".claude/rules", "main", "", "HTTP 500\n", 1),
            ],
            "could not list acme/widgets:.claude/rules at main (HTTP 500)",
        ),
        (
            "dir not JSON",
            vec![
                file_at("CLAUDE.md", "main", ROOT_CLAUDE),
                dir_call(".claude/rules", "main", "[{\"name\": ", "", 0),
            ],
            "(Unexpected end of JSON input)",
        ),
        (
            "dir says Not Found",
            vec![
                file_at("CLAUDE.md", "main", ROOT_CLAUDE),
                dir_call(".claude/rules", "main", "Not Found", "", 0),
                file_at("api/CLAUDE.md", "main", API_CLAUDE),
                file_err("docs/MISSING.md", "main", "Not Found", 1),
            ],
            "\"ok\":true",
        ),
        (
            "null entry",
            vec![
                file_at("CLAUDE.md", "main", ROOT_CLAUDE),
                dir_call(".claude/rules", "main", &null_entry, "", 0),
            ],
            "Cannot read properties of null (reading 'type')",
        ),
        (
            "unreadable paths",
            vec![
                file_at("CLAUDE.md", "main", ROOT_CLAUDE),
                dir_call(".claude/rules", "main", &rules_listing(), "", 0),
                file_at(".claude/rules/a.md", "main", scoped_bad),
            ],
            "its paths: frontmatter has a line this cannot read:   oops",
        ),
        (
            "empty inline paths",
            vec![
                file_at("CLAUDE.md", "main", ROOT_CLAUDE),
                dir_call(".claude/rules", "main", &rules_listing(), "", 0),
                file_at(
                    ".claude/rules/a.md",
                    "main",
                    "---\npaths: [\"\", '']\n---\n",
                ),
            ],
            "its paths: frontmatter is empty",
        ),
        (
            "no listed path",
            vec![
                file_at("CLAUDE.md", "main", ROOT_CLAUDE),
                dir_call(".claude/rules", "main", &rules_listing(), "", 0),
                file_at(".claude/rules/a.md", "main", "---\npaths:\nx: 1\n---\n"),
            ],
            "lists no path",
        ),
    ];
    for (i, (label, calls, expect)) in failures.into_iter().enumerate() {
        let c = conv_fixture(&format!("lc-fail-{i}"), calls, &base_profile(), PATCH);
        let o = c.out.to_str().unwrap();
        conv_case(
            label,
            &c,
            &[
                "--profile",
                &c.profile,
                "--ref",
                "main",
                "--diff",
                &c.diff,
                "--out",
                o,
            ],
            expect,
        )
        .await;
    }

    let c = conv_fixture(
        "lc-out",
        conventions_calls(RULE_A, &rules_listing()),
        &base_profile(),
        PATCH,
    );
    let o = c.fx.dir.join("no/dir/out.md");
    conv_case(
        "out unwritable",
        &c,
        &[
            "--profile",
            &c.profile,
            "--ref",
            "main",
            "--diff",
            &c.diff,
            "--out",
            o.to_str().unwrap(),
        ],
        "could not write --out",
    )
    .await;
}

/// A scoped rule whose glob V8 cannot compile kills the script with a stack
/// trace and exit 1; the port prints nothing and exits 1 too.
#[tokio::test]
async fn load_conventions_crashes_where_js_crashes() {
    let rule = "---\npaths: \"{?x,y}\"\n---\n";
    let c = conv_fixture(
        "lc-crash",
        conventions_calls(rule, &rules_listing()),
        &base_profile(),
        PATCH,
    );
    let o = c.out.to_str().unwrap();
    let args: Vec<String> = [
        "--profile",
        c.profile.as_str(),
        "--ref",
        "main",
        "--diff",
        c.diff.as_str(),
        "--out",
        o,
    ]
    .map(String::from)
    .to_vec();
    let js = node(&script(LOAD_CONVENTIONS), &args, &c.fx.dir, &c.fx, None);
    let rs = lc::main(&rs_tools(&c.fx), &args).await;
    assert!(js.stderr.contains("Nothing to repeat"), "{}", js.stderr);
    assert_eq!(
        (rs.stdout.as_str(), Some(rs.code)),
        (js.stdout.as_str(), js.code)
    );
    assert_eq!(js.stdout, "");
    assert!(!c.out.exists());
}

// ---------------------------------------------------------------------------
// assemble-methodology.mjs

async fn method_case(
    label: &str,
    fx: &Fx,
    root: &Path,
    args: &[&str],
    out: &Path,
    expect: &str,
) -> String {
    let r = root.to_string_lossy().into_owned();
    same(
        label,
        fx,
        &root.join(ASSEMBLE),
        Port::Methodology { root: &r },
        args,
        &fx.dir,
        None,
        &[out.to_path_buf()],
        expect,
    )
    .await
}

#[tokio::test]
async fn assemble_methodology_matches_js() {
    let fx = fixture("am", vec![]);
    let r = root();
    let out = fx.dir.join("methodology.md");
    let o = out.to_str().unwrap();
    let w = |name: &str, body: &str| {
        let p = fx.dir.join(name);
        std::fs::write(&p, body).unwrap();
        p.to_string_lossy().into_owned()
    };
    let rules = w("rules.md", RULES);
    let sha = lp::sha256_hex(RULES);
    let empty = w("empty.md", "");
    let missing = fx.dir.join("missing.md").to_string_lossy().into_owned();
    let unclosed = w("unclosed.md", "<!-- slot: tribunal/intro -->\nx");
    let nested = w(
        "nested.md",
        "<!-- slot: tribunal/a -->\n<!-- slot: tribunal/b -->\n<!-- /slot -->",
    );
    let stray = w("stray.md", "<!-- /slot -->");
    let marker = w("marker.md", "ok\n<!--  /slot  -->");
    let twice = w("twice.md", "<!-- slot: tribunal/intro -->\na\n<!-- /slot -->\n<!-- slot: tribunal/intro -->\nb\n<!-- /slot -->");
    let crlf = w(
        "crlf.md",
        "<!-- slot: tribunal/intro -->\r\nx\r\n<!-- /slot -->\r\n",
    );
    let dir = fx.dir.to_string_lossy().into_owned();
    let cases: Vec<(&str, Vec<&str>, &str)> = vec![
        (
            "backend with rules",
            vec![
                "--kind",
                "backend",
                "--rules",
                &rules,
                "--rules-sha256",
                &sha,
                "--out",
                o,
            ],
            "\"unknown\":[\"tribunal/no-such-slot\"]",
        ),
        (
            "frontend bare",
            vec!["--kind", "frontend", "--out", o],
            "\"rulesSha256\":null",
        ),
        (
            "rules without sha",
            vec!["--kind", "frontend", "--rules", &rules, "--out", o],
            "\"filled\":[\"intro\"]",
        ),
        (
            "empty rules",
            vec!["--kind", "backend", "--rules", &empty, "--out", o],
            "\"filled\":[]",
        ),
        (
            "no --out",
            vec!["--kind", "backend"],
            "--out <file> is required",
        ),
        (
            "bad kind",
            vec!["--kind", "fullstack", "--out", o],
            "--kind must be one of backend, frontend, got \\\"fullstack\\\"",
        ),
        ("no kind", vec!["--out", o], "got \\\"\\\""),
        (
            "rules missing",
            vec!["--kind", "backend", "--rules", &missing, "--out", o],
            "could not read --rules",
        ),
        (
            "rules is a dir",
            vec!["--kind", "backend", "--rules", &dir, "--out", o],
            "EISDIR",
        ),
        (
            "sha mismatch",
            vec![
                "--kind",
                "backend",
                "--rules",
                &rules,
                "--rules-sha256",
                "abc",
                "--out",
                o,
            ],
            "not the abc load-profile.mjs recorded",
        ),
        (
            "sha without rules",
            vec!["--kind", "backend", "--rules-sha256", "abc", "--out", o],
            "--rules-sha256 given without --rules",
        ),
        (
            "unclosed",
            vec!["--kind", "backend", "--rules", &unclosed, "--out", o],
            "rules: slot \\\"tribunal/intro\\\" never closes",
        ),
        (
            "nested",
            vec!["--kind", "backend", "--rules", &nested, "--out", o],
            "rules:2: slot",
        ),
        (
            "stray close",
            vec!["--kind", "backend", "--rules", &stray, "--out", o],
            "rules:1: a slot closes that never opened",
        ),
        (
            "loose marker",
            vec!["--kind", "backend", "--rules", &marker, "--out", o],
            "rules:2: not a slot marker",
        ),
        (
            "block twice",
            vec!["--kind", "backend", "--rules", &twice, "--out", o],
            "appears twice",
        ),
        (
            "crlf",
            vec!["--kind", "backend", "--rules", &crlf, "--out", o],
            "not a slot marker",
        ),
        (
            "out unwritable",
            vec!["--kind", "backend", "--out", &dir],
            "EISDIR: illegal operation on a directory, open",
        ),
    ];
    for (label, args, expect) in cases {
        method_case(label, &fx, &r, &args, &out, expect).await;
    }
}

/// Template edge cases need a template the plugin does not ship: the script
/// is copied (byte for byte) into a scratch plugin root with its own templates.
#[tokio::test]
async fn assemble_methodology_matches_js_on_template_edges() {
    let fx = fixture("am-root", vec![]);
    let fake = fx.dir.join("plugin");
    std::fs::create_dir_all(fake.join("skills/review-pr/scripts")).unwrap();
    std::fs::create_dir_all(fake.join("methodology/templates")).unwrap();
    std::fs::copy(root().join(ASSEMBLE), fake.join(ASSEMBLE)).unwrap();
    let fake = fake.canonicalize().unwrap();
    let out = fx.dir.join("m.md");
    let o = out.to_str().unwrap();
    let rules = fx.dir.join("r.md");
    std::fs::write(&rules, RULES).unwrap();
    let rl = rules.to_str().unwrap();
    for (label, template, expect) in [
        ("plain", "intro\n<!-- slot: intro -->\ndefault\n<!-- /slot -->\nend\n", "\"filled\":[\"intro\"]"),
        ("duplicate slot", "<!-- slot: intro -->\n<!-- /slot -->\n<!-- slot: x -->\n<!-- /slot -->\n<!-- slot: intro -->\n<!-- /slot -->", "template: slot \\\"intro\\\" appears twice"),
        ("namespaced slot", "<!-- slot: tribunal/intro -->\n<!-- /slot -->", "template: a slot name carries a namespace"),
        ("unclosed", "<!-- slot: intro -->", "template: slot \\\"intro\\\" never closes"),
        ("bad marker", "x\n<!-- slot:intro -->", "template:2: not a slot marker"),
    ] {
        std::fs::write(fake.join("methodology/templates/backend.md"), template).unwrap();
        method_case(label, &fx, &fake, &["--kind", "backend", "--rules", rl, "--out", o], &out, expect).await;
    }
    method_case(
        "template missing",
        &fx,
        &fake,
        &["--kind", "frontend", "--out", o],
        &out,
        "ENOENT: no such file or directory, open",
    )
    .await;
}

// ---------------------------------------------------------------------------
// bulk comparisons against the exported JavaScript functions

const ORACLE: &str = r#"
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { createHash } from "node:crypto";
const root = process.argv[2];
const mod = (p) => import(pathToFileURL(root + "/" + p).href);
const lc = await mod("skills/review-pr/scripts/load-conventions.mjs");
const am = await mod("skills/review-pr/scripts/assemble-methodology.mjs");
const op = await mod("lib/operator.mjs");
const { kind, cases } = JSON.parse(readFileSync(0, "utf8"));
const run = (f) => { try { return { ok: f() }; } catch (e) { return { err: e.message }; } };
const fns = {
  json: (c) => run(() => JSON.stringify(JSON.parse(c))),
  glob: ([g, p]) => run(() => lc.globToRegExp(g).test(p)),
  scope: (t) => run(() => lc.scopeOf(t)),
  changed: (p) => run(() => lc.changedPaths(p)),
  slots: ([t, r]) => run(() => { const a = am.assemble(t, r); return [a.text, a.slots, a.filled, a.unknown]; }),
  remote: (u) => run(() => op.repoFromRemoteUrl(u)),
  home: (h) => run(() => { process.env.HOME = h; return op.operatorConfigPath({}); }),
  opconfig: (p) => run(() => JSON.stringify(op.readOperatorConfig(p))),
  sha: (t) => run(() => createHash("sha256").update(t).digest("hex")),
  uri: (t) => run(() => encodeURIComponent(t)),
};
process.stdout.write(JSON.stringify(cases.map(fns[kind])) + "\n");
"#;

fn batch(kind: &str, cases: &[Value]) -> Vec<Value> {
    let dir = scratch(&format!("loaders-oracle-{kind}"));
    let file = dir.join("oracle.mjs");
    std::fs::write(&file, ORACLE).unwrap();
    let rec = recordings(&dir, vec![]);
    let input = json!({"kind": kind, "cases": cases}).to_string();
    let run = run_script(
        file.to_str().unwrap(),
        &[root().to_str().unwrap()],
        &dir,
        &rec,
        Some(&input),
    );
    match last_json(&run.stdout) {
        Value::Array(a) => {
            assert_eq!(a.len(), cases.len(), "{}", run.stderr);
            a
        }
        other => panic!("oracle: {other} {}", run.stderr),
    }
}

/// `{ok}` / `{err}` from a Rust result, in the oracle's shape.
fn res<T: Into<Value>>(r: Result<T, String>) -> Value {
    match r {
        Ok(v) => json!({"ok": v.into()}),
        Err(e) => json!({"err": e}),
    }
}

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
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len())]
    }
    fn concat(&mut self, xs: &[&str], max: usize, sep: &str) -> String {
        let n = self.below(max + 1);
        (0..n).map(|_| self.pick(xs)).collect::<Vec<_>>().join(sep)
    }
}

/// Asserts the oracle and the port agree on every case, and that at least
/// `min` cases fall in each class `classes` names (a substring of the
/// oracle's answer), so a generator that misses a class cannot pass.
fn compare(kind: &str, cases: &[Value], rust: &[Value], classes: &[(&str, usize)]) {
    let js = batch(kind, cases);
    for (class, min) in classes {
        let n = js.iter().filter(|j| j.to_string().contains(class)).count();
        assert!(
            n >= *min,
            "{kind}: only {n} cases answer {class:?}, want {min}"
        );
    }
    let mut bad = Vec::new();
    for ((c, j), r) in cases.iter().zip(&js).zip(rust) {
        if j != r {
            bad.push(format!("{c}\n  js:   {j}\n  rust: {r}"));
        }
    }
    assert!(
        bad.is_empty(),
        "{kind}: {} of {} differ:\n{}",
        bad.len(),
        cases.len(),
        bad.iter().take(8).cloned().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn json_parse_matches_v8() {
    let frags = [
        "{",
        "}",
        "[",
        "]",
        ",",
        ":",
        "\"a\"",
        "\"",
        "\\",
        "\\u",
        "\\u00e9",
        "\\x",
        "\\n",
        "1",
        "0",
        "-",
        ".",
        "e",
        "E",
        "+",
        "true",
        "tru",
        "null",
        "nul",
        "false",
        "f",
        " ",
        "\n",
        "\r",
        "\r\n",
        "\t",
        "x",
        "é",
        "中",
        "\u{1}",
        "01",
        "1.5e3",
        "1e999",
        "-0",
        "NaN",
        "undefined",
        "Infinity",
        "[object Object]",
        "aaaaaaaaaaaa",
        "\"k\":",
        "12345678901234567890",
        "\u{feff}",
        "\u{2028}",
        "\"10\":",
        "\"2\":",
    ];
    let doc = text_of(&base_profile());
    let doc_units: Vec<char> = doc.chars().collect();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    let mut cases = Vec::new();
    while cases.len() < 6000 {
        let s = if rng.below(2) == 0 {
            rng.concat(&frags, 12, "")
        } else {
            let mut d = doc_units.clone();
            for _ in 0..=rng.below(3) {
                let at = rng.below(d.len() + 1);
                match rng.below(3) {
                    0 if at < d.len() => {
                        d.remove(at);
                    }
                    1 => {
                        let f: Vec<char> = rng.pick(&frags).chars().collect();
                        d.splice(at..at, f);
                    }
                    _ if at < d.len() => {
                        d[at] = rng
                            .pick(&["x", "\"", "}", ",", " ", "0"])
                            .chars()
                            .next()
                            .unwrap()
                    }
                    _ => {}
                }
            }
            d.into_iter().collect()
        };
        // A lone surrogate escape is a declared difference (U+FFFD here).
        let lower = s.to_ascii_lowercase();
        if lower
            .match_indices("\\ud")
            .any(|(i, _)| lower[i + 3..].starts_with(|c: char| "89abcdef".contains(c)))
        {
            continue;
        }
        cases.push(Value::String(s));
    }
    for fixed in [
        "",
        " ",
        "x",
        "{",
        "{\"a\"",
        "{\"a\":",
        "{\"a\":1",
        "{\"a\":1,",
        "[1,]",
        "[1",
        "\"abc",
        "\"\\x\"",
        "1e",
        "-",
        "1.",
        "{}x",
        "tru",
        "trux",
        "tr\"",
        "tr1",
        "[nul 1]",
        "nul-",
        "\"\\",
        "\"\\u12",
        "\"\\u",
        "{\"a\":1}\r\n\r\nx",
        "{\"a\":1}\r\rx",
        "{\"a\":1}\n\rx",
        "1e+",
        "-x",
        "1.e5",
        "[01]",
        "-01",
        "{1:1}",
        "x123456789012345678901",
        "{\"aaaaaaaaaa\": x123456789}",
        "undefined",
        "NaN",
        "Infinity",
        "[object Object]",
        "NaN ",
        "\"\t\"",
        "x1111111111111111111",
        "x11111111111111111111",
        "{\"a\":1,\"a\":2}",
        "{\"__proto__\":1}",
        "-0",
        "1E400",
        "[-1e-400]",
        "\u{feff}{}",
        "{\"a\" : 1 , }",
    ] {
        cases.push(Value::String(fixed.to_owned()));
    }
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| res(op::json_parse(c.as_str().unwrap()).map(|v| json_stringify(Some(&v)))))
        .collect();
    compare(
        "json",
        &cases,
        &rust,
        &[
            ("\"ok\"", 300),
            ("Unexpected end of JSON input", 100),
            ("is not valid JSON\"", 100),
            ("\"...", 50),
            ("...\\\"", 50),
            ("Unexpected string", 10),
            ("Unexpected number", 10),
            ("after JSON at", 100),
            ("Expected property name", 20),
            ("Expected ',' or '}'", 20),
            ("Expected ',' or ']'", 20),
            ("Expected ':'", 20),
            ("double-quoted", 20),
            ("Unterminated string", 10),
            ("Bad control", 10),
            ("Bad escaped", 10),
            ("Bad Unicode", 10),
            ("No number after minus", 10),
            ("Unterminated fractional", 5),
            ("Exponent part", 10),
            ("line 2", 20),
        ],
    );
}

#[test]
fn glob_matching_matches_js() {
    let gparts = [
        "*", "**", "**/", "?", "{", "}", ",", "a", "b", ".", "/", "-", "x", "[", "]", "(", ")",
        "+", "\\", "^", "$", "|", "é", "{a,b}", "{,}", "?a", "a?", "??", "{a???}", "{?x,y}",
        "{x,??}", "{*??}", "{a?,b}", "{.?}",
    ];
    let pparts = [
        "a", "b", ".", "/", "x", "-", "ab", "{", "}", "?", "*", "[", "]", "é", "\\", "\n", "\r",
        "😀", ",", "|",
    ];
    let mut rng = Rng(0x2545f4914f6cdd1d);
    let mut cases = Vec::new();
    for _ in 0..8000 {
        let g = rng.concat(&gparts, 7, "");
        let p = rng.concat(&pparts, 7, "");
        cases.push(json!([g, p]));
    }
    for (g, p) in [
        ("src/**", "src/a/b.ts"),
        ("**/*.ts", "a.ts"),
        ("{a?}", "a"),
        ("{a??}", ""),
        ("{a???}", "a"),
        ("{*?}", "x"),
        ("{*??}", "x"),
        ("{{?}", "{"),
        ("{a{2,}", "a{2"),
        ("x{", "x{"),
    ] {
        cases.push(json!([g, p]));
    }
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| {
            let g = c[0].as_str().unwrap();
            let p = c[1].as_str().unwrap();
            res(lc::Glob::compile(g).map(|re| re.test(p)))
        })
        .collect();
    compare(
        "glob",
        &cases,
        &rust,
        &[("true", 200), ("false", 2000), ("Nothing to repeat", 50)],
    );
}

#[test]
fn scope_of_matches_js() {
    let lines = [
        "paths:",
        "paths: [src/**, 'lib/*']",
        "paths : \"web/**\"",
        "paths:[]",
        "paths: [ , ]",
        "paths: []x",
        "paths:\t[a,\"b\" ,c]",
        "paths: \"\"",
        "paths: ''",
        "paths: '\"",
        "  - src/**",
        "  - \"a b\"",
        "  -  ",
        "  -   ",
        "\t-\tx",
        "  - ",
        "  -x",
        "- top",
        "other: 1",
        "",
        "  ",
        "  # comment",
        "  - a\r",
        "paths:\r",
        "paths: [a\rb]",
        " \u{2028}- x",
        "  -\u{a0}y",
        "  - 'q'",
        "  - \"q'",
        "pathsx: 1",
        "Paths: a",
        "  oops",
        "paths:",
        "paths:\n  - a\n  bad",
        "paths: x\u{2028}",
        "  - \u{feff}",
        "  -\t\u{2028}",
        "---",
    ];
    let ends = [
        "\n---\n",
        "\n---",
        "\n---x",
        "\n--- \n",
        "",
        "\n---\nbody\n---\n",
        "\r\n---\r\n",
    ];
    let starts = [
        "---\n",
        "---\n",
        "---\n",
        "---\r\n",
        "",
        " ---\n",
        "---\n\n---\n",
    ];
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let mut cases = Vec::new();
    for _ in 0..5000 {
        let t = format!(
            "{}{}{}{}",
            rng.pick(&starts),
            rng.concat(&lines, 6, "\n"),
            rng.pick(&ends),
            rng.pick(&["", "body", "x\n"])
        );
        cases.push(Value::String(t));
    }
    let rust: Vec<Value> = cases
        .iter()
        .map(
            |c| res(lc::scope_of(c.as_str().unwrap()).map(|g| g.map_or(Value::Null, |g| json!(g)))),
        )
        .collect();
    compare(
        "scope",
        &cases,
        &rust,
        &[
            ("\"ok\":null", 300),
            ("\"ok\":[", 300),
            ("cannot read", 30),
            ("is empty", 30),
            ("lists no path", 30),
        ],
    );
}

#[test]
fn changed_paths_match_js() {
    let lines = [
        "--- a/src/x.ts",
        "+++ b/src/x.ts",
        "--- /dev/null",
        "+++ \"b/sp ace.ts\"",
        "+++ b/t\tab",
        "+++ b/crlf\r",
        "--- a/",
        "+++ c/x",
        "+++ b/x\u{2028}y",
        "diff --git a/x b/x",
        "+++ \"a/q\"\"",
        "+++  b/x",
        "---a/x",
        "+++ b/é.ts",
        "+++ \"b/\"",
        "--- b/dup",
        "+++ b/dup",
        "+++ \"b/x",
        "+++ b/\t",
        "--- a/\"q\"",
        "+++ \"a/\r\"",
    ];
    let mut rng = Rng(0x0dd_ba11_cafe_f00d);
    let cases: Vec<Value> = (0..3000)
        .map(|_| {
            let sep = if rng.below(5) == 0 { "\r\n" } else { "\n" };
            Value::String(rng.concat(&lines, 8, sep))
        })
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| json!({"ok": lc::changed_paths(c.as_str().unwrap())}))
        .collect();
    compare(
        "changed",
        &cases,
        &rust,
        &[("\"ok\":[]", 100), ("sp ace", 300), ("\"t\"", 100)],
    );
}

#[test]
fn slot_assembly_matches_js() {
    let lines = [
        "<!-- slot: a -->",
        "<!-- slot: b -->",
        "<!-- slot: tribunal/a -->",
        "<!-- slot: tribunal/b -->",
        "<!-- slot: tribunal/c -->",
        "<!-- slot: other/a -->",
        "<!-- /slot -->",
        "<!-- /slot -->",
        "text",
        "",
        "<!-- slot: A -->",
        "<!--slot",
        "<!-- /slot-->",
        "<!-- slots -->",
        "<!-- slot_x",
        "<!-- slot: a/b/c -->",
        "<!-- slot: -a -->",
        "<!-- slot: a -->\r",
        "<!--\t/ slot",
        "body line",
        "<!-- slot: x-1 -->",
        "<!-- slot: b -->",
        "<!-- slot:  a -->",
        "<!--\u{a0}slot-",
        "<!-- slot: a -->\nx\n<!-- /slot -->",
        "<!-- slot: tribunal/a -->\ny\n<!-- /slot -->",
        "<!-- slot: tribunal/b -->\n<!-- /slot -->",
    ];
    let mut rng = Rng(0xfeed_beef_1357_9bdf);
    let mut cases = Vec::new();
    for _ in 0..5000 {
        let t = rng.concat(&lines, 8, "\n");
        let r = match rng.below(3) {
            0 => Value::Null,
            _ => Value::String(rng.concat(&lines, 8, "\n")),
        };
        cases.push(json!([t, r]));
    }
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| {
            res(
                am::assemble(c[0].as_str().unwrap(), c[1].as_str(), am::NAMESPACE)
                    .map(|a| json!([a.text, a.slots, a.filled, a.unknown])),
            )
        })
        .collect();
    compare(
        "slots",
        &cases,
        &rust,
        &[
            ("\"ok\"", 300),
            ("never closes", 100),
            ("opens inside", 100),
            ("never opened", 100),
            ("not a slot marker", 100),
            ("appears twice", 5),
            ("carries a namespace", 20),
            ("tribunal/", 20),
        ],
    );
}

#[test]
fn operator_helpers_match_js() {
    let mut rng = Rng(0xabad_1dea_0bad_f00d);
    let mut urls = Vec::new();
    for _ in 0..3000 {
        let one = |rng: &mut Rng| {
            format!(
                "{}{}{}{}/{}{}",
                rng.pick(&["", "https://", "git@", "ssh://git@", "  "]),
                rng.pick(&[
                    "github.com",
                    "gitlab.com",
                    "github.comx",
                    "GITHUB.com",
                    "xgithub.com"
                ]),
                rng.pick(&[":", "/", "//", ""]),
                rng.pick(&["acme", "", "a.b", "a b", "ac/me", "a_b-c"]),
                rng.pick(&[
                    "widgets",
                    "widgets.git",
                    ".git",
                    "w.git.git",
                    "w/",
                    "w//",
                    "w/x",
                    "",
                    "é"
                ]),
                rng.pick(&["", "\n", "/", ".git/", " ", "\u{feff}"]),
            )
        };
        let u = if rng.below(4) == 0 {
            format!("{}{}", one(&mut rng), one(&mut rng))
        } else {
            one(&mut rng)
        };
        urls.push(Value::String(u));
    }
    let rust: Vec<Value> = urls
        .iter()
        .map(|u| json!({"ok": op::repo_from_remote_url(u.as_str().unwrap())}))
        .collect();
    compare(
        "remote",
        &urls,
        &rust,
        &[("\"ok\":null", 300), ("\"ok\":\"", 150)],
    );

    let homes: Vec<Value> = [
        "/home/a", "/home/a/", "/home//a", "/a/../b", "rel", "./x", "/", "//", "/x/./y/.",
        "a/../..", "/..", "../../a/", "/é/ü",
    ]
    .iter()
    .map(|h| json!(h))
    .collect();
    let rust: Vec<Value> = homes
        .iter()
        .map(|h| json!({"ok": op::operator_config_path(None, h.as_str().unwrap())}))
        .collect();
    compare("home", &homes, &rust, &[("review-harness.json", 13)]);

    let dir = scratch("loaders-opconfig");
    let bodies = [
        "{}",
        "{\"defaultRepo\":\"acme/widgets\"}",
        "{\"defaultRepo\":[\"acme/widgets\"]}",
        "{\"defaultRepo\":\"bad\"}",
        "{\"defaultRepo\":null}",
        "{\"defaultRepo\":\"\"}",
        "[]",
        "null",
        "1",
        "\"s\"",
        "",
        "{",
        "{\"x\":1,\"10\":2,\"2\":3}",
        "{\"defaultRepo\":{\"a\":1}}",
        "{\"defaultRepo\":12}",
        "{\"defaultRepo\":[[\"a/b\"]]}",
        "{\"defaultRepo\":[null,\"a/b\"]}",
        "{\"defaultRepo\":\"a/b\",\"defaultRepo\":\"c/d\"}",
        "{\"defaultRepo\":false}",
    ];
    let mut paths: Vec<Value> = bodies
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let p = dir.join(format!("c{i}.json"));
            std::fs::write(&p, b).unwrap();
            json!(p.to_string_lossy())
        })
        .collect();
    paths.push(json!(dir.join("absent.json").to_string_lossy()));
    paths.push(json!(dir.to_string_lossy()));
    paths.push(json!(""));
    let rust: Vec<Value> = paths
        .iter()
        .map(|p| {
            let c = op::read_operator_config(p.as_str().unwrap());
            let v = match c.error {
                Some(e) => json!({"error": e, "path": c.path}),
                None => {
                    json!({"config": Value::Object(c.config), "path": c.path, "present": c.present})
                }
            };
            json!({"ok": json_stringify(Some(&v))})
        })
        .collect();
    compare(
        "opconfig",
        &paths,
        &rust,
        &[
            ("present\\\":true", 3),
            ("present\\\":false", 2),
            ("must hold a JSON object", 3),
            ("defaultRepo must be", 5),
            ("is not valid JSON", 3),
        ],
    );
}

#[test]
fn hashing_and_uri_encoding_match_node() {
    let parts = [
        "", "a", "é", "😀", "/", " ", "\n", "~", "*", "'", "(", ")", "!", "%", "?", "&", "=", "#",
        "中", "\u{7f}", "\u{0}",
    ];
    let mut rng = Rng(0x5eed_5eed_5eed_5eed);
    let mut cases: Vec<Value> = (0..1500)
        .map(|_| Value::String(rng.concat(&parts, 12, "")))
        .collect();
    for n in [55, 56, 63, 64, 65, 119, 120, 1000] {
        cases.push(Value::String("x".repeat(n)));
    }
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| json!({"ok": lp::sha256_hex(c.as_str().unwrap())}))
        .collect();
    compare("sha", &cases, &rust, &[("\"ok\"", 1500)]);
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| json!({"ok": lp::encode_uri_component(c.as_str().unwrap())}))
        .collect();
    compare("uri", &cases, &rust, &[("%F0%9F%98%80", 100), ("~*", 1)]);
}

// ---------------------------------------------------------------------------
// adversarial review: edges the generated cases above do not reach

/// After `\`, V8 reports a character past Latin-1 as an unexpected token
/// (`ReportUnexpectedCharacter`), not as a bad escape; a Latin-1 one is a bad
/// escape. The generator above only puts ASCII after a backslash.
#[test]
fn json_parse_words_a_non_latin1_escape_as_v8_does() {
    let cases: Vec<Value> = [
        "\"\\€\"",
        "\"\\é\"",
        "\"\\中x\"",
        "[\"\\Ā\"]",
        "\"\\Ā",
        "{\"a\":\"\\ſ\"}",
        "{\"aaaaaaaaaaaaaaaa\":\"\\€bbbbbbbbbbbbbbbbbbb\"}",
        "{\"\\€\":1}",
        "\"\\\u{ff}\"",
        "\"\\\u{100}\"",
        "\"\\u00٣1\"",
        "\"\\uABCx\"",
    ]
    .iter()
    .map(|s| json!(s))
    .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| res(op::json_parse(c.as_str().unwrap()).map(|v| json_stringify(Some(&v)))))
        .collect();
    compare(
        "json",
        &cases,
        &rust,
        &[("Unexpected token '", 7), ("Bad escaped character", 2)],
    );
}

#[test]
fn helper_edge_cases_match_js() {
    let globs: Vec<Value> = [
        ("{**}", "a/b"),
        ("{**}", "ab"),
        ("{a**?}", "axx"),
        ("{*{?}", "x"),
        ("{a{?}", "a"),
        ("x}", "x}"),
        ("{a,b}?", "ac"),
        ("{,}", ""),
        ("**", "a\u{2028}b"),
        ("?", "😀"),
        ("??", "😀"),
        ("{😀?}", ""),
        ("*", "a\nb"),
        ("{a.?}", "a"),
        ("{a\\?}", "a"),
        ("{a$?}", "a"),
        ("{a]?}", "a"),
        ("{a(?}", "a"),
        ("{^?}", ""),
        ("{a+?}", "a"),
        ("{?}", ""),
        ("**/", ""),
        ("**/x", "x"),
        ("a**/b", "ab"),
        ("{a}{b}", "ab"),
        ("{a,b,c}{", "c{"),
    ]
    .iter()
    .map(|(g, p)| json!([g, p]))
    .collect();
    let rust: Vec<Value> = globs
        .iter()
        .map(|c| {
            res(lc::Glob::compile(c[0].as_str().unwrap()).map(|re| re.test(c[1].as_str().unwrap())))
        })
        .collect();
    compare(
        "glob",
        &globs,
        &rust,
        &[("true", 5), ("Nothing to repeat", 1)],
    );

    let scopes: Vec<Value> = [
        "---\n\n---",
        "---\n---\n",
        "---\npaths\u{2028}: a\n---",
        "---\npaths: \u{feff}[a]\u{feff}\n---",
        "---\npaths: [a,,b]\n---",
        "---\npaths: ['a',\"b\"]\n---",
        "---\npaths: 'a'b'\n---",
        "---\npaths:\n\t- x\n\u{a0}- y\n---",
        "---\npaths:\n  - x\nfoo\n  - y\n---",
        "---\npaths: [\u{2028}]\n---",
        "---\npaths:\n  -\u{2028} \n---",
        "---\npaths:\n  - \u{2028}\n---",
        "---\npaths:\n  -\u{feff}\u{feff}\n---",
        "---\nx\n---\n---\npaths: a\n---",
        "---\npaths: a\n---x\n---\n",
        "---\npaths:\n\n  - a\n---",
        "---\npaths: [ 'a' ]\n---",
        "---\npaths: \"a\n---",
        "---\npaths:  -  a\n---",
    ]
    .iter()
    .map(|s| json!(s))
    .collect();
    let rust: Vec<Value> = scopes
        .iter()
        .map(
            |c| res(lc::scope_of(c.as_str().unwrap()).map(|g| g.map_or(Value::Null, |g| json!(g)))),
        )
        .collect();
    compare(
        "scope",
        &scopes,
        &rust,
        &[("\"ok\":[", 5), ("cannot read", 1)],
    );

    let patches: Vec<Value> = [
        "+++ \"b/x\"\tfoo",
        "+++ b/\"x\"",
        "--- a/x\u{2029}",
        "+++ \"b/a\tb\"",
        "+++ b/a\t\tb",
        "+++ \"a/b/c\"",
        "+++ \"\"",
        "+++ \"a/\"\"",
        "+++ a/\u{85}",
        "--- a/x\n--- a/x\n+++ b/x\t\n+++ b/x",
    ]
    .iter()
    .map(|s| json!(s))
    .collect();
    let rust: Vec<Value> = patches
        .iter()
        .map(|c| json!({"ok": lc::changed_paths(c.as_str().unwrap())}))
        .collect();
    compare("changed", &patches, &rust, &[("\"ok\":[]", 2), ("b/c", 1)]);

    let slots: Vec<Value> = [
        ("<!--\u{feff}slot", Value::Null),
        ("<!-- slotä", Value::Null),
        ("<!-- /\u{2028}slot", Value::Null),
        (
            "<!-- slot: a -->\n<!-- /slot -->",
            json!("<!-- slot: tribunal/a -->\nX\n<!-- /slot -->\n<!-- slot: tribunal/a -->\n<!-- /slot -->"),
        ),
        (
            "<!-- slot: a -->\n<!-- /slot -->",
            json!("<!-- slot: other/a -->\n<!-- /slot -->\n<!-- slot: other/a -->\n<!-- /slot -->"),
        ),
        (
            "<!-- slot: a -->\n<!-- /slot -->",
            json!("<!-- slot: a -->\nY\n<!-- /slot -->"),
        ),
        (
            "<!-- slot: a -->\n<!-- /slot -->",
            json!("<!-- slot: tribunal/z -->\n<!-- /slot -->\n<!-- slot: tribunal/y -->\n<!-- /slot -->"),
        ),
        (
            "<!-- slot: 0 -->\n<!-- /slot -->",
            json!("<!-- slot: tribunal/0 -->\nq\n<!-- /slot -->"),
        ),
        ("<!--slot: a -->", Value::Null),
        ("<!-- slot:a -->", Value::Null),
        ("<!-- slot: a-->", Value::Null),
        ("x", json!("")),
        (
            "<!-- slot: a -->\nd\n<!-- /slot -->",
            json!("<!-- slot: tribunal/a -->\n<!-- /slot -->"),
        ),
    ]
    .iter()
    .map(|(t, r)| json!([t, r]))
    .collect();
    let rust: Vec<Value> = slots
        .iter()
        .map(|c| {
            res(
                am::assemble(c[0].as_str().unwrap(), c[1].as_str(), am::NAMESPACE)
                    .map(|a| json!([a.text, a.slots, a.filled, a.unknown])),
            )
        })
        .collect();
    compare(
        "slots",
        &slots,
        &rust,
        &[
            ("not a slot marker", 4),
            ("appears twice", 1),
            ("tribunal/z", 1),
        ],
    );

    let urls: Vec<Value> = [
        "github.com:a/b/github.com/c/d",
        "https://github.com//a/b",
        "github.com:a/b.git/",
        "github.com:a/.git.git",
        "\u{a0}github.com:a/b\u{3000}",
        "github.com:a/b\r",
        "github.com:a/b\u{85}",
        "github.comgithub.com/a/b",
        "github.com:a/b.git.",
        "github.com/a/b?x",
        "github.com:-/.",
        "github.com:a/b\u{2028}",
    ]
    .iter()
    .map(|s| json!(s))
    .collect();
    let rust: Vec<Value> = urls
        .iter()
        .map(|u| json!({"ok": op::repo_from_remote_url(u.as_str().unwrap())}))
        .collect();
    compare("remote", &urls, &rust, &[("\"ok\":null", 2), ("c/d", 1)]);
}

/// File entries whose `name` is not a string, under a directory: JavaScript
/// tests and sorts each by its `String()` (a missing name sorts last), then
/// reads `<dir>/<that string>`. Also entries that are not file objects.
#[tokio::test]
async fn load_conventions_matches_js_on_odd_listing_names() {
    let mut prof = base_profile();
    prof["conventionFiles"] = json!([".claude/rules/*", "x/*.md"]);
    let listing = json!([
        {"type": "file", "name": 5}, {"type": "file"}, {"type": "file", "name": null},
        {"type": "file", "name": {"a": 1}}, {"type": "file", "name": ["b", "c"]},
        {"type": "file", "name": "a"}, {"type": "file", "name": [null, 1.5e21]},
        "file", 7, [], {"type": ["file"], "name": "q"}, {"type": "file", "name": true}
    ])
    .to_string();
    let mut calls = vec![dir_call(".claude/rules", "main", &listing, "", 0)];
    for n in [
        "5",
        "undefined",
        "null",
        "[object Object]",
        "b,c",
        "a",
        ",1.5e+21",
        "true",
    ] {
        calls.push(file_err(
            &format!(".claude/rules/{n}"),
            "main",
            "gh: Not Found (HTTP 404)",
            1,
        ));
    }
    let names = ["Ab.md", "_.md", "aB.md", "\u{1F600}.md", "\u{FF5E}.md"];
    let x_listing: Vec<Value> = names
        .iter()
        .map(|n| json!({"type": "file", "name": n}))
        .collect();
    calls.push(dir_call("x", "main", &json!(x_listing).to_string(), "", 0));
    for n in names {
        calls.push(file_at(&format!("x/{n}"), "main", "x"));
    }
    let c = conv_fixture("lc-odd-names", calls, &prof, PATCH);
    let o = c.out.to_str().unwrap();
    conv_case(
        "odd names",
        &c,
        &[
            "--profile",
            &c.profile,
            "--ref",
            "main",
            "--diff",
            &c.diff,
            "--out",
            o,
        ],
        "\".claude/rules/true\",\".claude/rules/undefined\"]",
    )
    .await;
}

/// A saved profile that is a JSON string holding the profile: it validates,
/// but `profile.repo` and `profile.conventionFiles` read as undefined.
#[tokio::test]
async fn load_conventions_matches_js_on_a_string_profile() {
    let enc = Value::String(text_of(&base_profile()));
    let c = conv_fixture("lc-string-profile", vec![], &enc, PATCH);
    let o = c.out.to_str().unwrap();
    let args = [
        "--profile",
        c.profile.as_str(),
        "--ref",
        "main",
        "--diff",
        c.diff.as_str(),
        "--out",
        o,
    ];
    conv_case("string profile", &c, &args, "\"out\":null").await;
    let mut with_repo = args.to_vec();
    with_repo.extend(["--repo", REPO]);
    conv_case(
        "string profile, --repo",
        &c,
        &with_repo,
        "the profile describes undefined, not acme/widgets",
    )
    .await;
}

/// The EACCES and ELOOP wording of Node's fs errors, for a profile, a
/// settings file and a --save target. Skipped when permissions do not bind
/// (running as root).
#[tokio::test]
async fn load_profile_fs_error_wording_matches_node() {
    use std::os::unix::fs::PermissionsExt;
    let fx = fixture("lp-fs-errors", vec![]);
    let bare = text_of(&without(base_profile(), "projectRules"));
    let locked = fx.dir.join("locked.json");
    std::fs::write(&locked, &bare).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let cfg = fx.dir.join("cfg.json");
    std::fs::write(&cfg, "{}").unwrap();
    std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o000)).unwrap();
    let ro = fx.dir.join("ro");
    std::fs::create_dir_all(&ro).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    let restore = || {
        for (p, m) in [(&locked, 0o644), (&cfg, 0o644), (&ro, 0o755)] {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
        }
    };
    if std::fs::read(&locked).is_ok() {
        restore();
        return;
    }
    let ok = fx.dir.join("ok.json");
    std::fs::write(&ok, &bare).unwrap();
    let looped = fx.dir.join("loop");
    std::os::unix::fs::symlink(&looped, &looped).unwrap();
    let save = ro.join("s.json");
    profile_case(
        "EACCES --profile",
        &fx,
        &["--repo", REPO, "--profile", locked.to_str().unwrap()],
        &fx.dir,
        None,
        &[],
        "EACCES: permission denied, open",
    )
    .await;
    profile_case(
        "ELOOP --profile",
        &fx,
        &["--repo", REPO, "--profile", looped.to_str().unwrap()],
        &fx.dir,
        None,
        &[],
        "ELOOP: too many symbolic links encountered, open",
    )
    .await;
    profile_case(
        "EACCES --save",
        &fx,
        &[
            "--repo",
            REPO,
            "--profile",
            ok.to_str().unwrap(),
            "--save",
            save.to_str().unwrap(),
        ],
        &fx.dir,
        None,
        &[],
        "could not write --save",
    )
    .await;
    profile_case(
        "EACCES settings file",
        &fx,
        &["--ref", "main"],
        &fx.dir,
        Some(cfg.to_str().unwrap()),
        &[],
        "is not valid JSON: EACCES",
    )
    .await;
    restore();
}

/// Profiles at the edges of the PROFILE-LOOKUP validation, through the CLI,
/// and a profile whose JSON has a backslash before a non-Latin-1 character.
#[tokio::test]
async fn load_profile_matches_js_on_validation_edges() {
    let fx = fixture("lp-validation-edges", vec![]);
    let bp = without(base_profile(), "projectRules");
    let edit = |f: &dyn Fn(&mut Value)| {
        let mut v = bp.clone();
        f(&mut v);
        v
    };
    let variants = [
        edit(&|v| {
            v["foci"]["2"] = json!("x");
            v["foci"]["10"] = json!("c sentence");
        }),
        edit(&|v| v["crossRepoAnchors"] = json!([{"repo": ["acme/x"], "ref": 5}])),
        edit(&|v| {
            v["crossRepoAnchors"] = json!([{"repo": "acme/x", "ref": ["a", "b"]}, {"repo": [["acme/y"]], "ref": [null]}]);
        }),
        edit(&|v| v["deployBranches"] = json!(["a", 1, " b"])),
        edit(&|v| {
            v["schemaVersion"] = json!("1");
            v["reviewer"] = json!(null);
        }),
        edit(&|v| v["tribunal"] = json!([])),
        edit(&|v| {
            v["tribunal"] = json!({"inContextRiskPaths": [{"focus": "migration", "pattern": "m"}, {"focus": "perf", "pattern": "p"}]});
        }),
        edit(&|v| v["conventionFiles"] = json!(["a/*/b", "x/", "../y", "ok/*.md", 3])),
        edit(&|v| {
            v["riskPaths"] = json!([null, [], {"focus": " ", "pattern": "x"}, {"focus": "security", "pattern": "  "}, {"focus": "security", "pattern": "a{1"}]);
        }),
        edit(&|v| {
            v["foci"]["migration"] = json!("c sentence");
            v["foci"]["__proto__"] = json!("z");
        }),
        edit(&|v| {
            v["baseBranch"] = json!("-x");
            v["project"] = json!(" \u{feff} ");
            v["highRiskSurface"] = json!(5);
        }),
        edit(&|v| v["repo"] = json!("acme/widgets\n")),
        edit(&|v| {
            v["schemaVersion"] = json!(1.0);
            v["x"] = json!([1e21, -0.0, 0.1, 1e-7, 123_456_789_012_345_680_000.0]);
        }),
    ];
    for (i, v) in variants.iter().enumerate() {
        let p = fx.dir.join(format!("v{i}.json"));
        std::fs::write(&p, text_of(v)).unwrap();
        profile_case(
            &format!("validation edge {i}"),
            &fx,
            &["--repo", REPO, "--profile", p.to_str().unwrap()],
            &fx.dir,
            None,
            &[],
            "\"ok\"",
        )
        .await;
    }
    let p = fx.dir.join("escape.json");
    std::fs::write(&p, "{\"schemaVersion\": 1, \"project\": \"\\€\"}").unwrap();
    profile_case(
        "escape past Latin-1",
        &fx,
        &["--repo", REPO, "--profile", p.to_str().unwrap()],
        &fx.dir,
        None,
        &[],
        "is not valid JSON: Unexpected token '€'",
    )
    .await;
}
