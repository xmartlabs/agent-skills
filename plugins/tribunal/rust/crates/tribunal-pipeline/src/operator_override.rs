//! `tribunal override`: a person's verdict on one finding of a run, written
//! into that run's ledger row. Two kinds:
//!
//! - `--direction reinstated|rejected|withdrawn` sets `operatorOverride`, the
//!   panel-error label `eval/operator-override.mjs` scores. The consistency
//!   rules are the same: reinstated and withdrawn need a finding the panel
//!   killed, rejected one it kept.
//! - `--severity nit|important` sets `operatorSeverity`: the finding is real
//!   but less severe than reported. It is not a panel error, so it lives in
//!   its own field and the scorer's direction list does not change. A
//!   carried-forward finding with it stops blocking and grades at the new
//!   severity.
//!
//! Either way the reason is required, and the row and finding must be named
//! exactly: one row by its `ts`, one finding in it by a title prefix.

use serde_json::{json, Value};

const DIRECTIONS: [&str; 3] = ["reinstated", "rejected", "withdrawn"];
const SEVERITIES: [&str; 3] = ["nit", "important", "critical"];

#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Direction(String),
    Severity(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub ts: String,
    pub title_prefix: String,
    pub change: Change,
    pub reason: String,
    pub by: Option<String>,
    pub verified_at: Option<String>,
    /// When the person made the call, ISO 8601.
    pub at: String,
}

/// What [`apply`] changed.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub ledger: String,
    pub title: String,
    pub field: &'static str,
    pub value: Value,
    pub warnings: Vec<String>,
}

fn rank(s: Option<&str>) -> Option<usize> {
    SEVERITIES.iter().position(|x| Some(*x) == s)
}

/// The field and value a request writes on `finding`.
///
/// # Errors
/// When the request contradicts the finding (the consistency rules), names
/// an unknown direction or severity, or gives no reason.
pub fn annotation(finding: &Value, req: &Request) -> Result<(&'static str, Value), String> {
    if req.reason.trim().is_empty() {
        return Err("--reason is required: say how the finding was checked by hand".into());
    }
    let survived = finding.get("survived").and_then(Value::as_bool);
    let common = |mut v: Value| {
        v["by"] = json!(req.by);
        v["at"] = json!(req.at);
        v["verifiedAt"] = json!(req.verified_at);
        v["reason"] = json!(req.reason.trim());
        v
    };
    match &req.change {
        Change::Direction(d) => {
            if !DIRECTIONS.contains(&d.as_str()) {
                return Err(format!(
                    "--direction must be one of {}, got {d}",
                    DIRECTIONS.join(" | ")
                ));
            }
            let needs = d != "rejected";
            if survived != Some(!needs) {
                return Err(format!(
                    "{d} needs a finding the panel {}; this one has survived: {}",
                    if needs { "killed" } else { "kept" },
                    survived.map_or("absent".to_owned(), |s| s.to_string())
                ));
            }
            Ok(("operatorOverride", common(json!({"direction": d}))))
        }
        Change::Severity(s) => {
            let to = rank(Some(s)).ok_or_else(|| {
                format!(
                    "--severity must be one of {}, got {s}",
                    SEVERITIES.join(" | ")
                )
            })?;
            let own = finding.get("severity").and_then(Value::as_str);
            if survived != Some(true) {
                return Err("a severity change needs a finding the panel kept".into());
            }
            if !rank(own).is_some_and(|o| to < o) {
                return Err(format!(
                    "--severity {s} is not lower than the finding's {}",
                    own.unwrap_or("missing severity")
                ));
            }
            Ok((
                "operatorSeverity",
                common(json!({"severity": s, "from": own})),
            ))
        }
    }
}

