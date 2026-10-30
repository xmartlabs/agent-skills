//! The full text a lens is dispatched with, against the workflow's own
//! `reviewerAgent(lensPrompt({... diffBlock: lensDiffBlockFor(files) ...}))`.
//!
//! The workflow's side is read from `tests/goldens/span/diff_prompts.json`
//! (see `common`), with a SHA-256 of the workflow code each span came from, so
//! an edit there without regenerating fails here. Each sampled prompt is over
//! 1 KiB, so the file keeps a SHA-256 of each; the recipients constant is kept
//! whole.
//! Regenerate after changing a span, its cases or the workflow:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-pipeline --test diff_prompts`.

mod common;

use common::{assert_same, sample, span, Span, LOG_FN};
use proptest::prelude::*;
use serde_json::{json, Value};
use tribunal_core::lens::lens_prompt::{NOT_REACHED_PARAGRAPH, WORKFLOW_NOT_REACHED_PARAGRAPH};
use tribunal_core::profile::conventions_channel::conventions_channel;
use tribunal_pipeline::prompts::{
    lens_dispatch_prompt, methodology_line, methodology_path, DiffDelivery, DiffExposure,
    LensPromptInput, SCOPE_CLAUDE_RECIPIENTS,
};

const PROMPT_SPAN: Span = Span {
    start: "const SELF_FETCH_INSTRUCTION =",
    end: "return scopedDiffBody(files, LENS_DIFF_TAIL);\n};",
    deps: &["HEAD-WORKTREE", "LENS-PROMPT", "CONVENTIONS-CHANNEL"],
    pre: &[],
    fns: &["log"],
    ret: "{prompt: conventionsChannel(__args, __profile).reviewerPrompt(`METHODOLOGY FILE: ${__methodologyPath}`, lensPrompt({prNumber: __pr, repo: __repo, diffBlock: lensDiffBlockFor(__files), jiraTicket: prContext.jiraTicket, contextBlock: __contextBlock, dependents: __dependents, highRiskSurface: __profile.highRiskSurface, focus: __focus, files: __files})), counts: {lensReceiverCount, fullDiffReceiverCount, sectionedReceiverCount, deliveredBytes}, methodologyPath: `/tmp/tribunal-methodology-${__repo.replace(/[^A-Za-z0-9]+/g, \"-\")}-${__pr}.md`}",
};

fn case() -> impl Strategy<Value = Value> {
    (
        prop::option::of(Just("/tmp/peer-review-diff-12.patch")),
        prop::option::of(prop_oneof![Just(json!(48213)), Just(json!("48213"))]),
        prop_oneof![
            Just(json!({"path": "/tmp/peer-review-wt-12-abc", "kind": "worktree", "ok": true})),
            Just(json!({"path": null, "kind": "none", "ok": false})),
            Just(json!({"path": "", "kind": "clone", "ok": true})),
        ],
        prop::option::of(Just("0123456789abcdef0123456789abcdef01234567")),
        prop::option::of(prop_oneof![Just(json!("ABC-12")), Just(json!(""))]),
        prop::option::of(prop::collection::vec("[a-z]{1,8}/[a-z]{1,8}\\.ts", 1..4)),
        prop_oneof![Just(json!("correctness sentence")), Just(Value::Null)],
        prop::option::of(Just("auth and payments")),
        (any::<bool>(), "[a-zA-Z .\n]{0,40}", "[a-zA-Z .\n]{0,40}", prop_oneof![Just("acme/widgets-api"), Just("acme/widgets..api_v2")]),
    )
        .prop_map(|(diff_path, bytes, pin, sha, jira, files, focus, hrs, (conv, ctx, deps, repo))| {
            let mut pr = json!({});
            if let Some(s) = sha {
                pr["headRefOid"] = json!(s);
            }
            if let Some(b) = bytes {
                pr["diffBytes"] = b;
            }
            if let Some(j) = jira {
                pr["jiraTicket"] = j;
            }
            let mut profile = json!({"reviewer": "backend"});
            if let Some(h) = hrs {
                profile["highRiskSurface"] = json!(h);
            }
            let args = if conv {
                profile["conventionFiles"] = json!(["CLAUDE.md"]);
                json!({"conventionsPath": "/tmp/tribunal-conventions-acme-widgets-api-12.md", "conventionsSha256": "a".repeat(64)})
            } else {
                json!({})
            };
            json!({
                "diffPath": diff_path.unwrap_or(""),
                "diffMode": if diff_path.is_some() { "path" } else { "self-fetch" },
                "sectionIndex": null,
                "diffUsable": false,
                "prContext": pr,
                "headPin": pin,
                "DIFF_INJECT_BUDGET": 60000,
                "log": LOG_FN,
                "__args": args,
                "__profile": profile,
                "__methodologyPath": methodology_path(repo, "12"),
                "__pr": 12,
                "__repo": repo,
                "__files": files.map_or(Value::Null, Value::from),
                "__contextBlock": ctx,
                "__dependents": deps,
                "__focus": focus,
            })
        })
}

