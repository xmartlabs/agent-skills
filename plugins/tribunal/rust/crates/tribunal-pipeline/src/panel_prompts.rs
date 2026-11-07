//! The refutation panel's prompts: the Claude refuter (alone and batched),
//! the external heads, the strengthen pass, the debate's defense and judge,
//! the completeness critic, the panel-ceiling tie-break and the clean-verdict
//! sweep. The texts under `panel/` are cut from the workflow's own template
//! literals, each interpolation named `{{name}}`.

use serde_json::{Map, Value};
use tribunal_core::evidence::tsc_oracle::GROUNDED_LINE_WINDOW;
use tribunal_core::js;
use tribunal_core::lens::diff_sectioning::external_scope_lines;
use tribunal_core::lens::js_ext::to_js_string;
use tribunal_core::panel::debate_degrade::debate_defense_block;

use crate::decisions::PRECEDENCE_RULE;
use crate::synthesize::render;
use crate::verify::GROUNDED_DIAGNOSTIC;
use tribunal_context::runtime_case::RUNTIME_CASE;

const REFUTE: &str = include_str!("panel/refute.txt");
const REFUTE_TAIL: &str = include_str!("panel/refute-tail.txt");
const CONTEXT_INSTRUCTION: &str = include_str!("panel/context-instruction.txt");
const EXTERNAL_CONTEXT_INSTRUCTION: &str = include_str!("panel/external-context-instruction.txt");
const BATCH: &str = include_str!("panel/batch.txt");
const BATCH_CASE: &str = include_str!("panel/batch-case.txt");
const STRENGTHEN: &str = include_str!("panel/strengthen.txt");
const DEFENSE: &str = include_str!("panel/defense.txt");
const JUDGE: &str = include_str!("panel/judge.txt");
const CRITIC: &str = include_str!("panel/critic.txt");
const CRITIC_GAP: &str = include_str!("panel/critic-gap.txt");
const TIEBREAK: &str = include_str!("panel/tiebreak.txt");
const EXTERNAL: &str = include_str!("panel/external.txt");
const SWEEP: &str = include_str!("panel/sweep.txt");
const SWEEP_DIFF_LINES: &str = include_str!("panel/sweep-diff-lines.txt");

pub const EXTERNAL_SLICE_MARK: &str = "@@TRIBUNAL-DIFF-SLICE@@";

pub const REFUTER_LENSES: [&str; 3] = [
    "correctness/falsification — find the input or code path that proves this is NOT a real problem",
    "contract/security impact — find why this does NOT break the auth, data, or API contract",
    "reproducibility — find why this does NOT manifest given the surrounding code and call sites",
];

fn s(v: Option<&Value>) -> String {
    to_js_string(v)
}

fn get<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    js::get(v, k)
}

/// The checker diagnostic the grounded gate found near a finding that claims
/// no compile error, as a paragraph after its description. Empty otherwise,
/// which leaves every prompt as it was.
fn diagnostic_note(f: &Value) -> String {
    get(f, GROUNDED_DIAGNOSTIC)
        .and_then(Value::as_str)
        .map_or(String::new(), |d| {
            format!(
                "\n\nThe repository's own typecheck reported this within {GROUNDED_LINE_WINDOW} lines of the location: {d}\nIt is evidence about that code, not a verdict on the finding."
            )
        })
}

/// The failing case `--runtime-confirm` ran for the finding, as a paragraph
/// after its description. Empty when none confirmed, which leaves every
/// prompt as it was.
fn runtime_note(f: &Value) -> String {
    let Some(case) = get(f, RUNTIME_CASE).filter(|c| c.is_object()) else {
        return String::new();
    };
    let json = |v: Option<&Value>| v.map_or_else(|| "null".to_owned(), Value::to_string);
    let args = get(case, "args")
        .and_then(Value::as_array)
        .map_or_else(String::new, |a| {
            a.iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        });
    let expected = match get(case, "expect") {
        Some(e) if get(e, "throws") == Some(&Value::Bool(true)) => "to throw".to_owned(),
        Some(e) => format!("to return {}", json(get(e, "returns"))),
        None => "a different result".to_owned(),
    };
    format!(
        "\n\nA test generated from this finding ran at the PR head with the repository's own vitest and failed. It called {}({args}) and expected it {expected}. The failure: {}\nIt is evidence about that code, not a verdict on the finding: the expected value comes from the finding.",
        s(get(case, "function")),
        s(get(case, "failure"))
    )
}

fn described(f: &Value) -> String {
    format!(
        "{}{}{}",
        s(get(f, "description")),
        diagnostic_note(f),
        runtime_note(f)
    )
}

