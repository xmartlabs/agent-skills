//! Port of `lib/resolve-verified-anchors.mjs`: whether the code each
//! `verifiedBy` anchor quotes is in its file at the ref, read through
//! `gh api`, and whether the failures look like a broken lookup rather than
//! fabricated anchors.
//!
//! JavaScript semantics kept: a quote goes through `String()` (which throws,
//! uncaught, on an object with its own `toString`), `\s` and `trim` use the
//! ECMAScript whitespace set, the claimed line passes `Number.isInteger` and
//! stays a double, a `lookup-failed` detail is cut at 200 UTF-16 units, and
//! the arguments are split as the script splits them, where a flag's value
//! also counts as positional.

use std::path::Path;

use serde_json::{json, Map, Value};
use tribunal_core::evidence::js_ext::number_value;
use tribunal_core::js;
use tribunal_core::lens::js_ext::utf16_prefix_lossy;

use crate::exec::Tools;
use crate::incremental_scope::read_file_utf8;
use crate::load_profile::encode_uri_component;
use crate::operator::json_parse;
use crate::resolve_evidence_line::to_js_string;

/// The script's `maxBuffer`. Node counts stdout and stderr together.
const MAX_BUFFER: usize = 32 * 1024 * 1024;
const DETAIL_UNITS: usize = 200;

/// Ports `normalise` on a string already through `String()`: curly quotes and
/// en/em dashes to ASCII, whitespace runs to one space, trimmed.
fn normalise(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_space = false;
    for c in s.chars() {
        let c = match c {
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            '\u{2013}' | '\u{2014}' => '-',
            c => c,
        };
        if js::is_js_whitespace(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    js::trim(&out).to_owned()
}

/// `String(s == null ? "" : s)`.
fn quote_string(v: Option<&Value>) -> Result<String, String> {
    match v {
        None | Some(Value::Null) => Ok(String::new()),
        v => to_js_string(v).map_err(|e| format!("TypeError: {e}")),
    }
}

/// Ports `locateQuote`: 1-based start lines of every window that holds the
/// quote, with a wider second pass for a reflowed multi-line quote.
fn locate_quote(text: &str, quote: &str) -> Vec<usize> {
    let needle = normalise(quote);
    if needle.is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let span = js::trim(quote).split('\n').count().max(1);
    let window = |i: usize, n: usize| normalise(&lines[i..(i + n).min(lines.len())].join(" "));
    let mut hits: Vec<usize> = (0..(lines.len() + 1).saturating_sub(span))
        .filter(|&i| window(i, span).contains(&needle))
        .map(|i| i + 1)
        .collect();
    if hits.is_empty() && span > 1 {
        hits = (0..lines.len())
            .filter(|&i| window(i, span + 2).contains(&needle))
            .map(|i| i + 1)
            .collect();
    }
    hits
}

enum Fetched {
    Ok(String),
    Failed {
        status: &'static str,
        detail: String,
    },
}

/// Ports `fileAtRef`. Messages name `gh`, as the script's do, whatever
/// binary `Tools` runs.
async fn file_at_ref(tools: &Tools, repo: &str, ref_: &str, file: &str) -> Fetched {
    let path = file
        .split('/')
        .map(encode_uri_component)
        .collect::<Vec<_>>()
        .join("/");
    let endpoint = format!(
        "repos/{repo}/contents/{path}?ref={}",
        encode_uri_component(ref_)
    );
    let args = [
        "api",
        "-H",
        "Accept: application/vnd.github.raw",
        endpoint.as_str(),
    ];
    let (stderr, message) = match tools.gh(&args, None).await {
        Err(e) => {
            let code = crate::exec::spawn_error_code(&e);
            (String::new(), format!("spawnSync gh {code}"))
        }
        Ok(out) if out.stdout.len() + out.stderr.len() > MAX_BUFFER => {
            (out.stderr, "spawnSync gh ENOBUFS".to_owned())
        }
        Ok(out) if out.ok() => return Fetched::Ok(out.stdout),
        Ok(out) => {
            let mut message = format!("Command failed: gh {}", args.join(" "));
            if !out.stderr.is_empty() {
                message.push('\n');
                message.push_str(&out.stderr);
            }
            (out.stderr, message)
        }
    };
    if stderr
        .to_ascii_lowercase()
        .contains("no commit found for the ref")
    {
        return Fetched::Failed {
            status: "ref-not-found",
            detail: format!("no commit {ref_} in {repo}"),
        };
    }
    if stderr.contains("(HTTP 404)") {
        return Fetched::Failed {
            status: "file-not-at-ref",
            detail: format!("{file} is not in {repo} at {ref_}"),
        };
    }
    let trimmed = js::trim(&stderr);
    let detail = if trimmed.is_empty() {
        message.as_str()
    } else {
        trimmed
    };
    Fetched::Failed {
        status: "lookup-failed",
        detail: utf16_prefix_lossy(detail, DETAIL_UNITS),
    }
}

/// A `--cross-repo <owner/name>@<ref>` flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sibling {
    pub repo: String,
    pub ref_: String,
}

/// Ports `splitCrossRepoAnchor`: `(repo, ref, file)` when `file` starts with
/// a sibling's directory name.
fn split_cross_repo<'a>(
    file: &'a str,
    repo: &str,
    siblings: &'a [Sibling],
) -> Option<(&'a str, &'a str, &'a str)> {
    siblings.iter().filter(|s| s.repo != repo).find_map(|s| {
        let dir = s.repo.rsplit('/').next().unwrap_or_default();
        file.strip_prefix(dir)
            .and_then(|r| r.strip_prefix('/'))
            .map(|rest| (s.repo.as_str(), s.ref_.as_str(), rest))
    })
}

