//! Port of the ARTIFACT-TRUTH region: an external head's vote is what its CLI
//! artifact says, not what a relay reported. No artifact, or one without a
//! verdict, is an abstention; a relayed vote the artifact contradicts is
//! recorded as a correction.
//!
//! The JavaScript collected corrections in a module-level `voteCorrections`
//! array that the metrics row reads at the end of the run. Here the caller
//! owns that list and passes it in.

use serde_json::{Map, Value};

use super::js_ext;
use crate::artifact_key::artifact_key;
use crate::js;

/// One panel slot, as the harness built it. `relayed` is `Boolean(slot.task)`:
/// a slot with its own agent task relays a vote; a slot filled by a batched
/// driver relays none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelSlot {
    pub source: Option<String>,
    pub relayed: bool,
}

/// The JavaScript `voteCorrections`: one JSON object per correction, in the
/// shape the metrics row's `voteCorrectionsDetail` stores.
pub type VoteCorrections = Vec<Value>;

fn insert_defined(m: &mut Map<String, Value>, key: &str, v: Option<&Value>) {
    if let Some(v) = v {
        m.insert(key.to_owned(), v.clone());
    }
}

/// `{...(v || {})}`. The relay schema holds a vote to an object, but a string
/// or an array spreads its indices: `{...["x"]}` is `{"0": "x"}` and `{..."ab"}`
/// is `{"0": "a", "1": "b"}`, one key per UTF-16 unit. Half of a surrogate
/// pair cannot be held in a Rust string and becomes U+FFFD.
fn spread(v: Option<&Value>) -> Map<String, Value> {
    match v {
        Some(Value::Object(m)) => m.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .enumerate()
            .map(|(i, e)| (i.to_string(), e.clone()))
            .collect(),
        Some(Value::String(s)) => s
            .encode_utf16()
            .enumerate()
            .map(|(i, u)| {
                let unit = char::from_u32(u32::from(u)).unwrap_or(char::REPLACEMENT_CHARACTER);
                (i.to_string(), Value::String(unit.to_string()))
            })
            .collect(),
        _ => Map::new(),
    }
}

