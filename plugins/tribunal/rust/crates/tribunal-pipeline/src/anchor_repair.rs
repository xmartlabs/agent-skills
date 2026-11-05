//! Completes the `line` of a `verifiedBy` anchor from its `quote`, before the
//! VERIFIED-GATE judges the finding.
//!
//! A lens writes `line: 0` for code it read without line numbers, and the gate
//! then demotes a finding the panel upheld for a missing locator although the
//! quote is word for word what the gate asks for. The line is derived with the
//! anchor resolver, so a quote that is not in its file at the reviewed head
//! still fails the gate.

use serde_json::{json, Value};
use tribunal_context::exec::Tools;
use tribunal_context::resolve_verified_anchors;
use tribunal_core::js;
use tribunal_core::verified_gate::MIN_QUOTE_CHARS;

/// A finding in the panel's results whose anchors need a line: its slot there
/// and the positions of those anchors in its `verifiedBy`.
#[derive(Debug, PartialEq, Eq)]
pub struct Target {
    pub slot: usize,
    pub anchors: Vec<usize>,
}

fn needs_line(anchor: &Value) -> bool {
    let line = js::get(anchor, "line");
    let line_ok = js::is_integer(line) && line.and_then(Value::as_f64).is_some_and(|l| l >= 1.0);
    let file_ok = matches!(js::get(anchor, "file"), Some(Value::String(s)) if !s.is_empty());
    let quote_ok = matches!(
        js::get(anchor, "quote"),
        Some(Value::String(s)) if js::utf16_len(js::trim(s)) >= MIN_QUOTE_CHARS
    );
    !line_ok && file_ok && quote_ok
}

/// The anchors to resolve and the resolver's payload for them. Nits are
/// skipped: the gate does not judge them.
#[must_use]
pub fn broken_anchors(verified: &[Option<Value>]) -> (Vec<Target>, String) {
    let mut targets = Vec::new();
    let mut payload = Vec::new();
    for (slot, v) in verified.iter().enumerate() {
        let Some(finding) = v.as_ref().and_then(|v| js::get(v, "finding")) else {
            continue;
        };
        if js::get(finding, "severity") == Some(&json!("nit")) {
            continue;
        }
        let Some(anchors) = js::get(finding, "verifiedBy").and_then(Value::as_array) else {
            continue;
        };
        let idx: Vec<usize> = anchors
            .iter()
            .enumerate()
            .filter(|(_, a)| needs_line(a))
            .map(|(i, _)| i)
            .collect();
        if idx.is_empty() {
            continue;
        }
        payload.push(json!({
            "title": js::get(finding, "title").cloned().unwrap_or(Value::Null),
            "verifiedBy": idx.iter().map(|&i| anchors[i].clone()).collect::<Vec<_>>(),
        }));
        targets.push(Target { slot, anchors: idx });
    }
    (targets, js::stringify(&Value::Array(payload), ""))
}

fn index_of(r: &Value, key: &str) -> Option<usize> {
    js::get(r, key)
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
}

/// Writes each resolved line into its anchor and returns how many it wrote.
/// An anchor whose quote was not found, or whose lookup failed, is left as it
/// was, so the gate still demotes it.
pub fn apply_lines(verified: &mut [Option<Value>], targets: &[Target], results: &[Value]) -> usize {
    let mut written = 0;
    for r in results {
        let status = js::get(r, "status").and_then(Value::as_str);
        if !matches!(status, Some("found" | "found-shifted")) {
            continue;
        }
        let Some(line) = js::get(r, "resolvedLine")
            .and_then(Value::as_u64)
            .filter(|l| *l >= 1)
        else {
            continue;
        };
        let (Some(fi), Some(ai)) = (index_of(r, "findingIndex"), index_of(r, "anchorIndex")) else {
            continue;
        };
        let Some(&anchor) = targets.get(fi).and_then(|t| t.anchors.get(ai)) else {
            continue;
        };
        let slot = targets[fi].slot;
        let Some(Some(v)) = verified.get_mut(slot) else {
            continue;
        };
        let Some(a) = v
            .get_mut("finding")
            .and_then(|f| f.get_mut("verifiedBy"))
            .and_then(|b| b.get_mut(anchor))
        else {
            continue;
        };
        a["line"] = json!(line);
        written += 1;
    }
    written
}

