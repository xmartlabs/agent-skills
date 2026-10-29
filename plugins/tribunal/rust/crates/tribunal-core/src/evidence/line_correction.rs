//! Port of the LINE-CORRECTION region: move a candidate's line from the patch
//! line a lens cited to the file line the evidence resolver found, only when
//! the relayed correction names the line the candidate still carries.

use serde_json::Value;

use super::js_ext;
use crate::js;

/// Ports `applyLineCorrection`. Mutates `candidate` in place, as the
/// JavaScript does, and returns whether it did.
///
/// The candidate's line is compared through `Number()`, so a line relayed as
/// the string `"17"` (or `null` against a patch line of 0) still matches.
pub fn apply_line_correction(candidate: &mut Value, resolution: &Value) -> bool {
    if !js::truthy(Some(candidate)) || !js::truthy(Some(resolution)) {
        return false;
    }
    let corrected = js::get(resolution, "correctedLine");
    let patch = js::get(resolution, "patchLine");
    let (Some(corrected), Some(patch)) = (corrected, patch) else {
        return false;
    };
    if !js::is_integer(Some(corrected)) || corrected.as_f64().unwrap() < 1.0 {
        return false;
    }
    if !js::is_integer(Some(patch)) {
        return false;
    }
    let (c, p) = (corrected.as_f64().unwrap(), patch.as_f64().unwrap());
    if js_ext::to_number(js::get(candidate, "line")) != p || c == p {
        return false;
    }
    // Only an object has a `line` that converts to an integer, so every
    // candidate that gets here is one.
    let Some(obj) = candidate.as_object_mut() else {
        return false;
    };
    obj.insert("lineCorrectedFrom".into(), patch.clone());
    obj.insert("line".into(), corrected.clone());
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_matching_correction_applies() {
        let mut c = json!({"file": "src/b.ts", "line": 17});
        assert!(apply_line_correction(
            &mut c,
            &json!({"correctedLine": 41, "patchLine": 17})
        ));
        assert_eq!(
            c,
            json!({"file": "src/b.ts", "line": 41, "lineCorrectedFrom": 17})
        );
    }

    #[test]
    fn a_string_line_matches_and_a_string_correction_does_not() {
        let mut s = json!({"line": "17"});
        assert!(apply_line_correction(
            &mut s,
            &json!({"correctedLine": 41, "patchLine": 17})
        ));
        let mut x = json!({"line": 17});
        assert!(!apply_line_correction(
            &mut x,
            &json!({"correctedLine": "41", "patchLine": 17})
        ));
        assert!(!apply_line_correction(
            &mut x,
            &json!({"correctedLine": 17, "patchLine": 17})
        ));
        assert!(!apply_line_correction(&mut x, &Value::Null));
        assert_eq!(x, json!({"line": 17}));
    }
}
