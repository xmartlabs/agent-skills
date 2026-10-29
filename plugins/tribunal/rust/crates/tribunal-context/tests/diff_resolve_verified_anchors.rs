//! `lib/resolve-verified-anchors.mjs` against
//! `tribunal_context::resolve_verified_anchors`: the same anchors file and
//! arguments on both sides, run as the workflow runs it (`--relay`), against
//! the same recorded `gh`, compared on the relayed JSON.
//!
//! The script's side of the generated cases is read from
//! `tests/goldens/resolve-verified-anchors.json`: running the script for
//! every case made this one of the slowest test binaries. The file keeps the
//! text of the scripts and of the recorded `gh` it was made with, so editing
//! either without a regeneration fails here. Regenerate after changing them:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_resolve_verified_anchors`.
mod common;
#[path = "../../tribunal-core/tests/common/mod.rs"]
mod core_common;

use common::*;
use core_common::{assert_same, sample};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use proptest::prelude::*;
use proptest::sample::select;
use serde_json::{json, Value};
use tribunal_context::exec::Tools;
use tribunal_context::resolve_verified_anchors::run;
use tribunal_core::profile::js_ext::json_stringify;

const SCRIPT: &str = "lib/resolve-verified-anchors.mjs";
const ACCEPT: &str = "Accept: application/vnd.github.raw";

const REPOS: [&str; 3] = ["acme/api", "acme/web", "o/r"];
const REFS: [&str; 5] = ["abc1234", "main", "feature/x y", "v1@rc", "ünï"];
/// `--cross-repo` values, and the sibling the script makes of each.
const SIBLINGS: [(&str, Option<(&str, &str)>); 8] = [
    ("acme/web@development", Some(("acme/web", "development"))),
    ("acme/api@main", Some(("acme/api", "main"))),
    ("other/lib@v2.0", Some(("other/lib", "v2.0"))),
    ("acme/sdk@a@b", Some(("acme/sdk@a", "b"))),
    ("no-at-sign", None),
    ("@lead", None),
    ("x/empty@", Some(("x/empty", ""))),
    ("web@x", Some(("web", "x"))),
];
/// Two of these start with a sibling's directory name (`lib/`, `web/`).
const PATHS: [&str; 5] = [
    "src/a.ts",
    "src/b c.ts",
    "lib/ü/x.ts",
    "README.md",
    "web/app.tsx",
];
const LINES: [&str; 18] = [
    "export function total(items) {",
    "  return items.reduce((a, b) => a + b, 0);",
    "}",
    "const label = 'Total \u{2013} due';",
    "  // \u{201C}quoted\u{201D} comment, it\u{2019}s here",
    "\tif (x === null) return;",
    "const total = computeTotal(items);",
    "const total = computeTotal(items);",
    "",
    "   ",
    "import { a } from \"./a\";",
    "émoji 😀 𝒳 line",
    "const   spaced\t=  1;",
    "\u{3000}ideographic\u{00A0}space",
    "a",
    "return null;",
    "  where: { accountId, deletedAt: null },",
    "status --- check -- it",
];
fn stderrs() -> Vec<String> {
    vec![
        String::new(),
        "  \n\t".into(),
        "HTTP 401: Bad credentials (https://api.github.com/repos/acme/api/contents)\n".into(),
        "gh: no COMMIT found FOR the REF x\n".into(),
        "gh: Not Found (http 404)\n".into(),
        "error connecting to api.github.com\ncheck your internet connection or https://githubstatus.com\n".into(),
        "\u{3000}spaced stderr\u{FEFF}".into(),
        // The cut at 200 units falls between two pairs.
        format!("{}{}", "x".repeat(150), "😀".repeat(30)),
        format!("  {}\n", "y".repeat(300)),
        NOT_FOUND.into(),
    ]
}
const NOT_FOUND: &str = "gh: Not Found (HTTP 404)\n";
const NO_COMMIT: &str = "gh: No commit found for the ref nope (HTTP 422)\n";

fn fixed_files() -> Vec<Option<Value>> {
    vec![
        None,
        Some(Value::Null),
        Some(json!("")),
        Some(json!(5)),
        Some(json!(["src/a.ts"])),
        Some(json!("missing/file.ts")),
        Some(json!("src/a.ts/")),
    ]
}

fn fixed_lines() -> Vec<Option<Value>> {
    vec![
        None,
        Some(Value::Null),
        Some(json!("3")),
        Some(json!(2.5)),
        Some(json!(0)),
        Some(serde_json::from_str("-0.0").unwrap()),
        Some(json!(-4)),
        Some(serde_json::from_str("1e21").unwrap()),
        Some(json!(true)),
        Some(serde_json::from_str("3.0").unwrap()),
        Some(serde_json::from_str("9007199254740993").unwrap()),
    ]
}

fn fixed_quotes() -> Vec<Option<Value>> {
    vec![
        None,
        Some(Value::Null),
        Some(json!("")),
        Some(json!("   \n ")),
        Some(json!(0)),
        Some(json!(false)),
        Some(json!(1)),
        Some(json!(["const total", "computeTotal(items);"])),
        Some(json!({"a": 1})),
        Some(json!("this text is nowhere in any file")),
        Some(json!("\u{2014}\u{2013}")),
    ]
}

