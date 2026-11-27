//! Where candidate labels come from: an automated reviewer whose inline
//! comments the author marks as addressed, and how to read those comments.
//! A source is data, so another reviewer that marks comments the same way is
//! one more row in [`SOURCES`].

use std::sync::OnceLock;

use regex::Regex;

/// `(name, reviewer account, marker)`. The marker is the text the reviewer
/// writes into a comment once a later commit addressed it, followed by the
/// commit, or by a range whose last commit is the fix.
const SOURCES: &[(&str, &str, &str)] =
    &[("coderabbit", "coderabbitai[bot]", "Addressed in commit")];

/// The name `tribunal corpus collect` uses when `--source` is not given.
pub const DEFAULT_SOURCE: &str = "coderabbit";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentSource {
    pub name: String,
    pub reviewer: String,
    pub marker: String,
}

/// A reviewer comment read into its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedComment {
    pub title: String,
    pub text: String,
    /// The reviewer's own label for the point (`Major`, `Minor`), when it
    /// gives one.
    pub reviewer_severity: Option<String>,
}

/// The names [`source`] knows.
#[must_use]
pub fn names() -> Vec<&'static str> {
    SOURCES.iter().map(|s| s.0).collect()
}

#[must_use]
pub fn source(name: &str) -> Option<CommentSource> {
    SOURCES
        .iter()
        .find(|s| s.0 == name)
        .map(|&(n, r, m)| CommentSource {
            name: n.to_owned(),
            reviewer: r.to_owned(),
            marker: m.to_owned(),
        })
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("a fixed pattern compiles"))
}

impl CommentSource {
    #[must_use]
    pub fn with_overrides(mut self, reviewer: Option<String>, marker: Option<String>) -> Self {
        if let Some(r) = reviewer {
            self.reviewer = r;
        }
        if let Some(m) = marker {
            self.marker = m;
        }
        self
    }

    /// The fix commit named after the marker: the only commit, or the last
    /// one of a range. `None` when the body has no marker or the marker names
    /// no commit.
    #[must_use]
    pub fn addressed_fix(&self, body: &str) -> Option<String> {
        static SHA: OnceLock<Regex> = OnceLock::new();
        let at = body.find(&self.marker)?;
        let rest = &body[at + self.marker.len()..];
        let line = rest.lines().next().unwrap_or("");
        re(&SHA, r"\b[0-9a-f]{7,40}\b")
            .find_iter(line)
            .last()
            .map(|m| m.as_str().to_owned())
    }

    /// Title, text and the reviewer's severity, with HTML comments, collapsed
    /// sections, images and the marker line removed.
    #[must_use]
    pub fn parse_comment(&self, body: &str) -> ParsedComment {
        static HTML_COMMENT: OnceLock<Regex> = OnceLock::new();
        static SEVERITY: OnceLock<Regex> = OnceLock::new();
        static TAG: OnceLock<Regex> = OnceLock::new();
        let mut reviewer_severity = re(&SEVERITY, r"</picture>\s*([A-Za-z][A-Za-z ]*?)\*\*")
            .captures(body)
            .map(|c| c[1].trim().to_owned());
        let body = re(&HTML_COMMENT, r"(?s)<!--.*?-->").replace_all(body, "");
        let body = strip_details(&body);
        let mut title = None;
        let mut text = Vec::new();
        for line in body.lines() {
            let t = line.trim();
            if t.is_empty() || t.contains(&self.marker) || t.contains("<picture>") {
                continue;
            }
            if title.is_none() {
                let plain = re(&TAG, r"<[^>]+>").replace_all(t, "");
                if let Some(labels) = badge_labels(&plain) {
                    if reviewer_severity.is_none() {
                        reviewer_severity = labels.into_iter().find(|l| is_severity(l));
                    }
                    continue;
                }
                let bare = t.trim_start_matches("**").trim_end_matches("**").trim();
                title = Some(re(&TAG, r"<[^>]+>").replace_all(bare, "").trim().to_owned());
                continue;
            }
            text.push(t.to_owned());
        }
        let text = text.join(" ");
        ParsedComment {
            title: title.unwrap_or_default(),
            text: text.chars().take(1500).collect(),
            reviewer_severity,
        }
    }
}

