//! Python.

use super::{has_dir_segment, has_plain_word, Capability, Declaration, Language};

pub static PYTHON: Language = Language {
    name: "python",
    source: &[".py", ".pyi"],
    typed: &[".py", ".pyi"],
    checked: &[".py"],
    runnable: &[],
    is_test,
    declaration,
    is_comment,
    is_identifier,
    imports_module,
    cited_identifier: r"(?-u:\b)[A-Za-z][A-Za-z0-9]*(?:_[A-Za-z0-9]+)+(?-u:\b)",
    capabilities: &[Capability::Typecheck],
};

/// A `test` or `tests` directory, or a `test_*.py`, `*_test.py` or
/// `conftest.py` file.
#[must_use]
pub fn is_test(p: &str) -> bool {
    if has_dir_segment(p, &["test", "tests"]) {
        return true;
    }
    let last = p.rsplit('/').next().unwrap_or(p);
    let Some(stem) = last
        .strip_suffix(".pyi")
        .or_else(|| last.strip_suffix(".py"))
    else {
        return false;
    };
    stem == "conftest" || (stem.len() > 5 && (stem.starts_with("test_") || stem.ends_with("_test")))
}

fn name_end(s: &str, i: usize) -> Option<usize> {
    let b = s.as_bytes();
    match b.get(i) {
        Some(&c) if c.is_ascii_alphabetic() || c == b'_' => {}
        _ => return None,
    }
    let mut j = i + 1;
    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
        j += 1;
    }
    Some(j)
}

fn after_keyword(l: &str, kw: &str) -> Option<usize> {
    let rest = l.strip_prefix(kw)?;
    let ws = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    (ws > 0).then_some(kw.len() + ws)
}

/// A column-0 `def`, `async def` or `class`, or a column-0 `NAME =` or
/// `NAME: T =` assignment. A name starting with `_` is not exported.
#[must_use]
pub fn declaration(l: &str) -> Option<Declaration> {
    let found = |kind: &str, i: usize| -> Option<Declaration> {
        let end = name_end(l, i)?;
        let name = &l[i..end];
        Some(Declaration {
            exported: !name.starts_with('_'),
            kind: kind.to_owned(),
            name: name.to_owned(),
        })
    };
    if let Some(i) = after_keyword(l, "async") {
        return after_keyword(&l[i..], "def").and_then(|j| found("def", i + j));
    }
    if let Some(i) = after_keyword(l, "def") {
        return found("def", i);
    }
    if let Some(i) = after_keyword(l, "class") {
        return found("class", i);
    }
    let end = name_end(l, 0)?;
    let rest = l[end..].trim_start_matches([' ', '\t']);
    let rest = match rest.strip_prefix(':') {
        Some(annotated) => &annotated[annotated.find('=')?..],
        None => rest,
    };
    (rest.starts_with('=') && !rest.starts_with("==")).then(|| found("assign", 0))?
}

#[must_use]
pub fn is_comment(l: &str) -> bool {
    l.trim_start().starts_with('#')
}

#[must_use]
pub fn is_identifier(s: &str) -> bool {
    name_end(s, 0) == Some(s.len()) && !s.is_empty()
}

/// An `import` or `from ... import` line that names the target's module:
/// the file's stem, or its directory for an `__init__.py`.
#[must_use]
pub fn imports_module(line: &str, target: &str) -> bool {
    let base = target.rsplit('/').next().unwrap_or(target);
    let stem = base
        .strip_suffix(".pyi")
        .or_else(|| base.strip_suffix(".py"))
        .unwrap_or(base);
    let module = if stem == "__init__" {
        target.rsplit('/').nth(1).unwrap_or(stem)
    } else {
        stem
    };
    let t = line.trim_start();
    (t.starts_with("import ") || t.starts_with("from ")) && has_plain_word(t, module)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tests_are_test_directories_and_the_pytest_file_names() {
        for p in [
            "pkg/test_widgets.py",
            "pkg/widgets_test.py",
            "pkg/conftest.py",
            "tests/helpers.py",
            "pkg/test/data.py",
            "pkg/test_widgets.pyi",
        ] {
            assert!(is_test(p), "{p}");
        }
        for p in [
            "pkg/widgets.py",
            "pkg/testing.py",
            "pkg/contest.py",
            "pkg/test_.py",
            "pkg/_test.py",
            "pkg/latest.py",
            "pkg/test_widgets.txt",
        ] {
            assert!(!is_test(p), "{p}");
        }
    }

    fn decl(l: &str) -> Option<(bool, String, String)> {
        declaration(l).map(|d| (d.exported, d.kind, d.name))
    }

    #[test]
    fn a_declaration_is_a_column_zero_def_class_or_assignment() {
        let some = |e: bool, k: &str, n: &str| Some((e, k.to_owned(), n.to_owned()));
        assert_eq!(
            decl("def widget_names(items):"),
            some(true, "def", "widget_names")
        );
        assert_eq!(decl("async def load():"), some(true, "def", "load"));
        assert_eq!(decl("class Widget:"), some(true, "class", "Widget"));
        assert_eq!(decl("def _helper():"), some(false, "def", "_helper"));
        assert_eq!(decl("LIMIT = 3"), some(true, "assign", "LIMIT"));
        assert_eq!(decl("LIMIT: int = 3"), some(true, "assign", "LIMIT"));
        assert_eq!(
            decl("define = 1 == 2 and x"),
            some(true, "assign", "define")
        );
        for l in [
            "    def nested(self):",
            "defer()",
            "x == 3",
            "x: int",
            "import os",
            "widget.names = []",
            "",
        ] {
            assert_eq!(decl(l), None, "{l}");
        }
    }

    #[test]
    fn comments_and_identifiers() {
        assert!(is_comment("  # note") && !is_comment("x = 1  # y") && !is_comment("// a"));
        assert!(is_identifier("widget_names") && is_identifier("_x1"));
        assert!(!is_identifier("$a") && !is_identifier("1a") && !is_identifier(""));
        assert!(imports_module(
            "from pkg.widgets import widget_names",
            "pkg/widgets.py"
        ));
        assert!(imports_module(
            "    import pkg.widgets as w",
            "pkg/widgets.py"
        ));
        assert!(imports_module(
            "from . import widgets",
            "pkg/widgets/__init__.py"
        ));
        assert!(!imports_module(
            "from pkg.gadgets import widgets_all",
            "pkg/widgets.py"
        ));
        assert!(!imports_module("widgets = load()", "pkg/widgets.py"));
    }
}
