//! Port of the LENS-FOCI region: the topics each lens is told to restrict
//! itself to, from the profile's own focus sentences, and which of them the
//! changed paths force. The nested HIGH-RISK-SURFACE region is ported in
//! `high_risk_surface`.

use serde_json::{Map, Value};

use super::js_ext;
use super::profile_lookup::{RiskRule, PROFILE_REQUIRED_FOCI};
use crate::js;

/// Ports `fociFor`: `None` when the profile has no `foci`. A `null` profile
/// throws, as the property read does in JavaScript.
pub fn foci_for(profile: &Value) -> Result<Option<Value>, String> {
    if profile.is_null() {
        return Err(js_ext::null_property_error("foci"));
    }
    Ok(js::get(profile, "foci").map(js_ext::js_order))
}

/// Ports `fociLabelFor`: each focus sentence mapped back to its key. As in
/// `Object.fromEntries`, a sentence repeated under two keys keeps the first
/// position and the last key. `Object.entries` is followed for a `foci` that
/// is not an object, although `validate_profile` refuses one: a string yields
/// its characters by index, an array its elements, a number or boolean
/// nothing, and a missing or `null` one throws.
pub fn foci_label_for(profile: &Value) -> Result<Map<String, Value>, String> {
    if profile.is_null() {
        return Err(js_ext::null_property_error("foci"));
    }
    let entries: Vec<(String, String)> = match js::get(profile, "foci") {
        None | Some(Value::Null) => return Err("Cannot convert undefined or null to object".into()),
        Some(Value::Object(foci)) => js_ext::ordered_entries(foci)
            .into_iter()
            .map(|(k, v)| (k.clone(), js_ext::to_string(Some(v))))
            .collect(),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(i, v)| (i.to_string(), js_ext::to_string(Some(v))))
            .collect(),
        // Indexed by UTF-16 code unit. A character outside the BMP is two
        // entries in JavaScript, each a lone surrogate no Rust string can hold;
        // it is kept whole here, at the index of its first unit.
        Some(Value::String(text)) => {
            let mut at = 0;
            text.chars()
                .map(|c| {
                    let entry = (at.to_string(), c.to_string());
                    at += c.len_utf16();
                    entry
                })
                .collect()
        }
        Some(_) => Vec::new(),
    };
    let mut out = Map::new();
    for (key, sentence) in entries {
        out.insert(sentence, Value::String(key));
    }
    match js_ext::js_order(&Value::Object(out)) {
        Value::Object(m) => Ok(m),
        _ => unreachable!(),
    }
}

/// What `Set` and `Array.prototype.includes` compare: a primitive by value
/// (`1` and `1.0` are one value), an object by identity, which for values
/// read from one `foci` object means by the key that holds it.
#[derive(PartialEq)]
enum Identity<'a> {
    Missing,
    Null,
    Bool(bool),
    Number(f64),
    Str(&'a str),
    Object(&'a str),
}

fn identity<'a>(key: &'a str, v: Option<&'a Value>) -> Identity<'a> {
    match v {
        None => Identity::Missing,
        Some(Value::Null) => Identity::Null,
        Some(Value::Bool(b)) => Identity::Bool(*b),
        Some(Value::Number(n)) => Identity::Number(n.as_f64().unwrap_or(f64::NAN)),
        Some(Value::String(s)) => Identity::Str(s),
        Some(_) => Identity::Object(key),
    }
}

/// `Array.prototype.slice(0, end)` on a list of `len` items.
fn slice_end(end: f64, len: usize) -> usize {
    let rel = if end.is_nan() { 0.0 } else { end.trunc() };
    if rel < 0.0 {
        (len as f64 + rel).max(0.0) as usize
    } else {
        rel.min(len as f64) as usize
    }
}

/// Ports `selectFoci`. The forced foci come first, then the five defaults
/// not already forced, cut to `budget` slots but never below the forced
/// count. A default the profile lacks is a `null` slot, as `undefined` is in
/// a JSON array.
pub fn select_foci(
    files: &[String],
    budget: f64,
    foci: &Map<String, Value>,
    rules: &[RiskRule],
) -> Vec<Value> {
    let mut forced: Vec<(Identity, &Value)> = Vec::new();
    for r in rules {
        let v = foci.get(&r.focus);
        if js::truthy(v) && files.iter().any(|f| r.re.test(f)) {
            let id = identity(&r.focus, v);
            if !forced.iter().any(|(seen, _)| *seen == id) {
                forced.push((id, v.expect("truthy")));
            }
        }
    }
    let rest = PROFILE_REQUIRED_FOCI.iter().filter_map(|k| {
        let v = foci.get(*k);
        let id = identity(k, v);
        (!forced.iter().any(|(seen, _)| *seen == id))
            .then(|| v.map_or(Value::Null, js_ext::js_order))
    });
    let n = forced.len();
    let end = if budget.is_nan() {
        f64::NAN
    } else {
        budget.max(n as f64)
    };
    let all: Vec<Value> = forced
        .iter()
        .map(|(_, v)| js_ext::js_order(v))
        .chain(rest)
        .collect();
    let keep = slice_end(end, all.len());
    all.into_iter().take(keep).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::profile_lookup::risk_path_rules;
    use serde_json::json;

    fn profile() -> Value {
        json!({
            "foci": {
                "correctness": "c", "dataContract": "d", "security": "s",
                "performance": "p", "errorHandling": "e", "migration": "m"
            },
            "riskPaths": [
                {"focus": "migration", "pattern": "^prisma/migrations/"},
                {"focus": "security", "pattern": "auth"}
            ]
        })
    }

    #[test]
    fn a_risk_path_forces_its_focus_first() {
        let p = profile();
        let rules = risk_path_rules(&p).unwrap();
        let foci = p["foci"].as_object().unwrap();
        let out = select_foci(&["prisma/migrations/x.sql".into()], 5.0, foci, &rules);
        assert_eq!(
            out,
            [json!("m"), json!("c"), json!("d"), json!("s"), json!("p")]
        );
    }

    #[test]
    fn forced_foci_outrank_the_budget() {
        let p = profile();
        let rules = risk_path_rules(&p).unwrap();
        let foci = p["foci"].as_object().unwrap();
        let files = ["prisma/migrations/x.sql".into(), "src/auth.ts".into()];
        assert_eq!(
            select_foci(&files, 1.0, foci, &rules),
            [json!("m"), json!("s")]
        );
    }

    #[test]
    fn labels_map_sentences_back_to_keys() {
        let labels = foci_label_for(&profile()).unwrap();
        assert_eq!(labels["s"], json!("security"));
        assert_eq!(labels.len(), 6);
    }
}
