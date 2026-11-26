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
pub const KINDS: [&str; 4] = ["backend", "frontend", "e2e", "python"];

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
    let slots = slot_names(&parts)?;
    let given = blocks_of(rules, "rules", namespace)?;
    let (text, filled) = fill(&parts, &[&given]);
    let filled = filled.into_iter().next().unwrap_or_default();
    let unknown = unknown_blocks(&given, &slots, namespace);
    Ok(Assembled {
        text,
        slots,
        filled,
        unknown,
    })
}

type Blocks = Vec<(String, Vec<String>)>;

fn slot_names(parts: &[Part]) -> Result<Vec<String>, String> {
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
    Ok(slots)
}

/// The `namespace/` blocks of `text`; blocks of other namespaces are skipped.
fn blocks_of(text: Option<&str>, where_: &str, namespace: &str) -> Result<Blocks, String> {
    // `rules` is used only when truthy: an empty file fills nothing.
    let mut given: Blocks = Vec::new();
    if let Some(text) = text.filter(|r| !r.is_empty()) {
        for p in parse_slots(text, where_)? {
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
                return Err(format!("{where_}: block \"{full}\" appears twice"));
            }
            given.push((name.to_owned(), body));
        }
    }
    Ok(given)
}

/// The template with each slot taken from the first source that has it, and
/// the slots each source filled.
fn fill(parts: &[Part], sources: &[&Blocks]) -> (String, Vec<Vec<String>>) {
    let mut lines: Vec<String> = Vec::new();
    let mut filled = vec![Vec::new(); sources.len()];
    for p in parts {
        match p {
            Part::Text(t) => lines.push(t.clone()),
            Part::Slot { name, body } => {
                let hit = sources
                    .iter()
                    .enumerate()
                    .find_map(|(i, s)| s.iter().find(|(n, _)| n == name).map(|(_, b)| (i, b)));
                match hit {
                    Some((i, b)) => {
                        filled[i].push(name.clone());
                        lines.extend(b.iter().cloned());
                    }
                    None => lines.extend(body.iter().cloned()),
                }
            }
        }
    }
    (lines.join("\n"), filled)
}

fn unknown_blocks(given: &Blocks, slots: &[String], namespace: &str) -> Vec<String> {
    given
        .iter()
        .filter(|(n, _)| !slots.contains(n))
        .map(|(n, _)| format!("{namespace}/{n}"))
        .collect()
}

/// The namespace a language module's blocks use.
pub const LANG_NAMESPACE: &str = "lang";

/// A methodology built from a role template, an optional language module and
/// the repo's rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layered {
    pub assembled: Assembled,
    /// Slots the language module filled.
    pub from_module: Vec<String>,
}

