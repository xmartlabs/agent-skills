//! Siblings of what a PR changed: code outside the PR that shares a callee
//! with the changed lines, or writes a model they write. The completeness
//! critic is handed these so it can ask whether the same defect, or the same
//! fix, applies there. Found by scanning the diff and running `git grep` at
//! the pinned head, so the list is a heuristic and says so.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::blast_radius::is_test;
use crate::exec::Tools;

const CODE_GLOBS: [&str; 6] = ["*.ts", "*.tsx", "*.js", "*.jsx", "*.mjs", "*.cjs"];
const WRITE_OPS: [&str; 6] = [
    "create",
    "createMany",
    "update",
    "updateMany",
    "upsert",
    "deleteMany",
];
const DB_HANDLES: [&str; 5] = ["prisma", "tx", "client", "trx", "db"];
const MIN_CALLEE_LEN: usize = 6;
/// A callee called from more files than this is plumbing, not a sibling.
const MAX_SITES_PER_CALLEE: usize = 25;
const SITES_PER_NAME: usize = 3;
const MAX_CALLEES: usize = 12;
const MAX_CALLEE_SIBLINGS: usize = 6;
const MAX_MODELS: usize = 4;
const MAX_MODEL_SIBLINGS: usize = 8;
const SITES_PER_MODEL: usize = 4;
const TEXT_CHARS: usize = 100;

const STOP: [&str; 24] = [
    "function",
    "return",
    "require",
    "import",
    "export",
    "switch",
    "catch",
    "typeof",
    "String",
    "Number",
    "Boolean",
    "Array",
    "Object",
    "Promise",
    "parseInt",
    "parseFloat",
    "isArray",
    "isNaN",
    "setTimeout",
    "describe",
    "expect",
    "console",
    "Error",
    "Date",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sibling {
    pub file: String,
    pub line: u32,
    /// `shared-callee` or `same-model-write`.
    pub kind: &'static str,
    pub via: String,
    pub text: String,
}

/// What the changed lines call and write.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Symbols {
    /// Called names, in order of first appearance.
    pub callees: Vec<String>,
    /// `(model, operation)` of each write the PR adds.
    pub writes: Vec<(String, String)>,
    /// Non-test code files the diff touches.
    pub files: Vec<String>,
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

fn is_code_file(p: &str) -> bool {
    p.rsplit('.')
        .next()
        .is_some_and(|e| matches!(e, "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs"))
}

/// Names followed by `(` that are not a member access, with the names the
/// line declares itself.
fn calls_and_decls(line: &str) -> (Vec<String>, Vec<String>) {
    let b = line.as_bytes();
    let mut calls = Vec::new();
    let mut decls = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if is_word(b[i]) && (i == 0 || !is_word(b[i - 1])) {
            let start = i;
            while i < b.len() && is_word(b[i]) {
                i += 1;
            }
            let name = &line[start..i];
            let member = start > 0 && b[start - 1] == b'.';
            if i < b.len() && b[i] == b'(' && !member && !b[0].is_ascii_digit() {
                calls.push(name.to_owned());
            }
            if matches!(name, "function" | "const" | "let" | "class") {
                let rest = line[i..].trim_start();
                let n: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
                    .collect();
                if !n.is_empty() {
                    decls.push(n);
                }
            }
        } else {
            i += 1;
        }
    }
    (calls, decls)
}

/// `<handle>.<model>.<op>(` in a line.
fn writes_in(line: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for handle in DB_HANDLES {
        let mut from = 0;
        while let Some(at) = line[from..].find(&format!("{handle}.")) {
            let start = from + at;
            from = start + handle.len() + 1;
            if start > 0
                && (is_word(line.as_bytes()[start - 1]) || line.as_bytes()[start - 1] == b'.')
            {
                continue;
            }
            let rest = &line[from..];
            let model: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            let after = &rest[model.len()..];
            let Some(op_rest) = after.strip_prefix('.') else {
                continue;
            };
            let op: String = op_rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if !model.is_empty()
                && WRITE_OPS.contains(&op.as_str())
                && op_rest[op.len()..].starts_with('(')
            {
                out.push((model, op));
            }
        }
    }
    out
}

