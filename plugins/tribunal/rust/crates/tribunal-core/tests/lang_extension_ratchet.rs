//! No code outside `tribunal_core::lang` spells a source extension of the
//! reviewed code (`".ts"`, `"*.py"`, ...). A list of them belongs to a
//! `Language`, so a new language is one module and no call site.

use std::path::{Path, PathBuf};

const EXTENSIONS: [&str; 10] = [
    "ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts", "py", "pyi",
];

/// Files that name an extension for a reason that is not "which files are
/// source": the parser mode of one file, and the engine's own scripts.
const ALLOWED: [&str; 2] = [
    "tribunal-context/src/ast_bindings.rs",
    "tribunal-context/src/harness_version.rs",
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `"<.ext>"` or `"*.ext"` as a whole string literal on the line.
fn spells_an_extension(line: &str) -> bool {
    EXTENSIONS
        .iter()
        .any(|e| line.contains(&format!("\".{e}\"")) || line.contains(&format!("\"*.{e}\"")))
}

fn offenders(crates: &Path) -> Vec<String> {
    let mut files = Vec::new();
    for krate in std::fs::read_dir(crates).unwrap().flatten() {
        let src = krate.path().join("src");
        if src.is_dir() {
            rust_files(&src, &mut files);
        }
    }
    files.sort();
    let mut out = Vec::new();
    for file in files {
        let rel = file
            .strip_prefix(crates)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if rel.starts_with("tribunal-core/src/lang/") || ALLOWED.contains(&rel.as_str()) {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        let code = text.split("#[cfg(test)]").next().unwrap_or(&text);
        for (n, line) in code.lines().enumerate() {
            if !line.trim_start().starts_with("//") && spells_an_extension(line) {
                out.push(format!("{rel}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    out
}

#[test]
fn source_extensions_are_spelled_only_in_the_lang_module() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let found = offenders(crates);
    assert!(
        found.is_empty(),
        "a source extension outside tribunal_core::lang; ask the Language instead:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_scan_sees_an_extension_literal() {
    assert!(spells_an_extension(r#"    f.ends_with(".py")"#));
    assert!(spells_an_extension(r#"const G: [&str; 1] = ["*.tsx"];"#));
    assert!(!spells_an_extension(r#"row.get("ts")"#));
    assert!(!spells_an_extension(r#"Some("tsx") => SourceType::tsx(),"#));
}