/// Quotes `String()` throws on: the script dies uncaught.
fn crash_quotes() -> Vec<Value> {
    vec![json!({"toString": 1}), json!([[{"toString": "x"}]])]
}

fn fixed_anchors() -> Vec<Value> {
    vec![
        Value::Null,
        json!("src/a.ts"),
        json!(5),
        json!([]),
        json!({}),
        json!(true),
    ]
}

fn fixed_findings() -> Vec<Value> {
    vec![
        Value::Null,
        json!(5),
        json!({}),
        json!({"verifiedBy": "x"}),
        json!({"verifiedBy": null}),
        json!({"verifiedBy": {"0": {"file": "src/a.ts", "quote": "a"}}}),
        json!([]),
    ]
}

const TEXTS: [&str; 9] = [
    "",
    "not json",
    "\u{FEFF}[]",
    "null",
    "\"str\"",
    "5",
    "[]",
    "{}",
    "{\"findings\": null}",
];

#[derive(Debug, Clone)]
enum FileF {
    Own(usize),
    /// A sibling's directory name, then one of `PATHS`.
    Sibling(usize, usize),
    Fixed(Option<Value>),
}

#[derive(Debug, Clone)]
enum LineF {
    Offset(i64),
    Fixed(Option<Value>),
}

#[derive(Debug, Clone)]
enum QuoteF {
    Lines {
        at: usize,
        span: usize,
        dress: usize,
    },
    Fixed(Option<Value>),
    Crash(Value),
}

#[derive(Debug, Clone)]
enum AnchorF {
    Spec(FileF, LineF, QuoteF),
    Fixed(Value),
}

#[derive(Debug, Clone)]
enum FindingF {
    Anchors(Vec<AnchorF>),
    Fixed(Value),
}

#[derive(Debug, Clone, Copy)]
enum Answer {
    Ok,
    NotFound,
    RefMissing,
    Failed(usize, i32),
    Unrecorded,
}

/// What every lookup in one (repo, ref) pair answers.
#[derive(Debug, Clone, Copy)]
enum PairMode {
    PerFile,
    RefMissing,
    Broken,
    AllMissing,
}

#[derive(Debug, Clone)]
enum Payload {
    Array,
    Wrapped,
    WrappedObject,
    Text(usize),
}

#[derive(Debug, Clone)]
struct Case {
    repo: usize,
    ref_: usize,
    siblings: Vec<(usize, bool)>,
    contents: Vec<(Vec<usize>, bool, bool)>,
    modes: Vec<PairMode>,
    answers: Vec<Answer>,
    findings: Vec<FindingF>,
    payload: Payload,
    args: usize,
    stdin: bool,
    /// Every anchor well formed and in the reviewed repo, so a pair whose
    /// lookups all 404 makes every result `file-not-at-ref`.
    clean: bool,
}

fn file_f() -> BoxedStrategy<FileF> {
    prop_oneof![
        6 => (0..PATHS.len()).prop_map(FileF::Own),
        3 => (0..SIBLINGS.len(), 0..PATHS.len()).prop_map(|(s, p)| FileF::Sibling(s, p)),
        1 => select(fixed_files()).prop_map(FileF::Fixed),
    ]
    .boxed()
}

fn line_f() -> BoxedStrategy<LineF> {
    prop_oneof![
        5 => Just(LineF::Offset(0)),
        3 => (-3i64..=3).prop_map(LineF::Offset),
        1 => select(vec![-15i64, 15]).prop_map(LineF::Offset),
        2 => select(fixed_lines()).prop_map(LineF::Fixed),
    ]
    .boxed()
}

fn quote_f() -> BoxedStrategy<QuoteF> {
    prop_oneof![
        30 => (0usize..40, 1usize..=3, 0usize..8)
            .prop_map(|(at, span, dress)| QuoteF::Lines { at, span, dress }),
        4 => select(fixed_quotes()).prop_map(QuoteF::Fixed),
        1 => select(crash_quotes()).prop_map(QuoteF::Crash),
    ]
    .boxed()
}

fn anchor_f() -> BoxedStrategy<AnchorF> {
    prop_oneof![
        12 => (file_f(), line_f(), quote_f()).prop_map(|(f, l, q)| AnchorF::Spec(f, l, q)),
        1 => select(fixed_anchors()).prop_map(AnchorF::Fixed),
    ]
    .boxed()
}

fn finding_f() -> BoxedStrategy<FindingF> {
    prop_oneof![
        8 => proptest::collection::vec(anchor_f(), 0..4).prop_map(FindingF::Anchors),
        1 => select(fixed_findings()).prop_map(FindingF::Fixed),
    ]
    .boxed()
}

fn answer() -> BoxedStrategy<Answer> {
    prop_oneof![
        12 => Just(Answer::Ok),
        2 => Just(Answer::NotFound),
        1 => Just(Answer::RefMissing),
        3 => (0..stderrs().len(), select(vec![1, 1, 2, 4])).prop_map(|(s, c)| Answer::Failed(s, c)),
        1 => Just(Answer::Unrecorded),
    ]
    .boxed()
}

