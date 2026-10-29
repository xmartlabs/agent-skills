//! The refutation panel's prompts: the Claude refuter (alone and batched),
//! the external heads, the strengthen pass, the debate's defense and judge,
//! the completeness critic, the panel-ceiling tie-break and the clean-verdict
//! sweep. The texts under `panel/` are cut from the workflow's own template
//! literals, each interpolation named `{{name}}`.

use serde_json::{Map, Value};
use tribunal_core::js;
use tribunal_core::lens::diff_sectioning::external_scope_lines;
use tribunal_core::lens::js_ext::to_js_string;
use tribunal_core::panel::debate_degrade::debate_defense_block;

use crate::decisions::PRECEDENCE_RULE;
use crate::synthesize::render;

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
            ("description", &s(get(finding, "description"))),
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
                    ("description", &escape_xml(get(f, "description"))),
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
            ("description", &s(get(finding, "description"))),
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
            ("description", &s(get(finding, "description"))),
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
            ("description", &s(get(finding, "description"))),
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
            ("description", &s(get(finding, "description"))),
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