struct Resolution {
    file: Option<String>,
    claimed: Option<f64>,
    resolved: Option<usize>,
    delta: Option<f64>,
    note: Option<String>,
    status: &'static str,
}

impl Resolution {
    fn new(file: Option<String>, claimed: Option<f64>, status: &'static str, note: String) -> Self {
        Self {
            file,
            claimed,
            resolved: None,
            delta: None,
            note: Some(note),
            status,
        }
    }

    fn into_value(self, finding_index: usize, anchor_index: usize) -> Value {
        let num = |x: Option<f64>| x.map_or(Value::Null, number_value);
        let mut m = Map::new();
        m.insert("findingIndex".into(), finding_index.into());
        m.insert("anchorIndex".into(), anchor_index.into());
        m.insert("file".into(), self.file.into());
        m.insert("claimedLine".into(), num(self.claimed));
        m.insert("resolvedLine".into(), self.resolved.into());
        m.insert("delta".into(), num(self.delta));
        m.insert("note".into(), self.note.into());
        m.insert("status".into(), self.status.into());
        Value::Object(m)
    }
}

/// Ports `resolveAnchor` for a non-empty `repo`.
async fn resolve_anchor(
    tools: &Tools,
    anchor: &Value,
    ref_: &str,
    repo: &str,
    siblings: &[Sibling],
) -> Result<Resolution, String> {
    let file = match js::get(anchor, "file") {
        Some(Value::String(f)) => Some(f.as_str()),
        _ => None,
    };
    let claimed = claimed_line(anchor);
    let Some(file) = file.filter(|f| !f.is_empty()) else {
        return Ok(Resolution::new(
            file.map(str::to_owned),
            claimed,
            "malformed",
            "no file on the anchor".into(),
        ));
    };
    let quote_value = js::get(anchor, "quote");
    let malformed_quote = || {
        Resolution::new(
            Some(file.to_owned()),
            claimed,
            "malformed",
            "no quote on the anchor".into(),
        )
    };
    if !js::truthy(quote_value) {
        return Ok(malformed_quote());
    }
    let quote = quote_string(quote_value)?;
    if normalise(&quote).is_empty() {
        return Ok(malformed_quote());
    }

    let cross = split_cross_repo(file, repo, siblings);
    let (lookup_repo, lookup_ref, lookup_file) = cross.unwrap_or((repo, ref_, file));
    let cross_note = cross
        .is_some()
        .then(|| format!("resolved as a cross-repo anchor into {lookup_repo}@{lookup_ref}"));

    let text = match file_at_ref(tools, lookup_repo, lookup_ref, lookup_file).await {
        Fetched::Ok(text) => text,
        Fetched::Failed { status, detail } => {
            let note = match &cross_note {
                Some(c) => format!("{detail} ({c})"),
                None => detail,
            };
            return Ok(Resolution::new(
                Some(file.to_owned()),
                claimed,
                status,
                note,
            ));
        }
    };

    let hits = locate_quote(&text, &quote);
    let Some(&first) = hits.first() else {
        return Ok(Resolution::new(
            Some(file.to_owned()),
            claimed,
            "not-found",
            format!(
                "the quoted code does not appear in {lookup_file} at {lookup_repo}@{lookup_ref}"
            ),
        ));
    };
    let resolved = match claimed {
        None => first,
        Some(c) => hits.iter().copied().fold(first, |best, h| {
            if (h as f64 - c).abs() < (best as f64 - c).abs() {
                h
            } else {
                best
            }
        }),
    };
    let delta = claimed.map(|c| resolved as f64 - c);
    let multi_note = (hits.len() > 1).then(|| {
        format!(
            "the quote appears {} times in the file; resolved to the occurrence nearest the claimed line",
            hits.len()
        )
    });
    let note = match (multi_note, cross_note) {
        (Some(m), Some(c)) => Some(format!("{m} ({c})")),
        (m, c) => m.or(c),
    };
    Ok(Resolution {
        file: Some(file.to_owned()),
        claimed,
        resolved: Some(resolved),
        delta,
        note,
        status: if delta.is_none_or(|d| d == 0.0) {
            "found"
        } else {
            "found-shifted"
        },
    })
}

