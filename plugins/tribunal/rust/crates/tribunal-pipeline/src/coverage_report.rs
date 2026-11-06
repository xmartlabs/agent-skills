//! What a run actually verified, said where a reader can see it.
//!
//! Three things the report and the metrics row did not say before:
//!
//! - which repos each dispatch read outside the reviewed one, and at which
//!   ref (read off the saved transcripts, so it is what ran, not what the
//!   prompt asked for);
//! - that the grounded typecheck did not run, with the reason, instead of a
//!   marker only the metrics row carried;
//! - where the completeness critic lists something as "not checked" that a
//!   lens's own tool calls touched. The critic writes prose, so the match is
//!   by name against the commands the lenses ran and is a heuristic.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Value};
use tribunal_context::python_typecheck;
use tribunal_context::repo_locator::{RepoCwd, ENV_REPO_DIR};

/// One tool call, as text a search can run over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub name: String,
    pub text: String,
}

fn collect_calls(v: &Value, out: &mut Vec<Call>) {
    match v {
        Value::Object(m) => {
            if m.get("type").and_then(Value::as_str) == Some("tool_use") {
                if let Some(input) = m.get("input").and_then(Value::as_object) {
                    let name = m
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let text = input
                        .values()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ");
                    out.push(Call { name, text });
                }
            }
            for x in m.values() {
                collect_calls(x, out);
            }
        }
        Value::Array(a) => {
            for x in a {
                collect_calls(x, out);
            }
        }
        _ => {}
    }
}

/// The tool calls a saved transcript holds. A missing or unreadable file is
/// an empty list: the caller still reports the dispatch, with no reads.
#[must_use]
pub fn calls_in_transcript(path: &Path) -> Vec<Call> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            collect_calls(&v, &mut out);
        }
    }
    out
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("a fixed pattern"))
}

fn var_assignments(command: &str) -> Vec<(String, String)> {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        r"\b([A-Za-z_][A-Za-z0-9_]*)=([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)\b",
    )
    .captures_iter(command)
    .map(|c| (c[1].to_owned(), c[2].to_owned()))
    .collect()
}

fn expand_vars(command: &str) -> String {
    let mut text = command.to_owned();
    for (name, value) in var_assignments(command) {
        text = text
            .replace(&format!("${{{name}}}"), &value)
            .replace(&format!("${name}"), &value);
    }
    text
}