/// Reads a unified diff.
#[must_use]
pub fn changed_symbols(diff: &str) -> Symbols {
    let mut out = Symbols::default();
    let mut live = false;
    let mut declared: HashSet<String> = HashSet::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut ordered: Vec<String> = Vec::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("+++ b/") {
            live = is_code_file(rest) && !is_test(rest);
            if live && !out.files.iter().any(|f| f == rest) {
                out.files.push(rest.to_owned());
            }
            continue;
        }
        if line.starts_with("+++") || line.starts_with("---") || !live {
            continue;
        }
        let (sign, body) = match line.as_bytes().first() {
            Some(b'+') => ('+', &line[1..]),
            Some(b'-') => ('-', &line[1..]),
            _ => continue,
        };
        let trimmed = body.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') || trimmed.starts_with("/*") {
            continue;
        }
        let (calls, decls) = calls_and_decls(body);
        if sign == '+' {
            declared.extend(decls);
            for w in writes_in(body) {
                if !out.writes.contains(&w) {
                    out.writes.push(w);
                }
            }
        }
        for c in calls {
            if c.len() >= MIN_CALLEE_LEN && !STOP.contains(&c.as_str()) && seen.insert(c.clone()) {
                ordered.push(c);
            }
        }
    }
    out.callees = ordered
        .into_iter()
        .filter(|c| !declared.contains(c))
        .collect();
    out
}

fn short(s: &str) -> String {
    let t: String = s.trim().chars().take(TEXT_CHARS).collect();
    t.replace('`', "'")
}

struct Hit {
    file: String,
    line: u32,
    text: String,
}

fn parse_grep(stdout: &str) -> Vec<Hit> {
    stdout
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(3, ':');
            let file = p.next()?.to_owned();
            let line = p.next()?.parse().ok()?;
            Some(Hit {
                file,
                line,
                text: p.next().unwrap_or("").to_owned(),
            })
        })
        .collect()
}

async fn grep(tools: &Tools, dir: &Path, args: &[&str]) -> Vec<Hit> {
    let mut a: Vec<&str> = vec!["grep", "-n", "-I"];
    a.extend_from_slice(args);
    a.push("--");
    a.extend(CODE_GLOBS);
    match tools.git(&a, Some(dir)).await {
        Ok(o) if o.code == Some(0) => parse_grep(&o.stdout),
        _ => Vec::new(),
    }
}

fn is_update_op(op: &str) -> bool {
    matches!(op, "update" | "updateMany" | "upsert")
}

