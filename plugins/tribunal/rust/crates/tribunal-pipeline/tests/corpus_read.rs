//! `tribunal corpus read` with a scripted agent that charges a set cost per
//! dispatch, on a git repository built here and candidate files written here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use serde_json::{json, Value};
use tribunal_agent::{Agent, CliResult, DispatchRequest, DispatchResult, Exit};
use tribunal_context::corpus::schema::violations;
use tribunal_context::exec::Tools;
use tribunal_pipeline::corpus_read::{prompt, read, ReadArgs};

/// Answers by label; an unknown label fails like a crashed child.
struct ScriptedAgent {
    answers: HashMap<String, Value>,
    cost: f64,
    /// How long each dispatch takes, so dispatches overlap.
    delay_ms: u64,
    calls: Mutex<Vec<DispatchRequest>>,
}

impl Agent for ScriptedAgent {
    async fn run(&self, req: &DispatchRequest) -> DispatchResult {
        self.calls.lock().unwrap().push(req.clone());
        tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
        let answer = self.answers.get(&req.label).cloned();
        DispatchResult {
            exit: Exit::Code(if answer.is_some() { 0 } else { 1 }),
            output: Some(CliResult {
                subtype: "success".into(),
                is_error: answer.is_none(),
                result: None,
                structured_output: answer,
                num_turns: 1,
                session_id: None,
                duration_ms: 0,
                total_cost_usd: self.cost,
                model_usage: Default::default(),
                permission_denials: Vec::new(),
            }),
            stderr_tail: String::new(),
            tool_calls: Vec::new(),
            transcript: None,
        }
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "dev")
        .env("GIT_AUTHOR_EMAIL", "dev@example.com")
        .env("GIT_COMMITTER_NAME", "dev")
        .env("GIT_COMMITTER_EMAIL", "dev@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tribunal-pipeline-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const LINE: &str = "    return sum(item.price for item in items)";

/// A repository with one commit, and a candidate directory with two entries:
/// pull request 7 with items a, b and c, pull request 8 with item d.
fn setup(name: &str) -> (PathBuf, PathBuf, PathBuf, String) {
    let base = scratch(name);
    let repo = base.join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(
        repo.join("src/widgets.py"),
        format!("def widget_total(items):\n{LINE}\n"),
    )
    .unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "pin"]);
    let pin = git(&repo, &["rev-parse", "HEAD"]);
    let input = base.join("candidates");
    std::fs::create_dir_all(&input).unwrap();
    let item = |id: &str| {
        json!({"id": id, "file": "src/widgets.py", "line": 2, "quote": LINE.trim(), "description": format!("Point {id}."),
            "source": "reviewer-addressed", "tier": "candidate", "seenAt": pin, "fix": pin})
    };
    for (pr, ids) in [(7, vec!["a", "b", "c"]), (8, vec!["d"])] {
        let entry = json!({"repo": "acme/widgets", "pr": pr, "reviewedSha": pin, "status": "candidate",
            "knownReal": ids.iter().map(|i| item(i)).collect::<Vec<_>>(), "skipped": []});
        std::fs::write(
            input.join(format!("pr-acme-widgets-{pr}.json")),
            entry.to_string(),
        )
        .unwrap();
    }
    (base, repo, input, pin)
}

fn args(base: &Path, repo: &Path, input: &Path, max_usd: f64, concurrency: usize) -> ReadArgs {
    ReadArgs {
        input: input.to_owned(),
        checkout: repo.to_owned(),
        max_usd,
        model: "claude-sonnet-5-5".to_owned(),
        effort: "medium".to_owned(),
        concurrency,
        tmp: base.to_owned(),
        date: "2026-11-27".to_owned(),
    }
}

fn answer(verdict: &str, quote: &str) -> Value {
    json!({"verdict": verdict, "severity": "nit", "description": "The total counts items without a price.",
        "quote": quote, "reasoning": "read the function and the fix"})
}

fn agent(cost: f64, labels: &[(&str, Value)]) -> ScriptedAgent {
    ScriptedAgent {
        answers: labels
            .iter()
            .map(|(l, v)| ((*l).to_owned(), v.clone()))
            .collect(),
        cost,
        delay_ms: 0,
        calls: Mutex::new(Vec::new()),
    }
}

fn entry(input: &Path, pr: u64) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(input.join(format!("pr-acme-widgets-{pr}.json"))).unwrap(),
    )
    .unwrap()
}

