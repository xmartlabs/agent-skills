//! Completes the `line` of a `verifiedBy` anchor from its `quote`, before the
//! VERIFIED-GATE judges the finding.
//!
//! A lens writes `line: 0` for code it read without line numbers, and the gate
//! then demotes a finding the panel upheld for a missing locator although the
//! quote is word for word what the gate asks for. The line is derived with the
//! anchor resolver, so a quote that is not in its file at the reviewed head
//! still fails the gate.
//!
//! A finding with no `verifiedBy` at all gets one from its own evidence when the
//! evidence resolver already placed that evidence in the file at the head. When
//! the resolver did not find it, because it searches only the diff and the quote
//! is unchanged code of the file, the anchor is written without a line and the
//! line repair looks the quote up in the whole file at the head. A quote that is
//! not there either keeps no line, and the gate still demotes the finding.

use serde_json::{json, Value};
use tribunal_context::exec::Tools;
use tribunal_context::resolve_verified_anchors;
use tribunal_core::js;
use tribunal_core::verified_gate::MIN_QUOTE_CHARS;

const ANCHORED_KINDS: [&str; 2] = ["exact", "shifted"];
/// The resolver's kind for a quote that is in no line of the diff.
const NOT_IN_DIFF: &str = "not-found";

/// The anchor a finding's located evidence stands for: its file, the line the
/// evidence resolver placed it on and the evidence line itself.
fn evidence_anchor(finding: &Value) -> Option<Value> {
    let resolution = js::get(finding, "evidenceResolution")?;
    let kind = js::get(resolution, "matchKind").and_then(Value::as_str)?;
    let line = if kind == NOT_IN_DIFF {
        None
    } else if ANCHORED_KINDS.contains(&kind)
        && js::get(resolution, "evidenceLocated") == Some(&json!(true))
    {
        Some(
            js::get(resolution, "resolvedLine")
                .and_then(Value::as_u64)
                .filter(|l| *l >= 1)?,
        )
    } else {
        return None;
    };
    let file = match js::get(finding, "file") {
        Some(Value::String(s)) if !s.is_empty() => s,
        _ => return None,
    };
    let first_source = js::get(finding, crate::verify::EVIDENCE_BY_SOURCE)
        .and_then(|s| s.get(0))
        .and_then(|e| js::get(e, "evidence"))
        .and_then(Value::as_str);
    let Some(Value::String(evidence)) = js::get(finding, "evidence") else {
        return None;
    };
    let evidence = match (
        first_source,
        js::get(finding, "mergedFrom").and_then(|m| m.get(0)),
    ) {
        (Some(quote), _) => quote,
        (None, Some(Value::String(lens))) => evidence
            .strip_prefix(&format!("[{lens}] "))
            .unwrap_or(evidence),
        _ => evidence,
    };
    let first = evidence.lines().find(|l| !js::trim(l).is_empty())?;
    let quote = js::trim(first.strip_prefix(['+', '-', ' ']).unwrap_or(first));
    if js::utf16_len(quote) < MIN_QUOTE_CHARS {
        return None;
    }
    Some(match line {
        Some(line) => json!({"file": file, "line": line, "quote": quote}),
        None => json!({"file": file, "quote": quote}),
    })
}

/// Gives a finding with an empty `verifiedBy` the anchor its evidence stands
/// for, and returns how many it anchored. Evidence outside the diff gets an
/// anchor without a line for `repair_anchor_lines` to complete. Nits, evidence
/// the resolver found ambiguous, elsewhere or too generic, and findings that
/// already cite code are left as they were.
pub fn anchor_from_evidence(verified: &mut [Option<Value>]) -> usize {
    let mut anchored = 0;
    for v in verified.iter_mut().flatten() {
        let Some(finding) = v.get_mut("finding").filter(|f| f.is_object()) else {
            continue;
        };
        if js::get(finding, "severity") == Some(&json!("nit")) {
            continue;
        }
        let empty = match js::get(finding, "verifiedBy") {
            Some(Value::Array(a)) => a.is_empty(),
            Some(Value::Null) | None => true,
            Some(_) => false,
        };
        if !empty {
            continue;
        }
        if let Some(anchor) = evidence_anchor(finding) {
            finding["verifiedBy"] = json!([anchor]);
            anchored += 1;
        }
    }
    anchored
}

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

