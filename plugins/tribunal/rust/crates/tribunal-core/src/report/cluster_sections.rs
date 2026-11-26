use crate::profile::group_by::GroupByRule;
use serde_json::Value;

#[must_use]
pub fn cluster_sections(findings: &[Value], pr_files: &[String], rules: &[GroupByRule]) -> String {
    if rules.is_empty() {
        return String::new();
    }

    // Every area the PR touches, plus any area a finding names, so no
    // finding falls out of the section.
    let mut clusters: Vec<String> = pr_files
        .iter()
        .filter_map(|path| crate::profile::group_by::cluster_key(path, rules))
        .collect();
    clusters.extend(
        findings
            .iter()
            .filter_map(|finding| finding_cluster(finding, rules)),
    );
    clusters.sort();
    clusters.dedup();
    if clusters.is_empty() && findings.is_empty() {
        return String::new();
    }

    let mut out = String::from("\n\n---\n\n**Findings by area.**");
    let mut empty_clusters = Vec::new();
    for cluster in clusters {
        let matched: Vec<&Value> = findings
            .iter()
            .filter(|finding| finding_cluster(finding, rules).as_deref() == Some(cluster.as_str()))
            .collect();
        if matched.is_empty() {
            empty_clusters.push(cluster);
            continue;
        }
        out.push_str(&format!(
            "\n\n**{}**",
            crate::lens::coverage_gaps::code_span(&cluster)
        ));
        for finding in matched {
            out.push_str(&finding_line(finding));
        }
    }
    if !empty_clusters.is_empty() {
        let keys = empty_clusters
            .iter()
            .map(|cluster| crate::lens::coverage_gaps::code_span(cluster))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "\n\nNo findings in {} area(s): {keys}",
            empty_clusters.len()
        ));
    }

    let outside: Vec<&Value> = findings
        .iter()
        .filter(|finding| finding_cluster(finding, rules).is_none())
        .collect();
    if !outside.is_empty() {
        out.push_str("\n\n**Outside every area**");
        for finding in outside {
            out.push_str(&finding_line(finding));
        }
    }
    out
}

fn finding_cluster(finding: &Value, rules: &[GroupByRule]) -> Option<String> {
    finding
        .get("file")
        .and_then(Value::as_str)
        .and_then(|file| crate::profile::group_by::cluster_key(file, rules))
}

/// The findings twice: as the synthesizer gets them, without cluster keys,
/// and with the keys the run data and the area section read. Keeping the
/// first copy free of keys leaves the report prompt as it was.
#[must_use]
pub fn split_for_report(findings: &[Value], rules: &[GroupByRule]) -> (Vec<Value>, Vec<Value>) {
    let mut keyed = findings.to_vec();
    add_cluster_keys(&mut keyed, rules);
    (findings.to_vec(), keyed)
}

pub fn add_cluster_keys(findings: &mut [Value], rules: &[GroupByRule]) {
    if rules.is_empty() {
        return;
    }
    for finding in findings {
        if let Some(key) = finding_cluster(finding, rules) {
            finding["cluster"] = Value::String(key);
        }
    }
}

