//! The reviewed repo's own `typescript` parser, for the JavaScript and
//! TypeScript files oxc cannot parse cleanly. Nothing here applies to another
//! language: blast radius reads only JS/TS sources, and this module exists to
//! match what the scripts do with TypeScript's parser.
//!
//! The scripts parse every file with `ts.createSourceFile`, which recovers a
//! tree from code with syntax errors. oxc recovers differently, or not at
//! all, so on such a file the port and the script disagree. Every per-file
//! step therefore asks [`needs_typescript`] first: a file oxc parses with no
//! diagnostic, nested no deeper than [`TS_NESTING`], goes through oxc (the
//! differential tests hold the two parsers equal on that code); any other
//! file goes to the script's own function, run in the embedded engine
//! ([`crate::js_engine`]) with the `typescript.js` the script would load
//! (`loadTypescript([repoDir, cwd])`), read from disk. No `node` runs.
//!
//! The functions run are `js/blast-radius.mjs` and `js/attention-facts.mjs`
//! as this binary was built with them. The engine starts on the first file
//! that needs it and serves the rest of the run. Without a `typescript`
//! package, or when the engine cannot answer, the step keeps the oxc answer
//! and the file is listed as `unresolved` in [`TypeScriptFallback::report`].

use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::ast_bindings::{self, guarded, Bound, Ref, ScriptKind};
use crate::attention_facts::{self, Added, Edges, Fact};
use crate::js_engine::{JsEngine, Setup, Stack};

/// Nesting from which a file goes to TypeScript, well below where its
/// parser runs out of stack, so the engine's calibrated stack decides every
/// file near that depth.
pub const TS_NESTING: usize = 256;

/// The levels of `[` at which TypeScript's parser first runs out of stack
/// under `node` (v24.14.0, default stack, a fresh process; 468 levels of
/// object literals, 1655 of blocks). The engine's stack is sized to fail
/// there too.
pub const V8_ARRAY_DEPTH: u32 = 611;

const SOURCES: [(&str, &str); 3] = [
    (
        "attention-facts.mjs",
        include_str!("js/attention-facts.mjs"),
    ),
    ("blast-radius.mjs", include_str!("js/blast-radius.mjs")),
    ("relay.mjs", include_str!("js/relay.mjs")),
];

const DRIVER: &str = include_str!("js/typescript_driver.mjs");

/// Does `file` go to TypeScript: oxc reports a diagnostic on it, or it nests
/// deeper than [`TS_NESTING`].
#[must_use]
pub fn needs_typescript(file: &str, content: &str) -> bool {
    if ast_bindings::nesting_depth(content) > TS_NESTING {
        return true;
    }
    let (errors, fatal) = ast_bindings::parse_errors(file, content, ScriptKind::of(file));
    fatal || !errors.is_empty()
}

/// The entry file `require("typescript")` loads from `dir`: the nearest
/// `node_modules/typescript` at or above it, then its `exports` (the
/// `node`, `require` or `default` condition, in the order the package lists
/// them), or its `main`, or `index.js`. `NODE_PATH` and the global folders
/// are not searched.
#[must_use]
pub fn resolve_typescript(dir: &Path) -> Option<PathBuf> {
    fn condition(v: &Value) -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            Value::Object(m) => {
                if let Some(dot) = m.get(".") {
                    return condition(dot);
                }
                m.iter()
                    .filter(|(k, _)| matches!(k.as_str(), "node" | "require" | "default"))
                    .find_map(|(_, v)| condition(v))
            }
            Value::Array(a) => a.iter().find_map(condition),
            _ => None,
        }
    }
    for at in dir.ancestors() {
        if at.file_name().is_some_and(|n| n == "node_modules") {
            continue;
        }
        let pkg = at.join("node_modules").join("typescript");
        let Ok(text) = std::fs::read_to_string(pkg.join("package.json")) else {
            continue;
        };
        let manifest: Value = serde_json::from_str(&text).ok()?;
        let entry = manifest
            .get("exports")
            .and_then(condition)
            .or_else(|| manifest["main"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "index.js".to_owned());
        let base = pkg.join(entry);
        let candidates = [
            base.clone(),
            base.with_extension("js"),
            base.join("index.js"),
        ];
        return candidates.into_iter().find(|p| p.is_file());
    }
    None
}

/// What the script's function answered.
#[derive(Debug, PartialEq)]
enum Answer {
    Value(Value),
    /// It threw: the script's `catch` drops the step.
    Threw,
    /// No `typescript`, or the engine cannot answer this call.
    Unavailable,
}

/// The driver's answer line as an [`Answer`]. An answer serde cannot read
/// (a lone surrogate, which `JSON.stringify` escapes), a stand-in `node:*`
/// member, or no answer at all is one the engine cannot give.
fn answer_of(out: Option<&str>) -> Answer {
    match out.and_then(|o| serde_json::from_str::<Value>(o).ok()) {
        Some(o) if o["ok"] == json!(true) => Answer::Value(o["value"].clone()),
        Some(o) if o.get("unsupported").is_none() => Answer::Threw,
        _ => Answer::Unavailable,
    }
}

