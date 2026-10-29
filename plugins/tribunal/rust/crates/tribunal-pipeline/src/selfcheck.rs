//! `tribunal selfcheck`: `workflows/harness-selfcheck.js` as a program. It
//! audits the last metrics row with `audit_run`, diagnoses each violation in a
//! dispatch that must quote what it read, puts each diagnosis to a refuter and
//! a reproducer, and asks for a patch for the diagnoses neither refutes. The
//! patch is written down, not applied, unless the operator passed `--apply`:
//! this audits the harness that runs it, and a loop that also approves itself
//! has no outside check left. The battery is `cargo test` over this workspace.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use tribunal_agent::{Agent, DispatchRequest, DispatchResult};
use tribunal_context::audit_run::{self, Issue};
use tribunal_core::js;
use tribunal_core::lens::js_ext::to_js_string;

use crate::context::Runtime;
use crate::panel::join_all;

/// The operator's flags.
#[derive(Debug, Clone, Default)]
pub struct SelfcheckArgs {
    /// Apply the patch and keep it only if the battery stays green.
    pub apply: bool,
    pub model: String,
    pub effort: Option<String>,
    /// Skip `cargo test`, for a machine with no Rust toolchain.
    pub no_battery: bool,
}

fn diagnosis_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "violationId": {"type": "string"},
            "rootCause": {"type": "string"},
            "evidence": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {"source": {"type": "string"}, "quote": {"type": "string"}},
                    "required": ["source", "quote"],
                    "additionalProperties": false
                }
            },
            "confidence": {"type": "number"},
            "proposedFix": {"type": "string"},
            "filesToChange": {"type": "array", "items": {"type": "string"}},
            "ruledOut": {"type": "array", "items": {"type": "string"}}
        },
        "required": ["violationId", "rootCause", "evidence", "confidence", "proposedFix"],
        "additionalProperties": false
    })
}

fn verdict_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "refuted": {"type": "boolean"},
            "reason": {"type": "string"},
            "alternativeCause": {"type": ["string", "null"]}
        },
        "required": ["refuted", "reason"],
        "additionalProperties": false
    })
}

/// The last non-empty line of `metrics.jsonl`.
fn last_row(data_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(data_dir.join("metrics.jsonl")).ok()?;
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(str::to_owned)
}

/// What the diagnosis reads for the code that wrote `row`: the Rust source for
/// a row the binary wrote (`engine: "rust"`), the Workflow and its scripts for
/// any other.
#[must_use]
pub fn code_to_read(plugin_root: &Path, row: &Value) -> String {
    let root = plugin_root.display();
    if js::get(row, "engine").and_then(Value::as_str) == Some("rust") {
        format!("the Rust engine under {root}/rust/crates (the row's `engineVersion` names the build that wrote it)")
    } else {
        format!("{root}/workflows/tribunal.js and {root}/lib/")
    }
}

/// What the diagnosis is told about the run.
#[derive(Debug, Clone)]
pub struct RunFacts {
    pub pr: String,
    pub head_sha: String,
    pub artifact_dir: String,
    pub metrics_path: String,
    pub code: String,
}

#[must_use]
pub fn diagnose_prompt(v: &Issue, run: &RunFacts) -> String {
    format!(
        "A self-audit of the tribunal review harness failed an invariant. Find the ROOT CAUSE. Do not fix anything and do not edit any file.

Violation: {id}
Detail: {detail}
Run: PR #{pr} at {head}
Artifact directory: {dir}
Metrics row: the last line of {metrics}

Diagnose from evidence you read, not from a story that fits. This project has twice adopted a confident wrong diagnosis by skipping that: once concluding an external model's git worktree was degraded when it was fine, once concluding agents were re-invoking a CLI when counters were accumulating across runs in a shared directory. One `ls` and one `stat` would have refuted both.

So go to the artifacts: list the directory, read the .out and .meta files, check mtimes against the run. Then read the code that wrote the row: {code}. Every claim in rootCause needs an entry in evidence that quotes what you read and names where it came from.

Fill ruledOut with the causes you considered and what eliminated each. A diagnosis that ruled nothing out did not look.

Set confidence honestly: low confidence with real evidence is worth more than high confidence with a plausible story.",
        id = v.id,
        detail = v.detail,
        pr = run.pr,
        head = run.head_sha,
        dir = if run.artifact_dir.is_empty() {
            "(not recorded)"
        } else {
            &run.artifact_dir
        },
        metrics = run.metrics_path,
        code = run.code,
    )
}

