//! `tribunal panel` end to end: the review's preparation and Context phase
//! over a local git repo and a scripted `gh`, then the panel over the given
//! findings with a scripted agent. Everything is synthetic and local: the
//! repo is made here, `gh` is the compiled `tribunal-test-gh` answering only
//! the calls the run should make, and no model or network is reached.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use tribunal_agent::{FakeAgent, FakeAnswer};
use tribunal_context::exec::Tools;
use tribunal_context::operator::OperatorEnv;
use tribunal_pipeline::context::{RunArgs, Runtime};
use tribunal_pipeline::panel_only::{finding_results, panel_only, parse_findings, report};
use tribunal_pipeline::run::DispatchPolicy;

const REPO: &str = "acme/widgets";
const PR: &str = "7";

const LIMIT_BASE: &str =
    "export function underLimit(count: number, limit: number) {\n  return count < limit;\n}\n";
const LIMIT_HEAD: &str = "export function underLimit(count: number, limit: number) {\n  if (limit <= 0) return false;\n  return count <= limit;\n}\n";
const QUOTE: &str = "  if (limit <= 0) return false;";

struct World {
    root: PathBuf,
    repo: PathBuf,
    rt: Runtime,
    head: String,
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = Command::new("git")
            .args(["worktree", "prune"])
            .current_dir(&self.repo)
            .output();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `tribunal-test-gh`, a binary of `tribunal-context`, which Cargo builds
/// next to this test's `deps/` directory but names to its own tests only.
fn test_gh() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let gh = exe
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join(format!("tribunal-test-gh{}", std::env::consts::EXE_SUFFIX));
    assert!(
        gh.is_file(),
        "{} is missing: run `cargo build -p tribunal-context --bin tribunal-test-gh` \
         (with the same --target, if any), or `cargo test --workspace`",
        gh.display()
    );
    gh
}

fn call(args: &[&str], stdout: &str) -> Value {
    json!({"args": args, "stdout": stdout, "stderr": "", "code": 0})
}

