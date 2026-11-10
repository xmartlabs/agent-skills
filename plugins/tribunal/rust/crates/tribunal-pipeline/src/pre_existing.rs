//! Not from the workflow: findings about code the pull request did not change.
//!
//! A finding is marked [`PRE_EXISTING_CODE`] when every line of the code it
//! quotes is in the file at the reviewed head and none of those lines is one
//! the diff adds. That is a fact about lines, decided without a model, the
//! way SonarQube dates an issue by the last commit of its line. It is not a
//! claim about behaviour: a pull request can break old code by calling it in
//! a new way, and such a finding is marked too. A marked finding does not count
//! toward the verdict and the report lists it apart, for a separate ticket.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{json, Value};
use tribunal_core::js;

/// The finding field the verdict, the verdict basis and the row's `blocking`
/// read to keep a finding about unchanged code from blocking.
pub const PRE_EXISTING_CODE: &str = "preExistingCode";

/// A quoted line shorter than this, once trimmed, is too generic to place.
const MIN_QUOTE_LINE: usize = 8;

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A merged finding's evidence prefixes each source's quote with its lens,
/// `[correctness] `; the prefix is not code, so it is dropped before the
/// quote is looked up.
fn without_lens_tag(line: &str) -> &str {
    let t = line.trim_start();
    if let Some(rest) = t.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            let tag = &rest[..end];
            if !tag.is_empty()
                && tag
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return &rest[end + 1..];
            }
        }
    }
    t
}

/// The lines each file gains in a unified diff, by their number at the head.
#[must_use]
pub fn added_lines(diff: &str) -> HashMap<String, HashSet<u64>> {
    let mut out: HashMap<String, HashSet<u64>> = HashMap::new();
    let mut file: Option<String> = None;
    let mut next: Option<u64> = None;
    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ ") {
            file = path.strip_prefix("b/").map(str::to_owned);
            next = None;
            continue;
        }
        if line.starts_with("--- ") || line.starts_with("diff --git ") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("@@ ") {
            next = rest
                .split_whitespace()
                .find_map(|t| t.strip_prefix('+'))
                .and_then(|t| t.split(',').next())
                .and_then(|n| n.parse().ok());
            continue;
        }
        let (Some(f), Some(n)) = (file.as_ref(), next) else {
            continue;
        };
        if line.starts_with('+') {
            out.entry(f.clone()).or_default().insert(n);
            next = Some(n + 1);
        } else if !line.starts_with('-') && !line.starts_with('\\') {
            next = Some(n + 1);
        }
    }
    out
}

