//! Port of the LENS-PROMPT region: the prompt every review lens receives. A
//! model reads it, and the shared prefix above "YOUR PASS" is what the lenses'
//! prompt cache keys on, so the text must match byte for byte.

use serde_json::Value;

use super::js_ext::to_js_string;
use crate::js;

pub const LENS_TOOL_CALL_BUDGET: u32 = 40;

/// `LENS_BUDGET_INSTRUCTION`. It starts with a newline.
pub fn lens_budget_instruction() -> String {
    format!(
        r##"
Budget: about {LENS_TOOL_CALL_BUDGET} tool calls for this pass. This is a ceiling to aim under, not a target to reach and not a reason to stop early — a lens that gives up after a handful of reads has reviewed nothing, and that is a worse failure than being slow.
Past that budget, wrap up with what you have rather than continuing to dig. On this codebase, lens findings measurably stop increasing after roughly 30-40 tool calls while cost keeps climbing: passes that ran past 80 calls cost three times as much and found slightly fewer real defects than passes that finished inside 50. Continuing is not thoroughness at that point, it is re-reading.
If you stop with something still unexamined, list it in `notReached` — one entry per file or surface you meant to look at and did not. That array is how the run reports its own blind spots; leaving it empty when you did run out of room is worse than the gap itself."##
    )
}

/// Ports `lensPrompt`. `p` is the object the harness builds per lens; each
/// field is interpolated with JavaScript's `String()`, so a missing one reads
/// `undefined`. A truthy `files` that is not an array throws in JavaScript,
/// and is an `Err` with the same message here.
pub fn lens_prompt(p: &Value) -> Result<String, String> {
    let field = |k: &str| to_js_string(js::get(p, k));
    let jira = match js::get(p, "jiraTicket") {
        t if js::truthy(t) => to_js_string(t),
        _ => "none".to_owned(),
    };
    let dependents = match js::get(p, "dependents") {
        d if js::truthy(d) => format!("\n{}\n", to_js_string(d)),
        _ => String::new(),
    };
    let files = match js::get(p, "files") {
        Some(Value::Array(files)) => format!(
            "\nAND restrict it to ONLY these changed files — other lenses own the rest of the PR, so anything you report outside this list is duplicated work that will be discarded:\n{}",
            files
                .iter()
                .map(|f| format!("  - {}", to_js_string(Some(f))))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        f if js::truthy(f) => return Err("p.files.map is not a function".to_owned()),
        _ => String::new(),
    };
    Ok(format!(
        r##"Review PR #{pr} in repo {repo}.

<diff>
{diff_block}
</diff>

<context>
Jira: {jira}
{context}
</context>
{dependents}
{budget}

For each finding set:
- title: short one-line description
- file: file path
- line: the line number from a grep -n hit or a Read of that range (0 if you have neither)
- severity: "critical" | "important" | "nit"
- description: what the problem is and why it matters
- evidence: the exact code that shows it, quoted word-for-word from the diff or the file at HEAD — not a paraphrase, not a description of the code. This is what lets a reader check the claim without re-reading the PR, and a quote you cannot actually find is a finding you should not report. If there is genuinely nothing to cite, write exactly "No relevant quotes found" — this field is required, so an honest abstention is expected and useful; a vague or invented snippet is not.
- isBehaviorChanging: true if, were the finding real, what a caller, a user, a stored row or a job observes would change (a different value, status, throw, or work dropped); false for style, docs, logging-only and test-only findings
- touchesHighRiskSurface: true if it involves {high_risk}
- confidence: 0..1, how sure you are this is a REAL defect (not a stylistic guess)
- mechanicalClaim: OMIT (leave unset) for normal logic/behaviour findings. Set it to "type-error", "undefined-symbol", or "signature-mismatch" ONLY when the finding's core claim is that the code does not typecheck (a type mismatch, a reference to a missing import/symbol/property, or a wrong call signature) — a claim a compiler settles. When unsure, omit it.
- verifiedBy: REQUIRED on every "important" and "critical" finding (optional, may be omitted, on a "nit"). An array of {{file, line, quote}} — the code that must be TRUE for your claim to hold, not the line the diff changed (that's "evidence" above). If your finding depends on a type, enum, function signature, or config value having a certain shape, paste that shape here — transcribing it is the check: you cannot copy `enum upload_type {{ ... }}` into this field without noticing what member is missing from it. A finding that skips this field when required is downgraded to an open question before it ever reaches the report, regardless of what the panel decided.

===== YOUR PASS =====
<your_focus>
Restrict this pass to ONLY: {focus}. Ignore everything else — other lenses cover the rest.{files}
Return findings scoped to that focus only — everything above this line is the same for every lens on this PR; this section is yours.
</your_focus>"##,
        pr = field("prNumber"),
        repo = field("repo"),
        diff_block = field("diffBlock"),
        context = field("contextBlock"),
        budget = lens_budget_instruction(),
        high_risk = field("highRiskSurface"),
        focus = field("focus"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn focus_sits_below_the_marker() {
        let p =
            lens_prompt(&json!({"prNumber": 1, "repo": "o/r", "focus": "F", "files": ["a.ts"]}))
                .unwrap();
        let marker = p.find("===== YOUR PASS =====").unwrap();
        assert!(p.find("Restrict this pass to ONLY: F").unwrap() > marker);
        assert!(p.contains("Jira: none\nundefined\n</context>\n\n\nBudget: about 40"));
        assert!(p.ends_with("discarded:\n  - a.ts\nReturn findings scoped to that focus only — everything above this line is the same for every lens on this PR; this section is yours.\n</your_focus>"));
    }

    #[test]
    fn a_string_files_throws_as_in_javascript() {
        assert!(lens_prompt(&json!({"files": "a.ts"})).is_err());
        assert!(lens_prompt(&json!({"files": ""})).is_ok());
    }
}