/// `owner/name@ref` of every GitHub repo a call reads, with how many times.
/// A read with no ref in its text is `owner/name@(default)`; a code search
/// is `owner/name@(search)`.
#[must_use]
pub fn repo_reads(calls: &[Call]) -> BTreeMap<String, u64> {
    static PATH: OnceLock<Regex> = OnceLock::new();
    static REF: OnceLock<Regex> = OnceLock::new();
    static TREE: OnceLock<Regex> = OnceLock::new();
    static SEARCH: OnceLock<Regex> = OnceLock::new();
    let path = re(
        &PATH,
        r#"repos/([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)/(?:contents|git|compare|commits|pulls|issues|branches|tags|releases)([^\s"']*)"#,
    );
    let query_ref = re(&REF, r#"[?&]ref=([^&\s"'\\]+)"#);
    let tree_ref = re(&TREE, r#"git/trees/([^/?\s"'\\]+)"#);
    let search = re(&SEARCH, r"repo:([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)");
    let mut out: BTreeMap<String, u64> = BTreeMap::new();
    for call in calls.iter().filter(|c| c.name.eq_ignore_ascii_case("bash")) {
        let text = expand_vars(&call.text);
        for m in path.captures_iter(&text) {
            let reference = query_ref
                .captures(&m[2])
                .or_else(|| tree_ref.captures(&m[0]))
                .map_or("(default)", |r| {
                    r.get(1).map_or("(default)", |g| g.as_str())
                });
            *out.entry(format!("{}@{reference}", &m[1])).or_default() += 1;
        }
        for m in search.captures_iter(&text) {
            *out.entry(format!("{}@(search)", &m[1])).or_default() += 1;
        }
    }
    out
}

/// One dispatch's reads of repos other than `own_repo`, or `None` when it
/// read none.
#[must_use]
pub fn reads_entry(label: &str, kind: &str, calls: &[Call], own_repo: &str) -> Option<Value> {
    let reads: BTreeMap<String, u64> = repo_reads(calls)
        .into_iter()
        .filter(|(k, _)| {
            !k.split('@')
                .next()
                .is_some_and(|r| r.eq_ignore_ascii_case(own_repo))
        })
        .collect();
    if reads.is_empty() {
        return None;
    }
    Some(json!({ "label": label, "kind": kind, "reads": reads }))
}

fn names_path(text: &str, path: &str) -> bool {
    let is_path_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/');
    text.match_indices(path).any(|(at, _)| {
        let starts = text[..at]
            .chars()
            .next_back()
            .is_none_or(|c| c == '/' || !is_path_char(c));
        // A path ends where a non-path character follows, or a full stop that
        // ends a sentence rather than starting an extension.
        let rest = &text[at + path.len()..];
        let ends = match rest.chars().next() {
            None => true,
            Some('.') => !rest[1..].starts_with(|c: char| c.is_ascii_alphanumeric()),
            Some(c) => !is_path_char(c),
        };
        starts && ends
    })
}

/// The files of `files` that some tool call names as a whole path: a Read of
/// `/worktree/src/a.ts`, a `git show sha:src/a.ts`, a `sed -n 1,9p src/a.ts`.
/// The lens's own answer (`StructuredOutput`) is not a read. A Grep or Glob
/// over a directory names no file, so it does not count.
#[must_use]
pub fn files_opened(calls: &[Call], files: &[String]) -> Vec<String> {
    files
        .iter()
        .filter(|f| {
            calls
                .iter()
                .filter(|c| c.name != "StructuredOutput")
                .any(|c| names_path(&c.text, f))
        })
        .cloned()
        .collect()
}

/// A sentence of the critic's answer that says something was not checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotChecked {
    pub text: String,
    pub tokens: Vec<String>,
}

fn is_negative(sentence: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        r"(?i)\b(?:did not|didn't|do not|don't|could not|couldn't|never|not)\s+(?:\w+\s+)?(?:check|read|verif|trace|reach|examin|open|confirm)",
    )
    .is_match(sentence)
}

fn name_tokens(sentence: &str) -> Vec<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    let mut out: Vec<String> = Vec::new();
    for c in re(&R, r"`([^`\n]{3,80})`").captures_iter(sentence) {
        let t = c[1].trim();
        if !t.contains(char::is_whitespace)
            && (t.contains('/') || t.contains('.'))
            && !out.iter().any(|o| o == t)
        {
            out.push(t.to_owned());
        }
    }
    out
}

/// The "I did not check ..." sentences of the critic's answer, with the file
/// and endpoint names each one cites.
#[must_use]
pub fn not_checked_claims(critic_output: &str) -> Vec<NotChecked> {
    critic_output
        .split('\n')
        .flat_map(|l| l.split(". "))
        .map(str::trim)
        .filter(|s| !s.is_empty() && is_negative(s))
        .map(|s| NotChecked {
            text: s.chars().take(300).collect(),
            tokens: name_tokens(s),
        })
        .filter(|n| !n.tokens.is_empty())
        .collect()
}

fn needles(token: &str) -> Vec<String> {
    let t = token
        .trim_matches(|c: char| c == '/' || c == '.' || c == ',' || c == ';' || c == ')')
        .to_lowercase();
    let segs: Vec<&str> = t.split('/').filter(|s| !s.is_empty()).collect();
    let mut out = vec![segs.join("/")];
    if segs.len() >= 3 {
        out.push(segs[segs.len() - 2..].join("/"));
    }
    if let Some(last) = segs.last().filter(|s| s.len() >= 6 && segs.len() >= 2) {
        out.push((*last).to_owned());
    }
    out.retain(|n| n.len() >= 6);
    out.sort();
    out.dedup();
    out
}

/// A name the critic lists as not checked that a lens's commands contain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contradiction {
    pub claim: String,
    pub token: String,
    pub lenses: Vec<String>,
}

