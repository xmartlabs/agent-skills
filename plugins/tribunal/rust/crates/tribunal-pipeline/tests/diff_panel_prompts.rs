//! The panel's prompts against the workflow's own template literals.
//!
//! The workflow's side is read from
//! `tests/goldens/span/diff_panel_prompts.json` (see `common`), with a SHA-256
//! of the workflow code each span came from, so an edit there without
//! regenerating fails here. Each prompt is over 1 KiB, so the file keeps a
//! SHA-256 of each; a failure shows the port's prompt and the input.
//! Regenerate after changing a span, its cases or the workflow:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-pipeline --test diff_panel_prompts`.

mod common;

use common::{assert_same, sample, span, Span};
use proptest::prelude::*;
use serde_json::{json, Value};
use tribunal_pipeline::panel_prompts::{
    batch_prompt, context_instruction, critic_prompt, defense_prompt, external_context_instruction,
    external_prompt, judge_prompt, refute_prompt, strengthen_prompt, sweep_prompt, tiebreak_prompt,
    EXTERNAL_SLICE_MARK, REFUTER_LENSES,
};

const PRECEDENCE: (&str, &str) = ("const PRECEDENCE_RULE = `", "BY-DESIGN.`;");
const FILES: &[&str] = &["src/a.ts", "src/b/service.ts", "web/c.tsx"];

fn text() -> impl Strategy<Value = Value> {
    prop_oneof![
        4 => "[a-zA-Z <>&]{0,24}".prop_map(Value::from),
        1 => Just(json!("uses `src/b/service.ts` and src/a.ts:12")),
        1 => Just(json!("tab\t ü 🎉 {{title}}")),
        1 => Just(Value::Null),
        1 => Just(json!(7)),
    ]
}

fn finding() -> impl Strategy<Value = Value> {
    (
        text(),
        prop_oneof![prop::sample::select(FILES).prop_map(Value::from), Just(json!("<x>.ts"))],
        prop_oneof![Just(json!(12)), Just(json!(0)), Just(Value::Null), Just(json!("40"))],
        prop_oneof![Just(json!("critical")), Just(json!("important")), Just(json!("nit"))],
        text(),
        0u32..9,
        prop::option::of(prop::collection::vec(
            (prop::sample::select(FILES), Just(3)).prop_map(|(f, l)| json!({"file": f, "line": l, "quote": "q"})),
            0..2,
        )),
        (any::<bool>(), any::<bool>()),
    )
        .prop_map(|(title, file, line, sev, desc, slot, vb, (risk, beh))| {
            let mut f = json!({"title": title, "file": file, "severity": sev, "description": desc, "slotId": slot});
            if !line.is_null() {
                f["line"] = line;
            }
            if let Some(vb) = vb {
                f["verifiedBy"] = json!(vb);
            }
            if risk {
                f["touchesHighRiskSurface"] = json!(true);
            }
            if beh {
                f["isBehaviorChanging"] = json!(false);
            }
            f
        })
}

fn vote() -> impl Strategy<Value = Value> {
    (
        prop_oneof![Just("REFUTED"), Just("NOT_REFUTED")],
        prop_oneof![Just(json!(0.8)), Just(json!(1)), Just(Value::Null)],
        text(),
    )
        .prop_map(|(v, c, r)| {
            let mut o = json!({"verdict": v});
            if !c.is_null() {
                o["confidence"] = c;
            }
            if !r.is_null() {
                o["reason"] = r;
            }
            o
        })
}

fn decisions() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just("OPERATOR:\n- DEFERRED: retries".to_owned())
    ]
}

const AGENT: &str = "async (p, o) => { (globalThis.__calls ||= []).push({prompt: p, label: o.label}); return o.label && o.label.startsWith('judge') ? {decision: 'SURVIVES'} : 'the defense'; }";

fn calls() -> &'static str {
    "globalThis.__calls.splice(0)"
}

