//! Port of `lib/classify-verdict.sh`: whether an external head's saved answer
//! cast a verdict, and the confidence and reason it gave.
//!
//! The script's matching is BSD grep, sed and awk under en_US.UTF-8 on macOS,
//! and the classes below are what those tools do there, measured code point by
//! code point: `[[:space:]]` is the Unicode space set in grep and sed but only
//! ASCII plus U+00A0 in awk, `-i` folds a literal letter by its ASCII pair
//! alone, a bracket under `-i` also admits a character whose `towlower` or
//! `towupper` is in it (U+0130, U+0131, U+017F and U+212A for `[a-zA-Z]`),
//! awk's `tolower` maps U+0130 and U+212A to ASCII, and `cut -c` counts
//! characters. Lines split on `\n` only; a `\r` stays part of its line.

use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

pub(crate) const SPACE: &str =
    r"\t\x0B\x0C\r \u{A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}";
const AWK_SPACE: &str = r"\t\x0B\x0C\r \u{A0}";
const SEP: &str = r"[:：—–\-]";
const NOT_ALPHA_CI: &str = r"[^a-zA-Z\u{130}\u{131}\u{17F}\u{212A}]";
const NO_REASON: &str = "(no reason line — see the raw output)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failclass {
    None,
    Unparseable,
    Refused,
    Confined,
    Harness,
    Quota,
    Auth,
    Error,
}

impl Failclass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Unparseable => "unparseable",
            Self::Refused => "refused",
            Self::Confined => "confined",
            Self::Harness => "harness",
            Self::Quota => "quota",
            Self::Auth => "auth",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Refuted,
    NotRefuted,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Refuted => "REFUTED",
            Self::NotRefuted => "NOT_REFUTED",
        }
    }
}

/// What the script prints. `confidence` is the printed text, which can be a
/// string no number parses (`0.5.5`); `None` fields are lines it omits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    pub failclass: Failclass,
    pub verdict: Option<Verdict>,
    pub confidence: Option<String>,
    pub confidence_declared: Option<bool>,
    pub reason: Option<String>,
}

impl Classification {
    fn only(failclass: Failclass) -> Self {
        Self {
            failclass,
            verdict: None,
            confidence: None,
            confidence_declared: None,
            reason: None,
        }
    }

    /// The script's stdout, byte for byte.
    pub fn to_lines(&self) -> String {
        let mut out = format!("FAILCLASS: {}\n", self.failclass.as_str());
        if let Some(v) = self.verdict {
            out.push_str(&format!("VERDICT: {}\n", v.as_str()));
        }
        if let Some(c) = &self.confidence {
            out.push_str(&format!("CONFIDENCE: {c}\n"));
        }
        if let Some(d) = self.confidence_declared {
            out.push_str(&format!(
                "CONFIDENCE_DECLARED: {}\n",
                if d { "yes" } else { "no" }
            ));
        }
        if let Some(r) = &self.reason {
            out.push_str(&format!("REASON: {r}\n"));
        }
        out
    }
}

/// A literal under `grep -i` / `sed …/I`: ASCII letters and `ó` match their
/// case pair and nothing else.
pub(crate) fn ci(word: &str) -> String {
    word.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' => {
                format!("[{}{}]", c.to_ascii_lowercase(), c.to_ascii_uppercase())
            }
            'ó' | 'Ó' => "[óÓ]".to_owned(),
            _ => regex::escape(&c.to_string()),
        })
        .collect()
}

pub(crate) fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a fixed pattern compiles")
}

/// `cut -c1-<n>` of one line.
pub(crate) fn cut_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `printf '%s\n' "$(…)"` over text: trailing newlines dropped, split on `\n`.
pub(crate) fn subst_lines(text: &str) -> Vec<&str> {
    text.trim_end_matches('\n').split('\n').collect()
}

struct Patterns {
    harness: Regex,
    head_failure: Regex,
    star: Regex,
    heading: Regex,
    bullet: Regex,
    confined: Regex,
    labelled: Regex,
    bare: Regex,
    not_refuted: Regex,
    conf_line: Regex,
    conf_head: Regex,
    label_prefix: Regex,
    token_prefix: Regex,
    parenthetical: Regex,
    awk_meta: Regex,
    awk_heading: Regex,
    reason_label: Regex,
}