enum State {
    Untried,
    Ready(JsEngine),
    Unavailable,
}

pub struct TypeScriptFallback {
    dirs: Vec<PathBuf>,
    state: State,
    version: Option<String>,
    error: Option<String>,
    clean: HashMap<(String, u64), bool>,
    resolved: BTreeSet<String>,
    unresolved: BTreeSet<String>,
}

impl TypeScriptFallback {
    /// `dirs` are where `typescript` is looked for, in order: the script's
    /// `[repoDir, process.cwd()]`.
    #[must_use]
    pub fn new(dirs: Vec<PathBuf>) -> Self {
        Self {
            dirs,
            state: State::Untried,
            version: None,
            error: None,
            clean: HashMap::new(),
            resolved: BTreeSet::new(),
            unresolved: BTreeSet::new(),
        }
    }

    /// As the script looks for `typescript`: from the repo, then from the
    /// working directory.
    #[must_use]
    pub fn for_repo(repo_dir: &str) -> Self {
        let mut dirs = vec![PathBuf::from(repo_dir)];
        if let Ok(cwd) = std::env::current_dir() {
            dirs.push(cwd);
        }
        Self::new(dirs)
    }

    fn needs(&mut self, file: &str, content: &str) -> bool {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut h);
        *self
            .clean
            .entry((file.to_owned(), h.finish()))
            .or_insert_with(|| needs_typescript(file, content))
    }

    /// `{typescript, files, unresolved, error}`: the version that answered,
    /// the files it answered for, the files that needed it when it could
    /// not, and why it could not start.
    #[must_use]
    pub fn report(&self) -> Value {
        json!({
            "typescript": self.version,
            "files": self.resolved.iter().collect::<Vec<_>>(),
            "unresolved": self.unresolved.iter().collect::<Vec<_>>(),
            "error": self.error,
        })
    }

    async fn start(&mut self) -> Result<JsEngine, String> {
        let entry = self
            .dirs
            .iter()
            .find_map(|d| resolve_typescript(d))
            .ok_or_else(|| {
                "no typescript package from the repo or the working directory".to_owned()
            })?;
        let source = std::fs::read_to_string(&entry)
            .map_err(|e| format!("cannot read {}: {e}", entry.display()))?;
        let setup = Setup {
            scripts: vec![(entry.display().to_string(), source)],
            modules: SOURCES
                .iter()
                .map(|(n, s)| ((*n).to_owned(), (*s).to_owned()))
                .collect(),
            entry: ("typescript-driver.mjs".to_owned(), DRIVER.to_owned()),
            hello: "ts.version".to_owned(),
            call: "__tribunalTypescriptCall".to_owned(),
            stack: Stack::Calibrated {
                probe: "__tribunalTypescriptProbe".to_owned(),
                target: V8_ARRAY_DEPTH,
            },
        };
        let (engine, version) = JsEngine::start(setup).await?;
        self.version = Some(version);
        Ok(engine)
    }

    async fn call(&mut self, files: &[&str], func: &str, args: &Value) -> Answer {
        if matches!(self.state, State::Untried) {
            self.state = match self.start().await {
                Ok(engine) => State::Ready(engine),
                Err(e) => {
                    self.error = Some(e);
                    State::Unavailable
                }
            };
        }
        let answer = match &self.state {
            State::Ready(engine) => {
                let out = match serde_json::to_string(&json!({"fn": func, "args": args})) {
                    Ok(line) => engine.call(line).await,
                    Err(_) => None,
                };
                answer_of(out.as_deref())
            }
            _ => Answer::Unavailable,
        };
        let names = files.iter().map(|f| (*f).to_owned());
        if matches!(answer, Answer::Unavailable) {
            self.unresolved.extend(names);
        } else {
            self.resolved.extend(names);
        }
        answer
    }

    /// `bindingRefs`, `None` when the file binds nothing from `target` or the
    /// step threw.
    pub async fn binding_refs(
        &mut self,
        importer: &str,
        content: &str,
        target: &str,
        name: &str,
        aliases: &[(String, String)],
    ) -> Option<Bound> {
        let oxc = || {
            guarded(|| ast_bindings::binding_refs(importer, content, target, name, aliases))
                .flatten()
        };
        if !self.needs(importer, content) {
            return oxc();
        }
        let args = json!({"file": importer, "content": content, "target": target, "name": name, "aliases": aliases});
        match self.call(&[importer], "bindingRefs", &args).await {
            Answer::Value(v) => bound_of(&v),
            Answer::Threw => None,
            Answer::Unavailable => oxc(),
        }
    }

    /// `returnsPromise`; the outer `None` is a step that threw.
    pub async fn returns_promise(
        &mut self,
        file: &str,
        content: &str,
        name: &str,
    ) -> Option<Option<bool>> {
        let oxc = || guarded(|| ast_bindings::returns_promise(file, content, name));
        if !self.needs(file, content) {
            return oxc();
        }
        let args = json!({"file": file, "content": content, "name": name});
        match self.call(&[file], "returnsPromise", &args).await {
            Answer::Value(v) => Some(v.as_bool()),
            Answer::Threw => None,
            Answer::Unavailable => oxc(),
        }
    }

    async fn facts(
        &mut self,
        files: &[&str],
        func: &str,
        args: &Value,
    ) -> Option<Option<Vec<Fact>>> {
        match self.call(files, func, args).await {
            Answer::Value(v) => Some(Some(
                v.as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(attention_facts::fact_of)
                    .collect(),
            )),
            Answer::Threw => Some(None),
            Answer::Unavailable => None,
        }
    }

    /// `factsForFile` then `statusOrderFacts`, inside one `try`: `None` when
    /// either threw.
    pub async fn own_facts(
        &mut self,
        file: &str,
        content: &str,
        added: &Added,
    ) -> Option<Vec<Fact>> {
        let oxc = || {
            guarded(|| {
                let mut v = attention_facts::facts_for_file(file, content, added);
                v.extend(attention_facts::status_order_facts(file, content, added));
                v
            })
        };
        if !self.needs(file, content) {
            return oxc();
        }
        let args = json!({"file": file, "content": content, "added": added.lines()});
        let Some(first) = self.facts(&[file], "factsForFile", &args).await else {
            return oxc();
        };
        let Some(second) = self.facts(&[file], "statusOrderFacts", &args).await else {
            return oxc();
        };
        let mut v = first?;
        v.extend(second?);
        Some(v)
    }

    /// `testFacts`, `None` when it threw.
    pub async fn test_facts(
        &mut self,
        file: &str,
        content: &str,
        added: &Added,
    ) -> Option<Vec<Fact>> {
        let oxc = || guarded(|| attention_facts::test_facts(file, content, added));
        if !self.needs(file, content) {
            return oxc();
        }
        let args = json!({"file": file, "content": content, "added": added.lines()});
        self.facts(&[file], "testFacts", &args)
            .await
            .unwrap_or_else(oxc)
    }

    /// `contractFacts` over the base and head text, `None` when it threw.
    /// TypeScript answers when either side needs it.
    pub async fn contract_facts(
        &mut self,
        file: &str,
        base: &str,
        head: &str,
    ) -> Option<Vec<Fact>> {
        let oxc = || guarded(|| attention_facts::contract_facts(file, base, head));
        if !self.needs(file, base) && !self.needs(file, head) {
            return oxc();
        }
        let args = json!({"file": file, "base": base, "content": head});
        self.facts(&[file], "contractFacts", &args)
            .await
            .unwrap_or_else(oxc)
    }

    /// `edgesForFile`, `None` when it threw.
    pub(crate) async fn edges_for_file(&mut self, file: &str, content: &str) -> Option<Edges> {
        let oxc = || guarded(|| attention_facts::edges_for_file(file, content));
        if !self.needs(file, content) {
            return oxc();
        }
        let args = json!({"file": file, "content": content});
        match self.call(&[file], "edgesForFile", &args).await {
            Answer::Value(v) => attention_facts::edges_of(&v),
            Answer::Threw => None,
            Answer::Unavailable => oxc(),
        }
    }
}

