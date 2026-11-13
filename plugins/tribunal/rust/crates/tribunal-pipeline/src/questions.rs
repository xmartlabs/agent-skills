//! `--scenario-questions`: a planning step writes concrete scenario questions
//! about the change, and each question is answered in its own dispatch.
//!
//! Measured on labelled defects the lenses almost never find, a lens asked one
//! pointed question about a defect reports it nearly every time, while the
//! same lens reviewing open-endedly almost never does. A generator that has not
//! seen the defect covers more of them when it writes several independent
//! lists: three samples merged did, and so did one dispatch asked for three
//! passes, at about half the cost. A question answered alone is answered
//! better than the same question among a dozen. The answers become candidates like a
//! lens's findings and go through the same refutation panel.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use tokio::sync::Semaphore;
use tribunal_agent::{Agent, DispatchRequest, DispatchResult};
use tribunal_core::js;
use tribunal_core::lens::out_of_diff::reanchor_out_of_diff;

use crate::review::{
    answer_of, dispatch_lenses, futures_join_all, warm_cache_with, ReviewerSetup, ReviewerTask,
};
use crate::schemas;

/// The most generator samples one run may ask for.
pub const MAX_QUESTION_SAMPLES: u32 = 4;

/// The most questions answered on one run, after the samples are merged.
pub const MAX_QUESTIONS: usize = 48;

/// The model that answers the questions when `--question-model` is not given.
pub const DEFAULT_QUESTION_MODEL: &str = "claude-haiku-5-5";

/// The lens label a finding from an answered question carries.
pub const QUESTION_LENS: &str = "scenario-question";

/// What the generator is asked. It names no project, stack or domain: what is
/// specific to the reviewed repo reaches it through the methodology and the
/// conventions in the shared system prompt.
pub const GENERATOR_PROMPT: &str = r#"Do not look for defects in this pass and do not report findings. Your job is to plan the review: write the concrete scenarios a careful reviewer should play forward to test this change.

Go through every changed function, handler, job, script, component and test in the diff, and read the code around it that you need (callers, consumers of each value it returns or writes, sibling implementations of the same action, the config and docs it relies on). For each, think about:
- each failure path: every throw, catch, early return, null or sentinel return, and what each caller or consumer does with it;
- retries, re-deliveries, a second run of the same script or job, and what state the first attempt left behind;
- concurrent or out-of-order execution: two writes racing, a late response, a stalled job;
- in UI code, the user leaving, going Back, reloading or navigating mid-way, an effect re-running, a flag flipping mid-session;
- time: clocks, timeouts against another component's timeout, expiry;
- every contract the change sets or relies on: a caller, a consumer, another repository, a comment or docblock that states what the code does, a test that is meant to fail when the behaviour regresses.

Write each scenario as one question with the concrete trigger and the observable consequence to check, for example: "If the payment provider times out after charging the card, does the retry charge it again, and what does the user see?" Name the file and line it starts from. Plan three independent passes over the change. Each pass starts from a different part of the diff and looks for a different kind of failure, and a scenario one pass already asked does not appear in another. Give at most 12 questions per pass, 36 in all, each pass ranked by how likely the answer reveals a real defect, and prefer the ones a reviewer reading line by line would not ask. List the first pass's questions first, then the second's, then the third's."#;

/// What an answering dispatch gets after its lens's own prompt.
pub const ANSWER_PARAGRAPH: &str = "\n\nOne more question for this pass. Answer it from the code, reading whatever you need. If the answer shows a defect, report it as a finding like any other; if it does not, report nothing for it.\nQuestion: ";

/// How much of a question's text the metrics row keeps.
pub const QUESTION_TEXT_CHARS: usize = 300;

/// The first `max` characters of `s`, on a character boundary.
fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// One scenario question, as the generator wrote it.
#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    pub file: String,
    pub line: Option<u64>,
    pub text: String,
    /// Which generator sample wrote it, from 1.
    pub sample: usize,
    /// Which of the generator's passes it came from, when the generator said.
    pub pass: Option<u64>,
}

impl Question {
    fn anchor(&self) -> String {
        match self.line {
            Some(l) => format!("{}:{l}", self.file),
            None => self.file.clone(),
        }
    }
}

/// The questions in one generator answer. Entries without a file or a
/// question are skipped.
#[must_use]
pub fn questions_of(answer: Option<&Value>, sample: usize) -> Vec<Question> {
    let Some(Value::Array(items)) = answer.and_then(|a| js::get(a, "questions")) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|q| {
            let file = js::get(q, "file").and_then(Value::as_str)?.trim();
            let text = js::get(q, "question").and_then(Value::as_str)?.trim();
            if file.is_empty() || text.is_empty() {
                return None;
            }
            let line = js::get(q, "line")
                .and_then(Value::as_f64)
                .filter(|l| l.is_finite() && *l >= 1.0)
                .map(|l| l as u64);
            let pass = js::get(q, "pass")
                .and_then(Value::as_u64)
                .filter(|p| (1..=3).contains(p));
            Some(Question {
                file: file.to_owned(),
                line,
                text: text.to_owned(),
                sample,
                pass,
            })
        })
        .collect()
}

