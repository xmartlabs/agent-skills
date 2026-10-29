//! Port of the LENS-ROUTING region: which reviewer agent reads the diff, by
//! the reviewer kind the profile names.

use serde_json::Value;

use super::js_ext;
use crate::js;

pub const AGENT_NS: &str = "tribunal:";

/// `REVIEWER_AGENTS`, in its declaration order.
pub const REVIEWER_AGENTS: [(&str, &str); 2] = [
    ("backend", "tribunal:code-review-expert"),
    ("frontend", "tribunal:fe-code-review-expert"),
];

/// Ports `lensAgentFor`. The kind is used as a property key, so it goes
/// through `ToString` first: `["frontend"]` routes like `"frontend"`.
///
/// A kind that names an `Object.prototype` member (`"constructor"`,
/// `"toString"`, ...) finds an inherited function in JavaScript and returns
/// it; here it is an unknown kind. It cannot reach this function in
/// production: `validate_profile` admits only the two kinds above.
pub fn lens_agent_for(profile: &Value) -> Result<&'static str, String> {
    let kind = if js::truthy(Some(profile)) {
        js::get(profile, "reviewer")
    } else {
        Some(profile)
    };
    let key = js_ext::to_string(kind);
    REVIEWER_AGENTS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|&(_, agent)| agent)
        .ok_or_else(|| {
            format!(
                "no reviewer agent for kind {}",
                js_ext::json_stringify(kind)
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn each_kind_gets_its_own_reviewer() {
        assert_eq!(
            lens_agent_for(&json!({"reviewer": "backend"})),
            Ok("tribunal:code-review-expert")
        );
        assert_eq!(
            lens_agent_for(&json!({"reviewer": ["frontend"]})),
            Ok("tribunal:fe-code-review-expert")
        );
    }

    #[test]
    fn an_unknown_kind_is_refused_with_its_json() {
        assert_eq!(
            lens_agent_for(&json!({"reviewer": "nope"})).unwrap_err(),
            "no reviewer agent for kind \"nope\""
        );
        assert_eq!(
            lens_agent_for(&json!(null)).unwrap_err(),
            "no reviewer agent for kind null"
        );
        assert_eq!(
            lens_agent_for(&json!({})).unwrap_err(),
            "no reviewer agent for kind undefined"
        );
    }
}