fn case() -> impl Strategy<Value = Case> {
    let pair_mode = prop_oneof![
        12 => Just(PairMode::PerFile),
        1 => Just(PairMode::RefMissing),
        1 => Just(PairMode::Broken),
        3 => Just(PairMode::AllMissing),
    ];
    let contents = proptest::collection::vec(
        (
            proptest::collection::vec(0..LINES.len(), 0..20),
            any::<bool>(),
            any::<bool>(),
        ),
        PATHS.len(),
    );
    let payload = prop_oneof![
        8 => Just(Payload::Array),
        2 => Just(Payload::Wrapped),
        1 => Just(Payload::WrappedObject),
        2 => (0..TEXTS.len()).prop_map(Payload::Text),
    ];
    let args = prop_oneof![
        20 => Just(0usize),
        4 => Just(1usize),
        1 => 2usize..9,
    ];
    (
        (
            select(vec![0usize, 0, 0, 1, 2]),
            0..REFS.len(),
            proptest::collection::vec((0..SIBLINGS.len(), any::<bool>()), 0..3),
        ),
        contents,
        proptest::collection::vec(pair_mode, 4),
        proptest::collection::vec(answer(), 30),
        proptest::collection::vec(finding_f(), 0..5),
        payload,
        args,
        (
            proptest::bool::weighted(0.15),
            proptest::bool::weighted(0.15),
        ),
    )
        .prop_map(
            |(
                (repo, ref_, siblings),
                contents,
                modes,
                answers,
                findings,
                payload,
                args,
                (stdin, clean),
            )| {
                Case {
                    repo,
                    ref_,
                    siblings,
                    contents,
                    modes,
                    answers,
                    findings,
                    payload,
                    args,
                    stdin,
                    clean,
                }
            },
        )
}

/// `encodeURIComponent`, to spell the recorded calls. A wrong spelling here
/// only costs coverage: both sides then miss the same recording.
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn gh_args(repo: &str, ref_: &str, path: &str) -> Vec<String> {
    let path = path.split('/').map(enc).collect::<Vec<_>>().join("/");
    vec![
        "api".into(),
        "-H".into(),
        ACCEPT.into(),
        format!("repos/{repo}/contents/{path}?ref={}", enc(ref_)),
    ]
}

fn gh_answer(args: Vec<String>, stdout: &str, stderr: &str, code: i32) -> Value {
    json!({"args": args, "stdout": stdout, "stderr": stderr, "code": code})
}

fn content_lines(case: &Case, i: usize) -> Vec<&'static str> {
    case.contents[i].0.iter().map(|&k| LINES[k]).collect()
}

fn content_text(case: &Case, i: usize) -> String {
    let (_, crlf, trailing) = &case.contents[i];
    let sep = if *crlf { "\r\n" } else { "\n" };
    let mut text = content_lines(case, i).join(sep);
    if *trailing {
        text.push_str(sep);
    }
    text
}

/// A quote cut from `lines`, dressed the way a model transcribes one.
fn quote_of(lines: &[&str], at: usize, span: usize, dress: usize) -> String {
    if lines.is_empty() {
        return "const total = computeTotal(items);".into();
    }
    let at = at % lines.len();
    let take = |n: usize| lines[at..(at + n).min(lines.len())].to_vec();
    match dress {
        0 => take(span).join("\n"),
        1 => take(span)
            .iter()
            .map(|l| format!("   {}", l.trim()))
            .collect::<Vec<_>>()
            .join("\n"),
        2 => take(span)
            .join("\n")
            .replace('\'', "\u{2019}")
            .replace('"', "\u{201C}")
            .replace('-', "\u{2014}"),
        3 => {
            let chars: Vec<char> = lines[at].chars().collect();
            chars[chars.len().min(2)..chars.len().saturating_sub(2).max(chars.len().min(2))]
                .iter()
                .collect()
        }
        4 => format!("\n\n  {}\n ", take(span).join("\n")),
        5 => format!("{}Zq", take(span).join("\n")),
        6 => {
            let joined = take(3).join(" ");
            let mid = joined
                .char_indices()
                .map(|(i, _)| i)
                .find(|&i| i >= joined.len() / 2)
                .unwrap_or(0);
            format!("{}\n{}", &joined[..mid], &joined[mid..])
        }
        _ => take(span).join("\n").replace(' ', "\t\t"),
    }
}

/// The same findings with every anchor well formed, in `src/a.ts` or
/// `src/b c.ts`, which no sibling claims.
fn cleaned(findings: &[FindingF]) -> Vec<FindingF> {
    let quote = |q: &QuoteF| match q {
        QuoteF::Lines { .. } => q.clone(),
        _ => QuoteF::Lines {
            at: 0,
            span: 1,
            dress: 0,
        },
    };
    findings
        .iter()
        .map(|f| match f {
            FindingF::Fixed(_) => FindingF::Anchors(vec![]),
            FindingF::Anchors(list) => FindingF::Anchors(
                list.iter()
                    .enumerate()
                    .map(|(i, a)| match a {
                        AnchorF::Spec(_, line, q) => AnchorF::Spec(
                            FileF::Own(i % 2),
                            match line {
                                LineF::Offset(_) => line.clone(),
                                LineF::Fixed(_) => LineF::Offset(0),
                            },
                            quote(q),
                        ),
                        AnchorF::Fixed(_) => AnchorF::Spec(
                            FileF::Own(i % 2),
                            LineF::Offset(0),
                            quote(&QuoteF::Fixed(None)),
                        ),
                    })
                    .collect(),
            ),
        })
        .collect()
}

