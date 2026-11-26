// The backend role's presets must assemble the same bytes as the per-kind
// templates they replaced. The goldens were written by the release before
// the split, so any drift in what a lens reads fails here.

use std::fs;
use std::path::{Path, PathBuf};

use tribunal_context::assemble_methodology::{
    assemble_for, assemble_layered, lang_module_path, role_template_path, Methodology,
};

fn plugin_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn root() -> String {
    plugin_root().to_str().unwrap().to_owned()
}

fn here() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn rules_inputs() -> Vec<(&'static str, Option<String>)> {
    vec![
        ("none", None),
        (
            "shipped",
            Some(read(&plugin_root().join("templates/review-rules.md"))),
        ),
        (
            "sentinel",
            Some(read(&here().join("fixtures/methodology/sentinel-rules.md"))),
        ),
    ]
}

#[test]
fn backend_presets_assemble_the_frozen_bytes() {
    for preset in ["backend", "python"] {
        for (input, rules) in rules_inputs() {
            let golden = here().join(format!("goldens/methodology/{preset}-{input}.md"));
            let (got, _) = assemble_for(&root(), preset, None, rules.as_deref())
                .unwrap_or_else(|e| panic!("{preset}: {e}"));
            assert!(
                got.text == read(&golden),
                "{preset} with {input} rules differs from {}",
                golden.display()
            );
        }
    }
}

#[test]
fn the_sentinel_rules_fill_every_slot_the_old_templates_had() {
    let rules = read(&here().join("fixtures/methodology/sentinel-rules.md"));
    for preset in ["backend", "python"] {
        let (a, _) = assemble_for(&root(), preset, None, Some(&rules)).unwrap();
        assert!(a.unknown.is_empty(), "{preset}: {:?}", a.unknown);
        assert_eq!(a.filled.len(), 18, "{preset}: {:?}", a.filled);
        assert!(!a.text.contains("<!--"), "{preset}: a marker survived");
    }
}

#[test]
fn each_preset_names_its_layers() {
    let (_, m) = assemble_for(&root(), "backend", None, None).unwrap();
    assert_eq!(
        m,
        Methodology {
            role: "backend".into(),
            language: Some("typescript".into()),
            module: Some("lang/typescript.md".into()),
        }
    );
    let (_, m) = assemble_for(&root(), "python", None, None).unwrap();
    assert_eq!(m.language.as_deref(), Some("python"));
    assert_eq!(m.module.as_deref(), Some("lang/python.md"));
    let (_, m) = assemble_for(&root(), "frontend", None, None).unwrap();
    assert_eq!(
        (m.role.as_str(), m.language, m.module),
        ("frontend", None, None)
    );
}

#[test]
fn backend_with_python_as_the_language_is_the_python_preset() {
    let (a, _) = assemble_for(&root(), "backend", Some("python"), None).unwrap();
    let (b, _) = assemble_for(&root(), "python", None, None).unwrap();
    assert_eq!(a.text, b.text);
}

#[test]
fn a_language_with_no_module_gets_the_neutral_role_and_no_stack_words() {
    let (a, m) = assemble_for(&root(), "backend", Some("go"), None).unwrap();
    assert_eq!(m.module, None);
    assert_eq!(m.language.as_deref(), Some("go"));
    for token in [
        "Zod",
        "Prisma",
        "BullMQ",
        "pydantic",
        "Django",
        "SQLAlchemy",
        "try/except",
        "select_related",
        "$transaction",
        "TypeScript",
        "Python",
    ] {
        assert!(!a.text.contains(token), "neutral defaults mention {token}");
    }
    assert!(!a.text.contains("<!--"), "a marker survived");
}

#[test]
fn every_module_block_names_a_slot_of_the_role() {
    let role = read(Path::new(&role_template_path(&root(), "backend")));
    for language in ["typescript", "python"] {
        let path = lang_module_path(&root(), language);
        let text = read(Path::new(&path));
        let l = assemble_layered(&role, Some((path.as_str(), text.as_str())), None)
            .unwrap_or_else(|e| panic!("{language}: {e}"));
        assert!(!l.from_module.is_empty(), "{language}: fills nothing");
    }
}

#[test]
fn language_is_refused_where_it_cannot_apply() {
    for kind in ["python", "frontend", "e2e"] {
        let e = assemble_for(&root(), kind, Some("go"), None).unwrap_err();
        assert!(e.contains("--language"), "{kind}: {e}");
    }
    for bad in ["Go", "../role/backend", "", "go lang", "1c"] {
        let r = assemble_for(&root(), "backend", Some(bad), None);
        assert!(r.is_err(), "{bad:?} was accepted");
    }
}

mod profile_field {
    use serde_json::json;
    use tribunal_context::assemble_methodology::{language_warnings, profile_language};

    use super::root;

    #[test]
    fn a_backend_language_with_no_module_warns_that_defaults_are_neutral() {
        let p = json!({"reviewer": "backend", "language": "go"});
        assert_eq!(profile_language(&p).as_deref(), Some("go"));
        let w = language_warnings(&p, Some(&root()));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].contains("lang/go.md") && w[0].contains("neutral"),
            "{w:?}"
        );
    }

    #[test]
    fn a_shipped_language_or_no_field_warns_nothing() {
        let p = json!({"reviewer": "backend", "language": "python"});
        assert!(language_warnings(&p, Some(&root())).is_empty());
        let p = json!({"reviewer": "backend"});
        assert!(language_warnings(&p, Some(&root())).is_empty());
        assert_eq!(profile_language(&p), None);
    }

    #[test]
    fn a_language_on_another_reviewer_is_ignored_with_a_warning() {
        for reviewer in ["python", "frontend", "e2e"] {
            let p = json!({"reviewer": reviewer, "language": "go"});
            assert_eq!(profile_language(&p), None, "{reviewer}");
            let w = language_warnings(&p, Some(&root()));
            assert!(
                w.len() == 1 && w[0].contains("only to reviewer"),
                "{reviewer}: {w:?}"
            );
        }
    }

    #[test]
    fn a_malformed_language_is_ignored_with_a_warning() {
        for bad in [json!("Go"), json!("../x"), json!(3), json!("")] {
            let p = json!({"reviewer": "backend", "language": bad});
            assert_eq!(profile_language(&p), None, "{bad}");
            let w = language_warnings(&p, Some(&root()));
            assert!(
                w.len() == 1 && w[0].contains("lowercase name"),
                "{bad}: {w:?}"
            );
        }
    }

    #[test]
    fn without_a_plugin_root_the_module_check_is_skipped() {
        let p = json!({"reviewer": "backend", "language": "go"});
        assert!(language_warnings(&p, None).is_empty());
    }
}
