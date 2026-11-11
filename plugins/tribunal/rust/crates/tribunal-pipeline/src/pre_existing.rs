//! Not from the workflow: findings about code the pull request did not change.
//!
//! A finding is marked [`PRE_EXISTING_CODE`] when every line of the code it
//! quotes is in the file at the reviewed head and none of those lines is one
//! the diff adds. That is a fact about lines, decided without a model, the
//! way SonarQube dates an issue by the last commit of its line. It is not a
//! claim about behaviour, so one case is taken out: when a line the pull
//! request adds refers to the top-level declaration that holds the quote,
//! the pull request newly calls that code and the finding is not marked
//! ([`NEWLY_CALLED_CODE`] says so instead). A reference from another file
//! counts only when it binds to that declaration through an import or a
//! `require`; one from the same file, by name. A pull request can still break
//! old code without calling it anew (by changing what a function it calls
//! does, or by fixing one copy of a pattern and not another), and such a
//! finding is marked. A marked finding does not count toward the verdict and
//! the report lists it apart, for a separate ticket.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{json, Value};
use tribunal_context::ast_bindings::binding_refs;
use tribunal_context::blast_radius::{declarations, has_word, is_test, read_aliases};
use tribunal_context::import_adjacency::source_ext;
use tribunal_core::js;

/// The finding field the verdict, the verdict basis and the row's `blocking`
/// read to keep a finding about unchanged code from blocking.
pub const PRE_EXISTING_CODE: &str = "preExistingCode";

/// Set instead of [`PRE_EXISTING_CODE`] on a finding whose quoted lines are
/// all unchanged but whose code the pull request newly calls.
pub const NEWLY_CALLED_CODE: &str = "newlyCalledCode";

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
    unchanged_quote(finding, head_root, added).is_some()
}

/// The file, its text at the head, and where each quoted line sits there (the
/// copy nearest the finding's own line), when every quoted line is in the file
/// and none is a line the diff adds.
fn unchanged_quote(
    finding: &Value,
    head_root: &Path,
    added: &HashMap<String, HashSet<u64>>,
) -> Option<(String, String, Vec<u64>)> {
    let file = js::get(finding, "file").and_then(Value::as_str)?;
    // A merged finding keeps each source's quote apart. Only its joined
    // `evidence` carries `[lens] ` tags, read when a row predates the field.
    let lines: Vec<&str> =
        match js::get(finding, crate::verify::EVIDENCE_BY_SOURCE).and_then(Value::as_array) {
            Some(sources) => sources
                .iter()
                .filter_map(|e| js::get(e, "evidence").and_then(Value::as_str))
                .flat_map(str::lines)
                .collect(),
            None => {
                let evidence = js::get(finding, "evidence")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .lines();
                if js::get(finding, "mergedFrom").is_some() {
                    evidence.map(without_lens_tag).collect()
                } else {
                    evidence.collect()
                }
            }
        };
    if lines
        .iter()
        .all(|l| l.trim().is_empty() || l.contains("No relevant quotes found"))
    {
        return None;
    }
    let quoted: Vec<String> = lines
        .iter()
        .map(|l| norm(l))
        .filter(|l| l.len() >= MIN_QUOTE_LINE && !l.contains("No relevant quotes found"))
        .collect();
    if quoted.is_empty() {
        return None;
    }
    let text = std::fs::read_to_string(head_root.join(file)).ok()?;
    let head: Vec<String> = text.lines().map(norm).collect();
    let changed = added.get(file);
    let near = js::get(finding, "line")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut at = Vec::new();
    for q in &quoted {
        let hits: Vec<u64> = head
            .iter()
            .enumerate()
            .filter(|(_, h)| h.contains(q.as_str()))
            .map(|(i, _)| i as u64 + 1)
            .collect();
        if hits.is_empty() || hits.iter().any(|n| changed.is_some_and(|c| c.contains(n))) {
            return None;
        }
        at.extend(hits.iter().min_by_key(|n| n.abs_diff(near)));
    }
    Some((file.to_owned(), text, at))
}

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with("/*") || t.starts_with('*')
}

