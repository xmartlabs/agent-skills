//! An agent definition from the plugin's `agents/*.md`: its frontmatter and
//! the body that becomes the child's system prompt. Read at run time, so an
//! edit to a definition reaches the next run without a rebuild.

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDef {
    pub name: String,
    pub model: Option<String>,
    pub tools: Vec<String>,
    pub body: String,
}

/// Parses one definition file's text.
///
/// # Errors
/// When the text has no `---` frontmatter or the frontmatter names no agent.
pub fn parse(text: &str) -> Result<AgentDef, String> {
    let rest = text
        .strip_prefix("---\n")
        .ok_or("an agent definition starts with a --- frontmatter line")?;
    let end = rest
        .find("\n---\n")
        .ok_or("the agent definition's frontmatter is not closed")?;
    let (front, body) = (&rest[..end], &rest[end + 5..]);
    let mut def = AgentDef {
        name: String::new(),
        model: None,
        tools: Vec::new(),
        body: body.trim().to_owned(),
    };
    for line in front.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "name" => value.clone_into(&mut def.name),
            "model" if !value.is_empty() => def.model = Some(value.to_owned()),
            "tools" => {
                def.tools = value
                    .split(',')
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            _ => {}
        }
    }
    if def.name.is_empty() {
        return Err("the agent definition's frontmatter has no name".to_owned());
    }
    Ok(def)
}

/// Loads `agents/<name>.md` under the plugin root. `name` may carry the
/// plugin namespace (`tribunal:code-review-expert`), as the workflow names
/// its agents.
///
/// # Errors
/// When the file cannot be read or parsed, or names a different agent.
pub fn load(plugin_root: &Path, name: &str) -> Result<AgentDef, String> {
    let bare = name.rsplit(':').next().unwrap_or(name);
    let path = plugin_root.join("agents").join(format!("{bare}.md"));
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let def = parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if def.name != bare {
        return Err(format!(
            "{} defines {:?}, not {bare:?}",
            path.display(),
            def.name
        ));
    }
    Ok(def)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin_root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
    }

    #[test]
    fn every_shipped_definition_parses() {
        for name in [
            "code-review-expert",
            "fe-code-review-expert",
            "e2e-code-review-expert",
            "finding-verifier",
            "tribunal-mechanic",
            "tribunal-refuter",
            "tribunal-scribe",
            "tribunal-strengthener",
        ] {
            let def = load(&plugin_root(), &format!("tribunal:{name}")).unwrap();
            assert!(!def.tools.is_empty(), "{name}");
            assert!(!def.body.starts_with("---"), "{name}");
            assert!(def.body.len() > 200, "{name}");
        }
    }

    #[test]
    fn the_lens_definition_reads_read_and_bash() {
        let def = load(&plugin_root(), "code-review-expert").unwrap();
        assert_eq!(def.tools, ["Read", "Bash"]);
        assert_eq!(def.model.as_deref(), Some("claude-sonnet-5-5"));
        assert!(def.body.starts_with("**Before doing anything else"));
    }

    #[test]
    fn a_file_without_frontmatter_is_refused() {
        assert!(parse("just text").is_err());
        assert!(parse("---\nname: x\n").is_err());
        assert!(parse("---\nmodel: m\n---\nbody").is_err());
    }
}
