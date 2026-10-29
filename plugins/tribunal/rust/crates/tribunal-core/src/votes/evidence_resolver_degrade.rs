//! Port of the EVIDENCE-RESOLVER-DEGRADE region: the log line for an evidence
//! resolver that did not run names which of its failures happened, so it never
//! reads like a run that resolved everything.

use serde_json::Value;

use super::js_ext;
use crate::js;

/// Ports `evidenceResolverFailure`. `parsed` is the resolver's JSON answer
/// (`None` when nothing came back); `relay_reason` is why its relay was not
/// verified, and an empty one counts as none, as JavaScript's truthiness had it.
pub fn evidence_resolver_failure(
    parsed: Option<&Value>,
    candidate_count: usize,
    relay_reason: Option<&str>,
) -> String {
    let why = match parsed.filter(|p| js::truthy(Some(p))) {
        None => match relay_reason.filter(|r| !r.is_empty()) {
            Some(r) => format!("its relay was not verified: {r}"),
            None => "the dispatch returned nothing".to_owned(),
        },
        Some(p) => {
            let reason = js::get(p, "reason");
            if js::truthy(reason) {
                format!("the script reported {}", js_ext::to_js_string(reason))
            } else if js::get(p, "ran") == Some(&Value::Bool(false)) {
                "the script reported ran:false with no reason".to_owned()
            } else {
                "the script ran but relayed no results array".to_owned()
            }
        }
    };
    format!(
        "Evidence resolver did not run ({why}) — {candidate_count} candidate(s) went unchecked. This is NOT a finding that every evidence quote resolved."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn each_failure_reads_differently() {
        let dead = evidence_resolver_failure(None, 7, None);
        assert!(dead.contains("the dispatch returned nothing"));
        assert!(dead.contains("7 candidate(s) went unchecked"));
        let bad = evidence_resolver_failure(
            Some(&json!({"ran": false, "reason": "invalid-diff"})),
            7,
            None,
        );
        assert!(bad.contains("reported invalid-diff"));
        let relay = evidence_resolver_failure(None, 0, Some("checksum mismatch"));
        assert!(relay.contains("its relay was not verified: checksum mismatch"));
    }
}