/// The write operation on a `.<model>.<op>(` line.
fn op_of(text: &str, model: &str) -> String {
    let key = format!(".{model}.");
    text.find(&key)
        .map(|i| {
            text[i + key.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect()
        })
        .unwrap_or_default()
}

/// Directory steps between `file` and the nearest file the PR changes: 0 for
/// the same directory. Code that lives next to the change is where the same
/// defect is most likely to repeat, and where a reader looks first.
fn distance(file: &str, pr_files: &[String]) -> usize {
    let dir = |p: &str| -> Vec<String> {
        let mut v: Vec<String> = p.split('/').map(str::to_owned).collect();
        v.pop();
        v
    };
    let a = dir(file);
    pr_files
        .iter()
        .map(|f| {
            let b = dir(f);
            let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
            a.len() + b.len() - 2 * common
        })
        .min()
        .unwrap_or(usize::MAX)
}

fn outside(h: &Hit, pr_files: &[String]) -> bool {
    !is_test(&h.file) && !pr_files.contains(&h.file)
}

/// Siblings of `symbols` at the head checked out in `dir`.
pub async fn find(
    tools: &Tools,
    dir: &Path,
    symbols: &Symbols,
    pr_files: &[String],
) -> Vec<Sibling> {
    let mut found: Vec<(usize, Vec<Sibling>)> = Vec::new();
    for name in symbols.callees.iter().take(MAX_CALLEES) {
        let call = format!("{name}(");
        let mut sites: Vec<Sibling> = grep(tools, dir, &["-w", "-F", "-e", name])
            .await
            .into_iter()
            .filter(|h| outside(h, pr_files) && h.text.contains(&call))
            .filter(|h| {
                let t = h.text.trim_start();
                !(t.starts_with("function ")
                    || t.starts_with("export function ")
                    || t.starts_with("export async function ")
                    || t.starts_with("async function ")
                    || t.contains(&format!("const {name} =")))
            })
            .map(|h| Sibling {
                file: h.file,
                line: h.line,
                kind: "shared-callee",
                via: name.clone(),
                text: short(&h.text),
            })
            .collect();
        let n_files = sites
            .iter()
            .map(|s| s.file.as_str())
            .collect::<HashSet<_>>()
            .len();
        if sites.is_empty() || n_files > MAX_SITES_PER_CALLEE {
            continue;
        }
        sites.sort_by_key(|x| (distance(&x.file, pr_files), x.file.clone(), x.line));
        let best = distance(&sites[0].file, pr_files);
        sites.truncate(SITES_PER_NAME);
        found.push((best * 1000 + n_files, sites));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1[0].via.cmp(&b.1[0].via)));
    let mut out: Vec<Sibling> = found.into_iter().flat_map(|(_, s)| s).collect();
    out.truncate(MAX_CALLEE_SIBLINGS);

    let mut by_model: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for (m, op) in &symbols.writes {
        if !by_model.contains_key(m.as_str()) {
            order.push(m);
        }
        by_model.entry(m).or_default().push(op);
    }
    let mut per_model: Vec<(usize, Vec<Sibling>)> = Vec::new();
    for model in order.into_iter().take(MAX_MODELS) {
        let pattern = format!(r"\.{model}\.({})\(", WRITE_OPS.join("|"));
        let mut sites: Vec<Sibling> = grep(tools, dir, &["-E", "-e", &pattern])
            .await
            .into_iter()
            .filter(|h| outside(h, pr_files))
            .map(|h| Sibling {
                file: h.file,
                line: h.line,
                kind: "same-model-write",
                via: model.to_owned(),
                text: short(&h.text),
            })
            .collect();
        if sites.is_empty() {
            continue;
        }
        let total = sites.len();
        let pr_updates = symbols
            .writes
            .iter()
            .any(|(m, op)| m == model && is_update_op(op));
        sites.sort_by_key(|x| {
            let same_family = is_update_op(&op_of(&x.text, model)) == pr_updates;
            (
                distance(&x.file, pr_files),
                !same_family,
                x.file.clone(),
                x.line,
            )
        });
        sites.truncate(SITES_PER_MODEL);
        per_model.push((total, sites));
    }
    per_model.sort_by_key(|(total, _)| *total);
    let mut writers: Vec<Sibling> = per_model.into_iter().flat_map(|(_, s)| s).collect();
    writers.truncate(MAX_MODEL_SIBLINGS);
    out.extend(writers);
    out
}

