//! Port of the CLEAN-SWEEP region: when nothing faces a panel, one Codex sweep
//! over the diff either backs the clean verdict or raises candidates. This is
//! the gate that decides whether it runs, the parser for its strict answer
//! format, and the check that a polled answer is the sweep's and not prose.
//!
//! The JavaScript parsed lines with regexes using `\s`, `.`, `\d` and the `i`
//! flag. Each is spelled out by hand here because the `regex` crate disagrees
//! on all four: its `\s` adds U+0085 and drops U+FEFF, its `.` matches `\r`
//! and U+2028/2029, its `\d` is Unicode, and its case folding maps U+017F to
//! `s` and U+212A to `k`.

use serde::Serialize;
use serde_json::Value;

use super::js_ext;
use crate::js;

pub struct SweepGateInput<'a> {
    pub panel_eligible_count: usize,
    pub codex_installed: bool,
    pub codex_healthy: bool,
    /// The pinned head worktree; empty means none, as JavaScript's truthiness
    /// had it.
    pub external_cwd: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SweepGate {
    pub run: bool,
    pub reason: Option<&'static str>,
}

/// Ports `shouldRunCleanSweep`.
pub fn should_run_clean_sweep(input: &SweepGateInput) -> SweepGate {
    let skip = |reason| SweepGate {
        run: false,
        reason: Some(reason),
    };
    if input.panel_eligible_count > 0 {
        return skip("panels-exist");
    }
    if !input.codex_installed {
        return skip("codex-not-installed");
    }
    if !input.codex_healthy {
        return skip("codex-unhealthy");
    }
    if input.external_cwd.is_none_or(str::is_empty) {
        return skip("no-pinned-head");
    }
    SweepGate {
        run: true,
        reason: None,
    }
}

