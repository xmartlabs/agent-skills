//! Port of the STRENGTHEN-MERGE region: applying a strengthen pass to the
//! finding it was asked to improve, only when the pass says it worked, and
//! never lowering the severity.
//!
//! Severities are schema enums; one named after an `Object.prototype` member
//! (`"toString"`) is out of scope, since `in RANK` would find it inherited.

use serde::Serialize;
use serde_json::{Map, Value};

use super::js_ext::{or, to_js_string};
use crate::js;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Strengthen {
    pub finding: Value,
    pub outcome: &'static str,
    pub reason: Value,
}

/// `RANK[v]` when `v in RANK`; the property key is `String(v)`.
fn rank(v: Option<&Value>) -> Option<u8> {
    match to_js_string(v).as_str() {
        "nit" => Some(0),
        "important" => Some(1),
        "critical" => Some(2),
        _ => None,
    }
}

/// `applyStrengthen`. `None` is `undefined`.
pub fn apply_strengthen(original: &Value, strengthened: Option<&Value>) -> Strengthen {
    // `!s || typeof s !== "object"`: only objects and arrays get through.
    let s = match strengthened {
        Some(s @ (Value::Object(_) | Value::Array(_))) => s,
        _ => {
            return Strengthen {
                finding: original.clone(),
                outcome: "dispatch-failed",
                reason: Value::from("the strengthen dispatch returned nothing"),
            }
        }
    };
    if js::get(s, "strengthened") != Some(&Value::Bool(true)) {
        return Strengthen {
            finding: original.clone(),
            outcome: "unstrengthenable",
            reason: or(
                js::get(s, "unstrengthenableReason"),
                Value::from(
                    "the pass reported it could not strengthen the finding and gave no reason",
                ),
            ),
        };
    }
    // The rest pattern copies the remaining keys in order. `Map::remove` would
    // swap the last key into the hole, so the fields are filtered instead.
    let fields: Map<String, Value> = s
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| *k != "strengthened" && *k != "unstrengthenableReason")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let proposed = fields.get("severity");
    let current = js::get(original, "severity");
    let lowered = matches!((rank(proposed), rank(current)), (Some(p), Some(c)) if p < c);
    // Object spread: a key already present keeps its position.
    let mut finding: Map<String, Value> = original.as_object().cloned().unwrap_or_default();
    for (k, v) in &fields {
        finding.insert(k.clone(), v.clone());
    }
    if lowered {
        finding.insert("severity".into(), current.cloned().unwrap_or(Value::Null));
        finding.insert(
            "refineProposedSeverity".into(),
            proposed.cloned().unwrap_or(Value::Null),
        );
    }
    Strengthen {
        finding: Value::Object(finding),
        outcome: "strengthened",
        reason: Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_lower_severity_is_recorded_not_applied() {
        let original = json!({"title": "t", "severity": "important"});
        let r = apply_strengthen(
            &original,
            Some(&json!({"strengthened": true, "severity": "nit", "repro": "r"})),
        );
        assert_eq!(
            r.finding,
            json!({"title": "t", "severity": "important", "repro": "r", "refineProposedSeverity": "nit"})
        );
    }

    #[test]
    fn a_failed_pass_leaves_the_finding_alone() {
        let original = json!({"severity": "important"});
        let r = apply_strengthen(
            &original,
            Some(&json!({"strengthened": false, "severity": "nit"})),
        );
        assert_eq!((r.finding, r.outcome), (original, "unstrengthenable"));
        assert_eq!(
            apply_strengthen(&json!({}), None).outcome,
            "dispatch-failed"
        );
    }
}