struct Built {
    args: Vec<String>,
    payload: String,
    stdin: Option<String>,
    calls: Vec<Value>,
    /// (findingIndex, anchorIndex) of quotes reflowed across lines.
    reflowed: Vec<(usize, usize)>,
}

fn build(case: &Case, anchors_path: &Path) -> Built {
    let repo = REPOS[case.repo];
    let ref_ = REFS[case.ref_];

    let mut pairs = vec![(repo, ref_)];
    pairs.extend(case.siblings.iter().filter_map(|&(s, _)| SIBLINGS[s].1));
    let mut calls = Vec::new();
    let mut k = 0;
    for (pi, &(r, rf)) in pairs.iter().enumerate() {
        let mode = case.modes[pi % case.modes.len()];
        for (i, p) in PATHS.iter().enumerate() {
            let suffix = p.split_once('/').map(|(_, s)| s);
            for lookup in std::iter::once(*p).chain(suffix) {
                let args = gh_args(r, rf, lookup);
                let answer = match mode {
                    PairMode::PerFile => case.answers[k % case.answers.len()],
                    PairMode::RefMissing => Answer::RefMissing,
                    PairMode::Broken => Answer::Failed(2, 1),
                    PairMode::AllMissing => Answer::NotFound,
                };
                k += 1;
                calls.push(match answer {
                    Answer::Ok => gh_answer(args, &content_text(case, i), "", 0),
                    Answer::NotFound => gh_answer(args, "", NOT_FOUND, 1),
                    Answer::RefMissing => gh_answer(args, "", NO_COMMIT, 1),
                    Answer::Failed(s, code) => gh_answer(args, "", &stderrs()[s], code),
                    Answer::Unrecorded => continue,
                });
            }
        }
    }

    let mut reflowed = Vec::new();
    let specs = if case.clean {
        cleaned(&case.findings)
    } else {
        case.findings.clone()
    };
    let findings: Vec<Value> = specs
        .iter()
        .enumerate()
        .map(|(fi, f)| match f {
            FindingF::Fixed(v) => v.clone(),
            FindingF::Anchors(anchors) => {
                let list: Vec<Value> = anchors
                    .iter()
                    .enumerate()
                    .map(|(ai, a)| match a {
                        AnchorF::Fixed(v) => v.clone(),
                        AnchorF::Spec(file, line, quote) => {
                            let (file, idx) = match file {
                                FileF::Own(i) => (Some(json!(PATHS[*i])), *i),
                                FileF::Sibling(s, i) => {
                                    // Mostly a sibling this case passes, sometimes one it does not.
                                    let s = match case.siblings.get(s % 2) {
                                        Some((own, _)) => own,
                                        None => s,
                                    };
                                    let dir = SIBLINGS[*s].1.map_or("unknown", |(r, _)| {
                                        r.rsplit('/').next().unwrap_or("")
                                    });
                                    (Some(json!(format!("{dir}/{}", PATHS[*i]))), *i)
                                }
                                FileF::Fixed(v) => (v.clone(), 0),
                            };
                            let lines = content_lines(case, idx);
                            let (quote, at) = match quote {
                                QuoteF::Lines { at, span, dress } => {
                                    if *dress == 6 {
                                        reflowed.push((fi, ai));
                                    }
                                    let mut q = quote_of(&lines, *at, *span, *dress);
                                    if case.clean && q.trim().is_empty() {
                                        q = "const total = computeTotal(items);".into();
                                    }
                                    (Some(json!(q)), at % lines.len().max(1))
                                }
                                QuoteF::Fixed(v) => (v.clone(), 0),
                                QuoteF::Crash(v) => (Some(v.clone()), 0),
                            };
                            let line = match line {
                                LineF::Offset(o) => Some(json!(at as i64 + 1 + o)),
                                LineF::Fixed(v) => v.clone(),
                            };
                            let mut m = serde_json::Map::new();
                            for (key, v) in [("file", file), ("line", line), ("quote", quote)] {
                                if let Some(v) = v {
                                    m.insert(key.into(), v);
                                }
                            }
                            Value::Object(m)
                        }
                    })
                    .collect();
                json!({"title": format!("finding {fi}"), "verifiedBy": list})
            }
        })
        .collect();
    let payload = match case.payload {
        Payload::Array => Value::Array(findings).to_string(),
        Payload::Wrapped => json!({"findings": findings}).to_string(),
        Payload::WrappedObject => json!({"findings": {"0": findings}}).to_string(),
        Payload::Text(i) => TEXTS[i].to_owned(),
    };

    let path = if case.stdin {
        "-".to_owned()
    } else {
        anchors_path.to_string_lossy().into_owned()
    };
    let mut args: Vec<String> = match case.args {
        0 => vec![
            path,
            ref_.into(),
            "--repo".into(),
            repo.into(),
            "--relay".into(),
        ],
        1 => vec![
            path,
            ref_.into(),
            format!("--repo={repo}"),
            "--relay".into(),
        ],
        2 => vec![path, "--repo".into(), repo.into(), "--relay".into()],
        3 => vec![path, ref_.into(), "--relay".into()],
        4 => vec![path, ref_.into(), "--repo".into(), "--relay".into()],
        5 => vec![path, "--relay".into()],
        6 => vec![
            path,
            String::new(),
            "--repo".into(),
            repo.into(),
            "--relay".into(),
        ],
        7 => vec![
            "--repo".into(),
            repo.into(),
            path,
            ref_.into(),
            "--relay".into(),
        ],
        _ => vec![
            path,
            ref_.into(),
            format!("--repo={repo}=x"),
            "--relay".into(),
        ],
    };
    for &(s, eq) in &case.siblings {
        if eq {
            args.push(format!("--cross-repo={}", SIBLINGS[s].0));
        } else {
            args.push("--cross-repo".into());
            args.push(SIBLINGS[s].0.into());
        }
    }
    Built {
        args,
        stdin: case.stdin.then(|| payload.clone()),
        payload,
        calls,
        reflowed,
    }
}