/// One line per sibling, for the critic's prompt.
#[must_use]
pub fn render(siblings: &[Sibling]) -> Vec<String> {
    siblings
        .iter()
        .map(|s| format!("{}:{} [{} {}] `{}`", s.file, s.line, s.kind, s.via, s.text))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/src/pay.ts b/src/pay.ts\n--- a/src/pay.ts\n+++ b/src/pay.ts\n@@ -1,4 +1,8 @@\n-  const account = await getAccountByVid(id);\n+  let account = await getAccountByVid(id);\n+  const survivor = await resolveSurvivingAccount(id);\n+  await tx.aoaRequestsds.updateMany({ where });\n+  await prisma.dealDetail.count({ where });\n+  logger.info('x');\n+  // helperThing(call in a comment)\n+export async function resolveSurvivingAccount(id) {\ndiff --git a/src/pay.test.ts b/src/pay.test.ts\n--- a/src/pay.test.ts\n+++ b/src/pay.test.ts\n@@ -1 +1 @@\n+  await testOnlyHelper(1);\n";

    #[test]
    fn callees_are_unchanged_names_called_on_changed_lines() {
        let s = changed_symbols(DIFF);
        assert_eq!(s.callees, vec!["getAccountByVid"]);
        assert_eq!(s.files, vec!["src/pay.ts"]);
    }

    #[test]
    fn a_name_the_diff_declares_is_not_a_shared_callee() {
        let s = changed_symbols(DIFF);
        assert!(!s.callees.iter().any(|c| c == "resolveSurvivingAccount"));
    }

    #[test]
    fn writes_are_model_and_operation_and_reads_are_ignored() {
        let s = changed_symbols(DIFF);
        assert_eq!(
            s.writes,
            vec![("aoaRequestsds".to_owned(), "updateMany".to_owned())]
        );
    }

    #[test]
    fn a_member_call_and_a_test_file_add_nothing() {
        let s = changed_symbols(DIFF);
        assert!(!s
            .callees
            .iter()
            .any(|c| c == "info" || c == "testOnlyHelper"));
        assert!(!s.callees.iter().any(|c| c == "helperThing"));
    }

    #[test]
    fn render_names_the_edge_and_the_place() {
        let r = render(&[Sibling {
            file: "src/a.ts".into(),
            line: 9,
            kind: "shared-callee",
            via: "getAccountByVid".into(),
            text: "x".into(),
        }]);
        assert_eq!(r, vec!["src/a.ts:9 [shared-callee getAccountByVid] `x`"]);
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    #[tokio::test]
    async fn find_lists_other_callers_and_writers_outside_the_pr() {
        let dir = tempfile_dir();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src/pay.ts"),
            "await getAccountByVid(id);\nawait tx.aoaRequestsds.updateMany({});\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("src/invoice.ts"),
            "const a = await getAccountByVid(contactId);\nexport async function getAccountByVid(x) {}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("src/deals.ts"),
            "await prisma.aoaRequestsds.update({ where });\nawait prisma.aoaRequestsds.findMany({});\n",
        )
        .unwrap();
        std::fs::write(dir.join("src/invoice.test.ts"), "getAccountByVid(1);\n").unwrap();
        git(&dir, &["init", "-q"]);
        git(&dir, &["add", "-A"]);
        let symbols = Symbols {
            callees: vec!["getAccountByVid".into()],
            writes: vec![("aoaRequestsds".into(), "updateMany".into())],
            files: vec!["src/pay.ts".into()],
        };
        let found = find(
            &Tools::default(),
            &dir,
            &symbols,
            &["src/pay.ts".to_owned()],
        )
        .await;
        let got: Vec<(String, &str)> = found
            .iter()
            .map(|s| (format!("{}:{}", s.file, s.line), s.kind))
            .collect();
        assert_eq!(
            got,
            vec![
                ("src/invoice.ts:1".to_owned(), "shared-callee"),
                ("src/deals.ts:1".to_owned(), "same-model-write"),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_nearest_directory_ranks_first() {
        let pr = ["src/controllers/v2/payments.ts".to_owned()];
        assert_eq!(distance("src/controllers/v2/invoice.ts", &pr), 0);
        assert_eq!(distance("src/controllers/legacy/x.js", &pr), 2);
        assert!(
            distance("src/services/v2/deals.ts", &pr) > distance("src/controllers/v2/a.ts", &pr)
        );
    }

    #[tokio::test]
    async fn a_callee_with_many_distant_callers_still_lists_the_near_one() {
        let dir = tempfile_dir();
        std::fs::create_dir_all(dir.join("src/v2")).unwrap();
        std::fs::create_dir_all(dir.join("src/legacy")).unwrap();
        std::fs::write(dir.join("src/v2/pay.ts"), "await lookupAccount(id);\n").unwrap();
        std::fs::write(dir.join("src/v2/invoice.ts"), "await lookupAccount(id);\n").unwrap();
        for i in 0..9 {
            std::fs::write(
                dir.join(format!("src/legacy/c{i}.js")),
                "await lookupAccount(id);\n",
            )
            .unwrap();
        }
        git(&dir, &["init", "-q"]);
        git(&dir, &["add", "-A"]);
        let symbols = Symbols {
            callees: vec!["lookupAccount".into()],
            writes: vec![],
            files: vec!["src/v2/pay.ts".into()],
        };
        let found = find(
            &Tools::default(),
            &dir,
            &symbols,
            &["src/v2/pay.ts".to_owned()],
        )
        .await;
        assert_eq!(found[0].file, "src/v2/invoice.ts");
        assert_eq!(found.len(), SITES_PER_NAME);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tribunal-siblings-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