fn described_xml(f: &Value) -> String {
    let note = format!("{}{}", diagnostic_note(f), runtime_note(f))
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!("{}{note}", escape_xml(get(f, "description")))
}

/// `${f.file}${f.line ? ":" + f.line : ""}`.
fn location(f: &Value) -> String {
    let line = get(f, "line");
    if js::truthy(line) {
        format!("{}:{}", s(get(f, "file")), s(line))
    } else {
        s(get(f, "file"))
    }
}

/// `Array.prototype.join` over already-mapped values: `null` and `undefined`
/// print as nothing.
fn join_values(values: &[Option<&Value>], sep: &str) -> String {
    values
        .iter()
        .map(|v| match v {
            None | Some(Value::Null) => String::new(),
            v => s(*v),
        })
        .collect::<Vec<_>>()
        .join(sep)
}

/// `contextInstruction`: empty when nothing was decided.
#[must_use]
pub fn context_instruction(combined_decisions: &str) -> String {
    if combined_decisions.is_empty() {
        return String::new();
    }
    render(
        CONTEXT_INSTRUCTION,
        &[
            ("combinedDecisions", combined_decisions),
            ("precedenceRule", PRECEDENCE_RULE),
        ],
    )
}

/// `externalContextInstruction`.
#[must_use]
pub fn external_context_instruction(combined_decisions: &str) -> String {
    if combined_decisions.is_empty() {
        return String::new();
    }
    render(
        EXTERNAL_CONTEXT_INSTRUCTION,
        &[(
            "contextInstruction",
            &context_instruction(combined_decisions),
        )],
    )
}

/// One Claude refuter's prompt, before `withConventions`.
#[must_use]
pub fn refute_prompt(
    pr_number: &Value,
    repo: &str,
    diff_block: &str,
    context_instruction: &str,
    finding: &Value,
    lens: &str,
) -> String {
    render(
        REFUTE,
        &[
            ("prNumber", &s(Some(pr_number))),
            ("repo", repo),
            ("diffBlock", diff_block),
            (
                "contextAndTail",
                &format!("{context_instruction}{REFUTE_TAIL}"),
            ),
            ("lens", lens),
            ("title", &s(get(finding, "title"))),
            ("location", &location(finding)),
            ("severity", &s(get(finding, "severity"))),
            ("description", &described(finding)),
        ],
    )
}

/// `escapeXmlText`.
fn escape_xml(v: Option<&Value>) -> String {
    let text = match v {
        None | Some(Value::Null) => String::new(),
        v => s(v),
    };
    text.replace('<', "&lt;").replace('>', "&gt;")
}