/// What a side produced: the relayed object, or the uncaught TypeError.
fn js_value(js: &ScriptRun) -> (Value, Option<String>) {
    if js.code == Some(0) {
        let lines: Vec<&str> = js.stdout.split('\n').collect();
        assert_eq!(lines.len(), 3, "{:?}", js.stdout);
        assert!(lines[0].starts_with("RELAY-CRC "), "{:?}", js.stdout);
        let v = serde_json::from_str(lines[1])
            .unwrap_or_else(|e| panic!("relay line is not JSON ({e}): {}", lines[1]));
        return (v, Some(lines[1].to_owned()));
    }
    let thrown = js
        .stderr
        .lines()
        .find(|l| l.starts_with("TypeError: "))
        .map_or_else(
            || format!("exit {:?}: {}", js.code, js.stderr),
            str::to_owned,
        );
    (json!({ "threw": thrown }), None)
}

fn rust_value(out: Result<Value, String>) -> (Value, Option<String>) {
    match out {
        Ok(v) => {
            let text = json_stringify(Some(&v));
            (v, Some(text))
        }
        Err(e) => (json!({ "threw": e }), None),
    }
}

struct Outcome {
    input: Value,
    js: Value,
    rust: Value,
    js_text: Option<String>,
    rust_text: Option<String>,
    reflowed: Vec<(usize, usize)>,
}

/// The script's side of one case, recorded or run.
type JsSide = (Value, Option<String>);

fn check(
    dir: &Path,
    case: &Case,
    rt: &tokio::runtime::Runtime,
    recorded: Option<JsSide>,
) -> Outcome {
    std::fs::create_dir_all(dir).unwrap();
    let anchors_path = dir.join("anchors.json");
    let built = build(case, &anchors_path);
    std::fs::write(&anchors_path, &built.payload).unwrap();
    let rec = dir.join("gh-recordings.json");
    std::fs::write(&rec, json!({ "calls": built.calls }).to_string()).unwrap();
    let args: Vec<&str> = built.args.iter().map(String::as_str).collect();

    let (js, js_text) = recorded.unwrap_or_else(|| {
        js_value(&run_script(
            SCRIPT,
            &args,
            dir,
            &rec,
            built.stdin.as_deref(),
        ))
    });
    let rust = rt.block_on(run(
        &args,
        built.stdin.as_deref().unwrap_or(""),
        &tools(&rec),
    ));
    let (rust, rust_text) = rust_value(rust);
    Outcome {
        input: json!({"dir": dir.to_string_lossy(), "args": built.args, "payload": built.payload}),
        js,
        rust,
        js_text,
        rust_text,
        reflowed: built.reflowed,
    }
}

/// What the script reads besides its input: itself, what it imports, and the
/// recorded `gh` it calls.
const RECORDED_WITH: [&str; 3] = [
    "lib/resolve-verified-anchors.mjs",
    "lib/relay.mjs",
    "gh shim",
];
const REGENERATE: &str = "regenerate: TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_resolve_verified_anchors";

fn golden_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/resolve-verified-anchors.json")
}

fn recorded_with() -> Value {
    let texts: BTreeMap<&str, String> = RECORDED_WITH
        .iter()
        .map(|s| {
            let path = match *s {
                "gh shim" => shim_dir().join("gh"),
                lib => plugin_root().join(lib),
            };
            (*s, std::fs::read_to_string(path).unwrap())
        })
        .collect();
    json!(texts)
}

fn regenerating() -> bool {
    std::env::var("TRIBUNAL_REGEN_GOLDENS").is_ok_and(|v| v == "1")
}