/// Severity words reviewers put in a label line.
const SEVERITY_WORDS: &[&str] = &["critical", "major", "minor", "trivial", "nitpick"];
/// A label in a label line is a few words; a sentence is not a label.
const LABEL_MAX: usize = 40;

fn is_severity(label: &str) -> bool {
    SEVERITY_WORDS.contains(&label.to_lowercase().as_str())
}

/// The labels of a line such as `_Functional Correctness_ | _Major_ | _Quick
/// win_` or `**Major** · **Quick win**`: two or more short segments split by
/// `|` or `·`, with markup and symbols removed. `None` for any other line.
fn badge_labels(line: &str) -> Option<Vec<String>> {
    let parts: Vec<&str> = line.split(['|', '·']).collect();
    if parts.len() < 2 {
        return None;
    }
    let labels: Vec<String> = parts
        .iter()
        .map(|p| {
            p.chars()
                .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '&')
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    labels
        .iter()
        .all(|l| !l.is_empty() && l.chars().count() <= LABEL_MAX)
        .then_some(labels)
}

/// Removes every `<details>` block, nested ones included.
fn strip_details(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut depth = 0usize;
    let mut rest = body;
    while !rest.is_empty() {
        let open = rest.find("<details");
        let close = rest.find("</details>");
        match (open, close) {
            (Some(o), c) if c.is_none_or(|c| o < c) => {
                if depth == 0 {
                    out.push_str(&rest[..o]);
                }
                depth += 1;
                rest = &rest[o + "<details".len()..];
            }
            (_, Some(c)) => {
                if depth == 0 {
                    out.push_str(&rest[..c]);
                }
                depth = depth.saturating_sub(1);
                rest = &rest[c + "</details>".len()..];
            }
            _ => {
                if depth == 0 {
                    out.push_str(rest);
                }
                break;
            }
        }
    }
    out
}

/// Line comment and block comment openers of the common languages. A line
/// that starts with one is a comment for the quote's preference only, so a
/// language missing here loses that preference and nothing else.
const COMMENT_OPENERS: &[&str] = &["//", "/*", "<!--", "#", "--", ";", "\"\"\"", "'''"];
/// Openers that are a comment only when a space or the end of the line
/// follows them: `* text` in a block comment but not `*ptr = x`.
const SPACED_OPENERS: &[&str] = &["*", "%", "'", "*/"];

#[must_use]
pub fn is_comment_line(line: &str) -> bool {
    let t = line.trim_start();
    if COMMENT_OPENERS.iter().any(|o| t.starts_with(o)) {
        return true;
    }
    SPACED_OPENERS.iter().any(|o| {
        t.strip_prefix(o)
            .is_some_and(|r| r.is_empty() || r.starts_with(' ') || r.starts_with('\t'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cr() -> CommentSource {
        source("coderabbit").unwrap()
    }

    const BODY: &str = "<!-- generated:v1 -->\n\n**<picture><img src=\"x.svg\" alt=\"\"></picture> Major** · **Functional Correctness** · **Quick win**\n\n**Skip this test when the engine is below 1.3.**\n\nWith version 1.2, the import succeeds, but `widget_count` reaches `sql`.\n\n<details>\n<summary>Proposed fix</summary>\n\n```diff\n- a\n+ b\n```\n<details><summary>inner</summary>nested</details>\n</details>\n\n<!-- cr-comment:v1:abc -->\n\n✅ Addressed in commit 3a0af6f";

    #[test]
    fn a_single_commit_is_the_fix() {
        assert_eq!(cr().addressed_fix(BODY).as_deref(), Some("3a0af6f"));
    }

    #[test]
    fn the_last_commit_of_a_range_is_the_fix() {
        let body = "text\n\n✅ Addressed in commits 1a2b3c4 to 9f8e7d6c";
        assert_eq!(cr().addressed_fix(body).as_deref(), Some("9f8e7d6c"));
    }

    #[test]
    fn no_marker_or_no_commit_means_not_addressed() {
        assert_eq!(cr().addressed_fix("**Title**\n\nsome text"), None);
        assert_eq!(cr().addressed_fix("✅ Addressed in commit (pending)"), None);
    }

    #[test]
    fn a_marker_override_reads_another_wording() {
        let s = cr().with_overrides(Some("acme-bot".into()), Some("Resolved by".into()));
        assert_eq!(s.reviewer, "acme-bot");
        assert_eq!(
            s.addressed_fix("x\nResolved by 0123abc").as_deref(),
            Some("0123abc")
        );
        assert_eq!(s.addressed_fix("x\nAddressed in commit 0123abc"), None);
    }

    #[test]
    fn the_comment_is_read_into_title_text_and_severity() {
        let p = cr().parse_comment(BODY);
        assert_eq!(p.title, "Skip this test when the engine is below 1.3.");
        assert_eq!(
            p.text,
            "With version 1.2, the import succeeds, but `widget_count` reaches `sql`."
        );
        assert_eq!(p.reviewer_severity.as_deref(), Some("Major"));
    }

    #[test]
    fn label_lines_in_other_formats_give_the_severity_and_are_not_the_title() {
        for (header, sev) in [
            (
                "_🎯 Functional Correctness_ | _🟡 Minor_ | _⚡ Quick win_",
                "Minor",
            ),
            (
                "_🗄️ Data Integrity & Integration_ | _🟠 Major_ | _⚡ Quick win_",
                "Major",
            ),
            (
                "**🎯 Functional Correctness** | **🔴 Critical** | **⚡ Quick win**",
                "Critical",
            ),
        ] {
            let body = format!("<!-- x -->\n\n{header}\n\n**Gate the upload for pull-request runs.**\n\nThe job reaches the publish step.\n\n✅ Addressed in commit 1a2b3c4");
            let p = cr().parse_comment(&body);
            assert_eq!(
                p.title, "Gate the upload for pull-request runs.",
                "{header}"
            );
            assert_eq!(p.text, "The job reaches the publish step.");
            assert_eq!(p.reviewer_severity.as_deref(), Some(sev));
        }
    }

    #[test]
    fn a_sentence_with_a_pipe_is_not_a_label_line() {
        let p = cr().parse_comment(
            "**Use `a | b` here instead of chaining two filters on the frame.**\n\ntext",
        );
        assert_eq!(
            p.title,
            "Use `a | b` here instead of chaining two filters on the frame."
        );
    }

    #[test]
    fn a_plain_comment_takes_its_first_line_as_title() {
        let p = cr().parse_comment("The loop never ends\nwhen the list is empty.");
        assert_eq!(p.title, "The loop never ends");
        assert_eq!(p.text, "when the list is empty.");
        assert_eq!(p.reviewer_severity, None);
    }

    #[test]
    fn comment_lines_are_recognised_across_languages() {
        for l in [
            "# python or shell",
            "  // go, rust, ts",
            "/* c block */",
            " * inside a block",
            "-- sql",
            "<!-- html -->",
            "; lisp",
            "% erlang",
            "' basic",
            "\"\"\" a docstring",
        ] {
            assert!(is_comment_line(l), "{l}");
        }
        for l in [
            "*ptr = value",
            "x := widgets.Count()",
            "return a * b",
            "'a' == c",
            "%w",
            "def count(self):",
        ] {
            assert!(!is_comment_line(l), "{l}");
        }
    }

    #[test]
    fn unknown_sources_are_none() {
        assert!(source("nobody").is_none());
        assert_eq!(names(), vec!["coderabbit"]);
    }
}