fn rust_prompt(c: &Value) -> Value {
    let pr = &c["prContext"];
    let delivery = DiffDelivery {
        diff_path: c["diffPath"]
            .as_str()
            .filter(|p| !p.is_empty())
            .map(str::to_owned),
        diff_bytes: pr.get("diffBytes").and_then(Value::as_f64),
        head_pin: c["headPin"].clone(),
        head_sha: pr.get("headRefOid").cloned(),
    };
    let files: Option<Vec<String>> = serde_json::from_value(c["__files"].clone()).unwrap();
    let conventions = conventions_channel(&c["__args"], &c["__profile"]).unwrap();
    let mut exposure = DiffExposure::default();
    let prompt = lens_dispatch_prompt(
        &LensPromptInput {
            pr_number: &c["__pr"],
            repo: c["__repo"].as_str().unwrap(),
            jira_ticket: pr.get("jiraTicket"),
            context_block: c["__contextBlock"].as_str().unwrap(),
            dependents: c["__dependents"].as_str().unwrap(),
            high_risk_surface: c["__profile"].get("highRiskSurface"),
            focus: &c["__focus"],
            files: files.as_deref(),
        },
        &delivery,
        &conventions,
        &methodology_line(c["__methodologyPath"].as_str().unwrap()),
        &mut exposure,
    )
    .unwrap();
    json!({
        "prompt": prompt,
        "counts": {
            "lensReceiverCount": exposure.lens_receivers,
            "fullDiffReceiverCount": exposure.full_diff_receivers,
            "sectionedReceiverCount": exposure.sectioned_receivers,
            "deliveredBytes": exposure.delivered_bytes,
        },
        "methodologyPath": c["__methodologyPath"],
    })
}

#[test]
fn lens_dispatch_prompt_matches_the_workflow() {
    let cases = sample(case());
    let js = span(&PROMPT_SPAN, &cases);
    // Declared difference (`lens_prompt::NOT_REACHED_PARAGRAPH`): the port
    // asks for each gap's kind. Each port prompt carries it once; with the
    // Workflow's paragraph put back, the prompt is the Workflow's.
    let rust: Vec<Value> = cases
        .iter()
        .map(rust_prompt)
        .map(|mut r| {
            let p = r["prompt"].as_str().unwrap();
            assert_eq!(p.matches(NOT_REACHED_PARAGRAPH).count(), 1);
            r["prompt"] = json!(p.replace(NOT_REACHED_PARAGRAPH, WORKFLOW_NOT_REACHED_PARAGRAPH));
            r
        })
        .collect();
    assert_same("lens dispatch prompt", &cases, &rust, &js);
}

#[test]
fn the_workflow_still_hands_claude_recipients_the_whole_patch() {
    let js = span(
        &Span {
            start: "const SCOPE_CLAUDE_RECIPIENTS =",
            end: ";",
            deps: &[],
            pre: &[],
            fns: &[],
            ret: "SCOPE_CLAUDE_RECIPIENTS",
        },
        &[json!({})],
    );
    assert_eq!(js[0], json!(SCOPE_CLAUDE_RECIPIENTS));
}