fn clean_text(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

fn escape_markdown(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in clean_text(text).chars() {
        // `@` would mention a user or team when the review is posted.
        if "\\`*_[]()<>!#|@".contains(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn finding_line_number(finding: &Value) -> String {
    match finding.get("line") {
        Some(Value::Number(number)) if number.as_u64().is_some_and(|line| line > 0) => {
            number.to_string()
        }
        Some(Value::String(line))
            if !line.is_empty()
                && line.bytes().all(|byte| byte.is_ascii_digit())
                && line.bytes().any(|byte| byte != b'0') =>
        {
            line.clone()
        }
        _ => "?".to_owned(),
    }
}

/// One finding as a list line: severity, title and, when it has a file, its
/// location. Model text loses control characters and has its Markdown
/// escaped; the path is a code span and the line a number or `?`.
fn finding_line(finding: &Value) -> String {
    let text = |key: &str| escape_markdown(finding.get(key).and_then(Value::as_str).unwrap_or(""));
    let mut line = format!("\n- {} {}", text("severity"), text("title"));
    if let Some(file) = finding
        .get("file")
        .and_then(Value::as_str)
        .filter(|f| !f.is_empty())
    {
        let at = finding_line_number(finding);
        line.push_str(&format!(
            " ({}:{at})",
            crate::lens::coverage_gaps::code_span(file)
        ));
    }
    line
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
        assert!(
            text.find("**`apps/a/`**").unwrap()
                < text.find("No findings in 1 area(s): `apps/b/`").unwrap()
        );
        assert!(
            text.find("**`apps/z/`**").unwrap()
                < text.find("No findings in 1 area(s): `apps/b/`").unwrap()
        );
        assert!(text.contains("No findings in 1 area(s): `apps/b/`"));
        assert!(text.contains("**Outside every area**"));
        assert!(text.find("Zed").unwrap() < text.find("Outside").unwrap());
    }

    #[test]
    fn markdown_values_are_sanitized_and_code_spans_are_safe() {
        let findings = [f("apps/a/`odd\nfile.rs", "bad\n**title**")];
        let text = cluster_sections(&findings, &["apps/a/`odd\nfile.rs".into()], &rules());
        assert!(text.contains("``apps/a/`oddfile.rs``:3)"));
        assert!(text.contains(r"bad\*\*title\*\*"));
        assert!(!text.contains("bad\n**title**"));
    }

    #[test]
    fn invalid_finding_lines_render_as_unknown_and_numeric_strings_render_as_lines() {
        let findings = [
            json!({"file": "apps/a/a.rs", "title": "Null", "line": null}),
            json!({"file": "apps/a/b.rs", "title": "String", "line": "12"}),
            json!({"file": "apps/a/c.rs", "title": "Invalid string", "line": "twelve"}),
        ];
        let text = cluster_sections(
            &findings,
            &[
                "apps/a/a.rs".into(),
                "apps/a/b.rs".into(),
                "apps/a/c.rs".into(),
            ],
            &rules(),
        );

        assert!(text.contains("apps/a/a.rs`:?)"), "{text}");
        assert!(text.contains("apps/a/b.rs`:12)"), "{text}");
        assert!(text.contains("apps/a/c.rs`:?)"), "{text}");
        assert!(
            !text.contains(":null") && !text.contains(":\"12\""),
            "{text}"
        );
    }

    #[test]
    fn a_mention_in_a_title_is_escaped() {
        let findings = [json!({
            "file": "apps/a/a.rs",
            "title": "breaks @acme/security token check",
            "severity": "important",
            "line": 3
        })];
        let text = cluster_sections(&findings, &["apps/a/a.rs".into()], &rules());

        assert!(text.contains(r"breaks \@acme/security"), "{text}");
        assert!(!text.contains(" @acme"), "{text}");
    }

    #[test]
    fn no_section_when_the_pr_touches_no_area_and_has_no_findings() {
        assert_eq!(
            cluster_sections(&[], &["docs/README.md".into()], &rules()),
            ""
        );
    }

    #[test]
    fn model_written_markdown_is_escaped_in_severity_and_title() {
        let findings = [json!({
            "file": "apps/a/a.rs",
            "title": "See [here](http://x) ![i](y) <b>",
            "severity": "*critical*",
            "line": 12
        })];
        let text = cluster_sections(&findings, &["apps/a/a.rs".into()], &rules());

        assert!(
            text.contains(r"\*critical\* See \[here\]\(http://x\) \!\[i\]\(y\) \<b\>"),
            "{text}"
        );
    }

    #[test]
    fn added_cluster_key_matches_the_cluster_section() {
        let mut findings = [f("apps/a/a.rs", "A")];
        add_cluster_keys(&mut findings, &rules());
        let section = cluster_sections(&findings, &["apps/a/a.rs".into()], &rules());

        assert_eq!(findings[0]["cluster"], "apps/a/");
        assert!(section.contains(&format!(
            "**`{}`**",
            findings[0]["cluster"].as_str().unwrap()
        )));
    }
}