/// Fills `template` from the repo's `tribunal/` blocks first, then the
/// module's `lang/` blocks, then the template's own defaults. A module block
/// that names no slot is an error, since the plugin shipped it; a repo block
/// that names none is reported in `unknown`, as `assemble` reports it.
///
/// # Errors
/// A malformed template, module or rules file, or a module block that names
/// no slot.
pub fn assemble_layered(
    template: &str,
    module: Option<(&str, &str)>,
    rules: Option<&str>,
) -> Result<Layered, String> {
    let parts = parse_slots(template, "template")?;
    let slots = slot_names(&parts)?;
    let repo = blocks_of(rules, "rules", NAMESPACE)?;
    let lang = match module {
        Some((name, text)) => {
            let blocks = blocks_of(Some(text), name, LANG_NAMESPACE)?;
            let stray = unknown_blocks(&blocks, &slots, LANG_NAMESPACE);
            if !stray.is_empty() {
                return Err(format!(
                    "{name}: block(s) naming no slot of the role template: {}",
                    stray.join(", ")
                ));
            }
            blocks
        }
        None => Vec::new(),
    };
    let (text, mut filled) = fill(&parts, &[&repo, &lang]);
    let from_module = filled.pop().unwrap_or_default();
    let filled = filled.pop().unwrap_or_default();
    let unknown = unknown_blocks(&repo, &slots, NAMESPACE);
    Ok(Layered {
        assembled: Assembled {
            text,
            slots,
            filled,
            unknown,
        },
        from_module,
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
    assemble_for(plugin_root, kind, None, rules_text).map(|(a, _)| a)
}

/// The role and default language a kind stands for, when the kind is a
/// preset of a layered role. `frontend` and `e2e` keep a single template.
#[must_use]
pub fn preset(kind: &str) -> Option<(&'static str, &'static str)> {
    match kind {
        "backend" => Some(("backend", "typescript")),
        "python" => Some(("backend", "python")),
        _ => None,
    }
}

/// A language name the plugin can look up as a file: lowercase, starting
/// with a letter, then letters, digits and hyphens.
#[must_use]
pub fn is_language_name(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

#[must_use]
pub fn role_template_path(plugin_root: &str, role: &str) -> String {
    path_join(&[
        plugin_root,
        "methodology",
        "templates",
        "role",
        &format!("{role}.md"),
    ])
}

#[must_use]
pub fn lang_module_path(plugin_root: &str, language: &str) -> String {
    path_join(&[
        plugin_root,
        "methodology",
        "templates",
        "lang",
        &format!("{language}.md"),
    ])
}

/// The line a run and `load-profile` print when a backend profile names a
/// language the plugin has no module for.
#[must_use]
pub fn neutral_defaults_notice(language: &str) -> String {
    format!(
        "no language module for {language} (methodology/templates/lang/{language}.md); the backend role's neutral defaults apply"
    )
}

/// The profile's `language` when it can apply: a well-formed name on a
/// `backend` reviewer. Anything else is ignored, and `language_warnings`
/// says why.
#[must_use]
pub fn profile_language(profile: &Value) -> Option<String> {
    let reviewer = profile.get("reviewer").and_then(Value::as_str)?;
    let language = profile.get("language").and_then(Value::as_str)?;
    (reviewer == "backend" && is_language_name(language)).then(|| language.to_owned())
}

/// Warnings for a profile's `language` field. A new profile field is checked
/// where the engine reads it, as a warning, not in the ported validator.
/// `plugin_root` is where to look for the module; without it that check is
/// skipped.
#[must_use]
pub fn language_warnings(profile: &Value, plugin_root: Option<&str>) -> Vec<String> {
    let Some(language) = profile.get("language") else {
        return Vec::new();
    };
    let Some(name) = language.as_str().filter(|l| is_language_name(l)) else {
        return vec![format!(
            "language must be a lowercase name such as \"go\" (letters, digits and hyphens); {language} is ignored"
        )];
    };
    let reviewer = profile
        .get("reviewer")
        .and_then(Value::as_str)
        .unwrap_or("");
    if reviewer != "backend" {
        return vec![format!(
            "language applies only to reviewer \"backend\"; it is ignored for {}",
            serde_json::to_string(reviewer).expect("a string serializes")
        )];
    }
    match plugin_root {
        Some(root) if !std::path::Path::new(&lang_module_path(root, name)).is_file() => {
            vec![neutral_defaults_notice(name)]
        }
        _ => Vec::new(),
    }
}

/// Which layers a run's methodology came from. `module` is the module file
/// relative to `methodology/templates`, or `None` when the role's neutral
/// defaults filled the language slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Methodology {
    pub role: String,
    pub language: Option<String>,
    pub module: Option<String>,
}

/// The methodology for `kind`, with `language` overriding a preset's default
/// language.
///
/// # Errors
/// An unknown kind, a `language` on a kind that is not `backend`, a malformed
/// language name, an unreadable template, or what the assemblers refuse.
pub fn assemble_for(
    plugin_root: &str,
    kind: &str,
    language: Option<&str>,
    rules_text: Option<&str>,
) -> Result<(Assembled, Methodology), String> {
    if !KINDS.contains(&kind) {
        return Err(format!(
            "--kind must be one of {}, got {}",
            KINDS.join(", "),
            serde_json::to_string(kind).expect("a string serializes")
        ));
    }
    // A preset may name its own language; any other language needs `backend`.
    if let Some(l) = language {
        if kind != "backend" && preset(kind).map(|(_, d)| d) != Some(l) {
            return Err(format!(
                "--language applies only to --kind backend, not {kind}"
            ));
        }
    }
    let Some((role, default_language)) = preset(kind) else {
        let template = read_file_utf8(&template_path(plugin_root, kind))?;
        let a = assemble(&template, rules_text, NAMESPACE)?;
        let m = Methodology {
            role: kind.to_owned(),
            language: None,
            module: None,
        };
        return Ok((a, m));
    };
    let language = language.unwrap_or(default_language);
    if !is_language_name(language) {
        return Err(format!(
            "--language must be lowercase letters, digits and hyphens, got {}",
            serde_json::to_string(language).expect("a string serializes")
        ));
    }
    let template = read_file_utf8(&role_template_path(plugin_root, role))?;
    let module_path = lang_module_path(plugin_root, language);
    let module_name = format!("lang/{language}.md");
    let module_text = if std::path::Path::new(&module_path).is_file() {
        Some(read_file_utf8(&module_path)?)
    } else {
        None
    };
    let layered = assemble_layered(
        &template,
        module_text.as_deref().map(|t| (module_name.as_str(), t)),
        rules_text,
    )?;
    let m = Methodology {
        role: role.to_owned(),
        language: Some(language.to_owned()),
        module: module_text.map(|_| module_name),
    };
    Ok((layered.assembled, m))
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
    // Without --language the output keeps the shape the script printed;
    // the engine always passes it for a layered role.
    let language = flag(argv, "--language");
    let language = (!language.is_empty()).then_some(language);
    let (a, layers) = match assemble_for(
        plugin_root,
        &kind,
        language.as_deref(),
        rules_text.as_deref(),
    ) {
        Ok(r) => r,
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
    if language.is_some() {
        let opt = |v: Option<String>| v.map_or(Value::Null, Value::String);
        m.insert("role".into(), Value::String(layers.role));
        m.insert("language".into(), opt(layers.language));
        m.insert("module".into(), opt(layers.module));
    }
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

    const ROLE: &str = "a\n<!-- slot: x -->\nneutral x\n<!-- /slot -->\n<!-- slot: y -->\nneutral y\n<!-- /slot -->\n<!-- slot: z -->\nneutral z\n<!-- /slot -->\nb";
    const MODULE: &str = "<!-- slot: lang/x -->\nlang x\n<!-- /slot -->\n\n<!-- slot: lang/y -->\nlang y\n<!-- /slot -->\n";

    #[test]
    fn the_repo_wins_over_the_module_and_the_module_over_the_default() {
        let rules = "<!-- slot: tribunal/y -->\nrepo y\n<!-- /slot -->";
        let l = assemble_layered(ROLE, Some(("lang/t.md", MODULE)), Some(rules)).unwrap();
        assert_eq!(l.assembled.text, "a\nlang x\nrepo y\nneutral z\nb");
        assert_eq!(l.assembled.filled, ["y"]);
        assert_eq!(l.from_module, ["x"]);
    }

    #[test]
    fn without_a_module_every_slot_keeps_its_default() {
        let l = assemble_layered(ROLE, None, None).unwrap();
        assert_eq!(l.assembled.text, "a\nneutral x\nneutral y\nneutral z\nb");
        assert!(l.from_module.is_empty());
    }

    #[test]
    fn a_module_block_naming_no_slot_is_an_error_naming_it() {
        let module = "<!-- slot: lang/w -->\nw\n<!-- /slot -->";
        let e = assemble_layered(ROLE, Some(("lang/t.md", module)), None).unwrap_err();
        assert!(e.contains("lang/t.md") && e.contains("lang/w"), "{e}");
    }

    #[test]
    fn a_repo_block_naming_no_slot_is_only_reported() {
        let rules = "<!-- slot: tribunal/w -->\nw\n<!-- /slot -->";
        let l = assemble_layered(ROLE, Some(("lang/t.md", MODULE)), Some(rules)).unwrap();
        assert_eq!(l.assembled.unknown, ["tribunal/w"]);
    }

    #[test]
    fn a_module_ignores_repo_blocks_and_the_repo_ignores_module_blocks() {
        let rules = "<!-- slot: lang/z -->\nsmuggled\n<!-- /slot -->";
        let module = "<!-- slot: tribunal/z -->\nsmuggled\n<!-- /slot -->";
        let l = assemble_layered(ROLE, Some(("lang/t.md", module)), Some(rules)).unwrap();
        assert!(!l.assembled.text.contains("smuggled"));
    }

    #[test]
    fn stray_markers_are_refused() {
        assert!(parse_slots("<!--slot x", "t").is_err());
        assert!(parse_slots("<!-- slots", "t").is_ok());
    }
}
