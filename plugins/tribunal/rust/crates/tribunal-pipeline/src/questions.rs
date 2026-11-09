//! `--scenario-questions`: a planning step writes concrete scenario questions
//! about the change, and each question is answered in its own dispatch.
//!
//! Measured on labelled defects the lenses almost never find, a lens asked one
//! pointed question about a defect reports it nearly every time, while the
//! same lens reviewing open-endedly almost never does. A generator that has not
//! seen the defect covers more of them when it is sampled several times and
//! the samples are merged, and a question answered alone is answered better
//! than the same question among a dozen. The answers become candidates like a
//! lens's findings and go through the same refutation panel.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use tokio::sync::Semaphore;
use tribunal_agent::{Agent, DispatchRequest, DispatchResult};
use tribunal_core::js;
use tribunal_core::lens::out_of_diff::reanchor_out_of_diff;

use crate::review::{answer_of, futures_join_all, warm_cache_with, ReviewerSetup};
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

Write each scenario as one question with the concrete trigger and the observable consequence to check, for example: "If the payment provider times out after charging the card, does the retry charge it again, and what does the user see?" Name the file and line it starts from. Give at most 12 questions, ranked by how likely the answer reveals a real defect, and prefer the ones a reviewer reading line by line would not ask."#;

/// What an answering dispatch gets after its lens's own prompt.
pub const ANSWER_PARAGRAPH: &str = "\n\nOne more question for this pass. Answer it from the code, reading whatever you need. If the answer shows a defect, report it as a finding like any other; if it does not, report nothing for it.\nQuestion: ";

/// One scenario question, as the generator wrote it.
#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    pub file: String,
    pub line: Option<u64>,
    pub text: String,
    /// Which generator sample wrote it, from 1.
    pub sample: usize,
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
            Some(Question {
                file: file.to_owned(),
                line,
                text: text.to_owned(),
                sample,
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
pub const RANK_PROMPT: &str = "You rank the findings of one code review of a pull request. Read the code in the working directory (the pull request head) as needed. Order every finding from the one a reviewer would most want to see to the least. Return every id exactly once, each with a worth from 1 (noise) to 5 (must fix before merging). Use the full range; do not give most findings the same worth.\n\nMethod: for each finding, find the exact lines it concerns and trace the real path: callers, upstream validators, the producer of the input, the other side of the interface as it is at this snapshot, and the stated intent (description, paired change, comments). Ignore confident tone, severity labels, and security/race/injection vocabulary. Ask: does a concrete input or sequence produce a wrong outcome, and would anyone notice?\n\nThen try to refute it. Refuted if: another guard already covers it; an upstream check rules out the value (e.g. a balance, id, or count that earlier validation keeps in range); the producer never emits the feared value or code at this snapshot (later additions don't count); a UI control or in-flight flag already prevents the repeat action; the code matches the other side's own wording or the stated intent; ordering already prevents the race (e.g. a record created synchronously before the step that reads it). A \"missing handling\", \"no guard\", or \"ignores the input\" claim is usually deliberate or unreachable unless you can show the path. If a refutation holds, rank 1-2 however plausible it sounds.\n\nRank high (4-5) changes that introduce a concrete wrong outcome, including quiet ones, even when the finding is short and plain:\n- a test or assertion that relies on a premise the system does not enforce, or compares values of different time basis, precision, units, or normalisation (e.g. a timestamp against the clock at check time, a date across a day boundary on retry)\n- records silently dropped or misrouted when a lookup times out, fails, or returns null\n- substring or prefix matching where equality is required\n- effects, redirects, or handlers not gated on the condition they depend on\n- stale state surviving retry, reset, or partial failure\n- user-facing text promising what the system does not do\n\nRank middle (3): real but minor or easily recovered.\nRank low (1-2): misread code, deliberate behaviour, unchanged code, style, hypothetical future worries, issues that only exist because of later changes.\n\nAmong survivors, put the best-verified and most consequential first.";

/// A finding from an answered question stays in the report when the ranking
/// gives it at least this worth, or when this many candidates were merged into
/// it; below both it moves to the report's "questions to review" section.
pub const KEEP_WORTH: u64 = 3;
pub const KEEP_SUPPORT: usize = 4;

/// The `placement` a moved finding carries, which keeps it out of the verdict.
pub const LOWER_PRIORITY: &str = "lower-priority-question";

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
/// are never moved. Returns how many were ranked and moved.
pub fn place_lower_priority(
    surviving: &mut Vec<Value>,
    placed_aside: &mut Vec<Value>,
    worth: &[Option<u64>],
) -> (usize, usize) {
    let ranked = worth.iter().filter(|w| w.is_some()).count();
    let mut kept = Vec::new();
    let mut moved = 0;
    for (r, w) in surviving
        .drain(..)
        .zip(worth.iter().copied().chain(std::iter::repeat(None)))
    {
        let from_question = js::get(finding_of(&r), "lens") == Some(&json!(QUESTION_LENS));
        if from_question && !keeps(w, support_of(&r)) {
            let mut r = r;
            if let Value::Object(m) = &mut r {
                m.insert(
                    "placement".into(),
                    json!({"placement": LOWER_PRIORITY, "worth": w, "support": support_of(&Value::Object(m.clone()))}),
                );
            }
            placed_aside.push(r);
            moved += 1;
        } else {
            kept.push(r);
        }
    }
    *surviving = kept;
    (ranked, moved)
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
    let (ranked, moved) = place_lower_priority(surviving, placed_aside, &w);
    let summary = json!({"ranked": ranked, "movedToQuestionsToReview": moved, "keptInReport": surviving.len()});
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
    /// Every dispatch, labelled `questions:<n>` (generators),
    /// `question-warmup` or `question:<n>` (answers).
    pub dispatches: Vec<(String, DispatchResult)>,
    /// The lenses' setup, for the ranking after the panel.
    pub setup: Option<ReviewerSetup>,
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

    let mut asks = Vec::new();
    let mut routed = Vec::new();
    for (i, q) in out.questions.iter().enumerate() {
        let Some(slot) = route(&q.file, input.lens_files) else {
            continue;
        };
        let Some(prompt) = input.lens_prompts.get(slot) else {
            continue;
        };
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
    let results = run_all(agent, setup.concurrency, asks).await;
    for ((label, res), qi) in labels.into_iter().zip(results).zip(routed) {
        let q = &out.questions[qi];
        if let Some(Value::Array(findings)) = answer_of(&res)
            .as_ref()
            .and_then(|a| js::get(a, "findings"))
            .cloned()
        {
            out.answered += 1;
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
                    Ok(Some(kept)) => out.candidates.push(kept),
                    _ => out.dropped_out_of_diff += 1,
                }
            }
        }
        out.dispatches.push((label, res));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(file: &str, text: &str, sample: usize) -> Question {
        Question {
            file: file.into(),
            line: None,
            text: text.into(),
            sample,
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