fn words(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(str::to_lowercase)
        .collect()
}

/// Share of words two questions have in common, over the words either uses.
fn overlap(a: &str, b: &str) -> f64 {
    let (a, b) = (words(a), words(b));
    let all = a.union(&b).count();
    if all == 0 {
        return 0.0;
    }
    a.intersection(&b).count() as f64 / all as f64
}

/// Two questions on the same file, within this many lines of each other when
/// both name a line, whose wording overlaps this much ask the same thing. On
/// 797 questions from three generator samples over 22 pull requests, merging
/// at this level dropped none that covered a labelled defect.
const SAME_QUESTION_OVERLAP: f64 = 0.8;
const SAME_QUESTION_LINES: u64 = 5;

fn same_question(a: &Question, b: &Question) -> bool {
    let near = match (a.line, b.line) {
        (Some(x), Some(y)) => x.abs_diff(y) <= SAME_QUESTION_LINES,
        _ => true,
    };
    a.file == b.file && near && overlap(&a.text, &b.text) >= SAME_QUESTION_OVERLAP
}

/// The samples merged into one list: taken round-robin, one question of each
/// sample in its own rank order, so the cap keeps every sample's top questions
/// before anyone's tail; a question that repeats one already kept is dropped.
/// At most `cap` questions.
#[must_use]
pub fn merge_questions(samples: &[Vec<Question>], cap: usize) -> Vec<Question> {
    let mut kept: Vec<Question> = Vec::new();
    let longest = samples.iter().map(Vec::len).max().unwrap_or(0);
    'outer: for rank in 0..longest {
        for sample in samples {
            let Some(q) = sample.get(rank) else { continue };
            if kept.len() >= cap {
                break 'outer;
            }
            let repeat = kept.iter().any(|k| same_question(k, q));
            if !repeat {
                kept.push(q.clone());
            }
        }
    }
    kept
}

/// The answers run at this many times the run's concurrency (16 at the
/// default 8). Each is a Haiku dispatch that reads code for 1 to 3 minutes,
/// and at 8 the answers were 60 to 70% of a run's wall clock; one run at 16
/// took about 9 seconds a question against 15, at the same cost.
pub const ANSWER_CONCURRENCY_FACTOR: usize = 2;

/// A file with no behaviour to question: lockfiles, snapshots, minified or
/// built output, generated code, binaries and prose docs. A question about one
/// is not answered. Across 256 labelled question findings, the two on docs
/// were not defects, and none of the 247 labelled corpus defects is in a file
/// of these kinds. Tests, config and migrations stay: the corpus has real
/// defects in each.
#[must_use]
pub fn no_logic_file(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    let name = p.rsplit('/').next().unwrap_or(&p);
    const NAMES: [&str; 9] = [
        "package-lock.json",
        "npm-shrinkwrap.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "cargo.lock",
        "poetry.lock",
        "gemfile.lock",
        "composer.lock",
        "go.sum",
    ];
    const SUFFIXES: [&str; 23] = [
        ".lock", ".snap", ".min.js", ".min.css", ".map", ".pb.go", "_pb2.py", ".md", ".mdx",
        ".rst", ".png", ".jpg", ".jpeg", ".gif", ".ico", ".webp", ".pdf", ".woff", ".woff2",
        ".ttf", ".eot", ".zip", ".gz",
    ];
    const DIRS: [&str; 5] = [
        "__snapshots__",
        "node_modules",
        "dist",
        "generated",
        "__generated__",
    ];
    NAMES.contains(&name)
        || SUFFIXES.iter().any(|s| name.ends_with(s))
        || name.contains(".generated.")
        || p.split('/').rev().skip(1).any(|d| DIRS.contains(&d))
}

fn same_file(a: &str, b: &str) -> bool {
    a == b || a.ends_with(&format!("/{b}")) || b.ends_with(&format!("/{a}"))
}

/// Which lens's prompt answers a question: the first lens whose files include
/// the question's file, else the first lens that sees every file, else the
/// first lens. `None` only when there is no lens.
#[must_use]
pub fn route(question_file: &str, lens_files: &[Option<Vec<String>>]) -> Option<usize> {
    lens_files
        .iter()
        .position(|f| {
            f.as_ref()
                .is_some_and(|files| files.iter().any(|p| same_file(p, question_file)))
        })
        .or_else(|| lens_files.iter().position(Option::is_none))
        .or(if lens_files.is_empty() { None } else { Some(0) })
}

/// The prompt that answers `q` with the lens prompt `lens_prompt`.
#[must_use]
pub fn answer_prompt(lens_prompt: &str, q: &Question) -> String {
    format!(
        "{}{ANSWER_PARAGRAPH}{} {}\n",
        lens_prompt.trim_end(),
        q.anchor(),
        q.text
    )
}

/// The generator's prompt. When the diff is not in the shared system prompt,
/// the generator is told where it is.
#[must_use]
pub fn generator_prompt(diff_in_system_prompt: bool, diff_path: Option<&str>) -> String {
    match (diff_in_system_prompt, diff_path) {
        (false, Some(p)) => format!("{GENERATOR_PROMPT}\n\nThe PR diff is at {p}. Read it first."),
        _ => GENERATOR_PROMPT.to_owned(),
    }
}