/// Runs both repairs over every result the report gate will judge: the panel's
/// `verified` and the `confirmed` results that skipped the panel (grounded,
/// `--no-refuters`, which is the default, and the critic's gaps). Both lists
/// keep their order and length. Before this, only `verified` was repaired, so
/// with the panel off no anchor was ever completed.
pub async fn repair_anchors(
    tools: &Tools,
    head: Option<&str>,
    repo: &str,
    cross_repo: &[String],
    verified: &mut Vec<Option<Value>>,
    confirmed: &mut Vec<Value>,
    logs: &mut Vec<String>,
) {
    let split = verified.len();
    let mut all = std::mem::take(verified);
    all.extend(std::mem::take(confirmed).into_iter().map(Some));
    let anchored = anchor_from_evidence(&mut all);
    if anchored > 0 {
        logs.push(format!(
            "verifiedBy: anchored {anchored} finding(s) written without one from their evidence"
        ));
    }
    if let Some(head) = head.filter(|h| !h.is_empty()) {
        repair_anchor_lines(tools, head, repo, cross_repo, &mut all, logs).await;
    }
    *confirmed = all.split_off(split).into_iter().flatten().collect();
    *verified = all;
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

    fn evidenced(severity: &str, kind: &str, line: Value, by: Option<Value>) -> Option<Value> {
        let mut f = json!({
            "title": "t", "severity": severity, "file": "src/db.ts",
            "evidence": "+  const prisma = new PrismaClient({",
            "evidenceResolution": {"matchKind": kind, "evidenceLocated": kind != "not-found",
                                   "resolvedLine": line},
        });
        if let Some(by) = by {
            f["verifiedBy"] = by;
        }
        Some(json!({"finding": f}))
    }

    fn by_of(v: &Option<Value>) -> Value {
        v.as_ref().unwrap()["finding"]
            .get("verifiedBy")
            .cloned()
            .unwrap_or(Value::Null)
    }

    #[test]
    fn located_evidence_becomes_the_anchor_of_a_finding_without_one() {
        for kind in ["exact", "shifted"] {
            let mut v = vec![
                evidenced("important", kind, json!(41), None),
                evidenced("critical", kind, json!(41), Some(json!([]))),
            ];
            assert_eq!(anchor_from_evidence(&mut v), 2, "{kind}");
            for row in &v {
                assert_eq!(
                    by_of(row),
                    json!([{"file": "src/db.ts", "line": 41,
                            "quote": "const prisma = new PrismaClient({"}])
                );
            }
        }
    }

    #[test]
    fn nothing_changes_for_a_nit_unplaced_evidence_or_an_existing_anchor() {
        let cited = json!([{"file": "src/a.ts", "line": 3, "quote": "export const a = 1;"}]);
        let mut v = vec![
            evidenced("nit", "exact", json!(41), None),
            evidenced("nit", "not-found", Value::Null, None),
            evidenced("important", "ambiguous", Value::Null, None),
            evidenced("important", "elsewhere", json!(41), None),
            evidenced("important", "exact", Value::Null, None),
            evidenced("important", "exact", json!(41), Some(cited.clone())),
            None,
            Some(json!({"finding": {"severity": "important"}})),
        ];
        let before = v.clone();
        assert_eq!(anchor_from_evidence(&mut v), 0);
        assert_eq!(v, before);
        assert_eq!(by_of(&v[5]), cited);
    }

    #[test]
    fn evidence_outside_the_diff_becomes_an_anchor_without_a_line() {
        let mut v = vec![
            evidenced("important", "not-found", Value::Null, None),
            evidenced("critical", "not-found", Value::Null, Some(json!([]))),
        ];
        assert_eq!(anchor_from_evidence(&mut v), 2);
        for row in &v {
            assert_eq!(
                by_of(row),
                json!([{"file": "src/db.ts", "quote": "const prisma = new PrismaClient({"}])
            );
        }
        let (targets, _) = broken_anchors(&v);
        assert_eq!(targets.len(), 2, "both go to the line repair");
    }

    #[test]
    fn evidence_too_short_to_pass_the_gate_is_not_turned_into_an_anchor() {
        let mut v = vec![evidenced("important", "exact", json!(4), None)];
        v[0].as_mut().unwrap()["finding"]["evidence"] = json!("+ return x;");
        assert_eq!(anchor_from_evidence(&mut v), 0);
        v[0].as_mut().unwrap()["finding"]["evidence"] = json!("\n+  let total = a + b;\n+  more");
        assert_eq!(anchor_from_evidence(&mut v), 1);
        assert_eq!(by_of(&v[0])[0]["quote"], "let total = a + b;");
    }

    #[test]
    fn the_lens_tag_of_a_merged_finding_is_not_part_of_the_quote() {
        let mut v = vec![evidenced("important", "shifted", json!(41), None)];
        let f = &mut v[0].as_mut().unwrap()["finding"];
        f["evidence"] = json!(
            "[correctness] const prisma = new PrismaClient({\n  log: [],\n});\n[errorHandling] other()"
        );
        f["mergedFrom"] = json!(["correctness", "errorHandling"]);
        assert_eq!(anchor_from_evidence(&mut v), 1);
        assert_eq!(
            by_of(&v[0])[0]["quote"],
            "const prisma = new PrismaClient({"
        );
    }

    #[test]
    fn a_merged_finding_anchors_on_its_first_sources_own_quote() {
        let mut v = vec![evidenced("important", "shifted", json!(41), None)];
        let f = &mut v[0].as_mut().unwrap()["finding"];
        f["evidence"] = json!("[correctness] stale joined text\n[errorHandling] other()");
        f["mergedFrom"] = json!(["correctness", "errorHandling"]);
        f[crate::verify::EVIDENCE_BY_SOURCE] = json!([
            {"lens": "correctness", "evidence": "[first] = new PrismaClient({"},
            {"lens": "errorHandling", "evidence": "other()"},
        ]);
        assert_eq!(anchor_from_evidence(&mut v), 1);
        assert_eq!(by_of(&v[0])[0]["quote"], "[first] = new PrismaClient({");
    }
}