/// A repo with a base and a head commit touching three files, and a `gh`
/// that serves the PR from it.
fn world(name: &str) -> World {
    let root = std::env::temp_dir().join(format!("tribunal-panel-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/widgets.git",
        ],
    );
    write(&repo.join("src/limit.ts"), LIMIT_BASE);
    write(&repo.join("src/counter.ts"), "export let count = 0;\n");
    write(&repo.join("src/index.ts"), "export * from './limit';\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "base"]);
    let base = git(&repo, &["rev-parse", "HEAD"]);
    write(&repo.join("src/limit.ts"), LIMIT_HEAD);
    write(
        &repo.join("src/counter.ts"),
        "export let count = 0;\nexport const bump = () => ++count;\n",
    );
    write(
        &repo.join("src/index.ts"),
        "export * from './limit';\nexport * from './counter';\n",
    );
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "head"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let diff = format!("{}\n", git(&repo, &["diff", &base, &head]));

    let fx = root.join("fx");
    let view = json!({
        "title": "Guard the widget limit", "body": "", "author": {"login": "alice"},
        "baseRefName": "main", "baseRefOid": base, "headRefName": "feat/limit",
        "headRefOid": head, "additions": 4, "deletions": 1, "reviews": [],
        "files": [
            {"path": "src/counter.ts", "additions": 1, "deletions": 0},
            {"path": "src/index.ts", "additions": 1, "deletions": 0},
            {"path": "src/limit.ts", "additions": 2, "deletions": 1},
        ],
    });
    let compare_json = json!({"files": [
        {"filename": "src/counter.ts", "additions": 1, "deletions": 0},
        {"filename": "src/index.ts", "additions": 1, "deletions": 0},
        {"filename": "src/limit.ts", "additions": 2, "deletions": 1},
    ]});
    write(
        &fx.join("profile.json"),
        &json!({
            "schemaVersion": 1, "project": "widgets", "repo": REPO, "reviewer": "backend",
            "baseBranch": "main", "deployBranches": [],
            "foci": {
                "correctness": "correctness and logic",
                "dataContract": "data contracts",
                "security": "security and authorization",
                "performance": "performance",
                "errorHandling": "error handling",
            },
            "highRiskSurface": "auth and payments",
            "crossRepoAnchors": [],
            "riskPaths": [{"focus": "security", "pattern": "auth/"}],
        })
        .to_string(),
    );
    let compare = format!("repos/{REPO}/compare/{base}...{head}");
    let view_fields =
        "title,body,author,baseRefName,baseRefOid,headRefName,headRefOid,files,additions,deletions,reviews";
    // The one file the vote quote check reads, at the pinned head.
    let limit_at_head = format!(
        "{}\n",
        git(&repo, &["show", &format!("{head}:src/limit.ts")])
    );
    let calls = vec![
        call(
            &[
                "pr",
                "view",
                PR,
                "--repo",
                REPO,
                "--json",
                "baseRefName",
                "--jq",
                ".baseRefName",
            ],
            "main\n",
        ),
        call(
            &["pr", "view", PR, "--repo", REPO, "--json", view_fields],
            &view.to_string(),
        ),
        call(
            &[
                "api",
                &compare,
                "-H",
                "Accept: application/vnd.github.v3.diff",
            ],
            &diff,
        ),
        call(&["api", &compare], &compare_json.to_string()),
        call(
            &[
                "api",
                "--paginate",
                "--slurp",
                &format!("repos/{REPO}/pulls/{PR}/comments"),
            ],
            "[[]]\n",
        ),
        call(
            &[
                "api",
                "--paginate",
                "--slurp",
                &format!("repos/{REPO}/issues/{PR}/comments"),
            ],
            "[[]]\n",
        ),
        call(&["api", "user", "--jq", ".login"], "dev\n"),
        call(
            &[
                "api",
                "-H",
                "Accept: application/vnd.github.raw",
                &format!("repos/{REPO}/contents/src/limit.ts?ref={head}"),
            ],
            &limit_at_head,
        ),
    ];
    let recordings = root.join("gh-recordings.json");
    std::fs::write(&recordings, json!({ "calls": calls }).to_string()).unwrap();
    let data = root.join("data");
    let tmp = root.join("tmp");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&tmp).unwrap();
    let rt = Runtime {
        plugin_root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."),
        tmpdir: tmp.to_string_lossy().into_owned(),
        tools: Tools {
            gh: test_gh().to_string_lossy().into_owned(),
            env: vec![
                (OsString::from("TRIBUNAL_TEST_GH"), OsString::new()),
                (OsString::from("GH_RECORDINGS"), recordings.into_os_string()),
                (
                    OsString::from("GH_CALL_LOG"),
                    root.join("gh.log").into_os_string(),
                ),
            ],
            ..Tools::default()
        },
        operator: OperatorEnv {
            review_harness_config: None,
            home: root.to_string_lossy().into_owned(),
            cwd: repo.clone(),
        },
        data_dir: data,
    };
    World {
        root,
        repo,
        rt,
        head,
    }
}

fn args(w: &World) -> RunArgs {
    RunArgs {
        pr: PR.into(),
        repo: REPO.into(),
        head_sha: w.head.clone(),
        review_model: "m".into(),
        profile: w
            .root
            .join("fx/profile.json")
            .to_string_lossy()
            .into_owned(),
        codex: Some(false),
        gemini: Some(false),
        ..RunArgs::default()
    }
}

/// The label `panel_phase` gives a Claude refuter of `title`.
fn refute_label(title: &str) -> String {
    format!(
        "refute:{}:claude",
        title.chars().take(20).collect::<String>()
    )
}