/// Matches each cited name against what the lenses ran. `lenses` holds a
/// label and the calls of each lens dispatch.
#[must_use]
pub fn contradictions(claims: &[NotChecked], lenses: &[(String, Vec<Call>)]) -> Vec<Contradiction> {
    let mut out = Vec::new();
    for claim in claims {
        for token in &claim.tokens {
            let ns = needles(token);
            if ns.is_empty() {
                continue;
            }
            let mut who: Vec<String> = Vec::new();
            for (label, calls) in lenses {
                let hit = calls.iter().any(|c| {
                    let t = c.text.to_lowercase();
                    ns.iter().any(|n| t.contains(n.as_str()))
                });
                if hit && !who.contains(label) {
                    who.push(label.clone());
                }
            }
            if !who.is_empty() {
                out.push(Contradiction {
                    claim: claim.text.clone(),
                    token: token.clone(),
                    lenses: who,
                });
            }
        }
    }
    out
}

#[must_use]
pub fn contradiction_banner(found: &[Contradiction]) -> String {
    if found.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = found
        .iter()
        .map(|c| format!("  - `{}`: touched by {}", c.token, c.lenses.join(", ")))
        .collect();
    format!(
        "\n\n---\n\n**Listed as not checked, but a lens ran commands on it.** The completeness critic says it did not check these. A lens's tool calls name them, so someone looked, though this match is by name and does not show what was concluded:\n\n{}",
        lines.join("\n")
    )
}

/// What the typecheck's marker line says in words, or empty when the
/// typecheck ran or was not applicable.
#[must_use]
pub fn typecheck_banner(
    text: Option<&str>,
    repo: &str,
    resolution: &RepoCwd,
    applicable: bool,
) -> String {
    if !applicable {
        return String::new();
    }
    let Some(marker) = text.and_then(|t| {
        t.lines()
            .map(str::trim)
            .find(|l| l.starts_with("DEGRADED_"))
    }) else {
        return String::new();
    };
    let why = match (marker, resolution) {
        ("DEGRADED_WRONG_REPO", RepoCwd::Unresolved { cwd_origin, .. }) => format!(
            "The review ran from {}, not from a checkout of `{repo}`, and no local checkout of it was found. Run it from a checkout of `{repo}` or set `{ENV_REPO_DIR}` to one.",
            cwd_origin
                .as_deref()
                .map_or("a directory with no GitHub origin".to_owned(), |o| format!("`{o}`"))
        ),
        ("DEGRADED_WRONG_REPO", _) => {
            format!("The working directory is not a checkout of `{repo}`.")
        }
        ("DEGRADED_NO_PINNED_TREE", _) => {
            "The reviewed commit could not be cloned, and the typecheck does not write to the checkout it borrows packages from.".to_owned()
        }
        ("DEGRADED_NO_WORKTREE", _) => {
            "A worktree at the reviewed commit could not be made.".to_owned()
        }
        ("DEGRADED_SCHEMA_DRIFT", _) => "The Prisma schema at the reviewed commit differs from the checkout's, so type errors would not come from the PR.".to_owned(),
        ("DEGRADED_NO_CD", _) => "The worktree directory could not be entered.".to_owned(),
        _ => "The check stopped before it could report.".to_owned(),
    };
    format!(
        "\n\n---\n\n⚠️ **The typecheck did not run** (`{marker}`). {why} Nothing here was checked by a compiler, so a report with no type or import finding does not show that the code compiles."
    )
}

/// Whether a typecheck would have had something to check: the reviewed tree
/// has a `tsconfig.json`, or the PR touches Python. No tree to look in cannot
/// say, so it counts as applicable.
#[must_use]
pub fn typecheck_applies(tree: Option<&Path>, pr_files: &[String]) -> bool {
    tree.is_none_or(|t| {
        t.join("tsconfig.json").is_file() || pr_files.iter().any(|f| f.ends_with(".py"))
    })
}

/// Whether the typecheck's text is a result and not a marker for why none ran.
#[must_use]
pub fn typecheck_ran(text: &str) -> bool {
    !text.lines().map(str::trim).any(|l| {
        l == "NO_TSCONFIG" || l == python_typecheck::NO_CONFIG || l.starts_with("DEGRADED_")
    })
}