/// The golden, checked against the scripts on disk; `None` when regenerating.
fn recorded() -> Option<Value> {
    if regenerating() {
        return None;
    }
    let path = golden_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; {REGENERATE}", path.display()));
    let golden: Value = serde_json::from_str(&text).unwrap();
    assert!(
        golden["recordedWith"] == recorded_with(),
        "a script or the gh shim changed since {} was recorded; {REGENERATE}",
        path.display()
    );
    Some(golden)
}

/// A value as the golden keeps it. Inputs name the anchors file, and the
/// script echoes it back when the arguments put it in the ref's place; it
/// lives in the scratch directory, which differs on every run and machine.
fn portable(v: &Value, root: &str) -> Value {
    serde_json::from_str(&v.to_string().replace(root, "<SCRATCH>")).unwrap()
}

fn local(v: &Value, root: &str) -> Value {
    serde_json::from_str(&v.to_string().replace("<SCRATCH>", root)).unwrap()
}

/// Both sides over the shared sample, once per test binary, on every core.
fn outcomes() -> &'static [Outcome] {
    static CELL: OnceLock<Vec<Outcome>> = OnceLock::new();
    CELL.get_or_init(|| {
        let cases = sample(case());
        let root = scratch("verified-anchors-gen");
        let root_text = root.to_string_lossy().into_owned();
        let golden = recorded();
        let replay: Option<Vec<&Value>> = golden.as_ref().map(|g| {
            let recorded = g["cases"].as_array().unwrap();
            assert_eq!(
                recorded.len(),
                cases.len(),
                "the generator changed; {REGENERATE}"
            );
            recorded.iter().collect()
        });
        let next = AtomicUsize::new(0);
        let slots: Vec<Mutex<Option<Outcome>>> = cases.iter().map(|_| Mutex::new(None)).collect();
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
        std::thread::scope(|s| {
            for _ in 0..workers {
                s.spawn(|| {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(case) = cases.get(i) else { break };
                        let js = replay.as_ref().map(|r| {
                            (
                                local(&r[i]["js"], &root_text),
                                r[i]["jsText"]
                                    .as_str()
                                    .map(|t| t.replace("<SCRATCH>", &root_text)),
                            )
                        });
                        let o = check(&root.join(i.to_string()), case, &rt, js);
                        *slots[i].lock().unwrap() = Some(o);
                    }
                });
            }
        });
        let out: Vec<Outcome> = slots
            .into_iter()
            .map(|m| m.into_inner().unwrap().unwrap())
            .collect();
        let root = root_text;
        let inputs: Vec<Value> = out.iter().map(|o| portable(&o.input, &root)).collect();
        match &replay {
            Some(r) => {
                for (c, input) in r.iter().zip(&inputs) {
                    assert_eq!(&c["input"], input, "the generator changed; {REGENERATE}");
                }
            }
            None => {
                let cases: Vec<Value> = out
                    .iter()
                    .zip(&inputs)
                    .map(|(o, input)| {
                        json!({
                            "input": input,
                            "js": portable(&o.js, &root),
                            "jsText": o.js_text.as_ref().map(|t| t.replace(&*root, "<SCRATCH>")),
                        })
                    })
                    .collect();
                let text = serde_json::to_string_pretty(
                    &json!({"recordedWith": recorded_with(), "cases": cases}),
                )
                .unwrap();
                std::fs::write(golden_path(), text + "\n").unwrap();
            }
        }
        out
    })
}

#[test]
fn resolve_verified_anchors_matches_js_on_generated_cases() {
    let out = outcomes();
    let inputs: Vec<Value> = out.iter().map(|o| o.input.clone()).collect();
    let rust: Vec<Value> = out.iter().map(|o| o.rust.clone()).collect();
    let js: Vec<Value> = out.iter().map(|o| o.js.clone()).collect();
    assert_same("resolve-verified-anchors", &inputs, &rust, &js);
    for o in out {
        assert_eq!(o.rust_text, o.js_text, "printed JSON for {}", o.input);
    }
}

#[derive(Default, Debug)]
struct Seen {
    statuses: BTreeMap<String, usize>,
    reasons: BTreeMap<String, usize>,
    notes: BTreeMap<&'static str, usize>,
    crashed: usize,
    empty: usize,
    suspect_by_failure: usize,
    suspect_all_missing: usize,
    unresolved_not_suspect: usize,
    shifted_up: usize,
    shifted_down: usize,
    found_unclaimed: usize,
    reflowed_found: usize,
}

