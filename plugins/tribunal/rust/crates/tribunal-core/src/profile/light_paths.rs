use serde_json::Value;

use super::js_ext::JsRegex;

/// Compile valid string entries from `lightPaths`; invalid entries are returned as warnings.
pub fn rules(profile: &Value) -> (Vec<JsRegex>, Vec<String>) {
    let Some(value) = profile.get("lightPaths") else {
        return (Vec::new(), Vec::new());
    };
    let Some(entries) = value.as_array() else {
        return (
            Vec::new(),
            vec!["lightPaths must be an array of regular expressions".into()],
        );
    };
    let mut rules = Vec::new();
    let mut warnings = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let Some(pattern) = entry.as_str() else {
            warnings.push(format!("lightPaths[{i}] must be a string; ignored"));
            continue;
        };
        match JsRegex::new_ignore_case(pattern) {
            Ok(rule) => rules.push(rule),
            Err(error) => warnings.push(format!("lightPaths[{i}] is invalid ({error}); ignored")),
        }
    }
    (rules, warnings)
}

/// A file with no behaviour to question: lockfiles, snapshots, minified or
/// built output, generated code, binaries and prose docs. A question about one
/// is not answered. Across 256 labelled question findings, the two on docs
/// were not defects, and none of the 247 labelled corpus defects is in a file
/// of these kinds. Tests, config and migrations stay: the corpus has real
/// defects in each. Shared by question routing and `--light-no-logic`.
#[must_use]
pub fn no_logic_file(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    let name = p.rsplit('/').next().unwrap_or(&p);
    const NAMES: [&str; 9] = [
        "package-lock.json",
        "npm-shrinkwrap.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "cargo.lock",
        "poetry.lock",
        "gemfile.lock",
        "composer.lock",
        "go.sum",
    ];
    const SUFFIXES: [&str; 23] = [
        ".lock", ".snap", ".min.js", ".min.css", ".map", ".pb.go", "_pb2.py", ".md", ".mdx",
        ".rst", ".png", ".jpg", ".jpeg", ".gif", ".ico", ".webp", ".pdf", ".woff", ".woff2",
        ".ttf", ".eot", ".zip", ".gz",
    ];
    const DIRS: [&str; 5] = [
        "__snapshots__",
        "node_modules",
        "dist",
        "generated",
        "__generated__",
    ];
    NAMES.contains(&name)
        || SUFFIXES.iter().any(|s| name.ends_with(s))
        || name.contains(".generated.")
        || p.split('/').rev().skip(1).any(|d| DIRS.contains(&d))
}
