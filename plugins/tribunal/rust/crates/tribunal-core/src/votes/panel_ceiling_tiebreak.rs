//! Port of the PANEL-CEILING-TIEBREAK region: when more findings earned a
//! panel than the run can pay for and several tie exactly at the cutoff, find
//! that tied group, and check the model's ranking of it before trusting it.

use std::num::NonZeroUsize;

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use serde_json::Value;

use super::js_ext;
use crate::js;

/// Ports `PANEL_TIEBREAK_CAP`: a tied group larger than this is not sent to
/// the model.
pub const PANEL_TIEBREAK_CAP: usize = 12;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TiedGroup {
    pub boundary_priority: Value,
    pub guaranteed_ids: Vec<Value>,
    pub tied_group: Vec<Value>,
    pub slots_for_tied_group: i64,
}

fn num(v: &Value, key: &str) -> f64 {
    js::get(v, key).and_then(Value::as_f64).unwrap_or(f64::NAN)
}

/// Ports `panelCeilingTiedGroup`. Each summary is the object the caller built,
/// passed through whole; `priority` comes from `panelPriority`, a sum of
/// integers, and `i` is the summary's index, so both are always numbers.
/// `maxPanels` is the minimum of two values floored at 1, hence `NonZeroUsize`:
/// at 0 the JavaScript read `ranked[-1].priority` and threw.
pub fn panel_ceiling_tied_group(
    eligible_summaries: &[Value],
    max_panels_for_run: NonZeroUsize,
) -> Option<TiedGroup> {
    let max = max_panels_for_run.get();
    if eligible_summaries.len() <= max {
        return None;
    }
    let mut ranked: Vec<&Value> = eligible_summaries.iter().collect();
    ranked.sort_by(|a, b| {
        let by_priority = num(b, "priority") - num(a, "priority");
        let c = if by_priority != 0.0 && !by_priority.is_nan() {
            by_priority
        } else {
            num(a, "i") - num(b, "i")
        };
        c.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
    });
    let boundary = ranked[max - 1];
    let boundary_priority = num(boundary, "priority");
    let guaranteed_ids: Vec<Value> = ranked
        .iter()
        .filter(|f| num(f, "priority") > boundary_priority)
        .map(|f| js::get(f, "slotId").cloned().unwrap_or(Value::Null))
        .collect();
    let tied_group: Vec<Value> = ranked
        .iter()
        .filter(|f| num(f, "priority") == boundary_priority)
        .map(|f| (*f).clone())
        .collect();
    Some(TiedGroup {
        boundary_priority: js::get(boundary, "priority")
            .cloned()
            .unwrap_or(Value::Null),
        slots_for_tied_group: max as i64 - guaranteed_ids.len() as i64,
        guaranteed_ids,
        tied_group,
    })
}

/// The outcome of `resolvePanelCeilingTiebreak`, serialized as the JavaScript
/// object: `{ok: false, reason}` or `{ok: true, winningIds, losingIds}`.
#[derive(Debug, Clone, PartialEq)]
pub enum TiebreakResolution {
    Rejected(&'static str),
    Split {
        winning_ids: Vec<Value>,
        losing_ids: Vec<Value>,
    },
}

impl Serialize for TiebreakResolution {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(None)?;
        match self {
            Self::Rejected(reason) => {
                m.serialize_entry("ok", &false)?;
                m.serialize_entry("reason", reason)?;
            }
            Self::Split {
                winning_ids,
                losing_ids,
            } => {
                m.serialize_entry("ok", &true)?;
                m.serialize_entry("winningIds", winning_ids)?;
                m.serialize_entry("losingIds", losing_ids)?;
            }
        }
        m.end()
    }
}

/// `Array.prototype.slice` index resolution for one bound.
fn slice_index(i: i64, len: usize) -> usize {
    if i < 0 {
        (len as i64 + i).max(0) as usize
    } else {
        (i as usize).min(len)
    }
}

/// Ports `resolvePanelCeilingTiebreak`. `agent_order` is the model's `order`
/// field as it arrived, any JSON or nothing; the ids are compared the way a
/// JavaScript `Set` compares them.
pub fn resolve_panel_ceiling_tiebreak(
    tied_group: &[Value],
    slots_for_tied_group: i64,
    agent_order: Option<&Value>,
) -> TiebreakResolution {
    let mut want_ids: Vec<Option<&Value>> = Vec::new();
    for id in tied_group.iter().map(|f| js::get(f, "slotId")) {
        if !want_ids.iter().any(|w| js_ext::strict_eq(*w, id)) {
            want_ids.push(id);
        }
    }
    let order: &[Value] = match agent_order {
        Some(Value::Array(a)) => a,
        _ => &[],
    };
    let mut seen: Vec<&Value> = Vec::new();
    for id in order {
        let wanted = want_ids.iter().any(|w| js_ext::strict_eq(*w, Some(id)));
        let repeated = seen.iter().any(|s| js_ext::strict_eq(Some(s), Some(id)));
        if !wanted || repeated {
            return TiebreakResolution::Rejected("agent order was not exactly the tied slotIds");
        }
        seen.push(id);
    }
    if seen.len() != want_ids.len() {
        return TiebreakResolution::Rejected("agent order was missing one or more tied slotIds");
    }
    let cut = slice_index(slots_for_tied_group, order.len());
    TiebreakResolution::Split {
        winning_ids: order[..cut].to_vec(),
        losing_ids: order[cut..].to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn nz(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).unwrap()
    }

    #[test]
    fn a_boundary_tie_is_found() {
        let eligible: Vec<Value> = [(1, 8), (2, 4), (3, 4), (4, 4), (5, 2)]
            .iter()
            .enumerate()
            .map(|(i, (id, p))| json!({"slotId": id, "priority": p, "i": i}))
            .collect();
        let d = panel_ceiling_tied_group(&eligible, nz(3)).unwrap();
        assert_eq!(d.boundary_priority, json!(4));
        assert_eq!(d.guaranteed_ids, vec![json!(1)]);
        assert_eq!(d.tied_group.len(), 3);
        assert_eq!(d.slots_for_tied_group, 2);
        assert!(panel_ceiling_tied_group(&eligible[..2], nz(2)).is_none());
    }

    #[test]
    fn only_an_exact_permutation_is_trusted() {
        let tied = [
            json!({"slotId": 2}),
            json!({"slotId": 3}),
            json!({"slotId": 4}),
        ];
        let ok = resolve_panel_ceiling_tiebreak(&tied, 2, Some(&json!([4, 2, 3])));
        assert_eq!(
            serde_json::to_value(&ok).unwrap(),
            json!({"ok": true, "winningIds": [4, 2], "losingIds": [3]})
        );
        for bad in [
            json!([4, 2]),
            json!([4, 2, 3, 99]),
            json!([4, 4, 3]),
            json!(null),
            json!("nope"),
        ] {
            assert!(matches!(
                resolve_panel_ceiling_tiebreak(&tied, 2, Some(&bad)),
                TiebreakResolution::Rejected(_)
            ));
        }
        assert_eq!(PANEL_TIEBREAK_CAP, 12);
    }
}