#[test]
fn refuter_prompts_and_context_match() {
    let cases: Vec<Value> = sample((finding(), decisions(), 0usize..3, "[a-z ]{0,30}"))
        .into_iter()
        .map(|(mut f, d, l, t)| {
            if !f["title"].is_string() {
                f["title"] = json!(t);
            }
            json!({"finding": f, "combinedDecisions": d, "lens": l})
        })
        .collect();
    let ret = format!("(await claudeRefuter(finding, REFUTER_LENSES[lensIndex])(), {{calls: {}, ctx: contextInstruction, ext: externalContextInstruction}})", calls());
    let js = span(
        &Span {
            start: "const REFUTER_LENSES = [",
            end: "        ...(refuterAgentType ? { agentType: refuterAgentType } : {}),\n      },\n    );\n  };\n}",
            deps: &[],
            pre: &[PRECEDENCE],
            fns: &["agent", "withConventions", "refuterDiffBlockFor"],
            ret: &ret,
        },
        &cases
            .iter()
            .map(|c| {
                json!({
                    "finding": c["finding"], "combinedDecisions": c["combinedDecisions"], "lensIndex": c["lens"],
                    "prNumber": 77, "repo": "acme/widgets", "reviewModel": "m", "refuterAgentType": "r",
                    "REFUTATION_SCHEMA": {}, "agent": AGENT, "withConventions": "(p) => p",
                    "refuterDiffBlockFor": "() => 'DIFF BLOCK'",
                })
            })
            .collect::<Vec<_>>(),
    );
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| {
            let d = c["combinedDecisions"].as_str().unwrap();
            let f = &c["finding"];
            let title = tribunal_core::lens::js_ext::utf16_prefix_lossy(f["title"].as_str().unwrap(), 20);
            json!({
                "calls": [{"prompt": refute_prompt(&json!(77), "acme/widgets", "DIFF BLOCK", &context_instruction(d), f, REFUTER_LENSES[c["lens"].as_u64().unwrap() as usize]), "label": format!("refute:{title}:claude")}],
                "ctx": context_instruction(d),
                "ext": external_context_instruction(d),
            })
        })
        .collect();
    assert_same("refuter prompt", &cases, &rust, &js);
}

#[test]
fn batch_prompt_matches() {
    let cases: Vec<Value> = sample((prop::collection::vec(finding(), 1..4), decisions(), 0usize..3))
        .into_iter()
        .map(|(fs, d, l)| json!({"findings": fs, "ctx": context_instruction(&d), "lens": REFUTER_LENSES[l]}))
        .collect();
    let js = span(
        &Span {
            start: "const escapeXmlText = (s) =>",
            end: "no slotId not listed above.`;\n}",
            deps: &[],
            pre: &[],
            fns: &["refuterDiffBlockFor"],
            ret: "claudeRefuterBatchPrompt(findings, lens)",
        },
        &cases
            .iter()
            .map(|c| json!({"findings": c["findings"], "lens": c["lens"], "contextInstruction": c["ctx"], "prNumber": 77, "repo": "acme/widgets", "refuterDiffBlockFor": "() => 'DIFF BLOCK'"}))
            .collect::<Vec<_>>(),
    );
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| {
            json!(batch_prompt(
                c["findings"].as_array().unwrap(),
                c["lens"].as_str().unwrap(),
                &json!(77),
                "acme/widgets",
                "DIFF BLOCK",
                c["ctx"].as_str().unwrap()
            ))
        })
        .collect();
    assert_same("batch prompt", &cases, &rust, &js);
}

#[test]
fn strengthen_prompt_matches() {
    let cases: Vec<Value> = sample((finding(), prop::collection::vec(vote(), 0..3)))
        .into_iter()
        .map(|(f, v)| json!({"finding": f, "valid": v}))
        .collect();
    let js = span(
        &Span {
            start: "function strengthenPromptFor(finding, tally) {",
            end: ".join(\" | \")}`;\n}",
            deps: &[],
            pre: &[],
            fns: &["refuterDiffBlockFor"],
            ret: "strengthenPromptFor(finding, {valid})",
        },
        &cases
            .iter()
            .map(|c| json!({"finding": c["finding"], "valid": c["valid"], "prNumber": 77, "repo": "acme/widgets", "refuterDiffBlockFor": "() => 'DIFF BLOCK'"}))
            .collect::<Vec<_>>(),
    );
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| {
            json!(strengthen_prompt(
                &json!(77),
                "acme/widgets",
                "DIFF BLOCK",
                &c["finding"],
                c["valid"].as_array().unwrap()
            ))
        })
        .collect();
    assert_same("strengthen prompt", &cases, &rust, &js);
}

