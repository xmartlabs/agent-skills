//! Port of `lib/classify-head-failure.sh`: why an external head exited
//! non-zero, read from the lines its CLI marks as errors. The matching
//! classes are the script's BSD grep under en_US.UTF-8, as in
//! [`crate::classify_verdict`].

use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use crate::classify_verdict::{ci, cut_chars, re, subst_lines, Failclass, SPACE};

const NO_ERROR_LINE: &str = "no error line in the output";

/// What the script prints. `class` is `Quota`, `Auth` or `Error`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadFailure {
    pub class: Failclass,
    pub reason: String,
}

impl HeadFailure {
    /// The script's stdout, byte for byte.
    pub fn to_lines(&self) -> String {
        format!(
            "FAILCLASS: {}\nREASON: {}\n",
            self.class.as_str(),
            self.reason
        )
    }

    /// The line HEAD-FAILURE writes first into the head's artifact, which
    /// `classify-verdict.sh` reads back.
    pub fn artifact_line(&self) -> String {
        format!("HEAD FAILURE ({}): {}", self.class.as_str(), self.reason)
    }
}

struct Patterns {
    error_line: Regex,
    leading_space: Regex,
    quota: Regex,
    auth: Regex,
}

fn patterns() -> &'static Patterns {
    static CELL: OnceLock<Patterns> = OnceLock::new();
    CELL.get_or_init(|| {
        let s = format!("[{SPACE}]");
        Patterns {
            error_line: re(&format!(
                r"^{s}*(?:\[API Error|ERROR|Error|error)[\]:{SPACE}]"
            )),
            leading_space: re(&format!("^{s}+")),
            quota: re(&format!(
                "{}|{}[_ ]{}|{}|{}|{}|{}",
                ci("out of credits"),
                ci("insufficient"),
                ci("quota"),
                ci("exceeded your current quota"),
                ci("billing"),
                ci("credit balance"),
                ci("quota exceeded")
            )),
            auth: re(&format!(
                "(?:^|[^0-9])40[13](?:[^0-9]|$)|{}[sSzZ]{}|{}|{} ?{}|{}.?(?:{}.?)?{}|{}",
                ci("unauthori"),
                ci("ed"),
                ci("not logged in"),
                ci("log"),
                ci("in"),
                ci("invalid"),
                ci("api"),
                ci("key"),
                ci("authentication")
            )),
        }
    })
}

/// Classifies the text of a failed head's stderr or output.
pub fn classify_head_failure(text: &str) -> HeadFailure {
    let p = patterns();
    let errors: Vec<&str> = subst_lines(text)
        .into_iter()
        .filter(|l| p.error_line.is_match(l))
        .collect();
    let first = errors
        .first()
        .map(|l| cut_chars(&p.leading_space.replace(l, ""), 200))
        .unwrap_or_default();
    let class = if errors.iter().any(|l| p.quota.is_match(l)) {
        Failclass::Quota
    } else if errors.iter().any(|l| p.auth.is_match(l)) {
        Failclass::Auth
    } else {
        Failclass::Error
    };
    HeadFailure {
        class,
        reason: if first.is_empty() {
            NO_ERROR_LINE.to_owned()
        } else {
            first
        },
    }
}

/// The script's entry: `classify-head-failure.sh [file]`. A missing path, or
/// one that is not a regular file, reads as no output at all.
pub fn run(file: Option<&str>) -> HeadFailure {
    let text = file
        .filter(|f| !f.is_empty() && Path::new(f).is_file())
        .and_then(|f| std::fs::read(f).ok())
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    classify_head_failure(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_error_lines_are_read() {
        let hf = classify_head_failure(
            "prompt about billing\n  ERROR: Your workspace is out of credits\n",
        );
        assert_eq!(
            hf.to_lines(),
            "FAILCLASS: quota\nREASON: ERROR: Your workspace is out of credits\n"
        );
        let hf = classify_head_failure("billing 401\nERROR: stream closed\n");
        assert_eq!(hf.class, Failclass::Error);
        assert_eq!(
            classify_head_failure("[API Error: 401]").class,
            Failclass::Auth
        );
        assert_eq!(
            classify_head_failure("").reason,
            "no error line in the output"
        );
    }
}