const LOCKFILES: [&str; 4] = [
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
];

fn dependency_maps(dir: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    Some(json!([
        v.get("dependencies"),
        v.get("devDependencies"),
        v.get("peerDependencies")
    ]))
}

/// The files that say which packages are installed and differ between the
/// checkout the typecheck borrowed its dependencies from (`local`) and the
/// reviewed commit (`reviewed`). A lockfile is compared as bytes. Without any
/// lockfile the three dependency maps of `package.json` are compared, so a
/// version bump alone is not drift.
#[must_use]
pub fn deps_drift(local: &Path, reviewed: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut any_lock = false;
    for name in LOCKFILES {
        let (a, b) = (
            std::fs::read(local.join(name)).ok(),
            std::fs::read(reviewed.join(name)).ok(),
        );
        if a.is_none() && b.is_none() {
            continue;
        }
        any_lock = true;
        if a != b {
            out.push(name.to_owned());
        }
    }
    if !any_lock && dependency_maps(local) != dependency_maps(reviewed) {
        out.push("package.json".to_owned());
    }
    out
}

#[must_use]
pub fn deps_drift_banner(files: &[String], resolution: &RepoCwd) -> String {
    if files.is_empty() {
        return String::new();
    }
    let whence = match resolution {
        RepoCwd::Resolved {
            dir, branch, head, ..
        } => format!(
            "`{}` (on {} at {})",
            dir.display(),
            branch.as_deref().unwrap_or("an unknown branch"),
            head.as_deref().unwrap_or("an unknown commit")
        ),
        _ => "the working directory".to_owned(),
    };
    let names = files
        .iter()
        .map(|f| format!("`{f}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "\n\n---\n\n⚠️ **The typecheck ran on dependencies that may not match the PR.** {names} differ between the reviewed commit and {whence}, which supplied the installed packages. Errors about missing modules or types may come from that, and a clean result may not hold with the PR's versions."
    )
}

/// Anchors whose `file` ends in a note, such as `x.ts (BE staging)`. The
/// resolver looks that string up in the reviewed repo and reports the code as
/// missing, so the anchor says nothing about the repo it was meant for.
#[must_use]
pub fn anchor_format(verified_anchors: &Value) -> Value {
    static R: OnceLock<Regex> = OnceLock::new();
    let annotated: Vec<String> = verified_anchors
        .get("unresolvedDetail")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|d| d.get("file").and_then(Value::as_str))
        .filter(|f| re(&R, r"\s\([^)]*\)\s*$").is_match(f))
        .map(str::to_owned)
        .collect();
    json!({ "annotatedUnresolved": annotated.len(), "files": annotated })
}

/// What a run verified outside the reviewed repo and what the critic said it
/// had not checked, as the metrics row stores it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoverageFacts {
    pub cross_repo_reads: Vec<Value>,
    pub not_checked: Vec<NotChecked>,
    pub contradictions: Vec<Contradiction>,
}

impl CoverageFacts {
    #[must_use]
    pub fn not_checked_json(&self) -> Value {
        Value::Array(
            self.not_checked
                .iter()
                .map(|n| json!({ "text": n.text, "tokens": n.tokens }))
                .collect(),
        )
    }

    #[must_use]
    pub fn contradictions_json(&self) -> Value {
        Value::Array(
            self.contradictions
                .iter()
                .map(|c| json!({ "claim": c.claim, "token": c.token, "lenses": c.lenses }))
                .collect(),
        )
    }

    #[must_use]
    pub fn banner(&self) -> String {
        contradiction_banner(&self.contradictions)
    }
}