#[test]
fn debate_prompts_match() {
    let cases: Vec<Value> = sample((
        finding(),
        prop::collection::vec(vote(), 0..3),
        any::<bool>(),
        "[a-z ]{0,30}",
    ))
    .into_iter()
    .map(|(mut f, v, s, t)| {
        if !f["title"].is_string() {
            f["title"] = json!(t);
        }
        json!({"finding": f, "valid": v, "survived": s})
    })
    .collect();
    let js = span(
        &Span {
            start: "      const defense = await reviewerAgent(",
            end: "          agentType: LENS_AGENT,\n        },\n      );\n      // Vocabulary",
            deps: &["DEBATE-DEGRADE"],
            pre: &[],
            fns: &["reviewerAgent", "refuterDiffBlockFor"],
            ret: "globalThis.__calls.splice(0).map((c) => c.prompt)",
        },
        &cases
            .iter()
            .map(|c| {
                json!({
                    "workingFinding": c["finding"], "tally": {"valid": c["valid"], "survived": c["survived"]},
                    "prNumber": 77, "repo": "acme/widgets", "reviewModel": "m", "LENS_AGENT": "l", "ADJUDICATOR_SCHEMA": {},
                    "reviewerAgent": AGENT, "refuterDiffBlockFor": "() => 'DIFF BLOCK'",
                })
            })
            .collect::<Vec<_>>(),
    );
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| {
            let valid = c["valid"].as_array().unwrap();
            json!([
                defense_prompt(
                    &json!(77),
                    "acme/widgets",
                    "DIFF BLOCK",
                    &c["finding"],
                    valid
                ),
                judge_prompt(
                    "DIFF BLOCK",
                    &c["finding"],
                    valid,
                    Some(&json!("the defense")),
                    c["survived"] == true
                ),
            ])
        })
        .collect();
    assert_same("debate prompts", &cases, &rust, &js);
}

#[test]
fn critic_prompt_matches() {
    let cases: Vec<Value> = sample(prop::collection::vec(finding(), 0..4))
        .into_iter()
        .map(|fs| json!({"panelInput": fs}))
        .collect();
    let js = span(
        &Span {
            start: "  const raisedTitles =",
            end: "      agentType: LENS_AGENT,\n      schema: CRITIC_SCHEMA,\n    },\n  );",
            deps: &[],
            pre: &[],
            fns: &["reviewerAgent"],
            ret: "globalThis.__calls.splice(0)[0].prompt",
        },
        &cases
            .iter()
            .map(|c| json!({"panelInput": c["panelInput"], "prNumber": 77, "repo": "acme/widgets", "prContext": {"headRefOid": "feedface", "files": FILES}, "reviewModel": "m", "LENS_AGENT": "l", "CRITIC_SCHEMA": {}, "reviewerAgent": AGENT}))
            .collect::<Vec<_>>(),
    );
    let files: Vec<String> = FILES.iter().map(|f| (*f).to_owned()).collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| {
            json!(critic_prompt(
                &json!(77),
                "acme/widgets",
                Some(&json!("feedface")),
                &files,
                c["panelInput"].as_array().unwrap()
            ))
        })
        .collect();
    assert_same("critic prompt", &cases, &rust, &js);
}