/// What the ranking dispatch is told. It compares every finding the panel kept
/// against the others, because judged one by one, findings raised by a leading
/// question looked as real as the defects they were mixed with. The text came
/// from an evolutionary search over prompts against 250 labelled findings: on
/// pull requests held out of the search it separated real defects from noise
/// at AUC 0.75 to 0.79, against 0.71 for the hand-written prompt it started from.
pub const RANK_PROMPT: &str = "You rank the findings of one code review of a pull request. Read the code in the working directory (the pull request head) as needed. Order every finding from the one a reviewer would most want to see to the least. Return every id exactly once, each with a worth from 1 (noise) to 5 (must fix before merging). Use the full range; do not give most findings the same worth.\n\nMethod: for each finding, find the exact lines it concerns and trace the real path: callers, upstream validators, the producer of the input, the other side of the interface as it is at this snapshot, and the stated intent (description, paired change, comments). Ignore confident tone, severity labels, and security/race/injection vocabulary. Ask: does a concrete input or sequence produce a wrong outcome, and would anyone notice?\n\nThen try to refute it. Refuted if: another guard already covers it; an upstream check rules out the value (e.g. a balance, id, or count that earlier validation keeps in range); the producer never emits the feared value or code at this snapshot (later additions don't count); a UI control or in-flight flag already prevents the repeat action; the code matches the other side's own wording or the stated intent; ordering already prevents the race (e.g. a record created synchronously before the step that reads it). A \"missing handling\", \"no guard\", or \"ignores the input\" claim is usually deliberate or unreachable unless you can show the path. If a refutation holds, rank 1-2 however plausible it sounds.\n\nWorth is about what happens if this pull request merges as it is:\n- 5: a concrete input or sequence produces a wrong outcome that a user, the stored data or another system would see (a crash or failed request, a wrong amount or status, a record lost, duplicated or misrouted, a message not sent or sent wrong, a page that breaks), and you traced the path in the code.\n- 4: the same kind of outcome, but the path depends on a condition you could not confirm at this snapshot.\n- A path that needs the infrastructure to fail at one moment (the database, the network, a queue or a third-party service failing between two steps the code takes) is 4 at most: real and worth fixing, but not a reason to hold the merge. It can still be 5 when the pull request makes that failure likely: it adds the slow or flaky call, takes a second connection, or the step fails on ordinary input.\n\nRank middle (3): real but minor or easily recovered.\nRank low (1-2): misread code, deliberate behaviour, unchanged code, style, hypothetical future worries, issues that only exist because of later changes.\n\nAmong survivors, put the best-verified and most consequential first.";

/// A finding from an answered question stays in the report when the ranking
/// gives it at least this worth, or when this many candidates were merged into
/// it; below both it moves to the report's "questions to review" section. It
/// equals [`BLOCKING_WORTH`]: a kept question finding below it could not block
/// anyway, and on 28 runs of a pilot against pull requests that later broke
/// and ones that never did, moving worth 3 out too cut the report body by 38%
/// and changed no verdict.
pub const KEEP_WORTH: u64 = 4;
pub const KEEP_SUPPORT: usize = 4;

/// The `placement` a moved finding carries, which keeps it out of the verdict.
pub const LOWER_PRIORITY: &str = "lower-priority-question";

/// A question finding kept in the report blocks only when the ranking gives
/// it at least this worth, or [`KEEP_SUPPORT`] candidates were merged into it.
/// On 250 labelled findings, worth 4 or more was a real defect about 7 times
/// in 10, while the answers' own severity was inflated.
pub const BLOCKING_WORTH: u64 = 4;

/// The `--block-min-worth` the CLI applies when neither it nor
/// `--no-block-min-worth` is given. On a pilot of 14 pull requests that later
/// broke and 14 that never did, worth 5 left a blocker on about 7 broken
/// and 3 clean pull requests across three ranking draws, against 13 and 11
/// with severity alone.
pub const DEFAULT_BLOCK_MIN_WORTH: u64 = 5;

/// The finding field that keeps a kept question finding out of the verdict's
/// blockers. `verdict_for`, the verdict basis and the row's `blocking` read it.
pub const RANKED_BELOW_BLOCKING: &str = "rankedBelowBlocking";

/// The finding field that marks a kept finding the ranking put at or above
/// `--block-min-worth`. `verdict_for` counts an important one as a blocker
/// even when no lens flag escalates it, so a finding the report lists under
/// "Blocking" also turns the verdict into REQUEST CHANGES.
pub const RANKED_AT_BLOCKING: &str = "rankedAtBlocking";

/// Which setup the ranking after the panel runs with: the question stage's,
/// or, under `--block-min-worth` with no question stage, the lenses'. `None`
/// means no ranking runs.
#[must_use]
pub fn ranking_setup<'a>(
    questions: Option<&'a QuestionOutcome>,
    lens_setup: Option<&'a ReviewerSetup>,
    min_worth: Option<u64>,
) -> Option<&'a ReviewerSetup> {
    questions
        .and_then(|q| q.setup.as_ref())
        .or_else(|| min_worth.and(lens_setup))
}

