//! Port of the REF-EVIDENCE-CHECK region: the identifiers a refutation cites,
//! and the audit verdict once they have been looked up at the reviewed commit.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Value};

use super::js_ext;
use crate::js;

pub const REF_EVIDENCE_STOPLIST: [&str; 17] = [
    "JavaScript",
    "TypeScript",
    "PostgreSQL",
    "MySQL",
    "GitHub",
    "NOT_REFUTED",
    "PRE_DECIDED",
    "TECHNICALLY_WRONG",
    "NO_REFUTATION",
    "refutationType",
    "killReason",
    "slotStatus",
    "coreStands",
    "decisionQuestioned",
    "failureReason",
    "slotId",
    "confidenceDeclared",
];

// JavaScript's `\w` and `\b` without the `u` flag are ASCII, so both patterns
// spell them that way: `é` is a word boundary, as it is in JavaScript.
fn source_path() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?-u:\b)[A-Za-z0-9_./-]+\.(?:ts|tsx|js|jsx|mts|cts|prisma)(?-u:\b)").unwrap()
    })
}

fn inner_capital() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?-u:\b)[A-Za-z][A-Za-z0-9_]*[a-z][A-Z][A-Za-z0-9_]*(?-u:\b)").unwrap()
    })
}

/// Ports `citedIdentifiers`. `text` is a vote's `reason`, read as
/// `String(text || "")`. The result is deduplicated and sorted by UTF-16
/// units, as `[...set].sort()` does.
pub fn cited_identifiers(text: Option<&Value>) -> Vec<String> {
    let s = js_ext::string_or_empty(text);
    let mut out: Vec<String> = Vec::new();
    let mut add = |m: &str| {
        if !out.iter().any(|o| o == m) {
            out.push(m.to_owned());
        }
    };
    for m in source_path().find_iter(&s) {
        add(m.as_str());
    }
    for m in inner_capital().find_iter(&s) {
        let m = m.as_str();
        if js::utf16_len(m) >= 8 && !REF_EVIDENCE_STOPLIST.contains(&m) {
            add(m);
        }
    }
    out.sort_by(|a, b| js_ext::utf16_cmp(a, b));
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct RefEvidenceVerdict {
    pub ran: bool,
    pub cited: usize,
    pub missing: Vec<Value>,
    pub suspect: Option<bool>,
}

impl RefEvidenceVerdict {
    pub fn to_value(&self) -> Value {
        json!({
            "ran": self.ran,
            "cited": self.cited,
            "missing": self.missing,
            "suspect": self.suspect,
        })
    }
}

/// Ports `refEvidenceVerdict({ ran, cited, missing })`. A check that did not
/// run reports `suspect: null`, never `false`. `missing` drops falsy entries,
/// as `.filter(Boolean)` does.
pub fn ref_evidence_verdict(
    ran: bool,
    cited: Option<&[Value]>,
    missing: Option<&[Value]>,
) -> RefEvidenceVerdict {
    let cited = cited.map_or(0, <[Value]>::len);
    if !ran {
        return RefEvidenceVerdict {
            ran: false,
            cited,
            missing: Vec::new(),
            suspect: None,
        };
    }
    let gone: Vec<Value> = missing
        .unwrap_or(&[])
        .iter()
        .filter(|v| js::truthy(Some(v)))
        .cloned()
        .collect();
    RefEvidenceVerdict {
        ran: true,
        cited,
        suspect: Some(!gone.is_empty()),
        missing: gone,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_and_files_are_cited() {
        let t = json!("passes it through `maskEmailForLog()` imported from `./csv-safety.ts`");
        assert_eq!(
            cited_identifiers(Some(&t)),
            ["csv-safety.ts", "maskEmailForLog"]
        );
    }

    #[test]
    fn harness_vocabulary_and_short_nouns_are_not() {
        let t = json!("NOT_REFUTED with refutationType NO_REFUTATION, the OpsHub call");
        assert!(cited_identifiers(Some(&t)).is_empty());
    }

    #[test]
    fn a_check_that_did_not_run_is_not_clean() {
        let v = ref_evidence_verdict(false, Some(&[json!("x")]), None);
        assert_eq!(v.suspect, None);
        let v = ref_evidence_verdict(true, Some(&[json!("a")]), Some(&[Value::Null, json!("")]));
        assert_eq!(v.suspect, Some(false));
    }
}
