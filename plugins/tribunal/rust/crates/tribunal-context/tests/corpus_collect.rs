//! `tribunal corpus collect` on git repositories built here, one in Python and
//! one in Go, with the comment listing served by the recorded `gh`.
//!
//! Pull request 7 has commits c1, c2 and c3. Three comments were made on c1
//! and four on c2. The c2 ones hold only at c2, so c2 is the pin, and each c1
//! one is skipped there for a different reason: its fix is c2 itself, its file
//! is gone, or its quoted line changed. Pull request 9 has a comment on a
//! commit that only a fetch by sha brings in. A comment whose fix does not
//! exist and one whose commit does not exist are dropped.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use tribunal_context::corpus::collect::{collect, CollectArgs, DEFAULT_MAX_COMMENTS};
use tribunal_context::corpus::schema::violations;
use tribunal_context::corpus::source::source;

const REPO: &str = "acme/widgets";

struct Lang {
    name: &'static str,
    widgets: &'static str,
    legacy: &'static str,
    gadgets: &'static str,
    /// widgets file at c0, c1, c2, c3, with the same line numbers in every
    /// language.
    v: [&'static [&'static str]; 4],
    legacy_lines: &'static [&'static str],
    gadget_lines: &'static [&'static str],
}

const PYTHON: Lang = Lang {
    name: "python",
    widgets: "src/widgets.py",
    legacy: "src/legacy.py",
    gadgets: "src/gadgets.py",
    v: [
        &["def widget_count(items):", "    return len(items)"],
        &[
            "def widget_count(items):",
            "    return len(items)",
            "def widget_total(items):",
            "    return sum(item.price for item in items)",
        ],
        &[
            "def widget_count(items):",
            "    return len(items)",
            "def widget_total(items=()):",
            "    return sum(item.price for item in items)",
            "def widget_names(items):",
            "    # names of every widget, sorted by name",
            "    return sorted(item.name for item in items)",
            "    raise ValueError(\"widgets must not be empty\")",
            "    return list(filter(None, items))",
        ],
        &[
            "def widget_count(items):",
            "    return len(items)",
            "def widget_total(items=(), currency=None):",
            "    return sum(item.price for item in items)",
            "def widget_names(items=()):",
            "    # names of every widget, sorted by name",
            "    return sorted(i.name for i in items)",
            "    return [i for i in items if i]",
        ],
    ],
    legacy_lines: &["def legacy_widget_count():", "    return 0"],
    gadget_lines: &[
        "def gadget_count(gadgets):",
        "    return len(list(gadgets))",
    ],
};

const GO: Lang = Lang {
    name: "go",
    widgets: "pkg/widgets/widgets.go",
    legacy: "pkg/widgets/legacy.go",
    gadgets: "pkg/gadgets/gadgets.go",
    v: [
        &[
            "func WidgetCount(items []Widget) int {",
            "\treturn len(items)",
        ],
        &[
            "func WidgetCount(items []Widget) int {",
            "\treturn len(items)",
            "func WidgetTotal(items []Widget) float64 {",
            "\treturn sumPrices(items, func(w Widget) bool { return true })",
        ],
        &[
            "func WidgetCount(items []Widget) int {",
            "\treturn len(items)",
            "func WidgetTotal(items []Widget, opts ...Option) float64 {",
            "\treturn sumPrices(items, func(w Widget) bool { return true })",
            "func WidgetNames(items []Widget) []string {",
            "\t// names of every widget, sorted by name",
            "\treturn sortedNames(items, strings.Compare)",
            "\tpanic(\"widgets must not be empty\")",
            "\treturn filterNil(items, keepAll)",
        ],
        &[
            "func WidgetCount(items []Widget) int {",
            "\treturn len(items)",
            "func WidgetTotal(items []Widget, opts ...Option) (float64, error) {",
            "\treturn sumPrices(items, func(w Widget) bool { return true })",
            "func WidgetNames(items []Widget, opts ...Option) []string {",
            "\t// names of every widget, sorted by name",
            "\treturn sortedNames(items, compareFold)",
            "\treturn filterNil(items, isPresent)",
        ],
    ],
    legacy_lines: &["func LegacyWidgetCount() int { return 0 }", "}"],
    gadget_lines: &[
        "func GadgetCount(g []Gadget) int {",
        "\treturn len(g) + gadgetOffset",
    ],
};

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