/// Completes the lines of the panel's findings in place and logs what it did.
/// `cross_repo` is the profile's `--cross-repo` flags, as the resolver takes
/// them for the report's own anchor check.
pub async fn repair_anchor_lines(
    tools: &Tools,
    head: &str,
    repo: &str,
    cross_repo: &[String],
    verified: &mut [Option<Value>],
    logs: &mut Vec<String>,
) {
    let (targets, payload) = broken_anchors(verified);
    if targets.is_empty() {
        return;
    }
    let total: usize = targets.iter().map(|t| t.anchors.len()).sum();
    let mut argv: Vec<&str> = vec!["-", head, "--repo", repo, "--relay"];
    argv.extend(cross_repo.iter().map(String::as_str));
    match resolve_verified_anchors::run(&argv, &payload, tools).await {
        Ok(out) if js::get(&out, "ran") == Some(&json!(true)) => {
            let results = js::get(&out, "results")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice);
            let n = apply_lines(verified, &targets, results);
            logs.push(format!(
                "verifiedBy lines: completed {n} of {total} anchor(s) written without a line from their quote at {}",
                head.chars().take(8).collect::<String>()
            ));
        }
        Ok(_) => logs.push(format!(
            "verifiedBy lines: the resolver did not run, {total} anchor(s) keep the line they had"
        )),
        Err(e) => logs.push(format!(
            "verifiedBy lines: the resolver failed ({e}), {total} anchor(s) keep the line they had"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUOTE: &str = "const prisma = new PrismaClient({";

    fn finding(severity: &str, anchors: &Value) -> Option<Value> {
        Some(json!({"finding": {"title": "t", "severity": severity, "verifiedBy": anchors}}))
    }

    fn anchor(line: &Value, quote: &str) -> Value {
        json!({"file": "src/db.ts", "line": line, "quote": quote})
    }

    fn found(fi: usize, ai: usize, line: u64) -> Value {
        json!({"findingIndex": fi, "anchorIndex": ai, "status": "found-shifted", "resolvedLine": line})
    }

    #[test]
    fn only_an_anchor_without_a_positive_integer_line_is_sent() {
        let verified = vec![
            finding(
                "important",
                &json!([
                    anchor(&json!(12), QUOTE),
                    anchor(&json!(0), QUOTE),
                    anchor(&json!("7"), QUOTE),
                    anchor(&json!(null), "short"),
                ]),
            ),
            finding("nit", &json!([anchor(&json!(0), QUOTE)])),
            None,
        ];
        let (targets, payload) = broken_anchors(&verified);
        assert_eq!(
            targets,
            vec![Target {
                slot: 0,
                anchors: vec![1, 2]
            }]
        );
        let sent: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(sent[0]["verifiedBy"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_resolved_line_lands_on_its_anchor_and_nothing_else_moves() {
        let mut verified = vec![
            None,
            finding(
                "important",
                &json!([
                    anchor(&json!(12), QUOTE),
                    anchor(&json!(0), QUOTE),
                    anchor(&json!(0), QUOTE),
                ]),
            ),
        ];
        let (targets, _) = broken_anchors(&verified);
        let results = vec![
            found(0, 0, 41),
            json!({"findingIndex": 0, "anchorIndex": 1, "status": "not-found", "resolvedLine": null}),
        ];
        assert_eq!(apply_lines(&mut verified, &targets, &results), 1);
        let lines: Vec<u64> = verified[1].as_ref().unwrap()["finding"]["verifiedBy"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["line"].as_u64().unwrap())
            .collect();
        assert_eq!(lines, [12, 41, 0]);
    }

    #[test]
    fn a_repaired_finding_passes_the_gate_and_an_unfound_one_is_still_demoted() {
        use tribunal_core::verified_gate::finding_problems;
        let mut verified = vec![
            finding("important", &json!([anchor(&json!(0), QUOTE)])),
            finding("important", &json!([anchor(&json!(0), QUOTE)])),
        ];
        for v in verified.iter_mut().flatten() {
            v["finding"]["adjudicated"] = json!(true);
        }
        let (targets, _) = broken_anchors(&verified);
        let results = vec![
            found(0, 0, 9),
            json!({"findingIndex": 1, "anchorIndex": 0, "status": "not-found", "resolvedLine": null}),
        ];
        apply_lines(&mut verified, &targets, &results);
        let problems = |i: usize| finding_problems(&verified[i].as_ref().unwrap()["finding"]);
        assert!(problems(0).is_empty());
        assert_eq!(
            problems(1),
            ["verifiedBy[0].line is not a positive integer"]
        );
    }
}