fn worktrees(repo: &Path) -> usize {
    git(repo, &["worktree", "list", "--porcelain"])
        .lines()
        .filter(|l| l.starts_with("worktree "))
        .count()
}

#[tokio::test]
async fn proposals_are_written_and_nothing_else_changes() {
    let (base, repo, input, _) = setup("corpus-read-write");
    let before = worktrees(&repo);
    let a = agent(
        0.1,
        &[
            ("corpus-read:7:a", answer("defect", LINE.trim())),
            (
                "corpus-read:7:b",
                answer("minor", "return nothing at all here"),
            ),
            (
                "corpus-read:7:c",
                json!({"verdict": "not-a-defect", "description": "The fix changed a comment.", "reasoning": "r"}),
            ),
            ("corpus-read:8:d", answer("cannot-tell", LINE.trim())),
        ],
    );
    let s = read(&a, &Tools::default(), &args(&base, &repo, &input, 5.0, 2))
        .await
        .unwrap();
    assert_eq!(s["itemsRead"], 4);
    assert_eq!(s["failed"], 0);
    assert_eq!(
        s["verdicts"],
        json!({"cannot-tell": 1, "defect": 1, "minor": 1, "not-a-defect": 1})
    );
    assert_eq!(s["stoppedAtCap"], false);
    let e7 = entry(&input, 7);
    assert_eq!(e7["status"], "candidate");
    for i in 0..3 {
        assert_eq!(e7["knownReal"][i]["tier"], "candidate");
    }
    let p = |i: usize| e7["knownReal"][i]["proposal"].clone();
    assert_eq!(p(0)["verdict"], "defect");
    assert_eq!(p(0)["quoteFound"], true);
    assert_eq!(
        p(1)["quoteFound"],
        false,
        "a quote that is not a line of the file is marked"
    );
    assert_eq!(p(2)["quoteFound"], Value::Null);
    assert_eq!(p(0)["model"], "claude-sonnet-5-5");
    assert_eq!(p(0)["costUsd"], 0.1);
    assert_eq!(p(0)["date"], "2026-11-27");
    assert_eq!(violations(&e7), Vec::<String>::new());
    assert_eq!(violations(&entry(&input, 8)), Vec::<String>::new());
    assert_eq!(worktrees(&repo), before, "every worktree was removed");
    let calls = a.calls.lock().unwrap();
    assert!(calls
        .iter()
        .all(|c| c.tools == ["Read", "Bash"] && c.schema.is_some()));
    assert!(calls
        .iter()
        .all(|c| c.cwd.starts_with(&base) && c.cwd != repo));
}

#[tokio::test]
async fn the_cap_stops_new_dispatches_and_a_second_run_reads_the_rest() {
    let (base, repo, input, _) = setup("corpus-read-cap");
    let all = [
        ("corpus-read:7:a", answer("defect", LINE.trim())),
        ("corpus-read:7:b", answer("defect", LINE.trim())),
        ("corpus-read:7:c", answer("defect", LINE.trim())),
        ("corpus-read:8:d", answer("defect", LINE.trim())),
    ];
    // 0.40 a dispatch against a 1.00 cap: the first is priced at the 0.10
    // estimate, the second at the 0.40 seen, and a third would pass 1.00.
    let s = read(
        &agent(0.4, &all),
        &Tools::default(),
        &args(&base, &repo, &input, 1.0, 1),
    )
    .await
    .unwrap();
    assert_eq!(s["itemsRead"], 2);
    assert_eq!(s["itemsLeft"], 2);
    assert_eq!(s["stoppedAtCap"], true);
    let s = read(
        &agent(0.4, &all),
        &Tools::default(),
        &args(&base, &repo, &input, 1.0, 1),
    )
    .await
    .unwrap();
    assert_eq!(
        s["itemsPending"], 2,
        "items with a proposal are not read again"
    );
    assert_eq!(s["itemsRead"], 2);
    let read_all = [entry(&input, 7), entry(&input, 8)]
        .iter()
        .flat_map(|e| e["knownReal"].as_array().unwrap().clone())
        .all(|i| i.get("proposal").is_some());
    assert!(read_all);
}

