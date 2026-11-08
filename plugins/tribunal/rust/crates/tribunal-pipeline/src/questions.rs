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
