//! Port of the PROFILE-LOOKUP region: the validation of the one repo's review
//! profile, which every project fact a review uses passes through, and the
//! risk-path rules read from it.
//!
//! The profile is JSON written by a person, and the region judges its shape at
//! runtime, so it is read as a `serde_json::Value`. The region's three shell
//! fragment builders (`deployBranchCasePattern`, `deployBranchList`,
//! `crossRepoFlags`) are not ported: they only exist to splice profile fields
//! into bash, which the binary does not run.

use serde_json::{Map, Value};

use super::js_ext::{self, JsRegex};
use crate::js;

pub const PROFILE_SCHEMA_VERSION: f64 = 1.0;
pub const PROFILE_REVIEWER_KINDS: [&str; 2] = ["backend", "frontend"];
pub const PROFILE_REQUIRED_FOCI: [&str; 5] = [
    "correctness",
    "dataContract",
    "security",
    "performance",
    "errorHandling",
];
pub const PROFILE_OPTIONAL_FOCI: [&str; 1] = ["migration"];
/// The foci a changed path can force. A rule naming another focus is a
/// warning, not an error: the profile can be shared with other tools.
pub const PROFILE_RISK_FOCI: [&str; 3] = ["migration", "security", "dataContract"];

/// A risk-path rule: a changed path matching `re` forces `focus`.
#[derive(Debug, Clone)]
pub struct RiskRule {
    pub focus: String,
    pub re: JsRegex,
}

fn ascii_in(b: u8, extra: &[u8]) -> bool {
    b.is_ascii_alphanumeric() || extra.contains(&b)
}

/// `/^[A-Za-z0-9][A-Za-z0-9._\/-]*$/`. Without the `m` flag `$` matches only
/// at the end, so a trailing newline fails.
fn is_branch(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b[0].is_ascii_alphanumeric() && b[1..].iter().all(|&c| ascii_in(c, b"._/-"))
}

/// `/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/`.
fn is_repo(s: &str) -> bool {
    match s.split_once('/') {
        Some((owner, name)) => [owner, name]
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|c| ascii_in(c, b"_.-"))),
        None => false,
    }
}

fn is_text(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::String(s)) if !js::trim(s).is_empty())
}

fn has_dotdot_segment(s: &str) -> bool {
    s.split('/').any(|seg| seg == "..")
}

/// Ports `riskPathProblems`. `list` is `None` when the field is absent.
pub fn risk_path_problems(list: Option<&Value>, at: &str) -> Vec<String> {
    let Some(list) = list else {
        return Vec::new();
    };
    let Value::Array(rules) = list else {
        return vec![format!("{at} must be an array of {{focus, pattern}}")];
    };
    let mut problems = Vec::new();
    for (i, r) in rules.iter().enumerate() {
        let focus_ok = js::is_object_like(r) && is_text(js::get(r, "focus"));
        if !focus_ok {
            problems.push(format!("{at}[{i}].focus must be a focus key"));
            continue;
        }
        let Some(Value::String(pattern)) = js::get(r, "pattern").filter(|p| is_text(Some(p)))
        else {
            problems.push(format!(
                "{at}[{i}].pattern must be a non-empty regular expression"
            ));
            continue;
        };
        if let Err(message) = JsRegex::new_ignore_case(pattern) {
            problems.push(format!("{at}[{i}].pattern does not compile ({message})"));
        }
    }
    problems
}

/// The source `new RegExp(pattern)` compiles: an absent pattern is the empty
/// one, anything else goes through `ToString`.
fn pattern_source(p: Option<&Value>) -> String {
    match p {
        None => String::new(),
        p => js_ext::to_string(p),
    }
}