#[tokio::test]
async fn a_failed_dispatch_writes_nothing_and_removes_the_worktree() {
    let (base, repo, input, _) = setup("corpus-read-fail");
    let before = worktrees(&repo);
    let a = agent(0.1, &[("corpus-read:7:a", answer("defect", LINE.trim()))]);
    let s = read(&a, &Tools::default(), &args(&base, &repo, &input, 5.0, 1))
        .await
        .unwrap();
    assert_eq!(s["itemsRead"], 1);
    assert_eq!(s["failed"], 3);
    let e7 = entry(&input, 7);
    assert!(e7["knownReal"][1].get("proposal").is_none());
    assert!(e7["knownReal"][0].get("proposal").is_some());
    assert_eq!(worktrees(&repo), before);
}

#[tokio::test]
async fn a_cap_that_is_not_positive_is_refused() {
    let (base, repo, input, _) = setup("corpus-read-zero");
    let err = read(
        &agent(0.1, &[]),
        &Tools::default(),
        &args(&base, &repo, &input, 0.0, 1),
    )
    .await
    .unwrap_err();
    assert!(err.contains("--max-usd"), "{err}");
}

#[test]
fn the_prompt_names_no_language_of_its_own() {
    let entry = json!({"repo": "R", "pr": 1, "reviewedSha": "PIN"});
    let item =
        json!({"file": "FILE", "line": 3, "seenAt": "SEEN", "fix": "FIX", "description": "TEXT"});
    let text = prompt(&entry, &item).to_lowercase();
    let words: Vec<&str> = text
        .split(|c: char| !c.is_alphanumeric() && c != '.')
        .collect();
    for banned in [
        "python",
        "go",
        "golang",
        "typescript",
        "javascript",
        "java",
        "rust",
        "ruby",
        "php",
        "kotlin",
        "swift",
        "c#",
        "django",
        "react",
        "node",
        ".py",
        ".go",
        ".ts",
        ".js",
        ".rs",
        ".java",
    ] {
        assert!(
            !words.contains(&banned) && !text.contains(&format!("{banned} ")),
            "{banned} in the prompt"
        );
    }
    for part in ["FILE:3", "PIN", "SEEN", "git show FIX", "TEXT"] {
        assert!(prompt(&entry, &item).contains(part), "{part}");
    }
}

/// Two workers, 0.05 a dispatch, a 0.16 cap. The second worker's first
/// dispatch does not fit at the 0.10 estimate while the first is running; it
/// waits, and the real cost leaves room for three dispatches in all.
#[tokio::test]
async fn a_worker_over_the_cap_waits_for_running_dispatches_instead_of_stopping() {
    let (base, repo, input, _) = setup("corpus-read-wait");
    let all = [
        ("corpus-read:7:a", answer("defect", LINE.trim())),
        ("corpus-read:7:b", answer("defect", LINE.trim())),
        ("corpus-read:7:c", answer("defect", LINE.trim())),
        ("corpus-read:8:d", answer("defect", LINE.trim())),
    ];
    let mut slow = agent(0.05, &all);
    slow.delay_ms = 200;
    let s = read(
        &slow,
        &Tools::default(),
        &args(&base, &repo, &input, 0.16, 2),
    )
    .await
    .unwrap();
    assert_eq!(s["itemsRead"], 3, "{s}");
    assert_eq!(s["stoppedAtCap"], true);
    assert_eq!(s["errors"], json!([]));
}

/// Leaves a command running in the worktree it was given, in a process group
/// of its own, as a reader's Bash call can.
#[cfg(any(target_os = "linux", target_os = "macos"))]
struct LeavesACommandBehind {
    inner: ScriptedAgent,
    left: Mutex<Vec<std::process::Child>>,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl Agent for LeavesACommandBehind {
    async fn run(&self, req: &DispatchRequest) -> DispatchResult {
        use std::os::unix::process::CommandExt;
        let child = Command::new("sleep")
            .arg("30")
            .current_dir(&req.cwd)
            .process_group(0)
            .spawn()
            .unwrap();
        self.left.lock().unwrap().push(child);
        self.inner.run(req).await
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn a_command_a_reader_left_running_dies_with_its_worktree() {
    use std::os::unix::process::ExitStatusExt;
    let (base, repo, input, _) = setup("corpus-read-reap");
    let a = LeavesACommandBehind {
        inner: agent(0.1, &[("corpus-read:8:d", answer("defect", LINE.trim()))]),
        left: Mutex::new(Vec::new()),
    };
    read(&a, &Tools::default(), &args(&base, &repo, &input, 5.0, 1))
        .await
        .unwrap();
    let left = std::mem::take(&mut *a.left.lock().unwrap());
    assert!(!left.is_empty());
    for mut child in left {
        assert!(child.wait().unwrap().signal().is_some());
    }
}