fn patterns() -> &'static Patterns {
    static CELL: OnceLock<Patterns> = OnceLock::new();
    CELL.get_or_init(|| {
        let s = format!("[{SPACE}]");
        let tok_sep = format!("[{SPACE}._\\-]");
        let label = format!("(?:{}|{})", ci("verdict"), ci("veredicto"));
        let token = format!(
            "(?:{}{tok_sep}?{}|{}|{})",
            ci("not"),
            ci("refuted"),
            ci("refuted"),
            ci("confirmed")
        );
        let end = format!("(?:{NOT_ALPHA_CI}|$)");
        let a = format!("[{AWK_SPACE}]");
        Patterns {
            harness: re("PROMPT TRUNCATED IN TRANSIT|^HARNESS FAULT:"),
            head_failure: re(r"^HEAD FAILURE \((quota|auth|error)\): "),
            star: re(r"[*`_]"),
            heading: re(&format!("^{s}*#{{1,6}}{s}*")),
            bullet: re(&format!(r"^{s}*[\-+>]{s}*")),
            confined: re(&format!(
                "^{s}*{label}?{s}*{SEP}?{s}*(?:{}{tok_sep}?{}|{}){end}",
                ci("cannot"),
                ci("assess"),
                ci("abstain")
            )),
            labelled: re(&format!("^{s}*{label}{s}*{SEP}?{s}*{token}{end}")),
            bare: re(&format!("^{s}*{token}{end}")),
            not_refuted: re(&format!(
                "{}{tok_sep}?{}|{}",
                ci("not"),
                ci("refuted"),
                ci("confirmed")
            )),
            conf_line: re(&format!(
                "^{s}*(?:{}|{}){s}*{SEP}?{s}*[0-9]",
                ci("confidence"),
                ci("confianza")
            )),
            conf_head: re(&format!("^.*{SEP}{s}*")),
            label_prefix: re(&format!("^{s}*{label}{s}*{SEP}?{s}*")),
            token_prefix: re(&format!("^{token}{s}*[.:—–\\-]*{s}*")),
            parenthetical: re(&format!(r"^\(.*\){s}*$")),
            awk_meta: re(&format!(
                "^{a}*(?:confidence|confianza|refutationtype|decisionquestioned|killreason|coresstands|corestands|source|slotstatus){a}*:"
            )),
            awk_heading: re(&format!("^{a}*[A-Za-z][A-Za-z ]{{0,30}}:{a}*$")),
            reason_label: re(&format!(
                "^{s}*(?:{}|{}|{}){s}*[:：]{s}*",
                ci("reason"),
                ci("razón"),
                ci("razon")
            )),
        }
    })
}

/// awk's `tolower` as far as an ASCII pattern can tell.
fn awk_lower(line: &str) -> String {
    line.chars()
        .map(|c| match c {
            '\u{130}' => 'i',
            '\u{212A}' => 'k',
            _ => c.to_ascii_lowercase(),
        })
        .collect()
}

/// awk's `!NF` under the default field separator.
fn awk_blank(line: &str) -> bool {
    line.chars().all(|c| c == ' ' || c == '\t')
}

