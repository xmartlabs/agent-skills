//! Port of the DRAFT-COMMENTS-STRIP region: the fidelity checks run over the
//! body GitHub receives, so the `### Draft comments` section that
//! `post-review.mjs` strips has to go before they look.
//!
//! The report is the synthesizer's text, a string by construction; the
//! JavaScript throws on anything else, and this takes `&str`.

use crate::js;

const HEADING: &str = "### Draft comments";

/// `/^### Draft comments\b/` without the `u` flag: `\b` only knows ASCII word
/// characters, so `### Draft commentsé` still matches.
pub fn is_draft_heading(line: &str) -> bool {
    line.strip_prefix(HEADING).is_some_and(|rest| {
        !rest
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Ports `stripDraftCommentsFromBody`. Removes the first draft-comments
/// section, up to the next `### ` heading, plus one blank line before it.
/// Lines split on `\n` only, so a `\r` stays part of its line.
pub fn strip_draft_comments_from_body(report_text: &str) -> String {
    let lines: Vec<&str> = report_text.split('\n').collect();
    let Some(start) = lines.iter().position(|l| is_draft_heading(l)) else {
        return report_text.to_owned();
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.starts_with("### "))
        .map_or(lines.len(), |i| start + 1 + i);
    let mut cut = start;
    if cut > 0 && js::trim(lines[cut - 1]).is_empty() {
        cut -= 1;
    }
    lines[..cut]
        .iter()
        .chain(&lines[end..])
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_the_section_and_the_blank_line_before_it() {
        let body = "### A\n\n### Draft comments\n\n- one\n\n### B\n";
        assert_eq!(strip_draft_comments_from_body(body), "### A\n### B\n");
    }

    #[test]
    fn a_lookalike_heading_is_kept_but_a_non_ascii_suffix_is_not() {
        let keep = "### Draft commentary\n- x";
        assert_eq!(strip_draft_comments_from_body(keep), keep);
        assert_eq!(
            strip_draft_comments_from_body("x\n### Draft commentsé\n- y"),
            "x"
        );
    }
}
