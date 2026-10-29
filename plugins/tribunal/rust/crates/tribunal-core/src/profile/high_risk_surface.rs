//! Port of the HIGH-RISK-SURFACE region: the per-repo definition of a
//! high-risk surface, and the profile's risk paths standing in for a lens's
//! `touchesHighRiskSurface` flag.
//!
//! In the workflow this region sits inside LENS-FOCI; here it is its own
//! module.

use serde_json::{Map, Value};

use super::js_ext;
use super::profile_lookup::RiskRule;
use crate::js;

/// Ports `highRiskSurfaceFor`: `None` when the profile has no such field. A
/// `null` profile throws, as the property read does in JavaScript.
pub fn high_risk_surface_for(profile: &Value) -> Result<Option<Value>, String> {
    if profile.is_null() {
        return Err(js_ext::null_property_error("highRiskSurface"));
    }
    Ok(js::get(profile, "highRiskSurface").map(js_ext::js_order))
}

/// Ports `riskPathTouched`. `file` is whatever a finding carried; a
/// non-string is tested as its `ToString`, as `RegExp.prototype.test` does.
pub fn risk_path_touched(file: Option<&Value>, rules: Option<&[RiskRule]>) -> bool {
    if !js::truthy(file) {
        return false;
    }
    let file = js_ext::to_string(file);
    rules.unwrap_or_default().iter().any(|r| r.re.test(&file))
}

/// Ports `withRiskPathSurface`. A finding raised this way is a copy with both
/// flags set, keeping its own key order (an existing `touchesHighRiskSurface`
/// stays where it was); every other finding is returned as it came.
pub fn with_risk_path_surface(
    findings: Option<&[Value]>,
    rules: Option<&[RiskRule]>,
) -> Vec<Value> {
    findings
        .unwrap_or_default()
        .iter()
        .map(|f| {
            let raise = js::truthy(Some(f))
                && !js::truthy(js::get(f, "touchesHighRiskSurface"))
                && risk_path_touched(js::get(f, "file"), rules);
            if !raise {
                return js_ext::js_order(f);
            }
            let mut out: Map<String, Value> = f.as_object().cloned().unwrap_or_default();
            out.insert("touchesHighRiskSurface".into(), Value::Bool(true));
            out.insert("riskPathSurface".into(), Value::Bool(true));
            js_ext::js_order(&Value::Object(out))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::profile_lookup::risk_path_rules;
    use serde_json::json;

    fn rules() -> Vec<RiskRule> {
        risk_path_rules(&json!({"riskPaths": [
            {"focus": "security", "pattern": "auth|role"},
            {"focus": "dataContract", "pattern": "invoice|payment"}
        ]}))
        .unwrap()
    }

    #[test]
    fn a_risk_path_touches_the_surface() {
        let r = rules();
        assert!(risk_path_touched(
            Some(&json!("src/services/v2/payments.ts")),
            Some(&r)
        ));
        assert!(!risk_path_touched(
            Some(&json!("src/utils/strings.ts")),
            Some(&r)
        ));
        assert!(!risk_path_touched(Some(&json!("")), Some(&r)));
    }

    #[test]
    fn a_raised_finding_keeps_its_key_order() {
        let f = json!({"touchesHighRiskSurface": false, "file": "src/auth.ts", "title": "t"});
        let out = with_risk_path_surface(Some(&[f]), Some(&rules()));
        let keys: Vec<&str> = out[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["touchesHighRiskSurface", "file", "title", "riskPathSurface"]
        );
    }
}