/// The sed pass that strips markdown decoration, as the lines the script's
/// greps and awk read.
fn normalize(text: &str) -> Vec<String> {
    let p = patterns();
    let body = text.strip_suffix('\n').unwrap_or(text);
    let mut lines: Vec<String> = if text.is_empty() {
        Vec::new()
    } else {
        body.split('\n')
            .map(|line| {
                let line = p.star.replace_all(line, "");
                let line = p.heading.replace(&line, "");
                p.bullet.replace(&line, "").into_owned()
            })
            .collect()
    };
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn confidence(norm: &[String]) -> (String, bool) {
    let p = patterns();
    let Some(line) = norm.iter().rev().find(|l| p.conf_line.is_match(l)) else {
        return ("0.5".to_owned(), false);
    };
    let rest = p.conf_head.replace(line, "");
    let conf: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let b = conf.as_bytes();
    let declared = conf == "0"
        || conf == "1"
        || (b.len() >= 3 && b.starts_with(b"0.") && b[2].is_ascii_digit())
        || b.starts_with(b"1.0");
    if declared {
        (conf, true)
    } else {
        ("0.5".to_owned(), false)
    }
}

fn reason(norm: &[String], line: &str) -> String {
    let p = patterns();
    let r = p.label_prefix.replace(line, "");
    let r = p.token_prefix.replace(&r, "");
    let mut reason = p.parenthetical.replace(&r, "").into_owned();
    if reason.is_empty() {
        if let Some(n) = norm.iter().position(|l| l == line) {
            reason = norm[n + 1..]
                .iter()
                .find(|l| {
                    !awk_blank(l)
                        && !p.awk_meta.is_match(&awk_lower(l))
                        && !p.awk_heading.is_match(l)
                })
                .cloned()
                .unwrap_or_default();
        }
    }
    let reason = cut_chars(&p.reason_label.replace(&reason, ""), 400);
    if reason.is_empty() {
        NO_REASON.to_owned()
    } else {
        reason
    }
}

/// Classifies a file's text. `nonempty` is the script's `-s` test, which
/// reads the size on disk and so can be true of a file it then cannot read.
pub fn classify_text(text: &str, nonempty: bool) -> Classification {
    if !nonempty {
        return Classification::only(Failclass::Refused);
    }
    let p = patterns();
    let first = text.split('\n').next().unwrap_or("");
    if p.harness.is_match(first) {
        return Classification::only(Failclass::Harness);
    }
    if let Some(m) = p.head_failure.captures(first) {
        let failclass = match &m[1] {
            "quota" => Failclass::Quota,
            "auth" => Failclass::Auth,
            _ => Failclass::Error,
        };
        return Classification {
            reason: Some(cut_chars(&first[m[0].len()..], 200)),
            ..Classification::only(failclass)
        };
    }

    let norm = normalize(text);
    if norm.iter().any(|l| p.confined.is_match(l)) {
        return Classification::only(Failclass::Confined);
    }
    let line = norm
        .iter()
        .rev()
        .find(|l| p.labelled.is_match(l))
        .or_else(|| norm.iter().find(|l| p.bare.is_match(l)));
    let Some(line) = line else {
        return Classification::only(Failclass::Unparseable);
    };
    let verdict = if p.not_refuted.is_match(line) {
        Verdict::NotRefuted
    } else {
        Verdict::Refuted
    };
    let (conf, declared) = confidence(&norm);
    Classification {
        failclass: Failclass::None,
        verdict: Some(verdict),
        confidence: Some(conf),
        confidence_declared: Some(declared),
        reason: Some(reason(&norm, line)),
    }
}

/// Classifies the file at `path` as the script does: `-s` on its size, and
/// an unreadable file (or a directory) read as empty text.
pub fn classify_file(path: &Path) -> Classification {
    let nonempty = std::fs::metadata(path).is_ok_and(|m| m.len() > 0);
    let text = if nonempty {
        std::fs::read(path)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    } else {
        String::new()
    };
    classify_text(&text, nonempty)
}

/// The script's entry: `classify-verdict.sh <file>`.
///
/// # Errors
/// The usage error the script exits 1 with when `file` is empty.
pub fn run(file: &str) -> Result<Classification, String> {
    if file.is_empty() {
        return Err("usage: classify-verdict.sh <file>".to_owned());
    }
    Ok(classify_file(Path::new(file)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> String {
        classify_text(text, !text.is_empty()).to_lines()
    }

    #[test]
    fn a_labelled_verdict_wins_over_a_bare_headline() {
        assert_eq!(
            lines("REFUTED.\nwhy\n**VERDICT:** NOT REFUTED — holds\nCONFIDENCE: 0.9\n"),
            "FAILCLASS: none\nVERDICT: NOT_REFUTED\nCONFIDENCE: 0.9\nCONFIDENCE_DECLARED: yes\nREASON: holds\n"
        );
    }

    #[test]
    fn a_bare_headline_takes_the_next_line_of_argument() {
        assert_eq!(
            lines("REFUTED.\n\nCONFIDENCE: 95\nAnalysis:\n  the guard covers it\n"),
            "FAILCLASS: none\nVERDICT: REFUTED\nCONFIDENCE: 0.5\nCONFIDENCE_DECLARED: no\nREASON:   the guard covers it\n"
        );
    }

    #[test]
    fn failures_print_their_class() {
        assert_eq!(lines(""), "FAILCLASS: refused\n");
        assert_eq!(lines("x HARNESS FAULT: y"), "FAILCLASS: unparseable\n");
        assert_eq!(lines("HARNESS FAULT: y"), "FAILCLASS: harness\n");
        assert_eq!(lines("VERDICT: cannot-assess"), "FAILCLASS: confined\n");
        assert_eq!(
            lines("HEAD FAILURE (quota): out of credits\nVERDICT: REFUTED"),
            "FAILCLASS: quota\nREASON: out of credits\n"
        );
    }
}