fn evidence_lines(d: &Value) -> String {
    js::get(d, "evidence")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|e| {
            format!(
                "  - {}: {}",
                js::get(e, "source").and_then(Value::as_str).unwrap_or(""),
                js::get(e, "quote").and_then(Value::as_str).unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn text_of<'a>(v: &'a Value, key: &str) -> &'a str {
    js::get(v, key).and_then(Value::as_str).unwrap_or("")
}

#[must_use]
pub fn refute_prompt(v: &Issue, d: &Value) -> String {
    let ruled_out = js::get(d, "ruledOut")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("; ")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "(nothing)".into());
    format!(
        "REFUTE this root-cause diagnosis of a harness defect. Find the fact that proves it is NOT the cause. Default to refuted if you cannot positively confirm it.

Violation: {} ({})
Claimed root cause: {}
Evidence offered:
{}
Already ruled out by the diagnostician: {ruled_out}

Check the quotes yourself: a quote that is not really there is the cheapest refutation, and this harness has produced fabricated citations before. Then ask whether the evidence implies the cause or merely coexists with it. If a different cause fits the same evidence, name it in alternativeCause.",
        v.id,
        v.detail,
        text_of(d, "rootCause"),
        evidence_lines(d),
    )
}

#[must_use]
pub fn reproduce_prompt(v: &Issue, d: &Value, artifact_dir: &str) -> String {
    format!(
        "A harness self-audit reported: {} ({})
A diagnostician says the root cause is: {}

Your lens is reproduction, not argument. Can you make the failure appear or disappear on demand? Build the smallest concrete check (a file listing, a timestamp comparison, a two-line script against the real artifacts in {}) that separates this cause from a near neighbour. Run it and report what you observed.

If the cause is real, the check shows it. If your check cannot separate this cause from another explanation, say so and set refuted to true: a cause that cannot be told apart has not been established.",
        v.id,
        v.detail,
        text_of(d, "rootCause"),
        if artifact_dir.is_empty() {
            "the artifact directory"
        } else {
            artifact_dir
        },
    )
}

/// One diagnosis that survived both verifiers, with its violation.
#[derive(Debug, Clone)]
pub struct Confirmed {
    pub violation: Issue,
    pub diagnosis: Value,
}

#[must_use]
pub fn patch_prompt(confirmed: &[Confirmed], apply: bool, rust_dir: &Path) -> String {
    let defects = confirmed
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let files = js::get(&c.diagnosis, "filesToChange")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "(to determine)".into());
            format!(
                "--- Defect {}: {} ---\nRoot cause: {}\nProposed fix: {}\nFiles: {files}\nEvidence:\n{}",
                i + 1,
                c.violation.id,
                text_of(&c.diagnosis, "rootCause"),
                text_of(&c.diagnosis, "proposedFix"),
                evidence_lines(&c.diagnosis),
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let how = if apply {
        format!(
            "APPLY the fixes, then run `cargo test --workspace --locked` in {}. If it fails, REVERT everything you changed and report the failure: a green battery is the condition for the edit surviving, not a formality after it.",
            rust_dir.display()
        )
    } else {
        "Do NOT edit any file. Write the exact patch you would apply (the file, the current text, the replacement) so a person can read it before it touches anything.".to_owned()
    };
    format!(
        "Propose fixes for {} confirmed defect(s) in the tribunal review harness, then show you did not break it.

{defects}

{how}

Comments explain why, never what; the code reads like what surrounds it; an invariant you add is checked against real failure data, not a constructed example.

Report what you changed or would change and, explicitly, what you did NOT verify.",
        confirmed.len()
    )
}

async fn dispatch<A: Agent>(
    agent: &A,
    args: &SelfcheckArgs,
    label: &str,
    prompt: String,
    schema: Option<Value>,
    cwd: &Path,
    may_write: bool,
) -> DispatchResult {
    let mut req = DispatchRequest::new(label, prompt, cwd);
    req.model = Some(args.model.clone());
    req.effort.clone_from(&args.effort);
    let mut tools = vec!["Read", "Grep", "Glob", "Bash"];
    if may_write {
        tools.extend(["Edit", "Write"]);
    }
    req.tools = tools.into_iter().map(str::to_owned).collect();
    req.schema = schema;
    req.timeout = Duration::from_secs(30 * 60);
    agent.run(&req).await
}