fn write(dir: &Path, path: &str, lines: &[&str]) {
    let p = dir.join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, lines.join("\n") + "\n").unwrap();
}

fn commit(dir: &Path, msg: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
    git(dir, &["rev-parse", "HEAD"])
}

struct Shas {
    c1: String,
    c2: String,
    c3: String,
    x: String,
    y: String,
}

/// Builds the remote and a clone of its main branch. Returns the checkout.
fn build(base: &Path, l: &Lang) -> (PathBuf, Shas) {
    let work = base.join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q"]);
    write(&work, l.widgets, l.v[0]);
    write(&work, l.legacy, l.legacy_lines);
    commit(&work, "c0");
    git(&work, &["checkout", "-q", "-b", "pr7"]);
    write(&work, l.widgets, l.v[1]);
    let c1 = commit(&work, "c1");
    write(&work, l.widgets, l.v[2]);
    std::fs::remove_file(work.join(l.legacy)).unwrap();
    let c2 = commit(&work, "c2");
    write(&work, l.widgets, l.v[3]);
    let c3 = commit(&work, "c3");
    git(&work, &["checkout", "-q", "main"]);
    git(&work, &["checkout", "-q", "-b", "side"]);
    write(&work, l.gadgets, l.gadget_lines);
    let x = commit(&work, "x");
    git(&work, &["checkout", "-q", "main"]);
    write(&work, "README.md", &["widgets"]);
    let y = commit(&work, "y");
    let remote = base.join("remote.git");
    git(base, &["clone", "-q", "--bare", "work", "remote.git"]);
    git(&remote, &["update-ref", "refs/pull/7/head", &c3]);
    git(&remote, &["update-ref", "refs/keep/x", &x]);
    git(&remote, &["branch", "-D", "pr7", "side"]);
    git(
        &remote,
        &["config", "uploadpack.allowReachableSHA1InWant", "true"],
    );
    git(
        base,
        &["clone", "-q", "--no-local", "remote.git", "checkout"],
    );
    (base.join("checkout"), Shas { c1, c2, c3, x, y })
}

fn body(title: &str, severity: &str, fix: &str) -> String {
    format!(
        "<!-- generated -->\n\n**<picture><img src=\"s.svg\"></picture> {severity}** · **Functional Correctness**\n\n**{title}**\n\nWhy it matters, in one line.\n\n<details><summary>Prompt</summary>do this</details>\n\n✅ Addressed in commit {fix}"
    )
}

#[allow(clippy::too_many_arguments)]
fn comment(
    id: u64,
    created: &str,
    login: &str,
    b: &str,
    pr: u64,
    path: &str,
    start: Option<u64>,
    line: u64,
    commit: &str,
) -> Value {
    json!({
        "id": id, "created_at": created, "user": {"login": login}, "body": b,
        "pull_request_url": format!("https://api.github.com/repos/{REPO}/pulls/{pr}"),
        "path": path, "original_start_line": start, "original_line": line, "line": null,
        "original_commit_id": commit, "commit_id": commit,
        "html_url": format!("https://github.com/{REPO}/pull/{pr}#discussion_r{id}"),
    })
}