#[test]
fn tiebreak_prompt_matches() {
    let cases: Vec<Value> = sample((prop::collection::vec(finding(), 2..5), 1usize..3))
        .into_iter()
        .map(|(g, slots)| json!({"group": g, "slots": slots}))
        .collect();
    let js = span(
        &Span {
            start: "    const ranking = await agent(",
            end: "        agentType: SCRIBE_AGENT,\n      },\n    );",
            deps: &[],
            pre: &[],
            fns: &["agent"],
            ret: "globalThis.__calls.splice(0)[0].prompt",
        },
        &cases
            .iter()
            .map(|c| json!({"tieDiag": {"tiedGroup": c["group"], "slotsForTiedGroup": c["slots"], "boundaryPriority": 3}, "reviewModel": "m", "SCRIBE_AGENT": "s", "PANEL_TIEBREAK_SCHEMA": {}, "agent": AGENT}))
            .collect::<Vec<_>>(),
    );
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| {
            json!(tiebreak_prompt(
                c["group"].as_array().unwrap(),
                c["slots"].as_u64().unwrap() as usize,
                &json!(3)
            ))
        })
        .collect();
    assert_same("tiebreak prompt", &cases, &rust, &js);
}

#[test]
fn external_prompt_matches() {
    let cases: Vec<Value> = sample((finding(), decisions(), any::<bool>()))
        .into_iter()
        .map(|(f, d, file_slice)| json!({"finding": f, "ext": external_context_instruction(&d), "fileSlice": file_slice}))
        .collect();
    let js = span(
        &Span {
            start: "function externalPromptText(finding) {",
            end: "].join(\"\\n\");\n}",
            deps: &["DIFF-SECTIONING"],
            pre: &[("const EXTERNAL_SLICE_MARK = ", ";")],
            fns: &["externalDiffSlice"],
            ret: "externalPromptText(finding)",
        },
        &cases
            .iter()
            .map(|c| {
                json!({
                    "finding": c["finding"], "externalContextInstruction": c["ext"], "prContext": {"files": FILES},
                    "externalDiffSlice": if c["fileSlice"] == true { "() => ({file: '/x.slice'})" } else { "(f) => ({text: 'No diff excerpt is provided. Open ' + f.file})" },
                })
            })
            .collect::<Vec<_>>(),
    );
    let files: Vec<String> = FILES.iter().map(|f| (*f).to_owned()).collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| {
            let text = format!(
                "No diff excerpt is provided. Open {}",
                tribunal_core::lens::js_ext::to_js_string(c["finding"].get("file"))
            );
            let slice = (c["fileSlice"] != true).then_some(text.as_str());
            json!(external_prompt(
                &c["finding"],
                &files,
                c["ext"].as_str().unwrap(),
                slice
            ))
        })
        .collect();
    assert_same("external prompt", &cases, &rust, &js);
    assert!(rust
        .iter()
        .any(|r| r.as_str().unwrap().contains(EXTERNAL_SLICE_MARK)));
}

#[test]
fn sweep_prompt_matches() {
    let cases: Vec<Value> = sample((prop_oneof![Just(Value::Null), Just(json!("/tmp/x.patch"))], 0usize..45))
        .into_iter()
        .map(|(p, n)| json!({"diffPath": p, "files": (0..n).map(|i| format!("f{i}.ts")).collect::<Vec<_>>()}))
        .collect();
    let js = span(
        &Span {
            start: "    const sweepBlock = [",
            end: "    ].join(\"\\n\");",
            deps: &[],
            pre: &[],
            fns: &[],
            ret: "(() => { const t = sweepBlock; const a = t.indexOf(\"<<'__SWEEP_PROMPT_END__'\\n\") + \"<<'__SWEEP_PROMPT_END__'\\n\".length; return t.slice(a, t.indexOf(\"\\n__SWEEP_PROMPT_END__\", a)); })()",
        },
        &cases
            .iter()
            .map(|c| json!({"VOTE_DIR": "V", "sweepDir": "D", "sweepOut": "O", "diffPath": c["diffPath"], "prContext": {"files": c["files"]}, "externalCwd": "/wt", "SWEEP_TIMEOUT_S": 480}))
            .collect::<Vec<_>>(),
    );
    let rust: Vec<Value> = cases
        .iter()
        .map(|c| {
            let files: Vec<String> = c["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| f.as_str().unwrap().to_owned())
                .collect();
            json!(sweep_prompt(c["diffPath"].as_str(), &files, "/wt"))
        })
        .collect();
    assert_same("sweep prompt", &cases, &rust, &js);
}