/// `Number.isInteger(anchor?.line) ? anchor.line : null`.
fn claimed_line(anchor: &Value) -> Option<f64> {
    let line = js::get(anchor, "line");
    js::is_integer(line)
        .then(|| line.and_then(Value::as_f64))
        .flatten()
}

fn not_ran(reason: &str, ref_: Value, repo: Value) -> Value {
    json!({"ran": false, "reason": reason, "ref": ref_, "repo": repo, "results": []})
}

/// Ports `resolveVerifiedAnchors` as the main path calls it, with a non-empty
/// `ref_` and `repo`.
async fn resolve_verified_anchors(
    tools: &Tools,
    findings: Option<&Value>,
    ref_: &str,
    repo: &str,
    siblings: &[Sibling],
) -> Result<Value, String> {
    let Some(Value::Array(findings)) = findings else {
        return Ok(not_ran("findings-not-an-array", ref_.into(), repo.into()));
    };
    let mut results = Vec::new();
    let (mut resolved, mut failed_lookup, mut missing) = (0, 0, 0);
    for (fi, f) in findings.iter().enumerate() {
        let Some(Value::Array(anchors)) = js::get(f, "verifiedBy") else {
            continue;
        };
        for (ai, a) in anchors.iter().enumerate() {
            let r = resolve_anchor(tools, a, ref_, repo, siblings).await?;
            match r.status {
                "found" | "found-shifted" => resolved += 1,
                "ref-not-found" | "lookup-failed" => failed_lookup += 1,
                "file-not-at-ref" => missing += 1,
                _ => {}
            }
            results.push(r.into_value(fi, ai));
        }
    }
    let n = results.len();
    let all_missing = n > 1 && missing == n;
    Ok(json!({
        "ran": true,
        "ref": ref_,
        "repo": repo,
        "anchors": n,
        "resolved": resolved,
        "unresolved": n - resolved,
        "suspectLookup": failed_lookup > 0 || all_missing,
        "results": results,
    }))
}