fn page_args(page: u32) -> Vec<String> {
    [
        "api",
        "-X",
        "GET",
        "repos/acme/widgets/pulls/comments",
        "-f",
        "sort=created",
        "-f",
        "direction=desc",
        "-f",
        "per_page=100",
        "-f",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .chain([format!("page={page}")])
    .collect()
}

const CR: &str = "coderabbitai[bot]";

fn listing(l: &Lang, s: &Shas) -> (Vec<Value>, Vec<Value>) {
    let short = |sha: &str| sha[..7].to_owned();
    let mut page1 = Vec::new();
    for i in 0..98u64 {
        page1.push(comment(
            1000 + i,
            "2026-02-10T00:00:00Z",
            "a-person",
            "looks fine",
            7,
            l.widgets,
            None,
            1,
            &s.c1,
        ));
    }
    page1.push(comment(
        1100,
        "2026-02-09T00:00:00Z",
        CR,
        "**Rename this.**\n\nnot addressed yet",
        7,
        l.widgets,
        None,
        1,
        &s.c1,
    ));
    page1.push(comment(
        1101,
        "2026-02-09T00:00:01Z",
        CR,
        "**Pending.**\n\nstill open",
        7,
        l.widgets,
        None,
        2,
        &s.c1,
    ));
    let page2 = vec![
        comment(
            11,
            "2026-01-11T00:00:00Z",
            CR,
            &body("Commit that does not exist", "Major", "abcdef1"),
            11,
            l.widgets,
            None,
            1,
            &"0".repeat(40),
        ),
        comment(
            10,
            "2026-01-10T00:00:00Z",
            CR,
            &body("Fix that does not exist", "Major", "deadbee"),
            7,
            l.widgets,
            None,
            2,
            &s.c1,
        ),
        comment(
            9,
            "2026-01-09T00:00:00Z",
            CR,
            &body("Gadget count ignores the offset", "Minor", &short(&s.y)),
            9,
            l.gadgets,
            None,
            2,
            &s.x,
        ),
        comment(
            8,
            "2026-01-08T00:00:00Z",
            CR,
            &body("Empty list returns nothing useful", "Minor", &short(&s.c3)),
            7,
            l.widgets,
            None,
            9,
            &s.c2,
        ),
        comment(
            7,
            "2026-01-07T00:00:00Z",
            CR,
            &body("Raise is unreachable", "Minor", &short(&s.c3)),
            7,
            l.widgets,
            None,
            8,
            &s.c2,
        ),
        comment(
            6,
            "2026-01-06T00:00:00Z",
            CR,
            &body(
                "Names are not sorted case-insensitively",
                "Major",
                &short(&s.c3),
            ),
            7,
            l.widgets,
            Some(6),
            7,
            &s.c2,
        ),
        comment(
            5,
            "2026-01-05T00:00:00Z",
            CR,
            &body("Names has no default", "Minor", &short(&s.c3)),
            7,
            l.widgets,
            None,
            5,
            &s.c2,
        ),
        comment(
            4,
            "2026-01-04T00:00:00Z",
            CR,
            &body(
                "Total has no default",
                "Minor",
                &format!("{} to {}", short(&s.c1), short(&s.c3)),
            ),
            7,
            l.widgets,
            None,
            3,
            &s.c1,
        ),
        comment(
            3,
            "2026-01-03T00:00:00Z",
            CR,
            &body("Legacy count is always zero", "Major", &short(&s.c3)),
            7,
            l.legacy,
            None,
            1,
            &s.c1,
        ),
        comment(
            2,
            "2026-01-02T00:00:00Z",
            CR,
            &body("Total counts missing prices", "Major", &short(&s.c2)),
            7,
            l.widgets,
            None,
            4,
            &s.c1,
        ),
    ];
    (page1, page2)
}

fn tools_for(dir: &Path, page1: &[Value], page2: &[Value]) -> tribunal_context::exec::Tools {
    let calls = vec![
        json!({"args": page_args(1), "stdout": Value::Array(page1.to_vec()).to_string(), "stderr": "", "code": 0}),
        json!({"args": page_args(2), "stdout": Value::Array(page2.to_vec()).to_string(), "stderr": "", "code": 0}),
    ];
    common::tools(&common::recordings(dir, calls))
}

fn args(checkout: &Path, out: &Path) -> CollectArgs {
    CollectArgs {
        repo: REPO.to_owned(),
        checkout: checkout.to_owned(),
        out: out.to_owned(),
        markers: vec![source("coderabbit").unwrap()],
        line_changed: false,
        since: None,
        max_comments: DEFAULT_MAX_COMMENTS,
    }
}

fn read(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn ids(v: &Value, key: &str) -> Vec<String> {
    v[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect()
}

async fn run_case(l: &Lang) {
    let base = common::scratch(&format!("corpus-collect-{}", l.name));
    let (checkout, s) = build(&base, l);
    let (p1, p2) = listing(l, &s);
    let tools = tools_for(&base, &p1, &p2);
    let out = base.join("out");
    let summary = collect(&tools, &args(&checkout, &out)).await.unwrap();

    assert_eq!(summary["commentsRead"], 110, "{}", l.name);
    assert_eq!(summary["kept"], 10);
    assert_eq!(
        summary["droppedComments"],
        json!({"other-author": 98, "not-addressed": 2})
    );
    assert_eq!(
        summary["droppedItems"],
        json!({"commit-not-found": 1, "fix-not-found": 1})
    );
    assert_eq!(summary["itemsHeld"], 5);
    assert_eq!(summary["itemsSkipped"], 3);
    assert_eq!(summary["pullRequestsWritten"], 2);

    let pr7 = read(&out.join("pr-acme-widgets-7.json"));
    assert_eq!(
        pr7["reviewedSha"],
        json!(s.c2),
        "{}: the pin is where most items hold",
        l.name
    );
    assert_eq!(pr7["headSha"], json!(s.c3));
    assert_eq!(pr7["status"], "candidate");
    assert_eq!(
        ids(&pr7, "knownReal"),
        [
            "names-has-no-default",
            "names-are-not-sorted-case-insensitively",
            "raise-is-unreachable",
            "empty-list-returns-nothing-useful"
        ]
    );
    let reasons: Vec<(String, String)> = pr7["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["id"].as_str().unwrap().to_owned(),
                i["reason"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        reasons,
        [
            (
                "total-counts-missing-prices".to_owned(),
                "fix-already-in-pin".to_owned()
            ),
            (
                "legacy-count-is-always-zero".to_owned(),
                "file-missing-at-pin".to_owned()
            ),
            (
                "total-has-no-default".to_owned(),
                "quote-not-at-pin".to_owned()
            ),
        ]
    );
    let sorted = &pr7["knownReal"][1];
    assert_eq!(sorted["line"], 7, "code is quoted before the comment line");
    assert_eq!(sorted["quote"], json!(l.v[2][6].trim()));
    assert_eq!(sorted["fix"], json!(s.c3));
    assert_eq!(sorted["seenAt"], json!(s.c2));
    assert_eq!(sorted["expectedSeverity"], "important");
    assert_eq!(pr7["knownReal"][0]["expectedSeverity"], "nit");
    assert!(sorted["description"]
        .as_str()
        .unwrap()
        .starts_with("Names are not sorted case-insensitively. Why it matters"));
    assert!(!sorted["description"].as_str().unwrap().contains("Prompt"));

    let pr9 = read(&out.join("pr-acme-widgets-9.json"));
    assert_eq!(
        pr9["reviewedSha"],
        json!(s.x),
        "a commit only reachable by sha was fetched"
    );
    assert_eq!(pr9["headSha"], Value::Null);
    assert_eq!(pr9["knownReal"][0]["fix"], json!(s.y));
    assert_eq!(
        pr9["knownReal"][0]["quote"],
        json!(l.gadget_lines[1].trim())
    );
    assert!(!out.join("pr-acme-widgets-11.json").exists());

    for f in [&pr7, &pr9] {
        assert_eq!(violations(f), Vec::<String>::new());
    }
}

#[tokio::test]
async fn collects_from_a_python_repository() {
    run_case(&PYTHON).await;
}

#[tokio::test]
async fn collects_from_a_go_repository() {
    run_case(&GO).await;
}

#[tokio::test]
async fn max_comments_and_since_stop_the_listing() {
    let base = common::scratch("corpus-collect-limits");
    let (checkout, s) = build(&base, &PYTHON);
    let (p1, p2) = listing(&PYTHON, &s);
    let tools = tools_for(&base, &p1, &p2);
    let out = base.join("out");
    let mut a = args(&checkout, &out);
    a.max_comments = 100;
    let summary = collect(&tools, &a).await.unwrap();
    assert_eq!(summary["commentsRead"], 100);
    assert_eq!(summary["kept"], 0);
    let mut a = args(&checkout, &out);
    a.since = Some("2026-01-06".to_owned());
    let summary = collect(&tools, &a).await.unwrap();
    assert_eq!(
        summary["kept"], 6,
        "comments 6 to 11 are on or after the date"
    );
}

#[tokio::test]
async fn refuses_an_output_directory_with_a_labelled_entry() {
    let base = common::scratch("corpus-collect-refuse");
    let (checkout, s) = build(&base, &PYTHON);
    let (p1, p2) = listing(&PYTHON, &s);
    let tools = tools_for(&base, &p1, &p2);
    let out = base.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let labelled = out.join("pr-acme-widgets-7.json");
    let text = "{\"status\": \"labelled\", \"knownReal\": []}\n";
    std::fs::write(&labelled, text).unwrap();
    let err = collect(&tools, &args(&checkout, &out)).await.unwrap_err();
    assert!(err.contains("is not a candidate file"), "{err}");
    assert_eq!(std::fs::read_to_string(&labelled).unwrap(), text);
}

#[tokio::test]
async fn a_rerun_replaces_only_this_repositorys_candidates() {
    let base = common::scratch("corpus-collect-rerun");
    let (checkout, s) = build(&base, &PYTHON);
    let (p1, p2) = listing(&PYTHON, &s);
    let tools = tools_for(&base, &p1, &p2);
    let out = base.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let stale = out.join("pr-acme-widgets-99.json");
    let other = out.join("pr-acme-gadgets-5.json");
    for p in [&stale, &other] {
        std::fs::write(p, "{\"status\": \"candidate\"}\n").unwrap();
    }
    collect(&tools, &args(&checkout, &out)).await.unwrap();
    let first = std::fs::read_to_string(out.join("pr-acme-widgets-7.json")).unwrap();
    collect(&tools, &args(&checkout, &out)).await.unwrap();
    assert!(!stale.exists());
    assert!(other.exists());
    assert_eq!(
        std::fs::read_to_string(out.join("pr-acme-widgets-7.json")).unwrap(),
        first
    );
}

/// With `line-changed` on, a comment by anyone counts when a later commit of
/// the pull request changed its lines; the fix is that commit.
async fn run_line_changed(l: &Lang) {
    let base = common::scratch(&format!("corpus-collect-lines-{}", l.name));
    let (checkout, s) = build(&base, l);
    let short = |sha: &str| sha[..7].to_owned();
    let mut reply = comment(
        16,
        "2026-01-16T00:00:00Z",
        "reviewer-a",
        "Agreed.",
        7,
        l.widgets,
        None,
        3,
        &s.c1,
    );
    reply["in_reply_to_id"] = json!(11);
    let page = vec![
        reply,
        comment(
            15,
            "2026-01-15T00:00:00Z",
            CR,
            &body("Raise is unreachable", "Minor", &short(&s.c3)),
            7,
            l.widgets,
            None,
            8,
            &s.c2,
        ),
        comment(
            14,
            "2026-01-14T00:00:00Z",
            "reviewer-a",
            "Names has no default\nCallers pass nothing.",
            7,
            l.widgets,
            None,
            5,
            &s.c2,
        ),
        comment(
            13,
            "2026-01-13T00:00:00Z",
            CR,
            "**Sort names case-insensitively.**\n\nUpper case sorts first.",
            7,
            l.widgets,
            Some(6),
            7,
            &s.c2,
        ),
        comment(
            12,
            "2026-01-12T00:00:00Z",
            "author-dev",
            "Note to self: rename this",
            7,
            l.widgets,
            None,
            3,
            &s.c1,
        ),
        comment(
            11,
            "2026-01-11T00:00:00Z",
            "reviewer-a",
            "Count is fine as is?",
            7,
            l.widgets,
            None,
            4,
            &s.c1,
        ),
        comment(
            10,
            "2026-01-10T00:00:00Z",
            "reviewer-a",
            "Total needs a default\nAn empty call fails.",
            7,
            l.widgets,
            None,
            3,
            &s.c1,
        ),
    ];
    let calls = vec![
        json!({"args": page_args(1), "stdout": Value::Array(page).to_string(), "stderr": "", "code": 0}),
        json!({"args": ["api", "repos/acme/widgets/pulls/7"], "stdout": json!({"user": {"login": "author-dev"}}).to_string(), "stderr": "", "code": 0}),
    ];
    let tools = common::tools(&common::recordings(&base, calls));
    let out = base.join("out");
    let mut a = args(&checkout, &out);
    a.line_changed = true;
    let summary = collect(&tools, &a).await.unwrap();
    assert_eq!(summary["commentsRead"], 7, "{}", l.name);
    assert_eq!(summary["kept"], 6);
    assert_eq!(summary["droppedComments"], json!({"reply": 1}));
    assert_eq!(
        summary["droppedItems"],
        json!({"lines-unchanged": 1, "pr-author": 1})
    );
    assert_eq!(
        summary["itemsHeldBySource"],
        json!({"line-changed": 2, "reviewer-addressed": 1})
    );
    let pr7 = read(&out.join("pr-acme-widgets-7.json"));
    assert_eq!(pr7["reviewedSha"], json!(s.c2), "{}", l.name);
    let items = pr7["knownReal"].as_array().unwrap();
    let got: Vec<(&str, &str, &str, &str)> = items
        .iter()
        .map(|i| {
            (
                i["id"].as_str().unwrap(),
                i["source"].as_str().unwrap(),
                i["reviewerKind"].as_str().unwrap(),
                i["fix"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (
                "sort-names-case-insensitively",
                "line-changed",
                "bot",
                s.c3.as_str()
            ),
            (
                "names-has-no-default",
                "line-changed",
                "person",
                s.c3.as_str()
            ),
            (
                "raise-is-unreachable",
                "reviewer-addressed",
                "bot",
                s.c3.as_str()
            ),
        ]
    );
    assert_eq!(
        items[1]["description"],
        "Names has no default. Callers pass nothing."
    );
    assert_eq!(items[1]["reviewer"], "reviewer-a");
    assert_eq!(pr7["skipped"][0]["id"], "total-needs-a-default");
    assert_eq!(pr7["skipped"][0]["reason"], "quote-not-at-pin");
    assert_eq!(violations(&pr7), Vec::<String>::new());
}

#[tokio::test]
async fn line_changed_finds_fixes_in_a_python_repository() {
    run_line_changed(&PYTHON).await;
}

#[tokio::test]
async fn line_changed_finds_fixes_in_a_go_repository() {
    run_line_changed(&GO).await;
}

/// The comment on line 3 at c1 was fixed by c2, which changed that line. With
/// c1 as the only pin, it holds there and its fix is c2.
#[tokio::test]
async fn the_fix_is_the_first_commit_that_changed_the_lines() {
    let base = common::scratch("corpus-collect-first-change");
    let (checkout, s) = build(&base, &PYTHON);
    let page = vec![comment(
        10,
        "2026-01-10T00:00:00Z",
        "reviewer-a",
        "Total needs a default",
        7,
        PYTHON.widgets,
        None,
        3,
        &s.c1,
    )];
    let calls = vec![
        json!({"args": page_args(1), "stdout": Value::Array(page).to_string(), "stderr": "", "code": 0}),
        json!({"args": ["api", "repos/acme/widgets/pulls/7"], "stdout": json!({"user": {"login": "author-dev"}}).to_string(), "stderr": "", "code": 0}),
    ];
    let tools = common::tools(&common::recordings(&base, calls));
    let out = base.join("out");
    let mut a = args(&checkout, &out);
    a.line_changed = true;
    collect(&tools, &a).await.unwrap();
    let pr7 = read(&out.join("pr-acme-widgets-7.json"));
    assert_eq!(pr7["reviewedSha"], json!(s.c1));
    assert_eq!(pr7["knownReal"][0]["fix"], json!(s.c2));
    assert_eq!(pr7["knownReal"][0]["seenAt"], json!(s.c1));
}
