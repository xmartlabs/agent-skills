// A reviewer kind is spread over several files. These tests fail when a kind
// is added to one list and not to the others, or without its files.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use tribunal_context::assemble_methodology::{assemble, template_path, KINDS, NAMESPACE};
use tribunal_core::profile::lens_routing::{lens_agent_for, AGENT_NS, REVIEWER_AGENTS};
use tribunal_core::profile::profile_lookup::{validate_profile, PROFILE_REVIEWER_KINDS};

fn plugin_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn set(items: impl IntoIterator<Item = String>) -> BTreeSet<String> {
    items.into_iter().collect()
}

fn agent_of(kind: &str) -> &'static str {
    lens_agent_for(&json!({ "reviewer": kind }))
        .unwrap_or_else(|e| panic!("{kind} has no reviewer agent: {e}"))
}

#[test]
fn the_three_lists_name_the_same_kinds() {
    let kinds = set(KINDS.iter().map(|k| k.to_string()));
    let profile = set(PROFILE_REVIEWER_KINDS.iter().map(|k| k.to_string()));
    let routed = set(REVIEWER_AGENTS.iter().map(|(k, _)| k.to_string()));
    assert_eq!(kinds, profile, "assemble KINDS vs PROFILE_REVIEWER_KINDS");
    assert_eq!(kinds, routed, "assemble KINDS vs REVIEWER_AGENTS");
}

#[test]
fn every_kind_has_a_template_an_agent_and_a_matching_methodology() {
    let root = plugin_root();
    for kind in KINDS {
        let template = read(Path::new(&template_path(root.to_str().unwrap(), kind)));
        let agent = agent_of(kind);
        let name = agent.strip_prefix(AGENT_NS).unwrap();
        let wrapper = read(&root.join("agents").join(format!("{name}.md")));
        assert!(
            wrapper.contains(&format!("\nname: {name}\n")),
            "{kind}: agents/{name}.md does not declare name: {name}"
        );

        let shipped = wrapper
            .split("methodology/")
            .nth(1)
            .and_then(|rest| rest.split(['`', ' ', '\n']).next())
            .filter(|f| f.ends_with(".md"))
            .unwrap_or_else(|| panic!("{kind}: agents/{name}.md names no methodology file"));
        let plain = read(&root.join("methodology").join(shipped));
        let assembled = assemble(&template, None, NAMESPACE).unwrap();
        assert_eq!(
            plain, assembled.text,
            "{kind}: methodology/{shipped} is not templates/{kind}.md with every slot at its default"
        );
        assert!(
            !assembled.slots.is_empty(),
            "{kind}: the template has no slots"
        );
    }
}

#[test]
fn every_kind_has_a_shipped_profile_and_its_rules_reach_a_slot() {
    let root = plugin_root();
    let rules = read(&root.join("templates/review-rules.md"));
    for kind in KINDS {
        let path = root.join(format!("templates/review-profile.{kind}.json"));
        let profile: Value = serde_json::from_str(&read(&path)).unwrap();
        assert_eq!(profile["reviewer"], json!(kind), "{}", path.display());
        validate_profile(&profile, &profile["repo"])
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let template = read(Path::new(&template_path(root.to_str().unwrap(), kind)));
        let a = assemble(&template, Some(&rules), NAMESPACE).unwrap();
        assert!(a.unknown.is_empty(), "{kind}: {:?}", a.unknown);
    }
}