fn refuted() -> FakeAnswer {
    FakeAnswer::Structured(json!({
        "verdict": "REFUTED", "refutationType": "TECHNICALLY_WRONG", "confidence": 0.9,
        "reason": "the guard returns before the comparison", "source": "claude",
        "slotStatus": "OK", "introducedByPr": "yes", "proposedSeverity": "important",
        "killReason": "incorrect-premise",
        "contradiction": {"file": "src/limit.ts", "line": 2, "quote": QUOTE},
    }))
}

fn not_refuted() -> FakeAnswer {
    FakeAnswer::Structured(json!({
        "verdict": "NOT_REFUTED", "refutationType": "NO_REFUTATION", "confidence": 0.9,
        "reason": "the claim holds at head", "source": "claude", "slotStatus": "OK",
        "introducedByPr": "yes", "proposedSeverity": "nit", "coreStands": true,
    }))
}

const KILLED: &str = "Limit guard rejects a zero count";
const NIT: &str = "Counter bump is not atomic";
const OUTSIDE: &str = "Stale cache entry in the loader";

/// A claim the panel should kill, a nit a review would not panel, and a
/// claim on a file the PR does not touch.
fn findings() -> Vec<Value> {
    parse_findings(
        &json!([
            {"title": KILLED, "file": "src/limit.ts", "line": 3, "claim": "underLimit(0, 5) returns false"},
            {"title": NIT, "file": "src/counter.ts", "line": 2, "severity": "nit",
             "description": "bump reads and writes count in two steps"},
            {"title": OUTSIDE, "file": "src/loader/cache.ts", "line": 40,
             "description": "the cache keeps an entry after its widget is deleted"},
        ])
        .to_string(),
    )
    .unwrap()
}

/// The refine pass a `TECHNICALLY_WRONG` kill earns, answering that the
/// claim cannot be made stronger.
fn unstrengthenable() -> FakeAnswer {
    FakeAnswer::Structured(json!({
        "strengthened": false, "unstrengthenableReason": "the refuters quoted the guard",
        "title": KILLED, "file": "src/limit.ts", "severity": "important",
        "description": "underLimit(0, 5) returns false",
    }))
}

fn agent() -> FakeAgent {
    FakeAgent::new()
        .answer(refute_label(KILLED), refuted())
        .answer(
            format!("refine:{}", KILLED.chars().take(20).collect::<String>()),
            unstrengthenable(),
        )
        .answer(refute_label(NIT), not_refuted())
        .answer(refute_label(OUTSIDE), not_refuted())
}

fn by_title<'a>(results: &'a [Value], title: &str) -> &'a Value {
    results
        .iter()
        .find(|r| r["title"] == title)
        .unwrap_or_else(|| panic!("no result for {title}: {results:?}"))
}