fn bound_of(v: &Value) -> Option<Bound> {
    let o = v.as_object()?;
    let refs = o
        .get("refs")?
        .as_array()?
        .iter()
        .map(|r| {
            Some(Ref {
                line: usize::try_from(r["line"].as_u64()?).ok()?,
                call: r["call"].as_bool()?,
                awaited: r["awaited"].as_bool(),
                in_try: r["inTry"].as_bool(),
                text: r["text"].as_str()?.to_owned(),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Bound {
        refs,
        reexports: o.get("reexports").and_then(Value::as_bool).unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_line_reads_as_the_script_meant_it() {
        assert_eq!(
            answer_of(Some(r#"{"ok":true,"value":[1]}"#)),
            Answer::Value(json!([1]))
        );
        assert_eq!(
            answer_of(Some(r#"{"ok":true,"value":null}"#)),
            Answer::Value(Value::Null)
        );
        assert_eq!(
            answer_of(Some(r#"{"ok":false,"error":"x"}"#)),
            Answer::Threw
        );
        assert_eq!(
            answer_of(Some(
                r#"{"ok":false,"unsupported":"EMBEDDED-UNSUPPORTED: node:fs.x"}"#
            )),
            Answer::Unavailable
        );
        assert_eq!(
            answer_of(Some(r#"{"ok":true,"value":"\ud800"}"#)),
            Answer::Unavailable
        );
        assert_eq!(answer_of(None), Answer::Unavailable);
    }
}