/// The finding field holding the worth the ranking gave it, 1 to 5, on every
/// finding the ranking scored, so a threshold can be tried again offline.
pub const RANK_WORTH: &str = "rankWorth";

fn rank_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "ranking": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {"id": {"type": "string"}, "worth": {"type": "integer"}},
                    "required": ["id", "worth"]
                }
            }
        },
        "required": ["ranking"]
    })
}

fn finding_of(result: &Value) -> &Value {
    js::get(result, "finding").unwrap_or(result)
}

/// How many candidates were merged into the result's finding, at least one.
#[must_use]
pub fn support_of(result: &Value) -> usize {
    js::get(finding_of(result), "mergedFrom")
        .and_then(Value::as_array)
        .map_or(1, |m| m.len().max(1))
}

/// The ranking prompt for these results: one line per finding, numbered from 1.
#[must_use]
pub fn rank_prompt(repo: &str, pr: &str, head: &str, results: &[Value]) -> String {
    let mut out = format!(
        "Pull request {repo}#{pr}, head {head}. The review reported these {} findings:\n\n",
        results.len()
    );
    for (i, r) in results.iter().enumerate() {
        let f = finding_of(r);
        let s = |k: &str| {
            js::get(f, k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        let line = js::get(f, "line").map_or(String::new(), |l| match l {
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        });
        let evidence: String = s("evidence")
            .chars()
            .take(300)
            .collect::<String>()
            .replace('\n', " ");
        out.push_str(&format!(
            "[{}] {}:{} | {}\n    {}\n",
            i + 1,
            s("file"),
            line,
            s("title"),
            evidence
        ));
    }
    out.push_str("\nRank them.");
    out
}

/// The worth the ranking gave each result, by position; `None` where the
/// ranking left a result out or returned nothing.
#[must_use]
pub fn worths(answer: Option<&Value>, count: usize) -> Vec<Option<u64>> {
    let mut out = vec![None; count];
    if let Some(Value::Array(items)) = answer.and_then(|a| js::get(a, "ranking")) {
        for item in items {
            let id = js::get(item, "id")
                .and_then(Value::as_str)
                .and_then(|s| s.trim_matches(['[', ']']).parse::<usize>().ok());
            let worth = js::get(item, "worth").and_then(Value::as_u64);
            if let (Some(id), Some(w)) = (id, worth) {
                if (1..=count).contains(&id) {
                    out[id - 1] = Some(w);
                }
            }
        }
    }
    out
}

/// Whether a result from an answered question stays in the report.
#[must_use]
pub fn keeps(worth: Option<u64>, support: usize) -> bool {
    match worth {
        None => true,
        Some(w) => w >= KEEP_WORTH || support >= KEEP_SUPPORT,
    }
}

/// Moves the surviving results from answered questions that rank below the
/// bar into `placed_aside`, with a [`LOWER_PRIORITY`] placement. Lens results
/// are never moved. A kept question finding below [`BLOCKING_WORTH`] is marked
/// [`RANKED_BELOW_BLOCKING`], and so is any kept finding ranked below
/// `min_worth` when it is given (`--block-min-worth`). A finding the ranking
/// did not score is never marked. Returns how many were ranked, moved and
/// marked.
pub fn place_lower_priority(
    surviving: &mut Vec<Value>,
    placed_aside: &mut Vec<Value>,
    worth: &[Option<u64>],
    min_worth: Option<u64>,
) -> (usize, usize, usize) {
    let ranked = worth.iter().filter(|w| w.is_some()).count();
    let mut kept = Vec::new();
    let mut moved = 0;
    let mut below = 0;
    for (r, w) in surviving
        .drain(..)
        .zip(worth.iter().copied().chain(std::iter::repeat(None)))
    {
        let mut r = r;
        if let (Some(w), Some(Value::Object(f))) = (w, r.get_mut("finding")) {
            f.insert(RANK_WORTH.into(), json!(w));
        }
        let from_question = js::get(finding_of(&r), "lens") == Some(&json!(QUESTION_LENS));
        let below_blocking = (from_question
            && w.is_some_and(|w| w < BLOCKING_WORTH)
            && support_of(&r) < KEEP_SUPPORT)
            || min_worth.is_some_and(|n| w.is_some_and(|w| w < n));
        if from_question && !keeps(w, support_of(&r)) {
            if let Value::Object(m) = &mut r {
                m.insert(
                    "placement".into(),
                    json!({"placement": LOWER_PRIORITY, "worth": w, "support": support_of(&Value::Object(m.clone()))}),
                );
            }
            placed_aside.push(r);
            moved += 1;
        } else {
            if below_blocking {
                below += 1;
                if let Some(Value::Object(f)) = r.get_mut("finding") {
                    f.insert(RANKED_BELOW_BLOCKING.into(), json!(true));
                }
            } else if min_worth.is_some_and(|n| w.is_some_and(|w| w >= n)) {
                if let Some(Value::Object(f)) = r.get_mut("finding") {
                    f.insert(RANKED_AT_BLOCKING.into(), json!(true));
                }
            }
            kept.push(r);
        }
    }
    *surviving = kept;
    (ranked, moved, below)
}

fn result_key(v: &Value) -> String {
    let f = finding_of(v);
    match js::get(f, "slotId") {
        Some(id) if !id.is_null() => format!("slot:{id}"),
        _ => format!(
            "{}|{}|{}",
            js::get(f, "title").and_then(Value::as_str).unwrap_or(""),
            js::get(f, "file").and_then(Value::as_str).unwrap_or(""),
            js::get(f, "line").map_or(String::new(), Value::to_string)
        ),
    }
}

/// Carries what the ranking changed in `surviving` and `placed_aside` back to
/// `results`, the list the metrics row is built from, so the row records each
/// moved finding's placement and each `rankedBelowBlocking` mark.
pub fn sync_results(results: &mut [Value], surviving: &[Value], placed_aside: &[Value]) {
    let changed: std::collections::HashMap<String, &Value> = surviving
        .iter()
        .chain(placed_aside)
        .map(|v| (result_key(v), v))
        .collect();
    for r in results.iter_mut() {
        if let Some(v) = changed.get(&result_key(r)) {
            *r = (*v).clone();
        }
    }
}

/// Ranks the results the panel kept and moves the low ones from answered
/// questions out of the verdict. One dispatch; when it returns nothing, nothing
/// moves.
pub async fn rank_and_place<A: Agent>(
    agent: &A,
    setup: &ReviewerSetup,
    prompt: String,
    surviving: &mut Vec<Value>,
    placed_aside: &mut Vec<Value>,
    min_worth: Option<u64>,
) -> (DispatchResult, Value) {
    let mut req = DispatchRequest::new("question-rank", prompt, setup.cwd.clone());
    req.system_prompt = Some(RANK_PROMPT.to_owned());
    req.model = Some(setup.model.clone());
    req.effort = setup.effort.clone();
    req.tools = setup.agent.tools.clone();
    req.schema = Some(rank_schema());
    req.timeout = setup.timeout;
    let res = agent.run(&req).await;
    let w = worths(answer_of(&res).as_ref(), surviving.len());
    let (ranked, moved, below) = place_lower_priority(surviving, placed_aside, &w, min_worth);
    let summary = json!({"ranked": ranked, "movedToQuestionsToReview": moved, "keptInReport": surviving.len(), "belowBlocking": below});
    (res, summary)
}

/// Everything the question stage produced.
#[derive(Debug, Default)]
pub struct QuestionOutcome {
    pub samples: u32,
    pub model: String,
    /// Questions written across all samples, before the merge.
    pub generated: usize,
    pub questions: Vec<Question>,
    /// How many answering dispatches came back with an answer.
    pub answered: usize,
    /// The answers' findings that passed the out-of-diff gate, labelled
    /// [`QUESTION_LENS`], each with the question it answered.
    pub candidates: Vec<Value>,
    /// Findings dropped by the out-of-diff gate.
    pub dropped_out_of_diff: usize,
    /// Merged questions not answered because their file has no behaviour to
    /// question ([`no_logic_file`]).
    pub skipped_no_logic: usize,
    /// Every dispatch, labelled `questions:<n>` (generators),
    /// `question-warmup` or `question:<n>` (answers).
    pub dispatches: Vec<(String, DispatchResult)>,
    /// The lenses' setup, for the ranking after the panel.
    pub setup: Option<ReviewerSetup>,
    /// Wall-clock seconds of the whole stage, generators and answers.
    pub seconds: f64,
    /// One record per merged question: where it came from and what its
    /// answer gave, for the metrics row.
    pub records: Vec<Value>,
}

impl QuestionOutcome {
    /// The `scenarioQuestions` field of the metrics row.
    #[must_use]
    pub fn metrics(&self) -> Value {
        json!({
            "samples": self.samples,
            "answerModel": self.model,
            "generated": self.generated,
            "asked": self.questions.len(),
            "answered": self.answered,
            "candidates": self.candidates.len(),
            "droppedOutOfDiff": self.dropped_out_of_diff,
            "skippedNoLogicFile": self.skipped_no_logic,
            "seconds": (self.seconds * 10.0).round() / 10.0,
            "questions": self.records,
        })
    }
}

/// The inputs of [`question_stage`] that come from the lens stage.
pub struct QuestionInputs<'a> {
    /// The lenses' setup: the generators run with it.
    pub lens_setup: &'a ReviewerSetup,
    /// The model the answers run on.
    pub answer_model: &'a str,
    pub samples: u32,
    /// Each planned lens's full prompt and its file restriction, in plan order.
    pub lens_prompts: &'a [String],
    pub lens_files: &'a [Option<Vec<String>>],
    pub lens_schema: &'a Value,
    pub generator_prompt: String,
    pub pr_files: &'a [String],
    pub blast_radius: Option<&'a Value>,
}

async fn run_all<A: Agent>(
    agent: &A,
    concurrency: usize,
    reqs: Vec<DispatchRequest>,
) -> Vec<DispatchResult> {
    let gate = Arc::new(Semaphore::new(concurrency.max(1)));
    let runs = reqs.into_iter().map(|req| {
        let gate = Arc::clone(&gate);
        async move {
            let _permit = gate
                .acquire_owned()
                .await
                .expect("semaphore is never closed");
            agent.run(&req).await
        }
    });
    futures_join_all(runs).await
}

fn request(
    setup: &ReviewerSetup,
    model: &str,
    label: String,
    prompt: String,
    schema: Value,
) -> DispatchRequest {
    let mut req = DispatchRequest::new(label, prompt, setup.cwd.clone());
    req.system_prompt = Some(setup.agent.body.clone());
    req.model = Some(model.to_owned());
    req.effort = setup.effort.clone();
    req.tools = setup.agent.tools.clone();
    req.schema = Some(schema);
    req.timeout = setup.timeout;
    req
}

/// Generates the questions, merges the samples and answers each question in
/// its own dispatch.
pub async fn question_stage<A: Agent>(agent: &A, input: &QuestionInputs<'_>) -> QuestionOutcome {
    let started = std::time::Instant::now();
    let setup = input.lens_setup;
    let samples = input.samples.clamp(1, MAX_QUESTION_SAMPLES);
    let mut out = QuestionOutcome {
        samples,
        model: input.answer_model.to_owned(),
        setup: Some(setup.clone()),
        ..QuestionOutcome::default()
    };
    let gens: Vec<DispatchRequest> = (1..=samples)
        .map(|n| {
            request(
                setup,
                &setup.model,
                format!("questions:{n}"),
                input.generator_prompt.clone(),
                schemas::scenario_questions(),
            )
        })
        .collect();
    let labels: Vec<String> = gens.iter().map(|r| r.label.clone()).collect();
    let results = run_all(agent, setup.concurrency, gens).await;
    let mut per_sample = Vec::new();
    for (i, (label, res)) in labels.into_iter().zip(results).enumerate() {
        per_sample.push(questions_of(answer_of(&res).as_ref(), i + 1));
        out.dispatches.push((label, res));
    }
    out.generated = per_sample.iter().map(Vec::len).sum();
    out.questions = merge_questions(&per_sample, MAX_QUESTIONS);
    out.records = out
        .questions
        .iter()
        .enumerate()
        .map(|(i, q)| {
            json!({"position": i + 1, "sample": q.sample, "pass": q.pass, "file": q.file,
                   "line": q.line, "text": clip(&q.text, QUESTION_TEXT_CHARS),
                   "routed": false, "answered": false, "findings": 0, "droppedOutOfDiff": 0})
        })
        .collect();

    let mut asks = Vec::new();
    let mut routed = Vec::new();
    for (i, q) in out.questions.iter().enumerate() {
        if no_logic_file(&q.file) {
            out.records[i]["skipped"] = json!("no-logic-file");
            out.skipped_no_logic += 1;
            continue;
        }
        let Some(slot) = route(&q.file, input.lens_files) else {
            continue;
        };
        let Some(prompt) = input.lens_prompts.get(slot) else {
            continue;
        };
        out.records[i]["routed"] = json!(true);
        asks.push(request(
            setup,
            input.answer_model,
            format!("question:{}", i + 1),
            answer_prompt(prompt, q),
            input.lens_schema.clone(),
        ));
        routed.push(i);
    }
    if asks.len() >= 2 {
        let mut warm_setup = setup.clone();
        warm_setup.model = input.answer_model.to_owned();
        let w = warm_cache_with(agent, &warm_setup, input.lens_schema.clone()).await;
        out.dispatches.push(("question-warmup".into(), w));
    }
    let labels: Vec<String> = asks.iter().map(|r| r.label.clone()).collect();
    let answer_concurrency = setup.concurrency.max(1) * ANSWER_CONCURRENCY_FACTOR;
    let results = run_all(agent, answer_concurrency, asks).await;
    for ((label, res), qi) in labels.into_iter().zip(results).zip(routed) {
        if let Some(path) = res.transcript.as_deref() {
            out.records[qi]["filesRead"] = json!(crate::coverage_report::files_opened(
                &crate::coverage_report::calls_in_transcript(path),
                input.pr_files,
            ));
        }
        let q = &out.questions[qi];
        if let Some(Value::Array(findings)) = answer_of(&res)
            .as_ref()
            .and_then(|a| js::get(a, "findings"))
            .cloned()
        {
            out.answered += 1;
            out.records[qi]["answered"] = json!(true);
            for f in findings {
                let mut f = match f {
                    Value::Object(m) => m,
                    _ => Map::new(),
                };
                f.insert("lens".into(), json!(QUESTION_LENS));
                f.insert("mergedFrom".into(), json!([]));
                f.insert(
                    "scenarioQuestion".into(),
                    json!(format!("{} {}", q.anchor(), q.text)),
                );
                match reanchor_out_of_diff(&Value::Object(f), input.pr_files, input.blast_radius) {
                    Ok(Some(kept)) => {
                        out.candidates.push(kept);
                        bump(&mut out.records[qi], "findings");
                    }
                    _ => {
                        out.dropped_out_of_diff += 1;
                        bump(&mut out.records[qi], "droppedOutOfDiff");
                    }
                }
            }
        }
        out.dispatches.push((label, res));
    }
    out.seconds = started.elapsed().as_secs_f64();
    out
}

/// What [`lenses_with_questions`] returns.
pub struct LensesAndQuestions {
    pub dispatches: Vec<DispatchResult>,
    pub warmup: Option<DispatchResult>,
    /// Wall-clock seconds of the lens dispatches alone.
    pub lens_seconds: f64,
    pub questions: Option<QuestionOutcome>,
}

/// Runs the lenses and, when asked, the question stage at the same time. The
/// question stage reads the lens prompts and plan, never the lens answers, so
/// waiting for the lenses cost a run their whole duration.
pub async fn lenses_with_questions<A: Agent>(
    agent: &A,
    setup: &ReviewerSetup,
    tasks: &[ReviewerTask],
    warm: bool,
    questions: Option<&QuestionInputs<'_>>,
) -> LensesAndQuestions {
    let lenses = async {
        let started = std::time::Instant::now();
        let (dispatches, warmup) = dispatch_lenses(agent, setup, tasks, warm).await;
        (dispatches, warmup, started.elapsed().as_secs_f64())
    };
    let asked = async {
        match questions {
            Some(input) => Some(question_stage(agent, input).await),
            None => None,
        }
    };
    let ((dispatches, warmup, lens_seconds), questions) = tokio::join!(lenses, asked);
    LensesAndQuestions {
        dispatches,
        warmup,
        lens_seconds,
        questions,
    }
}

fn bump(record: &mut Value, key: &str) {
    record[key] = json!(js::get(record, key).and_then(Value::as_u64).unwrap_or(0) + 1);
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_question_text_is_clipped_on_a_character_boundary() {
        assert_eq!(super::clip("¿Qué pasa?", 4), "¿Qué");
        assert_eq!(super::clip("short", super::QUESTION_TEXT_CHARS), "short");
        let long = "x".repeat(super::QUESTION_TEXT_CHARS + 50);
        assert_eq!(
            super::clip(&long, super::QUESTION_TEXT_CHARS)
                .chars()
                .count(),
            super::QUESTION_TEXT_CHARS
        );
    }

    #[test]
    fn sync_results_carries_the_ranking_marks_to_the_full_list() {
        let a = json!({"survived": true, "finding": {"title": "a", "slotId": 0, "lens": QUESTION_LENS}});
        let b = json!({"survived": true, "finding": {"title": "b", "slotId": 1, "lens": QUESTION_LENS}});
        let refuted = json!({"survived": false, "finding": {"title": "c", "slotId": 2}});
        let mut results = vec![a.clone(), b.clone(), refuted.clone()];
        let mut surviving = vec![a, b];
        let mut aside = Vec::new();
        place_lower_priority(&mut surviving, &mut aside, &[Some(1), Some(4)], Some(5));
        sync_results(&mut results, &surviving, &aside);
        assert_eq!(results[0]["placement"]["placement"], json!(LOWER_PRIORITY));
        assert_eq!(results[1]["finding"][RANKED_BELOW_BLOCKING], json!(true));
        assert_eq!(results[0]["finding"][RANK_WORTH], json!(1));
        assert_eq!(results[1]["finding"][RANK_WORTH], json!(4));
        assert_eq!(results[2], refuted);
    }

    #[test]
    fn a_finding_ranked_at_the_bar_is_marked_only_under_block_min_worth() {
        let f = |t: &str| json!({"survived": true, "finding": {"title": t, "lens": "correctness"}});
        let mut surviving = vec![f("five"), f("four"), f("unranked")];
        let mut aside = Vec::new();
        place_lower_priority(
            &mut surviving,
            &mut aside,
            &[Some(5), Some(4), None],
            Some(5),
        );
        assert_eq!(surviving[0]["finding"][RANKED_AT_BLOCKING], json!(true));
        assert!(surviving[1]["finding"].get(RANKED_AT_BLOCKING).is_none());
        assert_eq!(surviving[1]["finding"][RANKED_BELOW_BLOCKING], json!(true));
        assert!(surviving[2]["finding"].get(RANKED_AT_BLOCKING).is_none());

        let mut no_gate = vec![f("five")];
        place_lower_priority(&mut no_gate, &mut aside, &[Some(5)], None);
        assert!(no_gate[0]["finding"].get(RANKED_AT_BLOCKING).is_none());
    }
    use super::*;

    fn q(file: &str, text: &str, sample: usize) -> Question {
        Question {
            file: file.into(),
            line: None,
            text: text.into(),
            sample,
            pass: None,
        }
    }

    #[test]
    fn merge_takes_samples_round_robin_and_drops_repeats_on_the_same_file() {
        let a = vec![
            q(
                "a.ts",
                "If the job retries after a timeout, is the row written twice?",
                1,
            ),
            q("b.ts", "Does the guard reject an empty list?", 1),
        ];
        let b = vec![
            q(
                "a.ts",
                "If the job retries after a timeout, is the row written twice again?",
                2,
            ),
            q("c.ts", "What does the caller do with a null result?", 2),
        ];
        let m = merge_questions(&[a, b], 10);
        let texts: Vec<&str> = m.iter().map(|q| q.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "If the job retries after a timeout, is the row written twice?",
                "Does the guard reject an empty list?",
                "What does the caller do with a null result?",
            ]
        );
    }

    #[test]
    fn the_same_wording_on_another_file_is_kept() {
        let a = vec![q("a.ts", "What does the caller do with a null result?", 1)];
        let b = vec![q("b.ts", "What does the caller do with a null result?", 2)];
        assert_eq!(merge_questions(&[a, b], 10).len(), 2);
    }

    #[test]
    fn the_cap_keeps_each_samples_top_questions_first() {
        let topics = ["retries", "timeouts", "rollback", "pagination", "locking"];
        let a: Vec<Question> = topics
            .iter()
            .map(|t| {
                q(
                    "a.ts",
                    &format!("What happens to {t} when the provider fails?"),
                    1,
                )
            })
            .collect();
        let b: Vec<Question> = topics
            .iter()
            .map(|t| {
                q(
                    "b.ts",
                    &format!("Does the caller handle {t} after a crash?"),
                    2,
                )
            })
            .collect();
        let m = merge_questions(&[a, b], 4);
        let samples: Vec<usize> = m.iter().map(|q| q.sample).collect();
        assert_eq!(samples, [1, 2, 1, 2]);
    }

    #[test]
    fn files_with_no_logic_are_told_apart_from_code_tests_and_config() {
        for f in [
            "package-lock.json",
            "web/yarn.lock",
            "pnpm-lock.yaml",
            "src/__snapshots__/Banner.test.tsx.snap",
            "public/app.min.js",
            "dist/index.js",
            "src/generated/client.ts",
            "src/api.generated.ts",
            "docs/runbook.md",
            "README.MD",
            "assets/logo.png",
        ] {
            assert!(no_logic_file(f), "{f}");
        }
        for f in [
            "src/jobs/sync.ts",
            "src/jobs/sync.test.ts",
            "package.json",
            "prisma/migrations/20260101_init/migration.sql",
            "src/types.ts",
            "scripts/build/run.ts",
            "src/distance.ts",
            "src/lockout.ts",
        ] {
            assert!(!no_logic_file(f), "{f}");
        }
    }

    #[test]
    fn route_prefers_the_lens_that_has_the_file() {
        let files = vec![
            Some(vec!["src/a.ts".to_owned()]),
            None,
            Some(vec!["src/b.ts".to_owned()]),
        ];
        assert_eq!(route("src/b.ts", &files), Some(2));
        assert_eq!(route("b.ts", &files), Some(2));
        assert_eq!(route("src/other.ts", &files), Some(1));
        assert_eq!(route("src/other.ts", &[Some(vec!["x".into()])]), Some(0));
        assert_eq!(route("src/other.ts", &[]), None);
    }

    #[test]
    fn questions_of_skips_entries_without_a_file_or_a_question() {
        let a = json!({"questions": [
            {"file": "src/a.ts", "line": 12, "question": "What happens on a 503?"},
            {"file": "", "question": "no file"},
            {"file": "src/b.ts", "question": "  "},
            {"file": "src/c.ts", "line": 0, "question": "Line zero is no line"}
        ]});
        let qs = questions_of(Some(&a), 2);
        assert_eq!(qs.len(), 2);
        assert_eq!(qs[0].line, Some(12));
        assert_eq!(qs[1].line, None);
        assert!(qs.iter().all(|q| q.sample == 2));
    }

    #[test]
    fn the_answer_prompt_ends_with_the_anchored_question() {
        let mut question = q("src/a.ts", "What happens on a 503?", 1);
        question.line = Some(7);
        let p = answer_prompt("LENS PROMPT\n\n", &question);
        assert!(p.starts_with("LENS PROMPT\n\nOne more question for this pass."));
        assert!(p.ends_with("Question: src/a.ts:7 What happens on a 503?\n"));
    }

    #[test]
    fn one_generator_dispatch_asks_for_three_passes_of_twelve() {
        // Measured 2026-11-09: one dispatch asked for three passes found as
        // many labelled defects as three single-pass dispatches, at about half
        // the cost of the question stage.
        assert!(GENERATOR_PROMPT.contains("Plan three independent passes"));
        assert!(GENERATOR_PROMPT.contains("at most 12 questions per pass, 36 in all"));
        assert!(!GENERATOR_PROMPT.contains("Give at most 12 questions, ranked"));
    }

    #[test]
    fn the_generator_is_told_where_the_diff_is_only_when_it_is_not_in_the_system_prompt() {
        assert_eq!(
            generator_prompt(true, Some("/tmp/d.patch")),
            GENERATOR_PROMPT
        );
        assert!(generator_prompt(false, Some("/tmp/d.patch"))
            .ends_with("The PR diff is at /tmp/d.patch. Read it first."));
        assert_eq!(generator_prompt(false, None), GENERATOR_PROMPT);
    }
}
