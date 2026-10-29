//! Port of the DEPENDENTS-BLOCK region: the `<dependents>` and `<attention>`
//! blocks every lens prompt carries, built from the blast-radius relay. A
//! model reads them, so the text must match byte for byte.
//!
//! The relay is the output of lib/blast-radius.mjs. Its `changed`, `symbols`,
//! `callers` and `facts` are arrays (or absent); a truthy value there that is
//! not iterable throws in JavaScript with a message V8 words by call site, and
//! that class is not modelled: such a value is read as empty. A null entry in
//! one of those arrays throws a fixed message, which is returned as `Err`.

use serde_json::Value;

use super::js_ext::to_js_string;
use crate::js;

const ATTENTION_QUESTIONS: [(&str, &str); 11] = [
    ("exit-before-finally", "does the finally read anything this exit leaves unset: a file to clean up, a lock, a list it iterates?"),
    ("unawaited-enqueue", "if the queue rejects, does the request still answer success, and is the job lost?"),
    ("sentinel-from-catch", "can the caller tell the error from the empty answer, and does it treat each one correctly?"),
    ("new-emit", "does every consumer of this event find the fields it keys on in this payload?"),
    ("payload-parity", "does a consumer of this event need the missing key to act on it, and does it drop the event without it?"),
    ("writer-parity", "do the readers of this model find this row, and is leaving those fields empty intended?"),
    ("job-without-handler", "will a worker process this job, or does it complete as unknown and get lost?"),
    ("status-order", "which status does a request that fails both checks get, and is that the one the client acts on?"),
    ("test-without-assertion", "can this test fail for the regression it names?"),
    ("mocks-subject", "does the test exercise the real code, or only the mock?"),
    ("contract-change", "what do the unchanged callers listed in <dependents> do with the new behaviour?"),
];

/// `ATTENTION_QUESTIONS[f.kind] || "is this line correct?"`. The lookup is on
/// a plain object, so a kind named after an `Object.prototype` member finds
/// that member and prints it.
fn attention_question(kind: Option<&Value>) -> String {
    let key = to_js_string(kind);
    if let Some((_, q)) = ATTENTION_QUESTIONS.iter().find(|(k, _)| *k == key) {
        return (*q).to_owned();
    }
    match key.as_str() {
        "constructor" => "function Object() { [native code] }".to_owned(),
        "__proto__" => "[object Object]".to_owned(),
        "__defineGetter__"
        | "__defineSetter__"
        | "hasOwnProperty"
        | "__lookupGetter__"
        | "__lookupSetter__"
        | "isPrototypeOf"
        | "propertyIsEnumerable"
        | "toString"
        | "valueOf"
        | "toLocaleString" => format!("function {key}() {{ [native code] }}"),
        _ => "is this line correct?".to_owned(),
    }
}

fn null_read(prop: &str) -> String {
    format!("Cannot read properties of null (reading '{prop}')")
}

/// Iteration of `x || []` over a relay value: an array, or a string by code
/// point. Anything else is read as empty (see the module note).
fn iterate(v: Option<&Value>) -> Vec<Value> {
    match v {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::String(s)) => s.chars().map(|c| Value::String(c.to_string())).collect(),
        _ => Vec::new(),
    }
}

/// The `owner` map both this region and OUT-OF-DIFF build: each changed
/// symbol to the file that changed it, first one wins. Only string keys can
/// be found by the string lookups that follow, so only those are kept.
pub(crate) fn changed_owners(br: &Value) -> Result<Vec<(String, Value)>, String> {
    let mut owner: Vec<(String, Value)> = Vec::new();
    for c in iterate(js::get(br, "changed")) {
        if c.is_null() {
            return Err(null_read("symbols"));
        }
        for sym in iterate(js::get(&c, "symbols")) {
            if let Value::String(sym) = sym {
                if !owner.iter().any(|(k, _)| *k == sym) {
                    // An absent file is undefined in JavaScript; both readers
                    // only test it for truthiness first, as they do null.
                    let file = js::get(&c, "file").cloned().unwrap_or(Value::Null);
                    owner.push((sym, file));
                }
            }
        }
    }
    Ok(owner)
}

/// `owner.get(String(via || "").split(" -> ")[0])`.
pub(crate) fn owner_of<'a>(owner: &'a [(String, Value)], via: Option<&Value>) -> Option<&'a Value> {
    let via = if js::truthy(via) {
        to_js_string(via)
    } else {
        String::new()
    };
    let root = via.split(" -> ").next().unwrap_or("");
    owner.iter().find(|(k, _)| k == root).map(|(_, f)| f)
}