/// The ledger with the annotation on the one finding the request names. Every
/// other line is kept byte for byte.
///
/// # Errors
/// When no finding or more than one matches, or [`annotation`] refuses.
pub fn apply(ledger: &str, req: &Request) -> Result<Applied, String> {
    let mut hits: Vec<(usize, usize)> = Vec::new();
    let mut rows: Vec<Option<Value>> = Vec::new();
    for (i, line) in ledger.split('\n').enumerate() {
        let row = serde_json::from_str::<Value>(line)
            .ok()
            .filter(|r| r.get("ts").and_then(Value::as_str) == Some(req.ts.as_str()));
        if let Some(findings) = row
            .as_ref()
            .and_then(|r| r.get("findings"))
            .and_then(Value::as_array)
        {
            for (j, f) in findings.iter().enumerate() {
                let title = f.get("title").and_then(Value::as_str).unwrap_or("");
                if title.starts_with(&req.title_prefix) {
                    hits.push((i, j));
                }
            }
        }
        rows.push(row);
    }
    let (i, j) = match hits.as_slice() {
        [one] => *one,
        [] => {
            return Err(format!(
                "no finding titled \"{}…\" in a row with ts {}",
                req.title_prefix, req.ts
            ))
        }
        many => {
            return Err(format!(
                "{} findings match \"{}…\" in rows with ts {}; give a longer title prefix",
                many.len(),
                req.title_prefix,
                req.ts
            ))
        }
    };
    let mut row = rows[i].take().expect("a hit has its row");
    let finding = &mut row["findings"][j];
    let (field, value) = annotation(finding, req)?;
    let title = finding["title"].as_str().unwrap_or("").to_owned();
    finding[field] = value.clone();
    let mut warnings = Vec::new();
    if req.by.is_none() {
        warnings.push("no --by: the call is not attributed to anyone".to_owned());
    }
    if req.verified_at.is_none() {
        warnings.push("no --verified-at: nobody can re-check the call against a tree".to_owned());
    }
    let out: Vec<String> = ledger
        .split('\n')
        .enumerate()
        .map(|(k, line)| {
            if k == i {
                row.to_string()
            } else {
                line.to_owned()
            }
        })
        .collect();
    Ok(Applied {
        ledger: out.join("\n"),
        title,
        field,
        value,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger() -> String {
        [
            json!({"ts": "t1", "findings": [{"title": "Retry drops the job", "severity": "important", "survived": true}]}).to_string(),
            "not json".to_owned(),
            json!({"ts": "t2", "findings": [
                {"title": "Retry drops the job", "severity": "important", "survived": true},
                {"title": "Token logged on error", "severity": "critical", "survived": false},
                {"title": "Token logged twice", "severity": "nit", "survived": true}
            ]}).to_string(),
            String::new(),
        ]
        .join("\n")
    }

    fn req(prefix: &str, change: Change) -> Request {
        Request {
            ts: "t2".into(),
            title_prefix: prefix.into(),
            change,
            reason: "read the handler at HEAD".into(),
            by: Some("dev".into()),
            verified_at: Some("abc123".into()),
            at: "2026-10-30T12:00:00Z".into(),
        }
    }

    #[test]
    fn a_downgrade_lands_on_the_one_named_finding_only() {
        let l = ledger();
        let a = apply(&l, &req("Retry", Change::Severity("nit".into()))).unwrap();
        assert_eq!(
            (a.field, a.title.as_str()),
            ("operatorSeverity", "Retry drops the job")
        );
        assert_eq!(a.value["severity"], "nit");
        assert_eq!(a.value["from"], "important");
        let before: Vec<&str> = l.split('\n').collect();
        let after: Vec<&str> = a.ledger.split('\n').collect();
        assert_eq!(after.len(), before.len());
        assert_eq!(
            (after[0], after[1], after[3]),
            (before[0], before[1], before[3])
        );
        let row: Value = serde_json::from_str(after[2]).unwrap();
        assert_eq!(
            row["findings"][0]["operatorSeverity"]["reason"],
            "read the handler at HEAD"
        );
        assert!(row["findings"][1].get("operatorSeverity").is_none());
        assert!(a.warnings.is_empty());
    }

    #[test]
    fn the_match_must_be_exactly_one_finding() {
        let l = ledger();
        assert!(apply(&l, &req("Token", Change::Severity("nit".into())))
            .unwrap_err()
            .contains("2 findings match"));
        assert!(apply(&l, &req("Nothing", Change::Severity("nit".into())))
            .unwrap_err()
            .contains("no finding"));
    }

    #[test]
    fn directions_follow_the_consistency_rules() {
        let l = ledger();
        let killed = "Token logged on";
        assert!(apply(&l, &req(killed, Change::Direction("reinstated".into()))).is_ok());
        assert!(apply(&l, &req(killed, Change::Direction("rejected".into()))).is_err());
        assert!(apply(&l, &req("Retry", Change::Direction("rejected".into()))).is_ok());
        assert!(apply(&l, &req("Retry", Change::Direction("withdrawn".into()))).is_err());
        assert!(apply(&l, &req("Retry", Change::Direction("dropped".into()))).is_err());
    }

    #[test]
    fn a_severity_change_must_lower_a_kept_finding() {
        let l = ledger();
        assert!(apply(&l, &req("Retry", Change::Severity("important".into()))).is_err());
        assert!(apply(&l, &req("Retry", Change::Severity("critical".into()))).is_err());
        assert!(apply(&l, &req("Token logged on", Change::Severity("nit".into()))).is_err());
        assert!(apply(
            &l,
            &req("Token logged twice", Change::Severity("nit".into()))
        )
        .is_err());
    }

    #[test]
    fn a_reason_is_required_and_missing_attribution_warns() {
        let l = ledger();
        let mut r = req("Retry", Change::Severity("nit".into()));
        r.reason = "  ".into();
        assert!(apply(&l, &r).unwrap_err().contains("--reason"));
        r.reason = "checked".into();
        r.by = None;
        r.verified_at = None;
        assert_eq!(apply(&l, &r).unwrap().warnings.len(), 2);
    }
}
