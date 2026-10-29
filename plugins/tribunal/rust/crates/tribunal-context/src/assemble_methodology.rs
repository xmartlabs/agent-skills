//! Port of `skills/review-pr/scripts/assemble-methodology.mjs`: the
//! methodology one review reads, built from the plugin's template for the
//! reviewer kind with each `<!-- slot: name -->` filled from the repo's own
//! review rules (`<!-- slot: tribunal/name -->` blocks).
//!
//! The script finds its templates beside itself; the port takes the plugin
//! root as a parameter and builds the same path.

use serde_json::{Map, Value};

use crate::load_profile::sha256_hex;
use crate::operator::{flag, path_join, read_file_utf8, write_file, CliOutput};

pub const NAMESPACE: &str = "tribunal";
pub const KINDS: [&str; 2] = ["backend", "frontend"];

/// `templatePath(kind)`: `<plugin root>/methodology/templates/<kind>.md`,
/// normalized as `path.join` normalizes it.
#[must_use]
pub fn template_path(plugin_root: &str, kind: &str) -> String {
    path_join(&[
        plugin_root,
        "methodology",
        "templates",
        &format!("{kind}.md"),
    ])
}

/// One part of a parsed file: a line outside any slot, or a slot's body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Text(String),
    Slot { name: String, body: Vec<String> },
}

fn is_name_part(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b[1..]
            .iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// `OPEN`: `<!-- slot: name -->` or `<!-- slot: ns/name -->`, the whole line.
fn open_name(line: &str) -> Option<&str> {
    let name = line.strip_prefix("<!-- slot: ")?.strip_suffix(" -->")?;
    let ok = match name.split_once('/') {
        Some((ns, n)) => is_name_part(ns) && is_name_part(n),
        None => is_name_part(name),
    };
    ok.then_some(name)
}

/// `LOOKS_LIKE_MARKER`: `/^<!--\s*\/?\s*slot\b/`.
fn looks_like_marker(line: &str) -> bool {
    let ws = |c: char| tribunal_core::js::is_js_whitespace(c);
    let Some(rest) = line.strip_prefix("<!--") else {
        return false;
    };
    let rest = rest.trim_start_matches(ws);
    let rest = rest
        .strip_prefix('/')
        .unwrap_or(rest)
        .trim_start_matches(ws);
    match rest.strip_prefix("slot") {
        Some(after) => !after
            .bytes()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_'),
        None => false,
    }
}

/// `parseSlots(text, where)`.
///
/// # Errors
/// A nested, unclosed or stray marker, worded as the script words it.
pub fn parse_slots(text: &str, where_: &str) -> Result<Vec<Part>, String> {
    let mut parts = Vec::new();
    let mut open: Option<(String, Vec<String>)> = None;
    for (i, line) in text.split('\n').enumerate() {
        if let Some(name) = open_name(line) {
            if let Some((outer, _)) = &open {
                return Err(format!(
                    "{where_}:{}: slot \"{name}\" opens inside \"{outer}\"",
                    i + 1
                ));
            }
            open = Some((name.to_owned(), Vec::new()));
            continue;
        }
        if line == "<!-- /slot -->" {
            let Some((name, body)) = open.take() else {
                return Err(format!(
                    "{where_}:{}: a slot closes that never opened",
                    i + 1
                ));
            };
            parts.push(Part::Slot { name, body });
            continue;
        }
        if looks_like_marker(line) {
            return Err(format!("{where_}:{}: not a slot marker: {line}", i + 1));
        }
        match &mut open {
            Some((_, body)) => body.push(line.to_owned()),
            None => parts.push(Part::Text(line.to_owned())),
        }
    }
    if let Some((name, _)) = open {
        return Err(format!("{where_}: slot \"{name}\" never closes"));
    }
    Ok(parts)
}

/// What `assemble` returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assembled {
    pub text: String,
    pub slots: Vec<String>,
    pub filled: Vec<String>,
    pub unknown: Vec<String>,
}