/// `--repo <value>`, else the first `--repo=<value>` up to its next `=`.
fn repo_arg(args: &[&str]) -> Option<String> {
    if let Some(i) = args.iter().position(|a| *a == "--repo") {
        if let Some(v) = args
            .get(i + 1)
            .filter(|v| !v.is_empty() && !v.starts_with("--"))
        {
            return Some((*v).to_owned());
        }
    }
    args.iter()
        .find(|a| a.starts_with("--repo="))
        .and_then(|a| a.split('=').nth(1))
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

/// Every `--cross-repo <v>` and `--cross-repo=<v>` with an `@` past its first
/// character, split at the last `@`.
fn sibling_args(args: &[&str]) -> Vec<Sibling> {
    args.iter()
        .enumerate()
        .filter_map(|(i, a)| {
            let v = if *a == "--cross-repo" {
                args.get(i + 1).copied()
            } else {
                a.strip_prefix("--cross-repo=")
            }?;
            let at = v.rfind('@').filter(|&at| at > 0)?;
            Some(Sibling {
                repo: v[..at].to_owned(),
                ref_: v[at + 1..].to_owned(),
            })
        })
        .collect()
}

/// The script's main path: `node resolve-verified-anchors.mjs <anchors.json>
/// <ref> --repo <owner/name> [--relay] [--cross-repo <owner/name>@<ref>]...`.
/// `args` are the arguments after the script; `stdin` is what an anchors path
/// of `-` reads. Returns the object the script prints (or relays).
///
/// # Errors
/// Where the script dies on an uncaught TypeError: a quote that is an object
/// with its own `toString`, or an array holding one.
pub async fn run(args: &[&str], stdin: &str, tools: &Tools) -> Result<Value, String> {
    let args: Vec<&str> = args.iter().copied().filter(|a| *a != "--relay").collect();
    let positional: Vec<&str> = args
        .iter()
        .copied()
        .filter(|a| !a.starts_with("--"))
        .collect();
    let path = positional.first().copied().unwrap_or_default();
    let ref_ = positional.get(1).copied();
    let repo = repo_arg(&args);
    let siblings = sibling_args(&args);

    let (Some(r), Some(repo)) = (ref_.filter(|r| !r.is_empty()), repo.as_deref()) else {
        return Ok(not_ran("usage", ref_.into(), repo.into()));
    };
    if path.is_empty() {
        return Ok(not_ran("usage", r.into(), repo.into()));
    }
    let text = if path == "-" {
        Some(stdin.to_owned())
    } else {
        read_file_utf8(Path::new(path))
    };
    let Some(parsed) = text.and_then(|t| json_parse(&t).ok()) else {
        return Ok(not_ran("findings-unparseable", r.into(), repo.into()));
    };
    let findings = match &parsed {
        Value::Array(_) => Some(&parsed),
        other => js::get(other, "findings"),
    };
    resolve_verified_anchors(tools, findings, r, repo, &siblings).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalising_folds_typography_and_whitespace() {
        assert_eq!(
            normalise("\u{FEFF} \u{2018}a\u{2019} \u{201C}b\u{201D}\u{2013}\u{2014}\u{3000}c "),
            "'a' \"b\"-- c"
        );
        assert_eq!(normalise("\u{0085}x"), "\u{0085}x");
    }

    #[test]
    fn a_reflowed_quote_is_found_in_the_wider_window() {
        let text = "one\nalpha beta\ngamma\ndelta\nfive";
        assert_eq!(locate_quote(text, "alpha\nbeta gamma delta"), vec![1, 2]);
        assert_eq!(locate_quote(text, "beta gamma"), Vec::<usize>::new());
        assert_eq!(locate_quote(text, "beta\ngamma"), vec![2]);
    }

    #[test]
    fn arguments_split_as_the_script_splits_them() {
        let args = [
            "a.json",
            "main",
            "--repo=o/r=x",
            "--cross-repo",
            "o/web@dev",
            "--cross-repo=o/sdk@a@b",
            "--cross-repo=@x",
        ];
        assert_eq!(repo_arg(&args).as_deref(), Some("o/r"));
        assert_eq!(
            sibling_args(&args),
            vec![
                Sibling {
                    repo: "o/web".into(),
                    ref_: "dev".into()
                },
                Sibling {
                    repo: "o/sdk@a".into(),
                    ref_: "b".into()
                },
            ]
        );
    }
}
