//! The languages of the reviewed code, and what the engine knows about each
//! without parsing it: which files are its source, which are tests, what a
//! top-level declaration, a comment and an identifier look like, and how a
//! finding's prose names its files.
//!
//! A language is one [`Language`] value in [`LANGUAGES`]. Code that reads the
//! reviewed repository asks this module and keeps no extension list of its
//! own (`tests/lang_extension_ratchet.rs` holds that line). To add a language,
//! add a module with its `Language` and list it in [`LANGUAGES`].
//!
//! The extension sets differ by use, because the uses differed before this
//! module existed and each keeps its behaviour: `.mjs` is a source file for
//! imports and grep but is not type checked, `.mts` is the reverse.

pub mod python;
pub mod typescript;

/// A top-level declaration found on one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    pub exported: bool,
    pub kind: String,
    pub name: String,
}

pub struct Language {
    pub name: &'static str,
    /// Source files for imports and `git grep`, as suffixes with the dot, in
    /// the order the globs are passed to `git grep`.
    pub source: &'static [&'static str],
    /// Files the language's type checker reports on.
    pub typed: &'static [&'static str],
    /// Files whose findings make a run eligible for the type check.
    pub checked: &'static [&'static str],
    /// Files a generated case can import and run.
    pub runnable: &'static [&'static str],
    pub is_test: fn(&str) -> bool,
    /// The declaration a line opens, when it starts at column 0.
    pub declaration: fn(&str) -> Option<Declaration>,
    pub is_comment: fn(&str) -> bool,
    pub is_identifier: fn(&str) -> bool,
    /// Whether `line` imports the module the file at the second argument
    /// defines. Read from the line alone, with no path resolution.
    pub imports_module: fn(&str, &str) -> bool,
    /// A regex for the identifiers worth looking up when prose cites code.
    pub cited_identifier: &'static str,
}

pub static LANGUAGES: [&Language; 2] = [&typescript::TYPESCRIPT, &python::PYTHON];

/// Extensions a finding's prose can cite that belong to no language, for
/// widening a refuter's scope.
pub const SCOPE_DATA: [&str; 3] = ["prisma", "sql", "json"];
/// The same, for the wrong-reference audit.
pub const REFERENCE_DATA: [&str; 1] = ["prisma"];

fn suffix_in(set: &'static [&'static str], path: &str) -> Option<&'static str> {
    set.iter()
        .copied()
        .filter(|e| path.ends_with(e))
        .max_by_key(|e| e.len())
}

impl Language {
    /// The [`Language::source`] suffix the path ends with.
    #[must_use]
    pub fn source_ext(&self, path: &str) -> Option<&'static str> {
        suffix_in(self.source, path)
    }

    #[must_use]
    pub fn is_source(&self, path: &str) -> bool {
        self.source_ext(path).is_some()
    }

    #[must_use]
    pub fn is_typed(&self, path: &str) -> bool {
        suffix_in(self.typed, path).is_some()
    }

    #[must_use]
    pub fn is_checked(&self, path: &str) -> bool {
        suffix_in(self.checked, path).is_some()
    }

    /// `*.ext` for every source extension, in order.
    #[must_use]
    pub fn grep_globs(&self) -> Vec<String> {
        self.source.iter().map(|e| format!("*{e}")).collect()
    }

    fn owns(&self, path: &str) -> bool {
        [self.source, self.typed, self.runnable]
            .into_iter()
            .any(|set| suffix_in(set, path).is_some())
    }
}

/// The language of `path` among `table`, by extension.
#[must_use]
pub fn language_in<'a>(table: &[&'a Language], path: &str) -> Option<&'a Language> {
    table.iter().copied().find(|l| l.owns(path))
}

/// The language of `path`, by extension.
#[must_use]
pub fn language_of(path: &str) -> Option<&'static Language> {
    language_in(&LANGUAGES, path)
}

/// Whether `path` is a test file of its language. A path of no known language
/// is not.
#[must_use]
pub fn is_test(path: &str) -> bool {
    language_of(path).is_some_and(|l| (l.is_test)(path))
}

