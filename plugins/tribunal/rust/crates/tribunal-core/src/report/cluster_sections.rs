use crate::profile::group_by::GroupByRule;
use serde_json::Value;

#[must_use]
pub fn cluster_sections(findings: &[Value], pr_files: &[String], rules: &[GroupByRule]) -> String {
    if rules.is_empty() {
        return String::new();
    }

    let mut clusters: Vec<String> = pr_files
        .iter()
        .filter_map(|path| crate::profile::group_by::cluster_key(path, rules))
        .collect();
    clusters.sort();
    clusters.dedup();

    let mut out = String::from("\n\n---\n\n**Findings by area.**");
    for cluster in clusters {
        out.push_str(&format!(
            "\n\n**{}**",
            crate::lens::coverage_gaps::code_span(&cluster)
        ));
        let matched: Vec<&Value> = findings
            .iter()
            .filter(|finding| {
                finding
                    .get("file")
                    .and_then(Value::as_str)
                    .and_then(|file| crate::profile::group_by::cluster_key(file, rules))
                    .as_deref()
                    == Some(cluster.as_str())
            })
            .collect();
        if matched.is_empty() {
            out.push_str("\n- No findings.");
        } else {
            for finding in matched {
                out.push_str(&format!(
                    "\n- {} {} ({}:{})",
                    clean_text(
                        finding
                            .get("severity")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                    ),
                    clean_text(finding.get("title").and_then(Value::as_str).unwrap_or("")),
                    crate::lens::coverage_gaps::code_span(
                        finding.get("file").and_then(Value::as_str).unwrap_or("")
                    ),
                    finding
                        .get("line")
                        .map_or_else(|| "?".into(), Value::to_string)
                ));
            }
        }
    }

    let outside: Vec<&Value> = findings
        .iter()
        .filter(|finding| {
            finding
                .get("file")
                .and_then(Value::as_str)
                .is_none_or(|file| crate::profile::group_by::cluster_key(file, rules).is_none())
        })
        .collect();
    if !outside.is_empty() {
        out.push_str("\n\n**Outside every area**");
        for finding in outside {
            out.push_str(&format!(
                "\n- {} {} ({}:{})",
                clean_text(
                    finding
                        .get("severity")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                ),
                clean_text(finding.get("title").and_then(Value::as_str).unwrap_or("")),
                crate::lens::coverage_gaps::code_span(
                    finding.get("file").and_then(Value::as_str).unwrap_or("")
                ),
                finding
                    .get("line")
                    .map_or_else(|| "?".into(), Value::to_string)
            ));
        }
    }
    out
}

pub fn add_cluster_keys(findings: &mut [Value], rules: &[GroupByRule]) {
    if rules.is_empty() {
        return;
    }
    for finding in findings {
        if let Some(key) = finding
            .get("file")
            .and_then(Value::as_str)
            .and_then(|file| crate::profile::group_by::cluster_key(file, rules))
        {
            finding["cluster"] = Value::String(key);
        }
    }
}

fn clean_text(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::group_by;
    use serde_json::{json, Value};

    fn rules() -> Vec<GroupByRule> {
        group_by::rules(&json!({"groupBy": ["apps/*/"]})).0
    }
    fn f(file: &str, title: &str) -> Value {
        json!({"file": file, "title": title, "line": 3, "severity": "important"})
    }

    #[test]
    fn no_rules_produce_no_section() {
        assert_eq!(
            cluster_sections(&[f("apps/a/x", "x")], &["apps/a/x".into()], &[]),
            ""
        );
    }

    #[test]
    fn lists_clusters_in_key_order_including_empty_ones_and_outside() {
        let findings = [
            f("apps/z/z.rs", "Zed"),
            f("other.rs", "Outside"),
            f("apps/a/a.rs", "Alpha"),
        ];
        let text = cluster_sections(
            &findings,
            &[
                "apps/z/z.rs".into(),
                "apps/b/b.rs".into(),
                "apps/a/a.rs".into(),
                "other.rs".into(),
            ],
            &rules(),
        );
        assert!(text.find("**`apps/a/`**").unwrap() < text.find("**`apps/b/`**").unwrap());
        assert!(text.find("**`apps/b/`**").unwrap() < text.find("**`apps/z/`**").unwrap());
        assert!(text.contains("- No findings."));
        assert!(text.contains("**Outside every area**"));
        assert!(text.find("Zed").unwrap() < text.find("Outside").unwrap());
    }

    #[test]
    fn markdown_values_are_sanitized_and_code_spans_are_safe() {
        let findings = [f("apps/a/`odd\nfile.rs", "bad\n**title**")];
        let text = cluster_sections(&findings, &["apps/a/`odd\nfile.rs".into()], &rules());
        assert!(text.contains("``apps/a/`oddfile.rs``:3)"));
        assert!(text.contains("bad**title**"));
        assert!(!text.contains("bad\n**title**"));
    }
}