/// Ports `riskRulesOf`. Only a `null` entry throws in JavaScript (reading
/// `.focus`), and a pattern that does not compile throws its `SyntaxError`;
/// both are `Err`. The `TypeError` text is V8's; the reason in a
/// `SyntaxError` is regress's (see `JsRegex::new_ignore_case`).
pub fn risk_rules_of(list: Option<&Value>) -> Result<Vec<RiskRule>, String> {
    let Some(Value::Array(items)) = list else {
        return Ok(Vec::new());
    };
    let mut kept = Vec::new();
    for r in items {
        if r.is_null() {
            return Err(js_ext::null_property_error("focus"));
        }
        if let Some(Value::String(f)) = js::get(r, "focus") {
            if PROFILE_RISK_FOCI.contains(&f.as_str()) {
                kept.push((f.clone(), r));
            }
        }
    }
    kept.into_iter()
        .map(|(focus, r)| {
            let re = JsRegex::new_ignore_case(&pattern_source(js::get(r, "pattern")))?;
            Ok(RiskRule { focus, re })
        })
        .collect()
}

/// Ports `riskPathRules`: the Workflow's forced foci.
pub fn risk_path_rules(profile: &Value) -> Result<Vec<RiskRule>, String> {
    if profile.is_null() {
        return Err(js_ext::null_property_error("riskPaths"));
    }
    risk_rules_of(js::get(profile, "riskPaths"))
}

/// Ports `inContextRiskPathRules`: `tribunal.inContextRiskPaths` when the
/// profile sets it, else `riskPaths`. A falsy `tribunal` (such as `null`) is
/// itself what `own` reads, so it yields no rules rather than `riskPaths`.
pub fn in_context_risk_path_rules(profile: &Value) -> Result<Vec<RiskRule>, String> {
    if profile.is_null() {
        return Err(js_ext::null_property_error("tribunal"));
    }
    let tribunal = js::get(profile, "tribunal");
    let own = if js::truthy(tribunal) {
        tribunal.and_then(|t| js::get(t, "inContextRiskPaths"))
    } else {
        tribunal
    };
    risk_rules_of(own.or_else(|| js::get(profile, "riskPaths")))
}

/// Ports `profileWarnings`. Errs only where JavaScript throws: a `null`
/// profile or a `null` rule.
pub fn profile_warnings(profile: &Value) -> Result<Vec<String>, String> {
    if profile.is_null() {
        return Err(js_ext::null_property_error("riskPaths"));
    }
    let mut lists = vec![("riskPaths", js::get(profile, "riskPaths"))];
    let tribunal = js::get(profile, "tribunal");
    if js::truthy(tribunal) {
        if let Some(own) = tribunal.and_then(|t| js::get(t, "inContextRiskPaths")) {
            lists.push(("tribunal.inContextRiskPaths", Some(own)));
        }
    }
    let mut warnings = Vec::new();
    for (at, list) in lists {
        let Some(Value::Array(rules)) = list else {
            continue;
        };
        for (i, r) in rules.iter().enumerate() {
            if r.is_null() {
                return Err(js_ext::null_property_error("focus"));
            }
            let focus = js::get(r, "focus");
            let known =
                matches!(focus, Some(Value::String(f)) if PROFILE_RISK_FOCI.contains(&f.as_str()));
            if !known {
                warnings.push(format!(
                    "{at}[{i}] forces \"{}\", which tribunal does not force; the rule is ignored here",
                    js_ext::to_string(focus)
                ));
            }
        }
    }
    if js::get(profile, "riskPaths").is_none() {
        warnings.push(
            "no riskPaths: no changed path counts as high-risk surface or forces a focus".into(),
        );
    }
    if js::get(profile, "projectRules").is_none() {
        warnings.push(
            "no projectRules: reviewers get the plugin's generic methodology, with none of this repo's own rules"
                .into(),
        );
    }
    Ok(warnings)
}

fn foci_problems(foci: &Map<String, Value>, errors: &mut Vec<String>) {
    for k in PROFILE_REQUIRED_FOCI {
        if !is_text(foci.get(k)) {
            errors.push(format!("foci.{k} must be a non-empty sentence"));
        }
    }
    let mut seen: Vec<(&str, &str)> = Vec::new();
    for (k, sentence) in js_ext::ordered_entries(foci) {
        let known = PROFILE_REQUIRED_FOCI.contains(&k.as_str())
            || PROFILE_OPTIONAL_FOCI.contains(&k.as_str());
        if !known {
            errors.push(format!("foci.{k} is not a known focus key"));
        } else if !is_text(Some(sentence)) {
            errors.push(format!("foci.{k} must be a non-empty sentence"));
        } else {
            let s = sentence.as_str().expect("text");
            match seen.iter().find(|(t, _)| *t == s) {
                Some((_, first)) => errors.push(format!(
                    "foci.{k} reuses the sentence of foci.{first}, so FOCI_LABEL could not attribute it"
                )),
                None => seen.push((s, k.as_str())),
            }
        }
    }
}