/// `lenses` is each lens dispatch's label and calls; `others` is every other
/// dispatch's label, kind and calls (critic, refuters).
#[must_use]
pub fn facts(
    own_repo: &str,
    lenses: &[(String, Vec<Call>)],
    others: &[(String, String, Vec<Call>)],
    critic_output: &str,
) -> CoverageFacts {
    let mut cross_repo_reads: Vec<Value> = lenses
        .iter()
        .filter_map(|(label, calls)| reads_entry(label, "lens", calls, own_repo))
        .collect();
    cross_repo_reads.extend(
        others
            .iter()
            .filter_map(|(label, kind, calls)| reads_entry(label, kind, calls, own_repo)),
    );
    let not_checked = not_checked_claims(critic_output);
    let contradictions = contradictions(&not_checked, lenses);
    CoverageFacts {
        cross_repo_reads,
        not_checked,
        contradictions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_anchor_path_with_a_note_after_it_is_counted() {
        let v = json!({"unresolvedDetail": [
            {"file": "src/a.ts (BE staging)", "claimedLine": 77, "status": "file-not-at-ref"},
            {"file": "src/b.ts", "claimedLine": 3, "status": "file-not-at-ref"},
        ]});
        let got = anchor_format(&v);
        assert_eq!(got["annotatedUnresolved"], 1);
        assert_eq!(got["files"][0], "src/a.ts (BE staging)");
        assert_eq!(
            anchor_format(&json!({"ran": false}))["annotatedUnresolved"],
            0
        );
    }

    #[test]
    fn facts_gather_reads_claims_and_contradictions() {
        let lens = vec![(
            "correctness".to_owned(),
            vec![bash(
                r#"gh api "repos/acme/api/contents/src/sync-results.ts?ref=staging""#,
            )],
        )];
        let others = vec![(
            "completeness-critic".to_owned(),
            "critic".to_owned(),
            vec![],
        )];
        let f = facts("acme/self", &lens, &others, CRITIC);
        assert_eq!(f.cross_repo_reads.len(), 1);
        assert_eq!(f.cross_repo_reads[0]["kind"], "lens");
        assert_eq!(f.not_checked.len(), 1);
        assert_eq!(f.contradictions.len(), 1);
        assert_eq!(f.contradictions_json()[0]["token"], "/v2/sync-results");
        assert!(f.banner().contains("touched by correctness"));
    }

    fn bash(text: &str) -> Call {
        Call {
            name: "Bash".into(),
            text: text.into(),
        }
    }

    fn call(name: &str, text: &str) -> Call {
        Call {
            name: name.into(),
            text: text.into(),
        }
    }

    #[test]
    fn a_file_counts_as_opened_only_when_a_call_names_its_whole_path() {
        let files: Vec<String> = [
            "src/a.ts",
            "src/b.ts",
            "src/c.ts",
            "src/d.ts",
            "src/e.ts",
            "api/client.ts",
        ]
        .map(String::from)
        .to_vec();
        let calls = [
            call("Read", "/tmp/worktree/src/a.ts"),
            bash("git show 1a2b3c:src/b.ts | sed -n 1,40p"),
            bash("grep -n retry src/sub.ts src/c.tsx"),
            call("Grep", "retry src"),
            call("StructuredOutput", "src/d.ts"),
            bash("echo checked src/e.ts."),
            call("Read", "/tmp/worktree/webapi/client.ts"),
        ];
        assert_eq!(
            files_opened(&calls, &files),
            ["src/a.ts", "src/b.ts", "src/e.ts"]
        );
    }

    #[test]
    fn a_shell_variable_is_expanded_before_the_repo_is_read() {
        let calls = [bash(
            r#"B=acme/api; gh api "repos/$B/contents/src/a.ts?ref=staging" --jq .content"#,
        )];
        assert_eq!(repo_reads(&calls).get("acme/api@staging"), Some(&1));
    }

    #[test]
    fn refs_come_from_the_query_the_tree_path_or_default() {
        let calls = [
            bash(r#"gh api "repos/acme/api/contents/a.ts?ref=staging""#),
            bash(r#"gh api "repos/acme/api/git/trees/staging?recursive=1""#),
            bash(r#"gh api repos/acme/web/contents/b.ts"#),
            bash(r#"gh api -X GET search/code -f q="x repo:acme/api""#),
        ];
        let got = repo_reads(&calls);
        assert_eq!(got.get("acme/api@staging"), Some(&2));
        assert_eq!(got.get("acme/web@(default)"), Some(&1));
        assert_eq!(got.get("acme/api@(search)"), Some(&1));
    }

    #[test]
    fn the_reviewed_repo_is_not_a_cross_repo_read() {
        let calls = [bash(r#"gh api "repos/acme/self/contents/a.ts?ref=abc""#)];
        assert!(reads_entry("lens", "lens", &calls, "Acme/Self").is_none());
        let calls = [bash(r#"gh api "repos/acme/api/contents/a.ts?ref=abc""#)];
        let e = reads_entry("lens", "lens", &calls, "acme/self").unwrap();
        assert_eq!(e["reads"]["acme/api@abc"], 1);
    }

    #[test]
    fn only_bash_calls_count_as_reads() {
        let calls = [Call {
            name: "Read".into(),
            text: "repos/acme/api/contents/a.ts".into(),
        }];
        assert!(repo_reads(&calls).is_empty());
    }

    const CRITIC: &str = "Nothing new to report: I traced no defect. I did not check backend rules at staging (`/v2/sync-results` and `/v2/records/list` filter semantics), the record id claims, or the spec's refused-event sections (lines ~715-900).";

    #[test]
    fn the_critics_not_checked_sentence_yields_the_names_it_cites() {
        let got = not_checked_claims(CRITIC);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].tokens, vec!["/v2/sync-results", "/v2/records/list"]);
    }

    #[test]
    fn a_sentence_that_says_it_did_check_is_not_a_claim() {
        let got = not_checked_claims("I read `utils/timeouts.ts` and found no defect.");
        assert!(got.is_empty());
    }

    #[test]
    fn a_lens_that_searched_the_name_contradicts_the_critic() {
        let lenses = vec![
            (
                "correctness".to_owned(),
                vec![bash(
                    r#"B=acme/api; gh api -X GET search/code -f q="sync-results repo:$B""#,
                )],
            ),
            (
                "dataContract".to_owned(),
                vec![bash(
                    r#"gh api "repos/acme/api/contents/src/records/list.ts?ref=staging""#,
                )],
            ),
            ("security".to_owned(), vec![bash("ls utils")]),
        ];
        let found = contradictions(&not_checked_claims(CRITIC), &lenses);
        let by_token: BTreeMap<&str, &Vec<String>> = found
            .iter()
            .map(|c| (c.token.as_str(), &c.lenses))
            .collect();
        assert_eq!(
            by_token["/v2/sync-results"],
            &vec!["correctness".to_owned()]
        );
        assert_eq!(
            by_token["/v2/records/list"],
            &vec!["dataContract".to_owned()]
        );
    }

    #[test]
    fn a_name_no_lens_touched_is_not_reported() {
        let lenses = vec![("security".to_owned(), vec![bash("ls utils")])];
        assert!(contradictions(&not_checked_claims(CRITIC), &lenses).is_empty());
    }

    #[test]
    fn the_banner_names_the_token_and_the_lenses() {
        let found = vec![Contradiction {
            claim: "c".into(),
            token: "/v2/x-y-z".into(),
            lenses: vec!["correctness".into(), "migration".into()],
        }];
        let b = contradiction_banner(&found);
        assert!(
            b.contains("`/v2/x-y-z`: touched by correctness, migration"),
            "{b}"
        );
        assert!(contradiction_banner(&[]).is_empty());
    }

    #[test]
    fn the_typecheck_banner_says_why_when_nothing_was_found() {
        let r = RepoCwd::Unresolved {
            cwd_origin: Some("acme/other".into()),
            searched: 3,
        };
        let b = typecheck_banner(Some("DEGRADED_WRONG_REPO"), "acme/widgets", &r, true);
        assert!(b.contains("did not run"), "{b}");
        assert!(b.contains("`acme/other`"), "{b}");
        assert!(b.contains("TRIBUNAL_REPO_DIR"), "{b}");
    }

    #[test]
    fn no_banner_when_the_typecheck_ran_or_did_not_apply() {
        let r = RepoCwd::Ambient;
        assert!(typecheck_banner(Some("NO_DIAGNOSTICS"), "a/b", &r, true).is_empty());
        assert!(typecheck_banner(Some("NO_TSCONFIG"), "a/b", &r, true).is_empty());
        assert!(typecheck_banner(None, "a/b", &r, true).is_empty());
        let un = RepoCwd::Unresolved {
            cwd_origin: None,
            searched: 0,
        };
        assert!(
            typecheck_banner(Some("DEGRADED_WRONG_REPO"), "a/b", &un, false).is_empty(),
            "a repo with nothing to type-check needs no warning"
        );
    }

    fn dir_with(tag: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("tribunal-deps-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for (n, c) in files {
            std::fs::write(d.join(n), c).unwrap();
        }
        d
    }

    #[test]
    fn a_differing_lockfile_is_drift_and_a_matching_one_is_not() {
        let a = dir_with("a", &[("package-lock.json", "{\"v\":1}")]);
        let same = dir_with("same", &[("package-lock.json", "{\"v\":1}")]);
        let other = dir_with("other", &[("package-lock.json", "{\"v\":2}")]);
        assert!(deps_drift(&a, &same).is_empty());
        assert_eq!(deps_drift(&a, &other), vec!["package-lock.json"]);
        let none = dir_with("none", &[]);
        assert_eq!(deps_drift(&none, &a), vec!["package-lock.json"]);
        for d in [a, same, other, none] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn without_a_lockfile_only_the_dependency_maps_count() {
        let a = dir_with(
            "pa",
            &[(
                "package.json",
                r#"{"version":"1.0.0","dependencies":{"x":"1"}}"#,
            )],
        );
        let bumped = dir_with(
            "pb",
            &[(
                "package.json",
                r#"{"version":"2.0.0","dependencies":{"x":"1"}}"#,
            )],
        );
        let changed = dir_with(
            "pc",
            &[(
                "package.json",
                r#"{"version":"1.0.0","dependencies":{"x":"2"}}"#,
            )],
        );
        assert!(deps_drift(&a, &bumped).is_empty());
        assert_eq!(deps_drift(&a, &changed), vec!["package.json"]);
        for d in [a, bumped, changed] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn the_typecheck_applies_to_a_tsconfig_or_python() {
        let ts = dir_with("ts", &[("tsconfig.json", "{}")]);
        let bare = dir_with("bare", &[]);
        assert!(typecheck_applies(Some(&ts), &[]));
        assert!(!typecheck_applies(Some(&bare), &["a.ts".to_owned()]));
        assert!(typecheck_applies(Some(&bare), &["a.py".to_owned()]));
        assert!(typecheck_applies(None, &[]));
        for d in [ts, bare] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn markers_are_not_results() {
        assert!(typecheck_ran("NO_DIAGNOSTICS"));
        assert!(typecheck_ran("a.ts(1,2): error TS2304: x"));
        assert!(!typecheck_ran("NO_TSCONFIG"));
        assert!(!typecheck_ran("DEGRADED_SCHEMA_DRIFT"));
        assert!(!typecheck_ran(python_typecheck::NO_CONFIG));
    }

    #[test]
    fn the_drift_banner_names_the_files_and_the_checkout() {
        let r = RepoCwd::Resolved {
            dir: "/w/widgets".into(),
            how: "sibling".into(),
            branch: Some("old".into()),
            head: Some("abc1234".into()),
        };
        let b = deps_drift_banner(&["package-lock.json".to_owned()], &r);
        assert!(b.contains("`package-lock.json`"), "{b}");
        assert!(b.contains("/w/widgets"), "{b}");
        assert!(b.contains("on old at abc1234"), "{b}");
        assert!(deps_drift_banner(&[], &r).is_empty());
    }

    #[test]
    fn calls_are_read_from_a_transcript_however_deep_they_sit() {
        let dir = std::env::temp_dir().join(format!("tribunal-cov-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("t.jsonl");
        std::fs::write(
            &file,
            concat!(
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"},{"type":"tool_use","name":"Bash","input":{"command":"gh api repos/acme/api/contents/a.ts?ref=staging"}}]}}"#,
                "\nnot json\n",
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/x/y.ts"}}]}}"#,
            ),
        )
        .unwrap();
        let calls = calls_in_transcript(&file);
        assert_eq!(calls.len(), 2);
        assert_eq!(repo_reads(&calls).get("acme/api@staging"), Some(&1));
        assert!(calls_in_transcript(&dir.join("missing.jsonl")).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