/// The extensions, without the dot, prose can cite: every language's typed
/// files and `data`.
#[must_use]
pub fn cited_extensions(table: &[&Language], data: &[&'static str]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for e in table
        .iter()
        .flat_map(|l| l.typed.iter().map(|e| e.trim_start_matches('.')))
        .chain(data.iter().copied())
    {
        if !out.contains(&e) {
            out.push(e);
        }
    }
    out
}

/// `name` as a whole word of `[A-Za-z0-9_]` in `s`.
#[must_use]
pub fn has_plain_word(s: &str, name: &str) -> bool {
    let b = s.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    !name.is_empty()
        && s.match_indices(name).any(|(i, _)| {
            let end = i + name.len();
            (i == 0 || !word(b[i - 1])) && (end == b.len() || !word(b[end]))
        })
}

/// A segment before the last one named one of `names`.
#[must_use]
pub fn has_dir_segment(p: &str, names: &[&str]) -> bool {
    let segs: Vec<&str> = p.split('/').collect();
    segs[..segs.len() - 1].iter().any(|s| names.contains(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_resolves_to_its_language_by_any_of_its_extension_sets() {
        for p in [
            "src/a.ts",
            "src/a.tsx",
            "a.js",
            "a.jsx",
            "a.mjs",
            "a.cjs",
            "a.mts",
            "a.cts",
        ] {
            assert_eq!(language_of(p).map(|l| l.name), Some("typescript"), "{p}");
        }
        for p in ["pkg/widgets.py", "pkg/widgets.pyi"] {
            assert_eq!(language_of(p).map(|l| l.name), Some("python"), "{p}");
        }
        for p in ["schema.prisma", "a.sql", "a.json", "README.md", "Makefile"] {
            assert!(language_of(p).is_none(), "{p}");
        }
    }

    #[test]
    fn each_use_keeps_its_own_extension_set() {
        let ts = &typescript::TYPESCRIPT;
        assert!(ts.is_source("a.mjs") && !ts.is_typed("a.mjs"));
        assert!(ts.is_typed("a.mts") && !ts.is_source("a.mts"));
        assert!(ts.is_checked("a.tsx") && !ts.is_checked("a.js"));
        assert_eq!(ts.source_ext("a.tsx"), Some(".tsx"));
        assert_eq!(
            ts.grep_globs(),
            ["*.ts", "*.tsx", "*.js", "*.jsx", "*.mjs", "*.cjs"]
        );
        let py = &python::PYTHON;
        assert!(py.is_typed("a.pyi") && py.is_checked("a.py") && !py.is_checked("a.pyi"));
        assert!(py.runnable.is_empty());
    }

    #[test]
    fn cited_extensions_are_the_typed_files_of_every_language_and_the_data_files() {
        assert_eq!(
            cited_extensions(&LANGUAGES, &SCOPE_DATA),
            ["ts", "tsx", "js", "jsx", "mts", "cts", "py", "pyi", "prisma", "sql", "json"]
        );
        assert_eq!(
            cited_extensions(&LANGUAGES, &REFERENCE_DATA),
            ["ts", "tsx", "js", "jsx", "mts", "cts", "py", "pyi", "prisma"]
        );
    }

    fn never(_: &str) -> bool {
        false
    }

    fn fake_test(p: &str) -> bool {
        p.ends_with("_spec.fk")
    }

    fn fake_decl(l: &str) -> Option<Declaration> {
        l.strip_prefix("thing ").map(|n| Declaration {
            exported: true,
            kind: "thing".into(),
            name: n.into(),
        })
    }

    #[test]
    fn a_language_added_to_a_table_answers_with_no_other_change() {
        static FAKE: Language = Language {
            name: "fake",
            source: &[".fk"],
            typed: &[".fk"],
            checked: &[],
            runnable: &[],
            is_test: fake_test,
            declaration: fake_decl,
            is_comment: never,
            is_identifier: never,
            imports_module: |_, _| false,
            cited_identifier: r"\bfk_[a-z]+\b",
        };
        let table: [&Language; 3] = [&typescript::TYPESCRIPT, &python::PYTHON, &FAKE];
        let l = language_in(&table, "src/widget_spec.fk").expect("the fake language owns .fk");
        assert_eq!(l.name, "fake");
        assert!((l.is_test)("src/widget_spec.fk"));
        assert_eq!((l.declaration)("thing widget").unwrap().name, "widget");
        assert!(cited_extensions(&table, &[]).contains(&"fk"));
        assert!(language_of("src/widget_spec.fk").is_none());
    }
}