/// Whether a line the diff adds, outside test files, refers to a top-level
/// declaration of `file` that holds one of the lines at `at`: through an
/// import or a `require` bound to it from another file, or by name from
/// elsewhere in `file`.
#[must_use]
pub fn newly_called(
    file: &str,
    text: &str,
    at: &[u64],
    head_root: &Path,
    added: &HashMap<String, HashSet<u64>>,
) -> bool {
    let decls = declarations(text);
    let holding: Vec<_> = decls
        .iter()
        .filter(|d| at.iter().any(|&n| d.at as u64 <= n && n <= d.end as u64))
        .collect();
    if holding.is_empty() {
        return false;
    }
    let tsconfig = std::fs::read_to_string(head_root.join("tsconfig.json")).ok();
    let aliases = read_aliases(tsconfig.as_deref());
    let own: Vec<&str> = text.split('\n').collect();
    for (other, lines) in added {
        if is_test(other) || source_ext(other).is_none() {
            continue;
        }
        if other == file {
            let hit = holding.iter().any(|d| {
                lines.iter().any(|&n| {
                    let n = n as usize;
                    (n < d.at || n > d.end)
                        && own
                            .get(n - 1)
                            .is_some_and(|l| !is_comment(l) && has_word(l, &d.name))
                })
            });
            if hit {
                return true;
            }
            continue;
        }
        let Ok(content) = std::fs::read_to_string(head_root.join(other)) else {
            continue;
        };
        for d in holding.iter().filter(|d| d.exported) {
            let bound = binding_refs(other, &content, file, &d.export_as, &aliases);
            if bound.is_some_and(|b| b.refs.iter().any(|r| lines.contains(&(r.line as u64)))) {
                return true;
            }
        }
    }
    false
}

/// What [`mark_pre_existing`] did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Marked {
    /// Findings marked [`PRE_EXISTING_CODE`].
    pub pre_existing: usize,
    /// Findings on unchanged lines left unmarked because the pull request
    /// newly calls their code ([`NEWLY_CALLED_CODE`]).
    pub newly_called: usize,
}