/// Ports `applyArtifactTruth`. `truth` is the classifier's map of artifact
/// key to entry (`None` when classification did not run, which leaves the
/// votes untouched). `slot_id` and `round` are always set here: every panel
/// finding has a slot, and the rounds are 0 and 1.
pub fn apply_artifact_truth(
    votes: &[Value],
    panel: &[PanelSlot],
    truth: Option<&Value>,
    slot_id: u64,
    round: u64,
    corrections: &mut VoteCorrections,
) -> Vec<Value> {
    if !js::truthy(truth) {
        return votes.to_vec();
    }
    let truth = truth.expect("truthy");
    votes
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let slot = panel.get(i);
            let source = match slot.and_then(|s| s.source.as_deref()) {
                Some(s) if !s.is_empty() && s != "claude" => s,
                _ => return v.clone(),
            };
            let key = artifact_key(Some(slot_id), source, Some(round));
            let t = js::get(truth, &key);
            let relayed = slot.is_some_and(|s| s.relayed);
            let has_vote = js::truthy(Some(v));
            let vf = |k: &str| if has_vote { js::get(v, k) } else { None };
            let reported = if has_vote {
                let mut m = Map::new();
                insert_defined(&mut m, "verdict", vf("verdict"));
                insert_defined(&mut m, "confidence", vf("confidence"));
                insert_defined(&mut m, "slotStatus", vf("slotStatus"));
                Value::Object(m)
            } else {
                serde_json::json!({"verdict": null, "confidence": null, "slotStatus": "CRASHED"})
            };
            let honoured =
                has_vote && vf("slotStatus") != Some(&Value::String("UNAVAILABLE".into()));
            let correction = |kind: &str| {
                let mut m = Map::new();
                m.insert("slotId".into(), Value::from(slot_id));
                m.insert("source".into(), Value::from(source));
                m.insert("round".into(), Value::from(round));
                m.insert("kind".into(), Value::from(kind));
                m.insert("reported".into(), reported.clone());
                m
            };
            let abstain = |reason: String, failure_reason: Value| {
                let mut m = spread(Some(v));
                m.insert("source".into(), Value::from(source));
                m.insert("verdict".into(), Value::from("NOT_REFUTED"));
                m.insert("confidence".into(), Value::from(0));
                m.insert("slotStatus".into(), Value::from("UNAVAILABLE"));
                m.insert("failureReason".into(), failure_reason);
                m.insert("reason".into(), Value::from(reason));
                Value::Object(m)
            };

            if !js::truthy(t) {
                if honoured {
                    corrections.push(Value::Object(correction("no-artifact")));
                }
                return abstain(
                    format!("{source}: no CLI artifact on disk — vote not honoured"),
                    Value::from("error"),
                );
            }
            let t = t.expect("truthy");
            let failclass = js::get(t, "failclass");
            let verdict = js::get(t, "verdict");
            let none = Value::String("none".into());
            if failclass != Some(&none) || !js::truthy(verdict) {
                let shown = js_ext::first_truthy(&[failclass, Some(&Value::from("unparseable"))])
                    .cloned()
                    .expect("the fallback is truthy");
                if honoured {
                    let mut m = correction("vote-without-verdict");
                    m.insert("failclass".into(), shown.clone());
                    corrections.push(Value::Object(m));
                }
                let failure_reason = if js::truthy(failclass) && failclass != Some(&none) {
                    failclass.cloned().expect("truthy")
                } else {
                    Value::from("unparseable")
                };
                return abstain(
                    format!(
                        "{source} unavailable ({}) — no verdict cast",
                        js_ext::to_js_string(Some(&shown))
                    ),
                    failure_reason,
                );
            }

            let t_confidence = js::get(t, "confidence").filter(|c| c.is_number());
            let confidence_declared = match js::get(t, "confidenceDeclared") {
                Some(Value::Bool(b)) => Value::Bool(*b),
                _ if t_confidence.is_some() => Value::Null,
                _ => Value::Bool(false),
            };
            let confidence_f = t_confidence.and_then(Value::as_f64).unwrap_or(0.5);
            let confidence = js_ext::number_value(confidence_f);
            let reported_confidence = vf("confidence").and_then(Value::as_f64).unwrap_or(0.5);
            let drifted = !has_vote
                || vf("slotStatus") == Some(&Value::String("UNAVAILABLE".into()))
                || !js_ext::strict_eq(vf("verdict"), verdict)
                || (reported_confidence - confidence_f).abs() > 0.01;
            if relayed && drifted {
                let mut m = correction("verdict-corrected");
                let mut actual = Map::new();
                insert_defined(&mut actual, "verdict", verdict);
                actual.insert("confidence".into(), confidence.clone());
                m.insert("actual".into(), Value::Object(actual));
                corrections.push(Value::Object(m));
            }
            let fallback = Value::String(format!("{source}: see {key}.out"));
            let reason =
                js_ext::first_truthy(&[js::get(t, "reason"), vf("reason"), Some(&fallback)])
                    .cloned()
                    .expect("the fallback is truthy");
            let mut m = spread(Some(v));
            m.insert("source".into(), Value::from(source));
            insert_defined(&mut m, "verdict", verdict);
            m.insert("confidence".into(), confidence);
            m.insert("confidenceDeclared".into(), confidence_declared);
            m.insert("slotStatus".into(), Value::from("OK"));
            // `failureReason: undefined`: the key stays where it was and
            // `JSON.stringify` drops it. Removing it without moving the keys
            // after it is the same JSON.
            m.shift_remove("failureReason");
            m.insert("reason".into(), reason);
            Value::Object(m)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn slot(source: &str, relayed: bool) -> PanelSlot {
        PanelSlot {
            source: Some(source.to_owned()),
            relayed,
        }
    }

    #[test]
    fn a_driver_slot_takes_the_artifact_verdict_without_a_correction() {
        let truth = json!({"0-codex": {"failclass": "none", "verdict": "NOT_REFUTED", "confidence": 0.99, "confidenceDeclared": true}});
        let mut c = Vec::new();
        let out = apply_artifact_truth(
            &[Value::Null],
            &[slot("codex", false)],
            Some(&truth),
            0,
            0,
            &mut c,
        );
        assert_eq!(out[0]["slotStatus"], json!("OK"));
        assert_eq!(out[0]["confidence"], json!(0.99));
        assert!(c.is_empty());
    }

    #[test]
    fn a_relayed_vote_without_an_artifact_verdict_abstains_and_is_recorded() {
        let truth = json!({"1-codex": {"failclass": "unparseable", "verdict": null}});
        let v = json!({"verdict": "REFUTED", "confidence": 0.33, "slotStatus": "OK"});
        let mut c = Vec::new();
        let out = apply_artifact_truth(&[v], &[slot("codex", true)], Some(&truth), 1, 0, &mut c);
        assert_eq!(out[0]["slotStatus"], json!("UNAVAILABLE"));
        assert_eq!(out[0]["failureReason"], json!("unparseable"));
        assert_eq!(c[0]["kind"], json!("vote-without-verdict"));
    }

    #[test]
    fn a_relayed_crash_is_still_a_correction() {
        let truth = json!({"0-codex": {"failclass": "none", "verdict": "REFUTED"}});
        let mut c = Vec::new();
        apply_artifact_truth(
            &[Value::Null],
            &[slot("codex", true)],
            Some(&truth),
            0,
            0,
            &mut c,
        );
        assert_eq!(c.len(), 1);
        assert_eq!(c[0]["reported"]["slotStatus"], json!("CRASHED"));
    }

    #[test]
    fn claude_votes_and_a_missing_truth_pass_through() {
        let v = json!({"verdict": "REFUTED"});
        let mut c = Vec::new();
        let out = apply_artifact_truth(
            std::slice::from_ref(&v),
            &[slot("claude", true)],
            Some(&json!({})),
            0,
            0,
            &mut c,
        );
        assert_eq!(out, vec![v.clone()]);
        let out = apply_artifact_truth(
            std::slice::from_ref(&v),
            &[slot("codex", true)],
            None,
            0,
            0,
            &mut c,
        );
        assert_eq!(out, vec![v]);
    }
}
