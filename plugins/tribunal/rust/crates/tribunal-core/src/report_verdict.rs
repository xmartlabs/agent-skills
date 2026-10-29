//! Port of the REPORT-VERDICT region: the report's "Verdict suggestion" line
//! is pinned to the harness verdict. A line naming exactly that verdict is left
//! alone; a missing line is inserted under the title; a line naming another
//! verdict, more than one, or none is replaced.
//!
//! The JavaScript matched lines with `/^#{1,6}[ \t]*Verdict suggestion:.*$/gim`.
//! That is spelled out by hand here because the `regex` crate disagrees with it
//! on what ends a line and on case folding (see `js`).

use serde::Serialize;

use crate::js;

const VERDICTS: [&str; 3] = ["REQUEST CHANGES", "APPROVE", "COMMENT"];
const LABEL: &str = "Verdict suggestion:";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Correction {
    pub from: Option<String>,
    pub to: String,
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pinned {
    pub text: String,
    pub corrected: Option<Correction>,
}

/// The one verdict a line names, or `None` when it names none or several.
pub fn stated_verdict(line: &str) -> Option<&'static str> {
    let upper = line.to_uppercase();
    let named: Vec<&'static str> = VERDICTS.into_iter().filter(|v| upper.contains(v)).collect();
    match named.as_slice() {
        [one] => Some(one),
        _ => None,
    }
}

/// Byte ranges of every verdict line, in order.
fn verdict_lines(body: &str) -> Vec<(usize, usize)> {
    js::line_spans(body)
        .into_iter()
        .filter(|&(start, end)| is_verdict_line(&body[start..end]))
        .collect()
}

fn is_verdict_line(line: &str) -> bool {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return false;
    }
    let rest = line[hashes..].trim_start_matches([' ', '\t']);
    js::starts_with_ascii_ci(rest, LABEL)
}

/// The first `## ` line, as `body.match(/^## .*$/m)` found it.
fn title_line(body: &str) -> Option<&str> {
    js::line_spans(body)
        .into_iter()
        .map(|(start, end)| &body[start..end])
        .find(|line| line.starts_with("## "))
}

pub fn pin_report_verdict(text: Option<&str>, verdict: &str) -> Pinned {
    let body = text.unwrap_or("");
    let pinned = format!("### Verdict suggestion: {verdict}");
    let lines = verdict_lines(body);

    if lines.is_empty() {
        let out = match title_line(body) {
            // `String.replace` with a string pattern replaces its first
            // occurrence, which can sit inside an earlier line.
            Some(title) => body.replacen(title, &format!("{title}\n\n{pinned}"), 1),
            None => format!("{pinned}\n\n{body}"),
        };
        return Pinned {
            text: out,
            corrected: Some(Correction {
                from: None,
                to: verdict.to_owned(),
                reason: "missing",
            }),
        };
    }

    let stated: Vec<Option<&'static str>> = lines
        .iter()
        .map(|&(s, e)| stated_verdict(&body[s..e]))
        .collect();
    if lines.len() == 1 && stated[0] == Some(verdict) {
        return Pinned {
            text: body.to_owned(),
            corrected: None,
        };
    }
    // `stated.find(x => x !== verdict)`: an unreadable line (null) differs too.
    let from = stated
        .iter()
        .find(|x| **x != Some(verdict))
        .copied()
        .flatten();
    let reason = if lines.len() > 1 {
        "multiple"
    } else if stated[0].is_none() {
        "unreadable"
    } else {
        "mismatch"
    };

    let mut out = String::with_capacity(body.len());
    let mut cursor = 0;
    for &(start, end) in &lines {
        out.push_str(&body[cursor..start]);
        out.push_str(&pinned);
        cursor = end;
    }
    out.push_str(&body[cursor..]);

    Pinned {
        text: out,
        corrected: Some(Correction {
            from: from.map(str::to_owned),
            to: verdict.to_owned(),
            reason,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matching_line_is_left_alone() {
        let body = "## Review\n\n### Verdict suggestion: APPROVE\n";
        let out = pin_report_verdict(Some(body), "APPROVE");
        assert_eq!(out.text, body);
        assert_eq!(out.corrected, None);
    }

    #[test]
    fn a_mismatch_is_rewritten() {
        let body = "## R\n#### verdict SUGGESTION: comment, unfinished\nrest";
        let out = pin_report_verdict(Some(body), "APPROVE");
        assert_eq!(out.text, "## R\n### Verdict suggestion: APPROVE\nrest");
        let c = out.corrected.unwrap();
        assert_eq!((c.from.as_deref(), c.reason), (Some("COMMENT"), "mismatch"));
    }

    #[test]
    fn a_missing_line_goes_under_the_title() {
        let out = pin_report_verdict(Some("intro\n## Title\nbody"), "COMMENT");
        assert_eq!(
            out.text,
            "intro\n## Title\n\n### Verdict suggestion: COMMENT\nbody"
        );
        assert_eq!(out.corrected.unwrap().reason, "missing");
    }

    #[test]
    fn seven_hashes_is_not_a_verdict_line() {
        let out = pin_report_verdict(Some("####### Verdict suggestion: APPROVE"), "APPROVE");
        assert_eq!(out.corrected.unwrap().reason, "missing");
    }

    #[test]
    fn a_carriage_return_ends_the_line() {
        let body = "### Verdict suggestion: COMMENT\r\nnext";
        let out = pin_report_verdict(Some(body), "APPROVE");
        assert_eq!(out.text, "### Verdict suggestion: APPROVE\r\nnext");
    }
}