impl Seen {
    fn add(&mut self, o: &Outcome) {
        let v = &o.rust;
        if v.get("threw").is_some() {
            self.crashed += 1;
            return;
        }
        if v["ran"] == false {
            *self
                .reasons
                .entry(v["reason"].as_str().unwrap().to_owned())
                .or_default() += 1;
            return;
        }
        let results = v["results"].as_array().unwrap();
        self.empty += usize::from(results.is_empty());
        let failed = results
            .iter()
            .any(|r| r["status"] == "ref-not-found" || r["status"] == "lookup-failed");
        if v["suspectLookup"] == true {
            if failed {
                self.suspect_by_failure += 1;
            } else {
                self.suspect_all_missing += 1;
            }
        } else if v["unresolved"].as_u64().unwrap() > 0 {
            self.unresolved_not_suspect += 1;
        }
        for r in results {
            let status = r["status"].as_str().unwrap();
            *self.statuses.entry(status.to_owned()).or_default() += 1;
            let note = r["note"].as_str().unwrap_or("");
            let found = status.starts_with("found");
            for (key, hit) in [
                ("no file", note == "no file on the anchor"),
                ("no quote", note == "no quote on the anchor"),
                ("multi-hit", note.contains("times in the file")),
                (
                    "multi-hit cross-repo",
                    note.contains("claimed line (resolved as a cross-repo"),
                ),
                (
                    "cross-repo found",
                    found && note.contains("cross-repo anchor"),
                ),
                (
                    "cross-repo failed",
                    !found && note.contains("(resolved as a cross-repo"),
                ),
                ("command failed", note.starts_with("Command failed: gh api")),
                ("unrecorded", note.starts_with("gh shim: unrecorded")),
                ("cut at 200", note.encode_utf16().count() == 200),
            ] {
                *self.notes.entry(key).or_default() += usize::from(hit);
            }
            let delta = r["delta"].as_f64().unwrap_or(0.0);
            self.shifted_up += usize::from(status == "found-shifted" && delta > 0.0);
            self.shifted_down += usize::from(status == "found-shifted" && delta < 0.0);
            self.found_unclaimed += usize::from(found && r["claimedLine"].is_null());
            let at = (
                r["findingIndex"].as_u64().unwrap() as usize,
                r["anchorIndex"].as_u64().unwrap() as usize,
            );
            self.reflowed_found += usize::from(found && o.reflowed.contains(&at));
        }
    }
}

#[test]
fn the_generated_cases_reach_every_branch() {
    let mut seen = Seen::default();
    for o in outcomes() {
        seen.add(o);
    }
    eprintln!("{seen:#?}");
    for s in [
        "found",
        "found-shifted",
        "not-found",
        "file-not-at-ref",
        "ref-not-found",
        "lookup-failed",
        "malformed",
    ] {
        assert!(
            seen.statuses.get(s).copied().unwrap_or(0) >= 20,
            "{s}: {seen:#?}"
        );
    }
    for r in ["usage", "findings-unparseable", "findings-not-an-array"] {
        assert!(
            seen.reasons.get(r).copied().unwrap_or(0) >= 5,
            "{r}: {seen:#?}"
        );
    }
    for (note, n) in &seen.notes {
        assert!(*n >= 5, "{note}: {seen:#?}");
    }
    assert!(seen.crashed >= 5 && seen.empty >= 10, "{seen:#?}");
    assert!(
        seen.suspect_by_failure >= 20
            && seen.suspect_all_missing >= 5
            && seen.unresolved_not_suspect >= 20,
        "{seen:#?}"
    );
    assert!(
        seen.shifted_up >= 10 && seen.shifted_down >= 10 && seen.found_unclaimed >= 10,
        "{seen:#?}"
    );
    assert!(seen.reflowed_found >= 5, "{seen:#?}");
}

// ---------------------------------------------------------------------------
// Witnesses: a `gh` the recordings cannot stand in for, and the declared
// differences.

fn node_bin() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|d| d.join("node"))
        .find(|p| p.is_file())
        .expect("node must be on PATH")
}