/// One violation's diagnosis and verification: `Ok` when both verifiers
/// failed to refute it, `Err` with the dropped entry otherwise.
async fn diagnose_and_verify<A: Agent>(
    agent: &A,
    args: &SelfcheckArgs,
    cwd: &Path,
    v: &Issue,
    run: &RunFacts,
) -> Result<Confirmed, Value> {
    let dropped = |why: &str, alternatives: Vec<Value>| json!({"id": v.id, "why": why, "alternatives": alternatives});
    let diag = dispatch(
        agent,
        args,
        &format!("diagnose:{}", v.id),
        diagnose_prompt(v, run),
        Some(diagnosis_schema()),
        cwd,
        false,
    )
    .await;
    let Some(d) = diag.structured().cloned() else {
        return Err(dropped("the diagnosis returned nothing", Vec::new()));
    };
    if js::get(&d, "evidence")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
    {
        return Err(dropped("no evidence cited", Vec::new()));
    }
    let (refute_label, reproduce_label) =
        (format!("refute:{}", v.id), format!("reproduce:{}", v.id));
    let (refute, reproduce) = tokio::join!(
        dispatch(
            agent,
            args,
            &refute_label,
            refute_prompt(v, &d),
            Some(verdict_schema()),
            cwd,
            false,
        ),
        dispatch(
            agent,
            args,
            &reproduce_label,
            reproduce_prompt(v, &d, &run.artifact_dir),
            Some(verdict_schema()),
            cwd,
            false,
        ),
    );
    let live: Vec<Value> = [refute.structured(), reproduce.structured()]
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    let alternatives: Vec<Value> = live
        .iter()
        .filter_map(|x| {
            js::get(x, "alternativeCause")
                .filter(|a| a.is_string())
                .cloned()
        })
        .collect();
    if live.len() < 2 {
        return Err(dropped(
            "a verifier did not return, so it is unverified",
            alternatives,
        ));
    }
    if let Some(x) = live
        .iter()
        .find(|x| js::get(x, "refuted") == Some(&json!(true)))
    {
        return Err(dropped(text_of(x, "reason"), alternatives));
    }
    Ok(Confirmed {
        violation: v.clone(),
        diagnosis: d,
    })
}

/// `cargo test` over the workspace: the summary line and whether it passed,
/// or why it did not run.
async fn battery(rust_dir: &Path) -> Value {
    let out = tokio::process::Command::new("cargo")
        .args(["test", "--workspace", "--locked", "--quiet"])
        .current_dir(rust_dir)
        .output()
        .await;
    match out {
        Err(e) => json!({"ran": false, "reason": format!("cannot run cargo: {e}")}),
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout);
            let (mut passed, mut failed) = (0u64, 0u64);
            for line in text.lines().filter(|l| l.starts_with("test result:")) {
                let n = |word: &str| {
                    line.split(';')
                        .find(|p| p.trim().ends_with(word))
                        .and_then(|p| p.split_whitespace().rev().nth(1))
                        .and_then(|n| n.parse::<u64>().ok())
                        .unwrap_or(0)
                };
                passed += n("passed");
                failed += n("failed");
            }
            json!({"ran": true, "green": o.status.success(), "passed": passed, "failed": failed})
        }
    }
}

