//! Port of the VOTE-QUOTES region: which REFUTED votes have their
//! contradicting quote looked up at the head commit, and how the lookup's
//! answers become `quoteFound`. A quote not in the file sets the vote aside; a
//! lookup that failed as a whole marks nothing.

use serde::Serialize;
use serde_json::Value;

use super::js_ext;
use crate::js;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuoteAnchor {
    pub file: Value,
    pub line: Value,
    pub quote: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CheckableQuote {
    pub s: usize,
    pub i: usize,
    pub anchor: QuoteAnchor,
}

/// Ports `checkableVoteQuotes`. `states` is the harness's `panelStates`, an
/// array or nothing; each state's `votes` is the array the panel produced, so
/// a truthy non-array `votes` (which JavaScript would throw on) cannot occur.
/// Votes and their `contradiction` are model output and read as JSON.
pub fn checkable_vote_quotes(states: Option<&[Value]>) -> Vec<CheckableQuote> {
    let mut out = Vec::new();
    for (s, st) in states.unwrap_or_default().iter().enumerate() {
        let skipped = js::truthy(js::get(st, "skipped"));
        let votes: &[Value] = match js::get(st, "votes") {
            Some(Value::Array(v)) if js::truthy(Some(st)) && !skipped => v,
            _ => &[],
        };
        for (i, v) in votes.iter().enumerate() {
            let c = if js::truthy(Some(v)) {
                js::get(v, "contradiction")
            } else {
                None
            };
            let quote = c.and_then(|c| js::get(c, "quote"));
            let quote_text = if js::truthy(quote) {
                js_ext::to_js_string(quote)
            } else {
                String::new()
            };
            let eligible = matches!(js::get(v, "introducedByPr"), Some(Value::String(_)))
                && js::get(v, "verdict") == Some(&Value::String("REFUTED".into()))
                && js::get(v, "refutationType") != Some(&Value::String("PRE_DECIDED".into()))
                && !js::trim(&quote_text).is_empty();
            if !eligible {
                continue;
            }
            let c = c.expect("a quote was read from it");
            let finding_file = js::get(st, "finding").and_then(|f| js::get(f, "file"));
            let file = js_ext::first_truthy(&[js::get(c, "file"), finding_file])
                .filter(|f| js::truthy(Some(f)))
                .cloned()
                .unwrap_or(Value::Null);
            let line = js::get(c, "line");
            out.push(CheckableQuote {
                s,
                i,
                anchor: QuoteAnchor {
                    file,
                    // Written back as JavaScript would: `3.0` in the input is `3`.
                    line: if js::is_integer(line) {
                        js_ext::number_value(js_ext::to_number(line))
                    } else {
                        Value::Null
                    },
                    quote: quote.cloned().unwrap_or(Value::Null),
                },
            });
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteVerdicts {
    pub found: Vec<Option<bool>>,
    pub broken_lookup: bool,
}

const QUOTE_LOOKUP_FAILURES: [&str; 2] = ["ref-not-found", "lookup-failed"];

fn status_is(r: &Value, want: &str) -> bool {
    js::get(r, "status") == Some(&Value::String(want.into()))
}

/// The array index a property key names, as `found[r.findingIndex] = ...`
/// resolves it: the key is `String(findingIndex)`, and only a canonical
/// non-negative integer string addresses an element. `"01"`, `1.5` and
/// `true` set a named property that `JSON.stringify` never writes.
fn element_index(v: Option<&Value>) -> Option<usize> {
    let key = js_ext::to_js_string(v);
    let canonical = key == "0"
        || (!key.starts_with('0') && !key.is_empty() && key.bytes().all(|b| b.is_ascii_digit()));
    if canonical {
        key.parse().ok()
    } else {
        None
    }
}

/// Ports `voteQuoteVerdicts`. `results` is the resolver's relayed `results`
/// array; `count` is the number of quotes sent. A null result throws in
/// JavaScript as soon as `.some` reaches it, which happens unless an earlier
/// result already reported a failed lookup; that error comes back as `Err`.
pub fn vote_quote_verdicts(results: Option<&Value>, count: usize) -> Result<QuoteVerdicts, String> {
    let list: &[Value] = match results {
        Some(Value::Array(a)) => a,
        _ => &[],
    };
    let mut some_failed = false;
    for r in list {
        if r.is_null() {
            return Err("Cannot read properties of null (reading 'status')".to_owned());
        }
        if QUOTE_LOOKUP_FAILURES.iter().any(|f| status_is(r, f)) {
            some_failed = true;
            break;
        }
    }
    let broken_lookup =
        some_failed || (list.len() > 1 && list.iter().all(|r| status_is(r, "file-not-at-ref")));
    let mut found = vec![None; count];
    if !broken_lookup {
        for r in list {
            let fi = js::get(r, "findingIndex");
            let n = js_ext::to_number(fi);
            if !(n >= 0.0 && n < count as f64) {
                continue;
            }
            let status = js_ext::to_js_string(js::get(r, "status"));
            let mark = if status.starts_with("found") {
                Some(true)
            } else if status == "not-found" || status == "file-not-at-ref" {
                Some(false)
            } else {
                None
            };
            if let (Some(mark), Some(idx)) = (mark, element_index(fi)) {
                if idx < count {
                    found[idx] = Some(mark);
                }
            }
        }
    }
    Ok(QuoteVerdicts {
        found,
        broken_lookup,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn refuted() -> Value {
        json!({"verdict": "REFUTED", "introducedByPr": "yes", "contradiction": {"file": "src/a.ts", "line": 10, "quote": "if (!x) return;"}})
    }

    #[test]
    fn only_checkable_refutations_are_picked() {
        let states = [
            json!({"finding": {"file": "src/a.ts"}, "votes": [refuted(), {"verdict": "NOT_REFUTED", "introducedByPr": "yes"}, null]}),
            json!({"finding": {"file": "src/c.ts"}, "skipped": true, "votes": [refuted()]}),
            json!({"finding": {"file": "src/d.ts"}, "votes": [{"verdict": "REFUTED", "introducedByPr": "yes", "contradiction": {"quote": "await db.save(row);"}}]}),
        ];
        let picked = checkable_vote_quotes(Some(&states));
        assert_eq!(picked.len(), 2);
        assert_eq!((picked[1].s, picked[1].i), (2, 0));
        assert_eq!(picked[1].anchor.file, json!("src/d.ts"));
        assert_eq!(picked[1].anchor.line, Value::Null);
        assert!(checkable_vote_quotes(None).is_empty());
    }

    #[test]
    fn lookup_results_become_found() {
        let v = vote_quote_verdicts(
            Some(&json!([
                {"findingIndex": 0, "status": "found"},
                {"findingIndex": 1, "status": "found-shifted"},
                {"findingIndex": 2, "status": "not-found"},
                {"findingIndex": 3, "status": "file-not-at-ref"},
                {"findingIndex": 4, "status": "malformed"},
                {"findingIndex": 9, "status": "not-found"},
            ])),
            5,
        )
        .unwrap();
        assert_eq!(
            v.found,
            [Some(true), Some(true), Some(false), Some(false), None]
        );
        assert!(!v.broken_lookup);
    }

    #[test]
    fn a_failed_lookup_marks_nothing() {
        let v = vote_quote_verdicts(
            Some(&json!([{"findingIndex": 0, "status": "not-found"}, {"findingIndex": 1, "status": "lookup-failed"}])),
            2,
        )
        .unwrap();
        assert!(v.broken_lookup && v.found.iter().all(Option::is_none));
    }

    #[test]
    fn index_keys_follow_property_key_rules() {
        assert_eq!(element_index(Some(&json!("1"))), Some(1));
        assert_eq!(element_index(Some(&json!([2]))), Some(2));
        assert_eq!(element_index(Some(&json!("01"))), None);
        assert_eq!(element_index(Some(&json!(1.5))), None);
    }
}