#[tokio::test]
async fn every_given_finding_is_paneled_and_judged_by_its_votes() {
    let w = world("judged");
    let agent = agent();
    let mut logs = Vec::new();
    let run = panel_only(
        &agent,
        &w.rt,
        &args(&w),
        &DispatchPolicy::default(),
        findings(),
        &mut logs,
    )
    .await
    .unwrap_or_else(|e| panic!("{e}\n{logs:#?}"));
    let results = finding_results(&run);
    assert_eq!(results.len(), 3, "{results:#?}");
    for r in &results {
        assert_eq!(r["panelRan"], true, "{r:#}");
    }

    // Round 0 votes REFUTED, the lone vote is escalated and votes REFUTED
    // again; a TECHNICALLY_WRONG kill earns a refine, which cannot strengthen
    // the claim, and the re-vote kills it.
    let killed = by_title(&results, KILLED);
    assert_eq!(killed["survived"], false, "{killed:#}");
    assert_eq!(killed["killReason"], "incorrect-premise");
    assert_eq!(killed["refuteFraction"], json!(1));
    assert_eq!(killed["escalated"], true, "{killed:#}");
    assert_eq!(killed["refined"], true, "{killed:#}");
    assert_eq!(killed["refineOutcome"], "unstrengthenable");
    assert_eq!(killed["votes"].as_array().unwrap().len(), 1, "{killed:#}");
    assert_eq!(killed["votes"][0]["round"], 1);
    assert_eq!(killed["debate"]["fired"], false);

    for title in [NIT, OUTSIDE] {
        let kept = by_title(&results, title);
        assert_eq!(kept["survived"], true, "{kept:#}");
        assert_eq!(kept["killReason"], Value::Null);
        assert_eq!(kept["refuteFraction"], json!(0));
        assert_eq!(kept["votes"][0]["verdict"], "NOT_REFUTED");
    }
    assert_eq!(by_title(&results, NIT)["severity"], "nit");

    // Only the panel ran: no dedup, no completeness critic (the PR has three
    // files, over the critic's threshold), no thread harvest, no synthesis.
    let mut labels: Vec<String> = agent.calls().into_iter().map(|c| c.label).collect();
    labels.sort();
    let mut want = vec![
        refute_label(KILLED),
        refute_label(KILLED),
        refute_label(KILLED),
        refute_label(NIT),
        refute_label(OUTSIDE),
        format!("refine:{}", KILLED.chars().take(20).collect::<String>()),
    ];
    want.sort();
    assert_eq!(labels, want);
    assert!(run.verify.typecheck.is_none(), "the grounded gate ran");
    assert!(run.verify.gate.confirmed.is_empty() && run.verify.gate.refuted.is_empty());

    // The refuters read the pinned checkout, and it is gone afterwards.
    let pin = &run.review.context.head_pin;
    assert_eq!(pin["ok"], true, "{pin}\n{logs:#?}");
    let tree = PathBuf::from(pin["path"].as_str().unwrap());
    let refute = agent
        .calls()
        .into_iter()
        .find(|c| c.label.starts_with("refute:"))
        .unwrap();
    assert_eq!(refute.cwd, tree);
    assert!(!tree.exists(), "the pinned worktree was not torn down");

    // No metrics row and no inflight marker.
    let written: Vec<String> = std::fs::read_dir(&w.rt.data_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(written.is_empty(), "{written:?}");

    let out = report(&run, &args(&w), &logs);
    assert_eq!(out["pr"], 7);
    assert_eq!(out["repo"], REPO);
    assert_eq!(out["headSha"], json!(w.head));
    assert_eq!(
        out["summary"],
        json!({"given": 3, "paneled": 3, "killed": 1, "unverified": 0})
    );
    assert_eq!(out["agentCost"]["agents"], 6);
    assert_eq!(out["agentCost"]["byKind"]["refuter"]["agents"], 5);
    assert_eq!(out["agentCost"]["byKind"]["strengthen"]["agents"], 1);
    assert_eq!(
        out["voteQuoteCheck"]["ran"], true,
        "{}",
        out["voteQuoteCheck"]
    );
    assert_eq!(out["voteQuoteCheck"]["notFound"], 0);
    // Every `gh` call the run made was recorded; an unrecorded one exits 99.
    let recorded: Value =
        serde_json::from_str(&std::fs::read_to_string(w.root.join("gh-recordings.json")).unwrap())
            .unwrap();
    let recorded: Vec<&Value> = recorded["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| &c["args"])
        .collect();
    let gh_log = std::fs::read_to_string(w.root.join("gh.log")).unwrap();
    for line in gh_log.lines() {
        let args: Value = serde_json::from_str(line).unwrap();
        assert!(
            recorded.contains(&&args),
            "unrecorded gh call {line}\n{gh_log}"
        );
    }
}

/// The same findings through the review's own gates would not all reach a
/// panel: the nit fails the severity gate. This pins what the mode bypasses.
#[test]
fn a_review_would_not_panel_the_nit() {
    use tribunal_pipeline::verify::{needs_panel, panel_plan};
    let f = findings();
    assert!(!needs_panel(&f[1], false));
    let plan = panel_plan(&f, &|_| false, 40.0, 150.0);
    assert_eq!(plan.bound.len(), 2);
}