/// Ports `attentionBlock`. `at` is `br && br.attention`.
pub fn attention_block(at: Option<&Value>) -> Result<String, String> {
    let Some(at) = at.filter(|a| js::truthy(Some(a))) else {
        return Ok(String::new());
    };
    let facts = match js::get(at, "facts") {
        Some(Value::Array(f)) => f.as_slice(),
        _ => &[],
    };
    if facts.is_empty() {
        return Ok(String::new());
    }
    let mut rows = Vec::with_capacity(facts.len());
    for f in facts {
        if f.is_null() {
            return Err(null_read("file"));
        }
        let s = |k: &str| to_js_string(js::get(f, k));
        rows.push(format!(
            "- {}:{} [{}] {}. Question: {}",
            s("file"),
            s("line"),
            s("kind"),
            s("detail"),
            attention_question(js::get(f, "kind"))
        ));
    }
    let shown = if js::truthy(js::get(at, "truncated")) {
        format!(
            " This shows the first {} of {}.",
            facts.len(),
            to_js_string(js::get(at, "total"))
        )
    } else {
        String::new()
    };
    Ok(format!(
        "<attention>\nLines this PR adds that match a defect class reviews of this kind have missed before, found by parsing the code at the pinned head.{shown} Each is a question to settle by reading, not a finding: report one only if the reading shows the defect, with the usual severity and evidence.\n{}\n</attention>",
        rows.join("\n")
    ))
}

/// Ports `dependentsBlock`. `br` is the blast-radius relay's data, or `None`
/// when the relay did not verify.
pub fn dependents_block(br: Option<&Value>) -> Result<String, String> {
    let br = br.filter(|b| js::truthy(Some(b)));
    let attention = attention_block(br.and_then(|b| js::get(b, "attention")))?;
    let Some(br) = br else {
        return Ok(attention);
    };
    let callers = match js::get(br, "callers") {
        Some(Value::Array(c)) if !c.is_empty() => c.as_slice(),
        _ => return Ok(attention),
    };
    let owner = changed_owners(br)?;
    let mut rows = Vec::with_capacity(callers.len());
    for c in callers {
        if c.is_null() {
            return Err(null_read("via"));
        }
        let get = |k: &str| js::get(c, k);
        let try_note = match get("inTry") {
            Some(Value::Bool(true)) => ", inside a try",
            Some(Value::Bool(false)) => ", no try around it",
            _ => "",
        };
        let kind = if !js::truthy(get("call")) {
            "reference".to_owned()
        } else if js::truthy(get("awaited")) {
            format!("call, awaited or returned{try_note}")
        } else {
            format!("call, NOT awaited{try_note}")
        };
        let from = match owner_of(&owner, get("via")) {
            Some(f) if js::truthy(Some(f)) => format!(" (changed in {})", to_js_string(Some(f))),
            _ => String::new(),
        };
        rows.push(format!(
            "- {}:{} [{kind}] via {}{from}: `{}`",
            to_js_string(get("file")),
            to_js_string(get("line")),
            to_js_string(get("via")),
            to_js_string(get("text"))
        ));
    }
    let shown = if js::truthy(js::get(br, "truncated")) {
        format!(
            "the first {} of {}",
            callers.len(),
            to_js_string(js::get(br, "total"))
        )
    } else {
        format!("all {}", callers.len())
    };
    let tail = if attention.is_empty() {
        String::new()
    } else {
        format!("\n\n{attention}")
    };
    Ok(format!(
        "<dependents>\nUnchanged code OUTSIDE this PR that uses what this PR changed, found by grep at the pinned head. It is a heuristic list: it can miss a caller and it includes harmless references. Calls nobody awaits come first, then awaited ones, then plain references. This shows {shown}.\n{}\n\nFor an entry relevant to your focus, read the caller and ask whether what the diff changed breaks what that caller assumes: a client that now fails fast instead of buffering, a new timeout, a different return or throw shape, a changed connection. When your pass below is restricted to some files, take only the entries whose \"changed in\" file is on that list. Reading these callers is sanctioned even though they sit in other files. Report a break anchored at the changed declaration in the PR, never at the caller's file, because a finding filed on a file outside this PR is dropped before review. Put the caller's line in verifiedBy.\n</dependents>{tail}",
        rows.join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_caller_names_the_changed_file() {
        let br = json!({
            "changed": [{"file": "src/jobs/queue.ts", "symbols": ["webhookQueue"]}],
            "callers": [{"file": "src/controllers/webhook.ts", "line": 4, "via": "webhookQueue -> addWebhookJob",
                "call": true, "awaited": false, "text": "addWebhookJob(req.body);"}],
        });
        let text = dependents_block(Some(&br)).unwrap();
        assert!(text.contains("- src/controllers/webhook.ts:4 [call, NOT awaited] via webhookQueue -> addWebhookJob (changed in src/jobs/queue.ts): `addWebhookJob(req.body);`"));
        assert!(text.contains("This shows all 1."));
    }

    #[test]
    fn nothing_to_list_is_no_block() {
        assert_eq!(dependents_block(None).unwrap(), "");
        assert_eq!(
            dependents_block(Some(&json!({"changed": [], "callers": [], "total": 0}))).unwrap(),
            ""
        );
    }

    #[test]
    fn a_prototype_kind_prints_the_member() {
        let at = json!({"facts": [{"kind": "toString"}]});
        assert!(attention_block(Some(&at))
            .unwrap()
            .contains("Question: function toString() { [native code] }"));
    }
}
