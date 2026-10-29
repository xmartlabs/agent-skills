//! Port of the DEDUP-VETO region: two same-file candidates the dedup step
//! wants to merge stay merged only when they sit within 40 lines of each other
//! or their titles share enough words.

use std::collections::HashSet;

use serde_json::Value;

use super::js_ext;
use crate::js;

pub const MERGE_LINE_WINDOW: f64 = 40.0;
pub const TITLE_OVERLAP_MIN: f64 = 0.4;
const TITLE_STOP: [&str; 27] = [
    "the", "a", "an", "of", "in", "on", "to", "for", "is", "are", "and", "or", "not", "with",
    "this", "that", "its", "it", "at", "by", "from", "as", "be", "can", "when", "if", "into",
];

/// `titleTokens`: lowercased with full Unicode case mapping (U+212A becomes
/// `k`), split on anything outside `[a-z0-9_]`, words of three or more
/// characters that are not stop words.
fn title_tokens(title: Option<&Value>) -> HashSet<String> {
    let s = if js::truthy(title) {
        js_ext::to_js_string(title)
    } else {
        String::new()
    };
    s.to_lowercase()
        .split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
        .filter(|t| t.len() > 2 && !TITLE_STOP.contains(t))
        .map(str::to_owned)
        .collect()
}

/// Ports `mergeableWithin`. Both candidates are lens findings, objects by the
/// lens schema; `line` and `title` are read with JavaScript's coercions.
pub fn mergeable_within(a: &Value, b: &Value) -> bool {
    let la = js_ext::to_number(js::get(a, "line"));
    let lb = js_ext::to_number(js::get(b, "line"));
    if la.is_finite()
        && lb.is_finite()
        && la > 0.0
        && lb > 0.0
        && (la - lb).abs() <= MERGE_LINE_WINDOW
    {
        return true;
    }
    let ta = title_tokens(js::get(a, "title"));
    let tb = title_tokens(js::get(b, "title"));
    if ta.is_empty() || tb.is_empty() {
        return false;
    }
    let shared = ta.intersection(&tb).count();
    shared as f64 / ta.len().min(tb.len()) as f64 >= TITLE_OVERLAP_MIN
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn near_lines_merge() {
        assert!(mergeable_within(
            &json!({"line": 10}),
            &json!({"line": "50"})
        ));
        assert!(!mergeable_within(
            &json!({"line": 10}),
            &json!({"line": 51})
        ));
    }

    #[test]
    fn shared_title_words_merge() {
        let a = json!({"line": 1, "title": "Missing transaction around merge"});
        let b = json!({"line": 900, "title": "merge has no transaction"});
        assert!(mergeable_within(&a, &b));
    }

    #[test]
    fn kelvin_sign_folds_to_k() {
        let a = json!({"title": "\u{212A}eyset rotation"});
        let b = json!({"title": "keyset"});
        assert!(mergeable_within(&a, &b));
    }
}