/// The whole self-check. The value is what `tribunal selfcheck` prints.
pub async fn selfcheck<A: Agent>(
    agent: &A,
    rt: &Runtime,
    args: &SelfcheckArgs,
    logs: &mut Vec<String>,
) -> Value {
    let metrics_path = rt.data_dir.join("metrics.jsonl");
    let Some(line) = last_row(&rt.data_dir) else {
        logs.push(format!(
            "No metrics row at {}. If reviews have been running, this is a broken path, not an idle harness: check the file exists before reading this as nothing to check.",
            metrics_path.display()
        ));
        return json!({"audited": false, "reason": "no-metrics-row", "violations": [], "diagnoses": []});
    };
    let row: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
    let artifact_dir = text_of(&row, "artifactDir").to_owned();
    let audit = match audit_run::run(&[artifact_dir.as_str()], &line) {
        Ok(a) => a,
        Err(e) => audit_run::Audit {
            passed: false,
            checked: 0,
            violations: vec![Issue {
                id: "audit-failed-to-run".into(),
                detail: e,
            }],
            notes: None,
        },
    };
    let pr = js::get(&row, "pr").map_or_else(|| "?".to_owned(), |p| to_js_string(Some(p)));
    let head_sha = text_of(&row, "headSha").to_owned();
    let engine = text_of(&row, "engine").to_owned();
    let notes = audit.notes.clone().unwrap_or_default();
    let issues = |l: &[Issue]| -> Vec<Value> {
        l.iter()
            .map(|i| json!({"id": i.id, "detail": i.detail}))
            .collect()
    };
    if !notes.is_empty() {
        logs.push(format!(
            "{} recorded note(s), which do not fail the audit: {}. Not diagnosed.",
            notes.len(),
            notes
                .iter()
                .map(|n| n.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let base = json!({
        "audited": true, "pr": row.get("pr").cloned().unwrap_or(Value::Null),
        "headSha": head_sha, "engine": engine,
        "violations": issues(&audit.violations), "notes": issues(&notes),
    });
    if audit.violations.is_empty() {
        logs.push(format!(
            "Self-check clean: the last run (PR #{pr}) satisfies every invariant. Nothing to diagnose."
        ));
        let mut out = base;
        out["diagnoses"] = json!([]);
        return out;
    }
    logs.push(format!(
        "{} invariant violation(s) on PR #{pr}: {}. Diagnosing each from the artifacts on disk.",
        audit.violations.len(),
        audit
            .violations
            .iter()
            .map(|v| v.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));

    let run = RunFacts {
        pr: pr.clone(),
        head_sha: head_sha.clone(),
        artifact_dir: artifact_dir.clone(),
        metrics_path: metrics_path.display().to_string(),
        code: code_to_read(&rt.plugin_root, &row),
    };
    let cwd = rt.plugin_root.clone();
    let results = join_all(
        audit
            .violations
            .iter()
            .map(|v| diagnose_and_verify(agent, args, &cwd, v, &run)),
    )
    .await;
    let (mut confirmed, mut dropped) = (Vec::new(), Vec::new());
    for r in results {
        match r {
            Ok(c) => confirmed.push(c),
            Err(d) => dropped.push(d),
        }
    }
    logs.push(format!(
        "Diagnosis: {} root cause(s) survived adversarial verification, {} dropped.",
        confirmed.len(),
        dropped.len()
    ));

    let rust_dir: PathBuf = rt.plugin_root.join("rust");
    let patch = if confirmed.is_empty() {
        Value::Null
    } else {
        let res = dispatch(
            agent,
            args,
            "propose-patch",
            patch_prompt(&confirmed, args.apply, &rust_dir),
            None,
            &cwd,
            args.apply,
        )
        .await;
        res.text()
            .map_or(Value::Null, |t| Value::String(t.to_owned()))
    };
    let battery = if args.no_battery {
        json!({"ran": false, "reason": "--no-battery"})
    } else {
        battery(&rust_dir).await
    };

    let mut out = base;
    out["confirmed"] = confirmed
        .iter()
        .map(|c| {
            json!({
                "id": c.violation.id,
                "rootCause": js::get(&c.diagnosis, "rootCause"),
                "confidence": js::get(&c.diagnosis, "confidence"),
                "proposedFix": js::get(&c.diagnosis, "proposedFix"),
                "evidence": js::get(&c.diagnosis, "evidence"),
            })
        })
        .collect();
    out["dropped"] = Value::Array(dropped);
    out["patch"] = patch;
    out["battery"] = battery;
    out["applied"] = json!(args.apply);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rust_row_points_the_diagnosis_at_the_rust_source() {
        let root = Path::new("/p");
        assert!(code_to_read(root, &json!({"engine": "rust"})).contains("/p/rust/crates"));
        assert!(code_to_read(root, &json!({})).contains("/p/workflows/tribunal.js"));
    }

    #[test]
    fn the_patch_prompt_applies_only_when_asked() {
        let c = [Confirmed {
            violation: Issue {
                id: "x".into(),
                detail: "d".into(),
            },
            diagnosis: json!({"rootCause": "r", "proposedFix": "f", "evidence": [{"source": "s", "quote": "q"}]}),
        }];
        let dry = patch_prompt(&c, false, Path::new("/p/rust"));
        assert!(dry.contains("Do NOT edit any file"));
        assert!(dry.contains("  - s: q"));
        let apply = patch_prompt(&c, true, Path::new("/p/rust"));
        assert!(apply.contains("cargo test --workspace --locked` in /p/rust"));
    }
}