/// Ports `sweepFamilyFor`.
pub fn sweep_family_for(gate: &SweepGate) -> Option<&'static str> {
    gate.run.then_some("codex")
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SweepAnchor {
    pub file: String,
    pub line: Value,
    pub quote: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SweepFinding {
    pub severity: String,
    pub title: String,
    pub file: String,
    pub line: Value,
    pub why: String,
    pub evidence: String,
    pub verified_by: Vec<SweepAnchor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HeadFailure {
    pub class: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SweepParse {
    pub clean: bool,
    pub unparseable: bool,
    pub findings: Vec<SweepFinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_failure: Option<HeadFailure>,
}

impl SweepParse {
    fn unparseable() -> Self {
        Self {
            clean: false,
            unparseable: true,
            findings: Vec::new(),
            head_failure: None,
        }
    }
}

const NO_QUOTE: &str = "No relevant quotes found";
const CLEAN_MARKER: &str = "NO FINDINGS ABOVE NIT";

fn skip_ws(s: &str) -> &str {
    s.trim_start_matches(js::is_js_whitespace)
}

/// A literal under the `i` flag without `u`: only ASCII letters fold.
fn strip_ci<'a>(s: &'a str, lit: &str) -> Option<&'a str> {
    js::starts_with_ascii_ci(s, lit).then(|| &s[lit.len()..])
}

/// `\s*(.+)$` without the `m` flag, on what follows a label: the capture runs
/// to the end of the line and `.` stops at `\r`, U+2028 and U+2029. Returns
/// the capture trimmed, which is all any caller keeps.
fn rest_of_line(s: &str) -> Option<String> {
    let body = skip_ws(s);
    let ok = if body.is_empty() {
        // All whitespace: backtracking hands `.+` the last character alone.
        s.chars().last().is_some_and(|c| !js::is_line_terminator(c))
    } else {
        !body.chars().any(js::is_line_terminator)
    };
    ok.then(|| js::trim(s).to_owned())
}

/// `\s*([^|:]+):(\d+)\s*\|`: a file up to the first colon (no pipe before
/// it), ASCII digits, then a pipe. Returns the trimmed file, the line digits
/// and what follows the pipe.
fn file_and_line(s: &str) -> Option<(String, &str, &str)> {
    let stop = s.find(['|', ':'])?;
    if stop == 0 || s.as_bytes()[stop] != b':' {
        return None;
    }
    let file = js::trim(&s[..stop]).to_owned();
    let after = &s[stop + 1..];
    let digits_len = after.bytes().take_while(u8::is_ascii_digit).count();
    if digits_len == 0 {
        return None;
    }
    let rest = skip_ws(&after[digits_len..]).strip_prefix('|')?;
    Some((file, &after[..digits_len], rest))
}

/// `Number(m[4])` on a run of ASCII digits.
fn digits_value(digits: &str) -> Value {
    js_ext::number_value(digits.parse::<f64>().expect("ASCII digits parse"))
}

struct FindingLine {
    severity: String,
    title: String,
    file: String,
    line: Value,
    why: String,
}

/// `^\s*FINDING:\s*(critical|important|nit)\s*\|\s*([^|]+?)\s*\|\s*([^|:]+):(\d+)\s*\|\s*(.+)$/i`
fn finding_line(line: &str) -> Option<FindingLine> {
    let rest = skip_ws(strip_ci(skip_ws(line), "FINDING:")?);
    let (severity, rest) = ["critical", "important", "nit"]
        .into_iter()
        .find_map(|w| strip_ci(rest, w).map(|r| (w, r)))?;
    let rest = skip_ws(rest).strip_prefix('|')?;
    let title_end = rest.find('|')?;
    if title_end == 0 {
        return None;
    }
    let title = js::trim(&rest[..title_end]).to_owned();
    let (file, digits, rest) = file_and_line(&rest[title_end + 1..])?;
    let why = rest_of_line(rest)?;
    Some(FindingLine {
        severity: severity.to_owned(),
        title,
        file,
        line: digits_value(digits),
        why,
    })
}

/// `^\s*EVIDENCE:\s*(.+)$/i`
fn evidence_line(line: &str) -> Option<String> {
    rest_of_line(strip_ci(skip_ws(line), "EVIDENCE:")?)
}

/// `^\s*VERIFIED:\s*([^|:]+):(\d+)\s*\|\s*(.+)$/i`
fn verified_line(line: &str) -> Option<SweepAnchor> {
    // No whitespace skip before the file: an all-blank file still matches,
    // because backtracking gives `[^|:]+` one of the blanks.
    let (file, digits, rest) = file_and_line(strip_ci(skip_ws(line), "VERIFIED:")?)?;
    Some(SweepAnchor {
        file,
        line: digits_value(digits),
        quote: rest_of_line(rest)?,
    })
}

/// `/^HEAD FAILURE \((quota|auth|error)\): (.*)$/m`: the first line, by any
/// JavaScript line terminator, that starts with the classified failure.
fn head_failure(text: &str) -> Option<HeadFailure> {
    js::line_spans(text).into_iter().find_map(|(start, end)| {
        let line = text[start..end].strip_prefix("HEAD FAILURE (")?;
        let (class, rest) = ["quota", "auth", "error"]
            .into_iter()
            .find_map(|c| line.strip_prefix(c).map(|r| (c, r)))?;
        let reason = rest.strip_prefix("): ")?;
        Some(HeadFailure {
            class: class.to_owned(),
            reason: js_ext::utf16_prefix(js::trim(reason), 200),
        })
    })
}

fn contains_ascii_ci(haystack: &str, needle: &str) -> bool {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    h.len() >= n.len() && h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

/// Ports `parseSweepFindings`. `text` is the sweep artifact as read, or
/// nothing.
pub fn parse_sweep_findings(text: Option<&str>) -> SweepParse {
    let t = text.unwrap_or("");
    if js::trim(t).is_empty() {
        return SweepParse::unparseable();
    }
    if let Some(hf) = head_failure(t) {
        return SweepParse {
            head_failure: Some(hf),
            ..SweepParse::unparseable()
        };
    }
    let lines: Vec<&str> = t.split('\n').collect();
    let mut findings = Vec::new();
    for i in 0..lines.len() {
        let Some(m) = finding_line(lines[i]) else {
            continue;
        };
        let ev = lines.get(i + 1).and_then(|l| evidence_line(l));
        let first_verified = i + if ev.is_some() { 2 } else { 1 };
        let verified_by = lines
            .iter()
            .skip(first_verified)
            .map_while(|l| verified_line(l))
            .collect();
        findings.push(SweepFinding {
            severity: m.severity,
            title: m.title,
            file: m.file,
            line: m.line,
            why: m.why,
            evidence: ev.unwrap_or_else(|| NO_QUOTE.to_owned()),
            verified_by,
        });
    }
    if !findings.is_empty() {
        return SweepParse {
            clean: false,
            unparseable: false,
            findings,
            head_failure: None,
        };
    }
    if contains_ascii_ci(t, CLEAN_MARKER) {
        return SweepParse {
            clean: true,
            unparseable: false,
            findings,
            head_failure: None,
        };
    }
    SweepParse::unparseable()
}

/// Ports `sweepArtifactLooksReal`: a polled answer counts only when it carries
/// the sweep's own shape.
pub fn sweep_artifact_looks_real(text: Option<&str>) -> bool {
    let t = text.unwrap_or("");
    t.contains("FINDING:") || t.contains(CLEAN_MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate(count: usize, installed: bool, healthy: bool, cwd: Option<&str>) -> SweepGate {
        should_run_clean_sweep(&SweepGateInput {
            panel_eligible_count: count,
            codex_installed: installed,
            codex_healthy: healthy,
            external_cwd: cwd,
        })
    }

    #[test]
    fn the_gate_names_why_it_skipped() {
        assert!(gate(0, true, true, Some("/tmp/wt")).run);
        assert_eq!(gate(0, true, true, Some("")).reason, Some("no-pinned-head"));
        assert_eq!(
            gate(1, true, true, Some("/tmp/wt")).reason,
            Some("panels-exist")
        );
        assert_eq!(
            gate(0, false, false, None).reason,
            Some("codex-not-installed")
        );
        assert_eq!(gate(0, true, false, None).reason, Some("codex-unhealthy"));
        assert_eq!(
            sweep_family_for(&gate(0, true, true, Some("/w"))),
            Some("codex")
        );
    }

    #[test]
    fn findings_parse_with_their_anchors() {
        let p = parse_sweep_findings(Some(
            "FINDING: important | Missing guard | src/a.ts:10 | a null id reaches the write\nEVIDENCE: await write(id)\nVERIFIED: src/b.ts:4 | export function write(id: string) {\nFINDING: NIT | Name | src/a.ts:20 | unclear",
        ));
        assert_eq!(p.findings.len(), 2);
        assert_eq!(p.findings[0].verified_by[0].file, "src/b.ts");
        assert_eq!(p.findings[1].severity, "nit");
        assert_eq!(p.findings[1].evidence, NO_QUOTE);
    }

    #[test]
    fn prose_is_unparseable_never_clean() {
        let p = parse_sweep_findings(Some("Looks good to me."));
        assert!(p.unparseable && !p.clean);
        assert!(parse_sweep_findings(None).unparseable);
        assert!(parse_sweep_findings(Some("no findings above nit.")).clean);
    }

    #[test]
    fn a_carriage_return_in_the_reason_stops_the_match() {
        let p = parse_sweep_findings(Some(
            "FINDING: nit | t | a.ts:1 | why\r\nNO FINDINGS ABOVE NIT",
        ));
        assert!(p.clean);
    }

    #[test]
    fn a_head_failure_names_its_class() {
        let p = parse_sweep_findings(Some("HEAD FAILURE (quota): ERROR: out of credits.\n"));
        assert_eq!(p.head_failure.unwrap().class, "quota");
    }

    #[test]
    fn narration_is_not_an_artifact() {
        assert!(!sweep_artifact_looks_real(Some(
            "Command executed with exit code 1."
        )));
        assert!(sweep_artifact_looks_real(Some("NO FINDINGS ABOVE NIT.")));
        assert!(!sweep_artifact_looks_real(None));
    }
}