/// `claudeRefuterBatchPrompt`, before `withConventions`.
#[must_use]
pub fn batch_prompt(
    findings: &[Value],
    lens: &str,
    pr_number: &Value,
    repo: &str,
    diff_block: &str,
    context_instruction: &str,
) -> String {
    let count = findings.len().to_string();
    let cases = findings
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let line = get(f, "line");
            let loc = if js::truthy(line) {
                format!("{}:{}", escape_xml(get(f, "file")), s(line))
            } else {
                escape_xml(get(f, "file"))
            };
            render(
                BATCH_CASE,
                &[
                    ("n", &(i + 1).to_string()),
                    ("count", &count),
                    ("slotId", &s(get(f, "slotId"))),
                    ("title", &escape_xml(get(f, "title"))),
                    ("location", &loc),
                    ("severity", &s(get(f, "severity"))),
                    ("description", &described_xml(f)),
                ],
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    render(
        BATCH,
        &[
            ("count", &count),
            ("prNumber", &s(Some(pr_number))),
            ("repo", repo),
            ("lens", lens),
            ("diffBlock", diff_block),
            ("contextInstruction", context_instruction),
            ("cases", &cases),
        ],
    )
}

/// `strengthenPromptFor`. `valid` is the tally's counted votes.
#[must_use]
pub fn strengthen_prompt(
    pr_number: &Value,
    repo: &str,
    diff_block: &str,
    finding: &Value,
    valid: &[Value],
) -> String {
    let reasons: Vec<Option<&Value>> = valid.iter().map(|v| get(v, "reason")).collect();
    render(
        STRENGTHEN,
        &[
            ("prNumber", &s(Some(pr_number))),
            ("repo", repo),
            ("diffBlock", diff_block),
            ("title", &s(get(finding, "title"))),
            ("location", &location(finding)),
            ("severity", &s(get(finding, "severity"))),
            ("description", &described(finding)),
            ("objections", &join_values(&reasons, " | ")),
        ],
    )
}

/// The debate's defense prompt, before the reviewer wrapper.
#[must_use]
pub fn defense_prompt(
    pr_number: &Value,
    repo: &str,
    diff_block: &str,
    finding: &Value,
    valid: &[Value],
) -> String {
    let objections = valid
        .iter()
        .map(|v| {
            format!(
                "{} ({}): {}",
                s(get(v, "verdict")),
                s(get(v, "confidence")),
                s(get(v, "reason"))
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");
    render(
        DEFENSE,
        &[
            ("prNumber", &s(Some(pr_number))),
            ("repo", repo),
            ("diffBlock", diff_block),
            ("title", &s(get(finding, "title"))),
            ("location", &location(finding)),
            ("description", &described(finding)),
            ("objections", &objections),
        ],
    )
}

/// The debate's judge prompt, before the reviewer wrapper.
#[must_use]
pub fn judge_prompt(
    diff_block: &str,
    finding: &Value,
    valid: &[Value],
    defense: Option<&Value>,
    survived: bool,
) -> String {
    let refutations = valid
        .iter()
        .map(|v| {
            format!(
                "- [{} conf={}] {}",
                s(get(v, "verdict")),
                s(get(v, "confidence")),
                s(get(v, "reason"))
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    render(
        JUDGE,
        &[
            ("diffBlock", diff_block),
            ("title", &s(get(finding, "title"))),
            ("location", &location(finding)),
            ("description", &described(finding)),
            ("refutations", &refutations),
            ("defense", &debate_defense_block(defense)),
            ("mechanical", if survived { "SURVIVES" } else { "REFUTED" }),
        ],
    )
}

/// The completeness critic's prompt, before the reviewer wrapper.
#[must_use]
pub fn critic_prompt(
    pr_number: &Value,
    repo: &str,
    head: Option<&Value>,
    files: &[String],
    panel_input: &[Value],
) -> String {
    let raised = panel_input
        .iter()
        .map(|f| {
            let line = get(f, "line");
            let at = if js::truthy(line) {
                format!(":{}", s(line))
            } else {
                String::new()
            };
            format!(
                "- {} ({}{at}, {})",
                s(get(f, "title")),
                s(get(f, "file")),
                s(get(f, "severity"))
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    render(
        CRITIC,
        &[
            ("prNumber", &s(Some(pr_number))),
            ("repo", repo),
            ("head", &s(head)),
            ("files", &files.join(", ")),
            ("raised", if raised.is_empty() { "(none)" } else { &raised }),
        ],
    )
}

/// The gap-audit critic's prompt: the diff and the pinned checkout are already
/// on disk, and the two lists it audits are computed by the caller.
#[must_use]
pub fn critic_gap_prompt(
    base: &CriticBase,
    diff_path: &str,
    uncovered: &[String],
    not_reached: &[String],
    siblings: &[String],
) -> String {
    let list = |v: &[String]| {
        if v.is_empty() {
            "(none)".to_owned()
        } else {
            v.iter()
                .map(|x| format!("- {x}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    };
    let raised = raised_list(base.panel_input);
    render(
        CRITIC_GAP,
        &[
            ("prNumber", &s(Some(base.pr_number))),
            ("repo", base.repo),
            ("head", &s(base.head)),
            ("diffPath", diff_path),
            ("files", &base.files.join(", ")),
            ("raised", if raised.is_empty() { "(none)" } else { &raised }),
            ("uncovered", &list(uncovered)),
            ("notReached", &list(not_reached)),
            ("siblings", &list(siblings)),
        ],
    )
}

/// The inputs both critic prompts share.
pub struct CriticBase<'a> {
    pub pr_number: &'a Value,
    pub repo: &'a str,
    pub head: Option<&'a Value>,
    pub files: &'a [String],
    pub panel_input: &'a [Value],
}

fn raised_list(panel_input: &[Value]) -> String {
    panel_input
        .iter()
        .map(|f| {
            let line = get(f, "line");
            let at = if js::truthy(line) {
                format!(":{}", s(line))
            } else {
                String::new()
            };
            format!(
                "- {} ({}{at}, {})",
                s(get(f, "title")),
                s(get(f, "file")),
                s(get(f, "severity"))
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The panel-ceiling tie-break prompt. `group` is the tied findings'
/// summaries.
#[must_use]
pub fn tiebreak_prompt(group: &[Value], slots: usize, priority: &Value) -> String {
    let rows: Vec<Value> = group
        .iter()
        .map(|f| {
            let mut m = Map::new();
            for k in [
                "slotId",
                "title",
                "file",
                "severity",
                "touchesHighRiskSurface",
                "isBehaviorChanging",
            ] {
                if let Some(v) = get(f, k) {
                    m.insert(k.to_owned(), v.clone());
                }
            }
            Value::Object(m)
        })
        .collect();
    render(
        TIEBREAK,
        &[
            ("count", &group.len().to_string()),
            ("slots", &slots.to_string()),
            ("priority", &s(Some(priority))),
            ("group", &js::stringify(&Value::Array(rows), "  ")),
        ],
    )
}

/// `externalPromptText`. `slice` is `externalDiffSlice(finding).text`, or
/// `None` when the slice goes in as a file at the mark.
#[must_use]
pub fn external_prompt(
    finding: &Value,
    known_files: &[String],
    external_context: &str,
    slice: Option<&str>,
) -> String {
    render(
        EXTERNAL,
        &[
            ("file", &s(get(finding, "file"))),
            (
                "scopeLines",
                &external_scope_lines(finding, Some(known_files)).join("\n"),
            ),
            ("title", &s(get(finding, "title"))),
            ("location", &location(finding)),
            ("description", &described(finding)),
            ("externalContext", external_context),
            ("slice", slice.unwrap_or(EXTERNAL_SLICE_MARK)),
        ],
    )
}

/// The clean-verdict sweep's prompt, as the heredoc hands it to Codex.
#[must_use]
pub fn sweep_prompt(diff_path: Option<&str>, files: &[String], cwd: &str) -> String {
    let mut variants = SWEEP_DIFF_LINES.lines();
    let with_path = variants.next().unwrap_or_default();
    let without = variants.next().unwrap_or_default();
    let line = match diff_path.filter(|p| !p.is_empty()) {
        Some(p) => with_path.replace("{{diffPath}}", p),
        None => {
            let mut list = files
                .iter()
                .take(40)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            if files.len() > 40 {
                list.push_str(&format!(" and {} more", files.len() - 40));
            }
            without.replace("{{files}}", &list)
        }
    };
    render(SWEEP, &[("diffLine", &line), ("cwd", cwd)])
}

#[cfg(test)]
mod grounded_diagnostic_tests {
    use super::*;
    use serde_json::json;

    const DIAG: &str =
        "src/a.ts(4): TS2345: Argument of type 'Promise<string>' is not assignable to parameter of type 'string'.";

    fn findings() -> (Value, Value) {
        let plain = json!({"title": "T", "file": "src/a.ts", "line": 3, "severity": "important", "description": "D"});
        let mut shown = plain.clone();
        shown[GROUNDED_DIAGNOSTIC] = json!(DIAG);
        (plain, shown)
    }

    fn every_prompt(f: &Value) -> Vec<String> {
        let pr = json!(7);
        let repo = "acme/web-app";
        vec![
            refute_prompt(&pr, repo, "DIFF", "CTX", f, "L"),
            strengthen_prompt(&pr, repo, "DIFF", f, &[]),
            defense_prompt(&pr, repo, "DIFF", f, &[]),
            judge_prompt("DIFF", f, &[], None, true),
            external_prompt(f, &["src/a.ts".to_owned()], "CTX", None),
        ]
    }

    #[test]
    fn every_panel_prompt_shows_the_diagnostic_after_the_description() {
        let (_, shown) = findings();
        let note = format!(
            "D\n\nThe repository's own typecheck reported this within 5 lines of the location: {DIAG}\nIt is evidence about that code, not a verdict on the finding."
        );
        for p in every_prompt(&shown) {
            assert!(p.contains(&note), "{p}");
        }
    }

    #[test]
    fn a_finding_without_one_gets_the_prompts_it_always_got() {
        let (plain, shown) = findings();
        let mut stripped = shown.clone();
        stripped
            .as_object_mut()
            .unwrap()
            .remove(GROUNDED_DIAGNOSTIC);
        assert_eq!(every_prompt(&plain), every_prompt(&stripped));
        for p in every_prompt(&plain) {
            assert!(!p.contains("typecheck reported"), "{p}");
        }
    }

    #[test]
    fn the_batch_prompt_escapes_the_diagnostic() {
        let (_, shown) = findings();
        let p = batch_prompt(&[shown], "L", &json!(7), "acme/web-app", "DIFF", "CTX");
        assert!(p.contains("'Promise&lt;string&gt;'"), "{p}");
        assert!(!p.contains("Promise<string>"), "{p}");
    }
}

#[cfg(test)]
mod runtime_case_tests {
    use super::*;
    use serde_json::json;

    fn findings() -> (Value, Value) {
        let plain = json!({"title": "T", "file": "src/a.ts", "line": 3, "severity": "important", "description": "D"});
        let mut shown = plain.clone();
        shown[RUNTIME_CASE] = json!({
            "function": "ratio",
            "args": [1, 0],
            "expect": {"returns": 0},
            "failure": "AssertionError: expected Infinity to deeply equal +0 <x>"
        });
        (plain, shown)
    }

    fn every_prompt(f: &Value) -> Vec<String> {
        let pr = json!(7);
        let repo = "acme/web-app";
        vec![
            refute_prompt(&pr, repo, "DIFF", "CTX", f, "L"),
            strengthen_prompt(&pr, repo, "DIFF", f, &[]),
            defense_prompt(&pr, repo, "DIFF", f, &[]),
            judge_prompt("DIFF", f, &[], None, true),
            external_prompt(f, &["src/a.ts".to_owned()], "CTX", None),
        ]
    }

    #[test]
    fn every_panel_prompt_shows_the_failing_case_after_the_description() {
        let (_, shown) = findings();
        let note = "D\n\nA test generated from this finding ran at the PR head with the repository's own vitest and failed. It called ratio(1, 0) and expected it to return 0. The failure: AssertionError: expected Infinity to deeply equal +0 <x>\nIt is evidence about that code, not a verdict on the finding: the expected value comes from the finding.";
        for p in every_prompt(&shown) {
            assert!(p.contains(note), "{p}");
        }
    }

    #[test]
    fn a_throw_expectation_reads_as_one() {
        let (_, mut shown) = findings();
        shown[RUNTIME_CASE]["expect"] = json!({"throws": true});
        let p = refute_prompt(&json!(7), "acme/web-app", "DIFF", "CTX", &shown, "L");
        assert!(
            p.contains("called ratio(1, 0) and expected it to throw."),
            "{p}"
        );
    }

    #[test]
    fn a_finding_without_one_gets_the_prompts_it_always_got() {
        let (plain, shown) = findings();
        let mut stripped = shown.clone();
        stripped.as_object_mut().unwrap().remove(RUNTIME_CASE);
        assert_eq!(every_prompt(&plain), every_prompt(&stripped));
        for p in every_prompt(&plain) {
            assert!(!p.contains("A test generated"), "{p}");
        }
    }

    #[test]
    fn the_batch_prompt_escapes_the_failure() {
        let (_, shown) = findings();
        let p = batch_prompt(&[shown], "L", &json!(7), "acme/web-app", "DIFF", "CTX");
        assert!(p.contains("+0 &lt;x&gt;"), "{p}");
        assert!(!p.contains("+0 <x>"), "{p}");
    }
}

#[cfg(test)]
mod gap_prompt_tests {
    use super::*;
    use serde_json::json;

    fn render_gap(uncovered: &[String], not_reached: &[String]) -> String {
        critic_gap_prompt(
            &CriticBase {
                pr_number: &json!(7),
                repo: "acme/web-app",
                head: Some(&json!("abc123")),
                files: &["src/a.ts".to_owned(), "src/b.ts".to_owned()],
                panel_input: &[
                    json!({"title": "T", "file": "src/a.ts", "line": 3, "severity": "important"}),
                ],
            },
            "/tmp/pr-7.patch",
            uncovered,
            not_reached,
            &["src/c.ts:4 [shared-callee x] `x(1)`".to_owned()],
        )
    }

    #[test]
    fn the_gap_prompt_carries_both_computed_lists_and_the_diff_path() {
        let p = render_gap(&["src/b.ts".to_owned()], &["db layer".to_owned()]);
        assert!(p.contains("/tmp/pr-7.patch"));
        assert!(p.contains("- src/b.ts"));
        assert!(p.contains("- db layer"));
        assert!(p.contains("- src/c.ts:4 [shared-callee x]"));
        assert!(p.contains("- T (src/a.ts:3, important)"));
        assert!(!p.contains("{{"));
    }

    #[test]
    fn the_gap_prompt_never_sends_the_critic_to_gh() {
        let p = render_gap(&[], &[]);
        assert!(!p.contains("gh pr"));
        assert!(!p.contains("gh api"));
        assert!(p.contains("Use at most 20 tool calls"));
        assert_eq!(p.matches("(none)").count(), 2);
    }

    #[test]
    fn the_gap_prompt_says_a_shared_subject_is_not_the_same_defect() {
        let p = render_gap(&[], &[]);
        assert!(p.contains("names a defect only when it describes the same failure"));
        assert!(p.contains("give that finding's title in the summary"));
    }
}