/// Whether every quoted line of `finding` sits, unchanged, in its file at
/// `head_root`. A finding with no usable quote, a file that cannot be read,
/// or a quoted line that is not in the file is not marked.
#[must_use]
pub fn is_pre_existing_code(
    finding: &Value,
    head_root: &Path,
    added: &HashMap<String, HashSet<u64>>,
) -> bool {
    let Some(file) = js::get(finding, "file").and_then(Value::as_str) else {
        return false;
    };
    let evidence = js::get(finding, "evidence")
        .and_then(Value::as_str)
        .unwrap_or("");
    if evidence
        .lines()
        .all(|l| l.trim().is_empty() || without_lens_tag(l).contains("No relevant quotes found"))
    {
        return false;
    }
    let quoted: Vec<String> = evidence
        .lines()
        .map(|l| norm(without_lens_tag(l)))
        .filter(|l| l.len() >= MIN_QUOTE_LINE && !l.contains("No relevant quotes found"))
        .collect();
    if quoted.is_empty() {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(head_root.join(file)) else {
        return false;
    };
    let head: Vec<String> = text.lines().map(norm).collect();
    let changed = added.get(file);
    quoted.iter().all(|q| {
        let hits: Vec<u64> = head
            .iter()
            .enumerate()
            .filter(|(_, h)| h.contains(q.as_str()))
            .map(|(i, _)| i as u64 + 1)
            .collect();
        !hits.is_empty() && !hits.iter().any(|n| changed.is_some_and(|c| c.contains(n)))
    })
}

/// Marks every kept result whose finding is about unchanged code, and
/// returns how many it marked.
pub fn mark_pre_existing(surviving: &mut [Value], head_root: &Path, diff: &str) -> usize {
    let added = added_lines(diff);
    let mut marked = 0;
    for r in surviving.iter_mut() {
        let is = js::get(r, "finding").is_some_and(|f| is_pre_existing_code(f, head_root, &added));
        if is {
            if let Some(Value::Object(f)) = r.get_mut("finding") {
                f.insert(PRE_EXISTING_CODE.into(), json!(true));
                marked += 1;
            }
        }
    }
    marked
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/src/a.ts b/src/a.ts\n--- a/src/a.ts\n+++ b/src/a.ts\n@@ -1,3 +1,4 @@\n const x = 1;\n+const added = compute(x);\n const old = legacy(x);\n-gone();\n const tail = 2;\n";

    #[test]
    fn added_lines_counts_head_line_numbers() {
        let a = added_lines(DIFF);
        assert_eq!(a["src/a.ts"], HashSet::from([2]));
    }

    fn head(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "tribunal-pre-existing-{}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/a.ts"),
            "const x = 1;\nconst added = compute(x);\nconst old = legacy(x);\nconst tail = 2;\n",
        )
        .unwrap();
        std::fs::write(root.join("src/new.ts"), "export const fresh = build();\n").unwrap();
        root
    }

    fn finding(file: &str, evidence: &str) -> Value {
        json!({"file": file, "line": 3, "evidence": evidence})
    }

    #[test]
    fn a_quote_on_unchanged_lines_is_pre_existing_code() {
        let a = added_lines(DIFF);
        let root = head("a_quote_on_u");
        assert!(is_pre_existing_code(
            &finding("src/a.ts", "const old = legacy(x);"),
            &root,
            &a
        ));
        assert!(is_pre_existing_code(
            &finding("src/a.ts", "const x = 1;\n   const   old = legacy(x);"),
            &root,
            &a
        ));
    }

    #[test]
    fn a_quote_touching_an_added_line_or_missing_is_not_marked() {
        let a = added_lines(DIFF);
        let root = head("a_quote_touc");
        assert!(!is_pre_existing_code(
            &finding("src/a.ts", "const added = compute(x);"),
            &root,
            &a
        ));
        assert!(!is_pre_existing_code(
            &finding(
                "src/a.ts",
                "const old = legacy(x);\nconst added = compute(x);"
            ),
            &root,
            &a
        ));
        assert!(!is_pre_existing_code(
            &finding("src/a.ts", "const nowhere = absent();"),
            &root,
            &a
        ));
        assert!(!is_pre_existing_code(
            &finding("src/a.ts", "No relevant quotes found"),
            &root,
            &a
        ));
        assert!(!is_pre_existing_code(&finding("src/a.ts", "x;"), &root, &a));
        assert!(!is_pre_existing_code(
            &finding("src/gone.ts", "const old = legacy(x);"),
            &root,
            &a
        ));
    }

    #[test]
    fn a_merged_finding_is_judged_on_its_quotes_without_the_lens_tags() {
        let a = added_lines(DIFF);
        let root = head("merged");
        assert!(is_pre_existing_code(
            &finding(
                "src/a.ts",
                "[correctness] const old = legacy(x);\n[scenario-question] const tail = 2;"
            ),
            &root,
            &a
        ));
        assert!(!is_pre_existing_code(
            &finding(
                "src/a.ts",
                "[correctness] const old = legacy(x);\n[security] const added = compute(x);"
            ),
            &root,
            &a
        ));
        assert!(is_pre_existing_code(
            &finding(
                "src/a.ts",
                "[correctness] const old = legacy(x);\n[security] No relevant quotes found"
            ),
            &root,
            &a
        ));
        assert!(!is_pre_existing_code(
            &finding(
                "src/a.ts",
                "[correctness] No relevant quotes found\n[security] No relevant quotes found"
            ),
            &root,
            &a
        ));
    }

    #[test]
    fn a_new_file_has_no_pre_existing_code() {
        let diff = "diff --git a/src/new.ts b/src/new.ts\nnew file mode 100644\n--- /dev/null\n+++ b/src/new.ts\n@@ -0,0 +1 @@\n+export const fresh = build();\n";
        let root = head("a_new_file_h");
        assert!(!is_pre_existing_code(
            &finding("src/new.ts", "export const fresh = build();"),
            &root,
            &added_lines(diff)
        ));
    }

    #[test]
    fn mark_pre_existing_marks_only_the_unchanged_code_finding() {
        let root = head("mark_pre_exi");
        let mut surviving = vec![
            json!({"survived": true, "finding": finding("src/a.ts", "const old = legacy(x);")}),
            json!({"survived": true, "finding": finding("src/a.ts", "const added = compute(x);")}),
        ];
        assert_eq!(mark_pre_existing(&mut surviving, &root, DIFF), 1);
        assert_eq!(surviving[0]["finding"][PRE_EXISTING_CODE], json!(true));
        assert!(surviving[1]["finding"].get(PRE_EXISTING_CODE).is_none());
    }
}