/// Marks every kept result whose finding is about unchanged code the pull
/// request does not newly call, and counts what it marked and spared.
pub fn mark_pre_existing(surviving: &mut [Value], head_root: &Path, diff: &str) -> Marked {
    let added = added_lines(diff);
    let mut out = Marked::default();
    for r in surviving.iter_mut() {
        let Some(found) = js::get(r, "finding").and_then(|f| unchanged_quote(f, head_root, &added))
        else {
            continue;
        };
        let (file, text, at) = found;
        let field = if newly_called(&file, &text, &at, head_root, &added) {
            out.newly_called += 1;
            NEWLY_CALLED_CODE
        } else {
            out.pre_existing += 1;
            PRE_EXISTING_CODE
        };
        if let Some(Value::Object(f)) = r.get_mut("finding") {
            f.insert(field.into(), json!(true));
        }
    }
    out
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
            &merged(
                "src/a.ts",
                "[correctness] const old = legacy(x);\n[scenario-question] const tail = 2;"
            ),
            &root,
            &a
        ));
        assert!(!is_pre_existing_code(
            &merged(
                "src/a.ts",
                "[correctness] const old = legacy(x);\n[security] const added = compute(x);"
            ),
            &root,
            &a
        ));
        assert!(is_pre_existing_code(
            &merged(
                "src/a.ts",
                "[correctness] const old = legacy(x);\n[security] No relevant quotes found"
            ),
            &root,
            &a
        ));
        assert!(!is_pre_existing_code(
            &merged(
                "src/a.ts",
                "[correctness] No relevant quotes found\n[security] No relevant quotes found"
            ),
            &root,
            &a
        ));
    }

    fn merged(file: &str, evidence: &str) -> Value {
        let mut f = finding(file, evidence);
        f["mergedFrom"] = json!(["correctness", "security"]);
        f
    }

    const BRACKET_DIFF: &str = "diff --git a/src/b.ts b/src/b.ts\n--- a/src/b.ts\n+++ b/src/b.ts\n@@ -1,2 +1,3 @@\n const x = 1;\n+[second] = pair(x);\n [first] = pair(x);\n";

    fn bracket_head(name: &str) -> std::path::PathBuf {
        let root = head(name);
        std::fs::write(
            root.join("src/b.ts"),
            "const x = 1;\n[second] = pair(x);\n[first] = pair(x);\n",
        )
        .unwrap();
        root
    }

    #[test]
    fn a_quote_that_opens_with_a_bracket_keeps_it() {
        let a = added_lines(BRACKET_DIFF);
        let root = bracket_head("bracket");
        assert!(is_pre_existing_code(
            &finding("src/b.ts", "[first] = pair(x);"),
            &root,
            &a
        ));
        let mut f = merged(
            "src/b.ts",
            "[correctness] const x = 1;\n[first] = pair(x);\n[security] No relevant quotes found",
        );
        f[crate::verify::EVIDENCE_BY_SOURCE] = json!([
            {"lens": "correctness", "evidence": "const x = 1;\n[first] = pair(x);"},
            {"lens": "security", "evidence": "No relevant quotes found"},
        ]);
        assert!(is_pre_existing_code(&f, &root, &a));
        // Without the field the tags cannot be told from code: the second
        // line loses `[first]` and its rest is also on the added line.
        f.as_object_mut()
            .unwrap()
            .shift_remove(crate::verify::EVIDENCE_BY_SOURCE);
        assert!(!is_pre_existing_code(&f, &root, &a));
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
        assert_eq!(
            mark_pre_existing(&mut surviving, &root, DIFF).pre_existing,
            1
        );
        assert_eq!(surviving[0]["finding"][PRE_EXISTING_CODE], json!(true));
        assert!(surviving[1]["finding"].get(PRE_EXISTING_CODE).is_none());
    }

    /// A diff that adds exactly these head lines, one hunk per line.
    fn adds(lines: &[(&str, u64)]) -> String {
        let mut d = String::new();
        for (file, n) in lines {
            d.push_str(&format!(
                "diff --git a/{file} b/{file}\n--- a/{file}\n+++ b/{file}\n@@ -{n},0 +{n},1 @@\n+added\n"
            ));
        }
        d
    }

    fn caller_head(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "tribunal-pre-existing-{}-{name}",
            std::process::id()
        ));
        let files = [
            (
                "src/svc.ts",
                "export const helper = () => 1;\n\nexport const legacy = async (x) => {\n  const old = await fetchOld(x);\n  return old;\n};\n\nexport const again = () => legacy(2);\n",
            ),
            (
                "src/caller.ts",
                "import { legacy } from './svc';\n\nexport const run = async () => {\n  await legacy(1);\n};\n",
            ),
            (
                "src/caller.test.ts",
                "import { legacy } from './svc';\n\nit('runs', async () => {\n  await legacy(1);\n});\n",
            ),
            (
                "src/other.ts",
                "import { legacy } from './elsewhere';\n\nexport const go = () => {\n  legacy(3);\n};\n",
            ),
            (
                "src/old.js",
                "exports.legacyJs = function (x) {\n  const old = fetchOld(x);\n  return old;\n};\n",
            ),
            (
                "src/user.js",
                "const { legacyJs } = require('./old');\nlegacyJs(1);\n",
            ),
        ];
        for (f, body) in files {
            std::fs::create_dir_all(root.join(f).parent().unwrap()).unwrap();
            std::fs::write(root.join(f), body).unwrap();
        }
        root
    }

    fn kept(file: &str, evidence: &str, line: u64) -> Value {
        json!({"survived": true, "finding": {"file": file, "line": line, "evidence": evidence}})
    }

    fn outcome(root: &Path, finding: Value, diff: &str) -> (Marked, Value) {
        let mut s = vec![finding];
        let m = mark_pre_existing(&mut s, root, diff);
        (m, s.remove(0)["finding"].clone())
    }

    #[test]
    fn unchanged_code_the_pull_request_newly_calls_is_not_marked() {
        let root = caller_head("new_caller");
        let quote = || kept("src/svc.ts", "const old = await fetchOld(x);", 4);
        // An added line in another file, bound to `legacy` by its import.
        let (m, f) = outcome(&root, quote(), &adds(&[("src/caller.ts", 4)]));
        assert_eq!(
            m,
            Marked {
                pre_existing: 0,
                newly_called: 1
            }
        );
        assert_eq!(f[NEWLY_CALLED_CODE], json!(true));
        assert!(f.get(PRE_EXISTING_CODE).is_none());
        // An added line elsewhere in the same file that names it.
        let (m, _) = outcome(&root, quote(), &adds(&[("src/svc.ts", 8)]));
        assert_eq!(m.newly_called, 1);
        // A `require` in a CommonJS file binds as an import does.
        let js = kept("src/old.js", "const old = fetchOld(x);", 2);
        let (m, _) = outcome(&root, js, &adds(&[("src/user.js", 2)]));
        assert_eq!(m.newly_called, 1);
    }

    #[test]
    fn a_call_that_is_old_in_a_test_or_bound_elsewhere_keeps_the_mark() {
        let root = caller_head("old_caller");
        let quote = || kept("src/svc.ts", "const old = await fetchOld(x);", 4);
        for diff in [
            // The call to `legacy` is on an unchanged line.
            adds(&[("src/caller.ts", 2)]),
            // A test file's call is not the pull request calling the code.
            adds(&[("src/caller.test.ts", 4)]),
            // `legacy` here comes from another module.
            adds(&[("src/other.ts", 4)]),
            // A line added to the declaration that holds the quote.
            adds(&[("src/svc.ts", 5)]),
        ] {
            let (m, f) = outcome(&root, quote(), &diff);
            assert_eq!(
                m,
                Marked {
                    pre_existing: 1,
                    newly_called: 0
                },
                "{diff}"
            );
            assert_eq!(f[PRE_EXISTING_CODE], json!(true));
            assert!(f.get(NEWLY_CALLED_CODE).is_none());
        }
        // `helper` holds no quoted line, so calling it spares nothing.
        let mut s = vec![kept("src/svc.ts", "const old = await fetchOld(x);", 4)];
        std::fs::write(
            root.join("src/caller.ts"),
            "import { legacy, helper } from './svc';\n\nexport const run = async () => {\n  helper();\n};\n",
        )
        .unwrap();
        assert_eq!(
            mark_pre_existing(&mut s, &root, &adds(&[("src/caller.ts", 4)])).pre_existing,
            1
        );
    }
}
