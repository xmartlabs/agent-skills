//! TypeScript and JavaScript.

use super::{has_dir_segment, has_plain_word, Capability, Declaration, Language};
use crate::js;

pub static TYPESCRIPT: Language = Language {
    name: "typescript",
    source: &[".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"],
    typed: &[".ts", ".tsx", ".js", ".jsx", ".mts", ".cts"],
    checked: &[".ts", ".tsx"],
    runnable: &[".ts", ".tsx", ".mts", ".cts", ".js", ".mjs", ".cjs"],
    is_test,
    declaration,
    is_comment,
    is_identifier,
    imports_module,
    cited_identifier: r"(?-u:\b)[A-Za-z][A-Za-z0-9_]*[a-z][A-Z][A-Za-z0-9_]*(?-u:\b)",
    capabilities: &Capability::ALL,
};

#[must_use]
pub fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// A byte of `[\w$]`.
#[must_use]
pub fn is_ident(b: u8) -> bool {
    is_word(b) || b == b'$'
}

/// TEST_RE: a `__tests__`, `__mocks__`, `test` or `tests` directory, or a
/// `.test.`/`.spec.` file with something after it.
#[must_use]
pub fn is_test(p: &str) -> bool {
    if has_dir_segment(p, &["__tests__", "__mocks__", "test", "tests"]) {
        return true;
    }
    let last = p.rsplit('/').next().unwrap_or(p);
    [".test.", ".spec."]
        .iter()
        .any(|m| last.match_indices(m).any(|(i, _)| i + m.len() < last.len()))
}

#[must_use]
pub fn skip_ws(s: &str, mut i: usize) -> usize {
    while let Some(c) = s[i..].chars().next() {
        if !js::is_js_whitespace(c) {
            break;
        }
        i += c.len_utf8();
    }
    i
}

/// `[A-Za-z_$][\w$]*` at `i`: the index after it.
#[must_use]
pub fn ident_end(s: &str, i: usize) -> Option<usize> {
    let b = s.as_bytes();
    match b.get(i) {
        Some(&c) if c.is_ascii_alphabetic() || c == b'_' || c == b'$' => {}
        _ => return None,
    }
    let mut j = i + 1;
    while j < b.len() && is_ident(b[j]) {
        j += 1;
    }
    Some(j)
}

/// `kw\s+` at `i`: the index after the whitespace.
#[must_use]
pub fn keyword_ws(s: &str, i: usize, kw: &str) -> Option<usize> {
    if !s[i..].starts_with(kw) {
        return None;
    }
    let j = skip_ws(s, i + kw.len());
    (j > i + kw.len()).then_some(j)
}

/// DECL_RE.
#[must_use]
pub fn declaration(l: &str) -> Option<Declaration> {
    let mut i = 0;
    let exported = match keyword_ws(l, i, "export") {
        Some(j) => {
            i = j;
            true
        }
        None => false,
    };
    for kw in ["default", "declare", "async"] {
        if let Some(j) = keyword_ws(l, i, kw) {
            i = j;
        }
    }
    let kinds = [
        "function*",
        "function",
        "const",
        "let",
        "var",
        "class",
        "enum",
        "type",
        "interface",
    ];
    for kind in kinds {
        let Some(j) = keyword_ws(l, i, kind) else {
            continue;
        };
        let end = ident_end(l, j)?;
        return Some(Declaration {
            exported,
            kind: kind.to_owned(),
            name: l[j..end].to_owned(),
        });
    }
    None
}

/// COMMENT_LINE: `//`, `/*` or `*` after leading whitespace.
#[must_use]
pub fn is_comment(l: &str) -> bool {
    let t = l.trim_start_matches(js::is_js_whitespace);
    t.starts_with("//") || t.starts_with("/*") || t.starts_with('*')
}

#[must_use]
pub fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// An `import`, `export ... from` or `require` line that names the target's
/// file stem.
#[must_use]
pub fn imports_module(line: &str, target: &str) -> bool {
    let base = target.rsplit('/').next().unwrap_or(target);
    let stem = TYPESCRIPT
        .source_ext(base)
        .map_or(base, |e| &base[..base.len() - e.len()]);
    let stem = if stem == "index" {
        target.rsplit('/').nth(1).unwrap_or(stem)
    } else {
        stem
    };
    (line.contains("require(")
        || line.contains(" from ")
        || line.trim_start().starts_with("import"))
        && has_plain_word(line, stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tests_are_test_directories_and_test_or_spec_files() {
        for p in [
            "src/__tests__/a.ts",
            "src/__mocks__/a.ts",
            "test/a.ts",
            "pkg/tests/a.js",
            "src/a.test.ts",
            "src/a.spec.tsx",
        ] {
            assert!(is_test(p), "{p}");
        }
        for p in ["src/a.ts", "src/test.ts", "src/a.test", "src/tests.ts"] {
            assert!(!is_test(p), "{p}");
        }
    }

    #[test]
    fn a_declaration_is_a_column_zero_keyword_and_a_name() {
        let d = declaration("export async function loadWidgets(id) {").unwrap();
        assert_eq!(
            (d.exported, d.kind.as_str(), d.name.as_str()),
            (true, "function", "loadWidgets")
        );
        let d = declaration("const $limit = 3;").unwrap();
        assert_eq!(
            (d.exported, d.kind.as_str(), d.name.as_str()),
            (false, "const", "$limit")
        );
        assert!(declaration("  function nested() {").is_none());
        assert!(declaration("def widget_names(items):").is_none());
    }

    #[test]
    fn comments_and_identifiers() {
        assert!(is_comment("  // note") && is_comment("/* a") && is_comment(" * b"));
        assert!(!is_comment("# note") && !is_comment("x = 1 // y"));
        assert!(is_identifier("$a_1") && !is_identifier("1a") && !is_identifier("a-b"));
        assert!(imports_module(
            "import { a } from './widgets';",
            "src/widgets.ts"
        ));
        assert!(imports_module(
            "const w = require('../widgets')",
            "src/widgets/index.js"
        ));
        assert!(!imports_module(
            "import { widgets } from './other';x",
            "src/gadgets.ts"
        ));
        assert!(!imports_module("const widgets = 1;", "src/widgets.ts"));
    }
}
