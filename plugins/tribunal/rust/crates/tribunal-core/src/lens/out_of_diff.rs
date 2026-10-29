//! Port of the OUT-OF-DIFF region: the pre-panel gate that drops a candidate
//! filed outside the PR, unless its own evidence ties it to a changed file, in
//! which case it is re-anchored there.
//!
//! A candidate's `file` is a string or absent: FINDINGS_SCHEMA and
//! CRITIC_SCHEMA declare it a string and the tool-call layer enforces that,
//! and bot findings build it from strings. A truthy non-string `file` (and a
//! non-string `changed` file from the blast-radius relay, which writes paths)
//! can reach `.endsWith` and throw in JavaScript; it is not modelled.

use serde_json::{json, Value};

use super::dependents_block::{changed_owners, owner_of};
use super::js_ext::js_key_order;
use crate::js;

/// Ports `fileInDiff`. An empty or absent file is kept.
pub fn file_in_diff(file: Option<&str>, files: &[String]) -> bool {
    let Some(file) = file.filter(|f| !f.is_empty()) else {
        return true;
    };
    files
        .iter()
        .any(|f| f == file || f.ends_with(file) || file.ends_with(f.as_str()) || f.contains(file))
}

fn string_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    match js::get(v, key) {
        Some(Value::String(s)) => Some(s),
        _ => None,
    }
}

/// `{ ...c, file, line, reanchored }`: existing keys keep their position,
/// new ones are appended, as object spread does.
fn moved(c: &Value, file: Value, line: Value, reanchored: Value) -> Value {
    let mut out = c.as_object().cloned().unwrap_or_default();
    out.insert("file".into(), file);
    out.insert("line".into(), line);
    out.insert("reanchored".into(), reanchored);
    Value::Object(out)
}

/// Ports `reanchorOutOfDiff`: `Some(c)` unchanged when it is in the diff, a
/// moved copy when its evidence ties it to a changed file, `None` to drop it.
/// `br` is the blast-radius relay's data. A null `changed` entry throws in
/// JavaScript, and is an `Err` with the same message here.
///
/// The candidate comes back in JavaScript's key order (see
/// `js_ext::js_key_order`), as it would from the parse and spread there.
pub fn reanchor_out_of_diff(
    c: &Value,
    files: &[String],
    br: Option<&Value>,
) -> Result<Option<Value>, String> {
    Ok(reanchor(c, files, br)?.map(|v| js_key_order(&v)))
}

fn reanchor(c: &Value, files: &[String], br: Option<&Value>) -> Result<Option<Value>, String> {
    let file = string_at(c, "file");
    if file_in_diff(file, files) {
        return Ok(Some(c.clone()));
    }
    let file = file.unwrap_or_default();
    let line = match js::get(c, "line") {
        None | Some(Value::Null) => Value::Null,
        Some(l) => l.clone(),
    };
    let from = json!({"file": file, "line": line});
    let anchors = match js::get(c, "verifiedBy") {
        Some(Value::Array(a)) => a.as_slice(),
        _ => &[],
    };
    let anchor = anchors.iter().find(|a| {
        js::truthy(Some(a))
            && string_at(a, "file").is_some_and(|f| !f.is_empty() && file_in_diff(Some(f), files))
    });
    if let Some(a) = anchor {
        let line = if js::is_integer(js::get(a, "line")) {
            js::get(a, "line").cloned().unwrap_or(Value::Null)
        } else {
            json!(0)
        };
        return Ok(Some(moved(
            c,
            json!(string_at(a, "file")),
            line,
            json!({"from": from, "via": "verifiedBy"}),
        )));
    }
    let Some(br) = br.filter(|b| js::truthy(Some(b))) else {
        return Ok(None);
    };
    let Some(Value::Array(callers)) = js::get(br, "callers") else {
        return Ok(None);
    };
    let owner = changed_owners(br)?;
    let same = |a: &str, b: &str| a == b || a.ends_with(b) || b.ends_with(a);
    let hit = callers
        .iter()
        .find(|x| js::truthy(Some(x)) && string_at(x, "file").is_some_and(|xf| same(xf, file)));
    let changed = hit.and_then(|h| owner_of(&owner, js::get(h, "via")));
    match changed {
        Some(Value::String(changed))
            if !changed.is_empty() && file_in_diff(Some(changed), files) =>
        {
            Ok(Some(moved(
                c,
                json!(changed),
                json!(0),
                json!({"from": from, "via": "dependents"}),
            )))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> Vec<String> {
        vec![
            "src/helpers/client.ts".into(),
            "src/helpers/client.test.ts".into(),
        ]
    }

    #[test]
    fn an_in_diff_anchor_moves_the_candidate() {
        let c = json!({"title": "t", "file": "src/services/accounts.ts", "line": 344,
            "verifiedBy": [{"file": "src/helpers/client.ts", "line": 80, "quote": "q"}]});
        let r = reanchor_out_of_diff(&c, &files(), None).unwrap().unwrap();
        assert_eq!(r["file"], "src/helpers/client.ts");
        assert_eq!(r["line"], 80);
        assert_eq!(
            r["reanchored"],
            json!({"from": {"file": "src/services/accounts.ts", "line": 344}, "via": "verifiedBy"})
        );
        let keys: Vec<&str> = r.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["title", "file", "line", "verifiedBy", "reanchored"]);
    }

    #[test]
    fn a_dependents_entry_moves_it_to_the_changed_declaration() {
        let br = json!({"changed": [{"file": "src/helpers/client.ts", "symbols": ["isNotFound"]}],
            "callers": [{"file": "src/services/accounts.ts", "via": "isNotFound"}]});
        let c = json!({"file": "src/services/accounts.ts"});
        let r = reanchor_out_of_diff(&c, &files(), Some(&br))
            .unwrap()
            .unwrap();
        assert_eq!(r["reanchored"]["via"], "dependents");
        assert_eq!(r["reanchored"]["from"]["line"], Value::Null);
        assert!(
            reanchor_out_of_diff(&json!({"file": "src/other.ts"}), &files(), Some(&br))
                .unwrap()
                .is_none()
        );
    }
}