/// The script with `bin` as its whole PATH, so `gh` is whatever `bin` holds.
fn script_with_path(args: &[&str], cwd: &Path, bin: &Path) -> ScriptRun {
    let out = Command::new(node_bin())
        .arg(plugin_root().join(SCRIPT))
        .args(args)
        .current_dir(cwd)
        .env("PATH", bin)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    ScriptRun {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn plain_tools(gh: &Path) -> Tools {
    Tools {
        gh: gh.to_string_lossy().into_owned(),
        ..Tools::default()
    }
}

fn write_anchors(dir: &Path, quote: &str) -> String {
    let p = dir.join("anchors.json");
    let anchors = json!([{"verifiedBy": [{"file": "src/a.ts", "line": 1, "quote": quote}]}]);
    std::fs::write(&p, anchors.to_string()).unwrap();
    p.to_string_lossy().into_owned()
}

/// Node names `gh` in a spawn failure, whatever the port's `Tools` runs.
#[tokio::test]
async fn a_gh_that_cannot_start_matches_js() {
    let dir = scratch("verified-anchors-enoent");
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let anchors = write_anchors(&dir, "const total = 1;");
    let args = [anchors.as_str(), "main", "--repo", "o/r", "--relay"];
    let js = script_with_path(&args, &dir, &bin);
    let (js, js_text) = js_value(&js);
    let rust = run(&args, "", &plain_tools(&bin.join("gh"))).await;
    let (rust, rust_text) = rust_value(rust);
    assert_eq!(js["results"][0]["note"], "spawnSync gh ENOENT", "{js}");
    assert_eq!(rust_text, js_text);
    assert_eq!(rust, js);
}

/// Both sides with `gh` replaced by a shell script that prints `n` bytes of
/// `a` and then `tail` (a `printf` format), to stderr when `fd` is 2.
async fn with_big_gh(dir: &Path, n: usize, tail: &str, fd: u8) -> (Value, Value) {
    let anchors = write_anchors(dir, "aaaaaaaaaaaa");
    let args = [anchors.as_str(), "main", "--repo", "o/r", "--relay"];
    let bin = dir.join(format!("bin-{n}-{fd}"));
    std::fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        format!("#!/bin/sh\n(head -c {n} /dev/zero | tr '\\000' a; printf '{tail}') >&{fd}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let (js, js_text) = js_value(&script_with_path(&args, dir, Path::new(&path)));
    let (rust, rust_text) = rust_value(run(&args, "", &plain_tools(&gh)).await);
    assert!(js_text.is_some() && rust_text.is_some(), "{js} {rust}");
    (js, rust)
}

/// `maxBuffer` is 32 MiB: output of exactly that size is read, one byte more
/// is ENOBUFS.
#[tokio::test]
async fn output_past_the_buffer_matches_js() {
    let dir = scratch("verified-anchors-enobufs");
    for (n, status) in [(33_554_432, "found"), (33_554_433, "lookup-failed")] {
        let (js, rust) = with_big_gh(&dir, n, "", 1).await;
        assert_eq!(js["results"][0]["status"], status, "{js}");
        assert_eq!(rust, js);
    }
}

/// Declared difference: the port measures `maxBuffer` on decoded text, where
/// an invalid byte has become a three-byte U+FFFD.
#[tokio::test]
async fn the_buffer_counting_decoded_text_is_a_declared_difference() {
    let dir = scratch("verified-anchors-enobufs-decoded");
    let (js, rust) = with_big_gh(&dir, 33_554_431, "\\377", 1).await;
    assert_eq!(js["results"][0]["status"], "found", "{js}");
    assert_eq!(rust["results"][0]["note"], "spawnSync gh ENOBUFS", "{rust}");
}

/// Declared difference: Node stops reading stderr at the overflow, the port
/// reads all of it, so only the port sees a 404 written past 32 MiB. Whether
/// Node gets the overflow before the 404 depends on how the pipe delivers the
/// last chunks, so on a loaded machine it sometimes reads the 404 too and
/// answers like the port; only the port's side is asserted strictly.
#[tokio::test]
async fn stderr_past_the_buffer_is_a_declared_difference() {
    let dir = scratch("verified-anchors-enobufs-stderr");
    let (js, rust) = with_big_gh(&dir, 33_554_433, "(HTTP 404)", 2).await;
    assert_eq!(rust["results"][0]["status"], "file-not-at-ref", "{rust}");
    let js_status = js["results"][0]["status"].as_str();
    assert!(
        matches!(js_status, Some("lookup-failed" | "file-not-at-ref")),
        "{js}"
    );
}

/// Declared difference: a `lookup-failed` detail cut at 200 UTF-16 units
/// inside a surrogate pair keeps the lone high half in the script, which
/// `JSON.stringify` escapes; the port has U+FFFD there.
#[tokio::test]
async fn a_detail_cut_inside_a_pair_is_a_declared_difference() {
    let dir = scratch("verified-anchors-cut");
    let anchors = write_anchors(&dir, "const total = 1;");
    let stderr = format!("{}😀 tail", "x".repeat(199));
    let rec = recordings(
        &dir,
        vec![gh_answer(
            gh_args("o/r", "main", "src/a.ts"),
            "",
            &stderr,
            1,
        )],
    );
    let args = [anchors.as_str(), "main", "--repo", "o/r", "--relay"];
    let js = run_script(SCRIPT, &args, &dir, &rec, None);
    let js_line = js.stdout.split('\n').nth(1).unwrap().to_owned();
    let rust = json_stringify(Some(&run(&args, "", &tools(&rec)).await.unwrap()));
    assert!(js_line.contains("x\\ud83d\""), "{js_line}");
    assert!(rust.contains("x\u{FFFD}\""), "{rust}");
    assert_eq!(js_line.replace("\\ud83d", "\u{FFFD}"), rust);
}

/// Declared difference: a quote that is a number literal beyond the double
/// range is Infinity to `JSON.parse`, so the script looks up "Infinity"; the
/// port reads null and reports the anchor as having no quote.
#[tokio::test]
async fn an_infinite_quote_is_a_declared_difference() {
    let dir = scratch("verified-anchors-infinity");
    let anchors = dir.join("anchors.json");
    std::fs::write(
        &anchors,
        r#"[{"verifiedBy":[{"file":"src/a.ts","line":1,"quote":1e400}]}]"#,
    )
    .unwrap();
    let rec = recordings(
        &dir,
        vec![gh_answer(
            gh_args("o/r", "main", "src/a.ts"),
            "x = Infinity;\n",
            "",
            0,
        )],
    );
    let args = [
        anchors.to_str().unwrap(),
        "main",
        "--repo",
        "o/r",
        "--relay",
    ];
    let js = run_script(SCRIPT, &args, &dir, &rec, None);
    let (js, _) = js_value(&js);
    let rust = run(&args, "", &tools(&rec)).await.unwrap();
    assert_eq!(js["results"][0]["status"], "found", "{js}");
    assert_eq!(rust["results"][0]["status"], "malformed", "{rust}");
    assert_eq!(rust["results"][0]["note"], "no quote on the anchor");
}