/// `assemble(template, rules, namespace)`.
///
/// # Errors
/// A malformed template or rules file, worded as the script words it.
pub fn assemble(template: &str, rules: Option<&str>, namespace: &str) -> Result<Assembled, String> {
    let parts = parse_slots(template, "template")?;
    let slots: Vec<String> = parts
        .iter()
        .filter_map(|p| match p {
            Part::Slot { name, .. } => Some(name.clone()),
            Part::Text(_) => None,
        })
        .collect();
    if let Some(dup) = slots
        .iter()
        .enumerate()
        .find(|(i, n)| slots.iter().position(|m| m == *n) != Some(*i))
    {
        return Err(format!("template: slot \"{}\" appears twice", dup.1));
    }
    if slots.iter().any(|n| n.contains('/')) {
        return Err("template: a slot name carries a namespace".into());
    }
    // `rules` is used only when truthy: an empty file fills nothing.
    let mut given: Vec<(String, Vec<String>)> = Vec::new();
    if let Some(rules) = rules.filter(|r| !r.is_empty()) {
        for p in parse_slots(rules, "rules")? {
            let Part::Slot { name: full, body } = p else {
                continue;
            };
            let (ns, name) = match full.split_once('/') {
                Some((ns, n)) => (Some(ns), n),
                None => (None, full.as_str()),
            };
            if ns != Some(namespace) {
                continue;
            }
            if given.iter().any(|(n, _)| n == name) {
                return Err(format!("rules: block \"{full}\" appears twice"));
            }
            given.push((name.to_owned(), body));
        }
    }
    let mut lines: Vec<String> = Vec::new();
    let mut filled = Vec::new();
    for p in &parts {
        match p {
            Part::Text(t) => lines.push(t.clone()),
            Part::Slot { name, body } => match given.iter().find(|(n, _)| n == name) {
                Some((_, b)) => {
                    filled.push(name.clone());
                    lines.extend(b.iter().cloned());
                }
                None => lines.extend(body.iter().cloned()),
            },
        }
    }
    let unknown = given
        .iter()
        .filter(|(n, _)| !slots.contains(n))
        .map(|(n, _)| format!("{namespace}/{n}"))
        .collect();
    Ok(Assembled {
        text: lines.join("\n"),
        slots,
        filled,
        unknown,
    })
}

/// `assembleKind({kind, rulesText})`, reading the template under
/// `plugin_root`.
///
/// # Errors
/// An unknown kind, an unreadable template, or what [`assemble`] refuses.
pub fn assemble_kind(
    plugin_root: &str,
    kind: &str,
    rules_text: Option<&str>,
) -> Result<Assembled, String> {
    if !KINDS.contains(&kind) {
        return Err(format!(
            "--kind must be one of {}, got {}",
            KINDS.join(", "),
            serde_json::to_string(kind).expect("a string serializes")
        ));
    }
    let template = read_file_utf8(&template_path(plugin_root, kind))?;
    assemble(&template, rules_text, NAMESPACE)
}

fn fail(error: String) -> CliOutput {
    let mut m = Map::new();
    m.insert("ok".into(), Value::Bool(false));
    m.insert("error".into(), Value::String(error));
    CliOutput::json_line(&m)
}

/// The script run as a command: `argv` is what follows the script path.
#[must_use]
pub fn main(plugin_root: &str, argv: &[String]) -> CliOutput {
    let kind = flag(argv, "--kind");
    let out_path = flag(argv, "--out");
    let rules_path = flag(argv, "--rules");
    let expected = flag(argv, "--rules-sha256");
    if out_path.is_empty() {
        return fail("--out <file> is required".into());
    }
    let mut rules_text = None;
    let mut rules_sha = None;
    if !rules_path.is_empty() {
        let text = match read_file_utf8(&rules_path) {
            Ok(t) => t,
            Err(e) => return fail(format!("could not read --rules {rules_path}: {e}")),
        };
        let sha = sha256_hex(&text);
        if !expected.is_empty() && expected != sha {
            return fail(format!(
                "--rules {rules_path} has sha256 {sha}, not the {expected} load-profile.mjs recorded"
            ));
        }
        rules_text = Some(text);
        rules_sha = Some(sha);
    } else if !expected.is_empty() {
        return fail("--rules-sha256 given without --rules".into());
    }
    let a = match assemble_kind(plugin_root, &kind, rules_text.as_deref()) {
        Ok(a) => a,
        Err(e) => return fail(e),
    };
    if let Err(e) = write_file(&out_path, &a.text) {
        return fail(e);
    }
    let strings = |v: Vec<String>| Value::Array(v.into_iter().map(Value::String).collect());
    let mut m = Map::new();
    m.insert("ok".into(), Value::Bool(true));
    m.insert("out".into(), Value::String(out_path));
    m.insert("sha256".into(), Value::String(sha256_hex(&a.text)));
    m.insert("kind".into(), Value::String(kind));
    m.insert(
        "rulesSha256".into(),
        rules_sha.map_or(Value::Null, Value::String),
    );
    m.insert("filled".into(), strings(a.filled));
    m.insert("unknown".into(), strings(a.unknown));
    CliOutput::json_line(&m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_replaces_its_slot() {
        let t = "a\n<!-- slot: x -->\ndefault\n<!-- /slot -->\nb";
        let r = "<!-- slot: tribunal/x -->\nmine\n<!-- /slot -->\n<!-- slot: other/x -->\nno\n<!-- /slot -->";
        let a = assemble(t, Some(r), NAMESPACE).unwrap();
        assert_eq!(a.text, "a\nmine\nb");
        assert_eq!(a.filled, ["x"]);
    }

    #[test]
    fn stray_markers_are_refused() {
        assert!(parse_slots("<!--slot x", "t").is_err());
        assert!(parse_slots("<!-- slots", "t").is_ok());
    }
}