/// `PROFILE_REPO_RE.test(x)` and friends coerce their argument with
/// `ToString`, so `{repo: ["a/b"]}` names the repo `a/b`.
fn anchor_ok(a: &Value, repo: Option<&Value>) -> bool {
    let field = |k: &str| {
        let v = js::get(a, k);
        if js::truthy(v) {
            js_ext::to_string(v)
        } else {
            String::new()
        }
    };
    js::truthy(Some(a))
        && is_repo(&field("repo"))
        && is_branch(&field("ref"))
        && !js_ext::strict_equals(js::get(a, "repo"), repo)
}

fn is_project_path(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && ascii_in(b[0], b"._-")
        && b[1..].iter().all(|&c| ascii_in(c, b"._/-"))
        && !has_dotdot_segment(s)
}

fn is_convention_file(s: &str) -> bool {
    let b = s.as_bytes();
    let dir = &s[..s.rfind('/').map_or(0, |i| i + 1)];
    !b.is_empty()
        && ascii_in(b[0], b"._*-")
        && b[1..].iter().all(|&c| ascii_in(c, b"._*/-"))
        && !has_dotdot_segment(s)
        && !dir.contains('*')
        && !s.ends_with('/')
}

/// Ports `validateProfile`. `expected_repo` is the run's `repo` argument
/// (`null` when absent). Returns the profile with its keys in JavaScript
/// order, as the workflow holds it.
///
/// A string profile is parsed as JSON first. When it is not valid JSON the
/// error names serde's reason, not V8's: the wording of `JSON.parse` errors is
/// engine-specific and is not reproduced.
pub fn validate_profile(raw: &Value, expected_repo: &Value) -> Result<Value, String> {
    let parsed;
    let p = match raw {
        Value::String(s) => {
            parsed = serde_json::from_str::<Value>(s)
                .map_err(|e| format!("tribunal profile: args.profile is not valid JSON ({e})"))?;
            &parsed
        }
        other => other,
    };
    let Value::Object(_) = p else {
        return Err("tribunal profile: args.profile is missing. SKILL.md must run scripts/load-profile.mjs and pass its profile; there is no default project.".into());
    };
    let get = |k: &str| js::get(p, k);
    let mut errors: Vec<String> = Vec::new();

    let version_ok = matches!(get("schemaVersion"), Some(Value::Number(n)) if n.as_f64() == Some(PROFILE_SCHEMA_VERSION));
    if !version_ok {
        errors.push(format!(
            "schemaVersion must be 1, got {}",
            js_ext::json_stringify(get("schemaVersion"))
        ));
    }
    if !is_text(get("project")) {
        errors.push("project must be a non-empty string".into());
    }
    let repo = get("repo");
    if !is_text(repo) || !is_repo(repo.and_then(Value::as_str).unwrap_or_default()) {
        errors.push(format!(
            "repo must be owner/name, got {}",
            js_ext::json_stringify(repo)
        ));
    } else if js::truthy(Some(expected_repo)) && !js_ext::strict_equals(repo, Some(expected_repo)) {
        errors.push(format!(
            "the profile describes {}, but this run reviews {}",
            js_ext::to_string(repo),
            js_ext::to_string(Some(expected_repo))
        ));
    }
    let reviewer = get("reviewer");
    if !matches!(reviewer, Some(Value::String(r)) if PROFILE_REVIEWER_KINDS.contains(&r.as_str())) {
        errors.push(format!(
            "reviewer must be one of {}, got {}",
            PROFILE_REVIEWER_KINDS.join(", "),
            js_ext::json_stringify(reviewer)
        ));
    }
    let base = get("baseBranch");
    if !is_text(base) || !is_branch(base.and_then(Value::as_str).unwrap_or_default()) {
        errors.push("baseBranch must be a branch name".into());
    }
    let deploy_ok = match get("deployBranches") {
        Some(Value::Array(bs)) => bs
            .iter()
            .all(|b| is_text(Some(b)) && is_branch(b.as_str().unwrap_or_default())),
        _ => false,
    };
    if !deploy_ok {
        errors.push("deployBranches must be an array of branch names (it may be empty)".into());
    }
    match get("foci") {
        Some(Value::Object(foci)) => foci_problems(foci, &mut errors),
        _ => errors.push("foci must be an object".into()),
    }
    if !is_text(get("highRiskSurface")) {
        errors.push("highRiskSurface must be a non-empty sentence".into());
    }
    let anchors_ok = match get("crossRepoAnchors") {
        Some(Value::Array(anchors)) => anchors.iter().all(|a| anchor_ok(a, repo)),
        _ => false,
    };
    if !anchors_ok {
        errors.push("crossRepoAnchors must be an array of {repo: owner/name, ref} naming other repos (it may be empty)".into());
    }
    errors.extend(risk_path_problems(get("riskPaths"), "riskPaths"));
    if let Some(rules) = get("projectRules") {
        if !matches!(rules, Value::String(s) if is_project_path(s)) {
            errors.push(
                "projectRules must be a path inside the repo, such as .claude/review-rules.md"
                    .into(),
            );
        }
    }
    if let Some(files) = get("conventionFiles") {
        let ok = match files {
            Value::Array(fs) => fs
                .iter()
                .all(|f| matches!(f, Value::String(s) if is_convention_file(s))),
            _ => false,
        };
        if !ok {
            errors.push("conventionFiles must be an array of paths inside the repo, with `*` only in the last segment, such as [\"CLAUDE.md\", \".claude/rules/*.md\"]".into());
        }
    }
    if let Some(tribunal) = get("tribunal") {
        match tribunal {
            Value::Object(_) => errors.extend(risk_path_problems(
                js::get(tribunal, "inContextRiskPaths"),
                "tribunal.inContextRiskPaths",
            )),
            _ => errors.push("tribunal must be an object".into()),
        }
    }
    if !errors.is_empty() {
        return Err(format!(
            "tribunal profile for {} is invalid:\n  - {}",
            js_ext::json_stringify(repo),
            errors.join("\n  - ")
        ));
    }
    Ok(js_ext::js_order(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile() -> Value {
        json!({
            "schemaVersion": 1,
            "project": "Acme",
            "repo": "acme/api",
            "reviewer": "backend",
            "baseBranch": "development",
            "deployBranches": ["staging", "production"],
            "foci": {
                "correctness": "c", "dataContract": "d", "security": "s",
                "performance": "p", "errorHandling": "e"
            },
            "highRiskSurface": "auth, payments",
            "crossRepoAnchors": [{"repo": "acme/web", "ref": "development"}],
            "riskPaths": [{"focus": "security", "pattern": "auth|role"}]
        })
    }

    #[test]
    fn a_valid_profile_passes_for_its_own_repo() {
        assert!(validate_profile(&profile(), &json!("acme/api")).is_ok());
        let err = validate_profile(&profile(), &json!("acme/web")).unwrap_err();
        assert!(err.contains("describes acme/api, but this run reviews acme/web"));
    }

    #[test]
    fn every_error_is_reported_at_once() {
        let mut p = profile();
        p["reviewer"] = json!("x");
        p["highRiskSurface"] = json!(" ");
        let err = validate_profile(&p, &Value::Null).unwrap_err();
        assert!(err.contains("reviewer must be one of backend, frontend, got \"x\""));
        assert!(err.contains("highRiskSurface must be a non-empty sentence"));
    }

    #[test]
    fn an_uncompilable_pattern_is_a_problem() {
        let problems = risk_path_problems(
            Some(&json!([{"focus": "security", "pattern": "("}])),
            "riskPaths",
        );
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0].starts_with(
                "riskPaths[0].pattern does not compile (Invalid regular expression: /(/i: "
            ),
            "{}",
            problems[0]
        );
    }

    #[test]
    fn rules_match_case_insensitively() {
        let rules = risk_path_rules(&profile()).unwrap();
        assert_eq!(rules.len(), 1);
        assert!(rules[0].re.test("src/AUTH/login.ts"));
    }
}
