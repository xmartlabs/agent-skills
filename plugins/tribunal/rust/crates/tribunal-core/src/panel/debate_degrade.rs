//! Port of the DEBATE-DEGRADE region: what a bounded debate does when its
//! defense or its adjudicator dispatch comes back dead.

use serde::Serialize;
use serde_json::Value;

use super::js_ext::or;
use super::panel_gates::Tally;
use crate::js;

const NO_DEFENSE: &str = "NO DEFENSE WAS PRODUCED — the defense dispatch returned nothing. Treat this as a MISSING ARGUMENT, not as a concession: judge the finding on the refutations and the code alone, and do not read the absence of a defense as evidence against it.";
const NO_RULING: &str = "The adjudicator returned no usable decision, so this finding kept the panel's mechanical outcome and was never judged on argument.";

/// `debateDefenseBlock`. `None` is `undefined`.
pub fn debate_defense_block(defense: Option<&Value>) -> String {
    let text = match defense {
        Some(Value::String(s)) => js::trim(s),
        _ => "",
    };
    if text.is_empty() {
        NO_DEFENSE.to_owned()
    } else {
        text.to_owned()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Ruling {
    pub survived: bool,
    pub outcome: &'static str,
    pub rationale: Value,
    pub adjudicated: bool,
}

/// `debateRuling`. `None` is `undefined`.
pub fn debate_ruling(ruling: Option<&Value>, tally: &Tally) -> Ruling {
    let decision = match ruling {
        Some(r) if js::truthy(Some(r)) => js::get(r, "decision").and_then(Value::as_str),
        _ => None,
    };
    match (ruling, decision) {
        (Some(r), Some(d @ ("SURVIVES" | "REFUTED"))) => Ruling {
            survived: d == "SURVIVES",
            outcome: if js::truthy(js::get(r, "changedFromPanel")) {
                "flipped"
            } else {
                "held"
            },
            rationale: or(js::get(r, "rationale"), Value::Null),
            adjudicated: true,
        },
        _ => Ruling {
            survived: tally.survived,
            outcome: "unadjudicated",
            rationale: Value::from(NO_RULING),
            adjudicated: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::panel_gates::tally_votes;
    use serde_json::json;

    #[test]
    fn a_dead_defense_is_stated_as_absent() {
        assert!(debate_defense_block(None).starts_with("NO DEFENSE WAS PRODUCED"));
        assert_eq!(debate_defense_block(Some(&json!("  hi  "))), "hi");
    }

    #[test]
    fn a_dead_ruling_keeps_the_panel_outcome() {
        let kept = tally_votes(&[], &[], None, &json!([]), true);
        let r = debate_ruling(Some(&json!({"decision": "MAYBE"})), &kept);
        assert_eq!(
            (r.survived, r.outcome, r.adjudicated),
            (true, "unadjudicated", false)
        );
        let r = debate_ruling(
            Some(&json!({"decision": "REFUTED", "changedFromPanel": true})),
            &kept,
        );
        assert_eq!(
            (r.survived, r.outcome, r.rationale),
            (false, "flipped", Value::Null)
        );
    }
}
