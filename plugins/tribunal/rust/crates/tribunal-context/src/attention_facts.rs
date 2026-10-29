//! Port of `lib/attention-facts.mjs`: lines a PR adds that match a defect
//! class a lens has missed before, found by parsing. Each fact is a question
//! for the lens, never a finding.
//!
//! The script parses with the reviewed repo's `typescript`; this port parses
//! with oxc through [`crate::ast_bindings::Tree`]. The facts, their order and
//! the cap are the script's. `parser` names oxc instead of `typescript
//! <version>`.

use std::collections::{HashMap, HashSet};

use oxc_ast::ast::{Argument, AssignmentOperator, AssignmentTarget, Expression, PropertyKey};
use oxc_ast::AstKind;
use oxc_span::GetSpan;
use serde_json::{json, Value};
use tribunal_core::js;

use crate::ast_bindings::{
    arg_string_like, squeeze, string_like, with_tree, Body, ScriptKind, Tag, Tree, PARSER,
};
use crate::blast_radius::{is_test, utf16_prefix, Repo};
use crate::import_adjacency::source_ext;

pub const MAX_FACTS: usize = 12;
const MAX_EDGE_FILES: usize = 40;

fn kind_rank(kind: &str) -> u8 {
    match kind {
        "payload-parity" | "writer-parity" | "status-order" => 0,
        "contract-change" | "exit-before-finally" | "job-without-handler" => 1,
        "mocks-subject" | "test-without-assertion" => 2,
        "unawaited-enqueue" => 3,
        "new-emit" => 4,
        "sentinel-from-catch" => 5,
        _ => 9,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub kind: &'static str,
    pub file: String,
    pub line: usize,
    pub detail: String,
}

impl Fact {
    fn new(kind: &'static str, file: &str, line: usize, detail: String) -> Self {
        Self {
            kind,
            file: file.to_owned(),
            line,
            detail,
        }
    }

    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({"kind": self.kind, "file": self.file, "line": self.line, "detail": self.detail})
    }
}

/// A `Set` of line numbers. Lines are doubles in the script (`Number(h[1])`
/// then `line++`), so a hunk start too long for an integer still compares the
/// way JavaScript compares it.
#[derive(Debug, Clone, Default)]
pub struct Added {
    order: Vec<f64>,
    bits: HashSet<u64>,
}

impl Added {
    fn add(&mut self, l: f64) {
        if self.bits.insert(l.to_bits()) {
            self.order.push(l);
        }
    }

    #[must_use]
    pub fn has(&self, line: usize) -> bool {
        #[allow(clippy::cast_precision_loss)]
        let l = line as f64;
        self.bits.contains(&l.to_bits())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The lines in insertion order, as the script's `Set` iterates them.
    #[must_use]
    pub fn lines(&self) -> Vec<f64> {
        self.order.clone()
    }

    fn any_within(&self, start: usize, end: usize) -> bool {
        #[allow(clippy::cast_precision_loss)]
        let (s, e) = (start as f64, end as f64);
        self.order.iter().any(|&l| l >= s && l <= e)
    }

    /// From explicit lines, for tests.
    #[must_use]
    pub fn of(lines: &[usize]) -> Self {
        let mut a = Self::default();
        for &l in lines {
            #[allow(clippy::cast_precision_loss)]
            a.add(l as f64);
        }
        a
    }
}

/// `^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@`: the new-side start digits.
fn hunk_new_start(raw: &str) -> Option<&str> {
    let b = raw.as_bytes();
    let digits = |i: usize| {
        let mut j = i;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        (j > i).then_some(j)
    };
    let count = |i: usize| {
        if b.get(i) == Some(&b',') {
            if let Some(j) = digits(i + 1) {
                return j;
            }
        }
        i
    };
    let rest = raw.strip_prefix("@@ -")?;
    let i = count(digits(raw.len() - rest.len())?);
    if !raw[i..].starts_with(" +") {
        return None;
    }
    let s = i + 2;
    let e = digits(s)?;
    raw[count(e)..].starts_with(" @@").then(|| &raw[s..e])
}

fn diff_file_name(raw: &str) -> Option<String> {
    let p = js::trim(&raw[4..]);
    let name = p.strip_prefix("b/").unwrap_or(p);
    (p != "/dev/null" && !name.is_empty()).then(|| name.to_owned())
}

/// `addedLines(diff)`: per file in first-seen order, the new-side lines the
/// diff adds (deletions never count).
#[must_use]
pub fn added_lines(diff: &str) -> Vec<(String, Added)> {
    let mut out: Vec<(String, Added)> = Vec::new();
    let mut file: Option<usize> = None;
    let mut line = 0f64;
    for raw in diff.split('\n') {
        if raw.starts_with("diff --git ") {
            file = None;
            continue;
        }
        if raw.starts_with("+++ ") {
            file = diff_file_name(raw).map(|name| match out.iter().position(|(f, _)| *f == name) {
                Some(i) => i,
                None => {
                    out.push((name, Added::default()));
                    out.len() - 1
                }
            });
            continue;
        }
        if let Some(start) = hunk_new_start(raw) {
            line = js::string_to_number(start);
            continue;
        }
        let Some(f) = file else { continue };
        if raw.starts_with("--- ") {
            continue;
        }
        if raw.starts_with('+') {
            out[f].1.add(line);
            line += 1.0;
        } else if raw.starts_with(' ') {
            line += 1.0;
        }
    }
    out
}

/// `index ([0-9a-f]{7,40})\.\.[0-9a-f]{7,40}` at the start of a line: the
/// base blob.
fn index_blob(raw: &str) -> Option<&str> {
    let rest = raw.strip_prefix("index ")?;
    let hex = |s: &str| {
        s.bytes()
            .take_while(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
            .count()
    };
    let n = hex(rest);
    if !(7..=40).contains(&n) || !rest[n..].starts_with("..") || hex(&rest[n + 2..]) < 7 {
        return None;
    }
    Some(&rest[..n])
}

/// `baseBlobs(diff)`: the blob each changed file had at the base, in the
/// script's `Map` order.
#[must_use]
pub fn base_blobs_ordered(diff: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut blob: Option<String> = None;
    for raw in diff.split('\n') {
        if raw.starts_with("diff --git ") {
            blob = None;
        }
        if let Some(b) = index_blob(raw) {
            blob = (!b.bytes().all(|c| c == b'0')).then(|| b.to_owned());
        }
        if let Some(rest) = raw.strip_prefix("+++ ") {
            if let Some(b) = &blob {
                let p = js::trim(rest);
                let name = p.strip_prefix("b/").unwrap_or(p);
                match out.iter_mut().find(|(f, _)| f == name) {
                    Some(slot) => slot.1.clone_from(b),
                    None => out.push((name.to_owned(), b.clone())),
                }
            }
        }
    }
    out
}

/// [`base_blobs_ordered`] for lookups.
#[must_use]
pub fn base_blobs(diff: &str) -> HashMap<String, String> {
    base_blobs_ordered(diff).into_iter().collect()
}

fn is_word(c: u16) -> bool {
    c < 128 && ((c as u8).is_ascii_alphanumeric() || c == u16::from(b'_'))
}

/// `^(add\w*Job|enqueue\w*)$`.
fn is_enqueue(name: &str) -> bool {
    let w = |s: &str| s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_');
    if let Some(rest) = name.strip_prefix("enqueue") {
        return w(rest);
    }
    name.strip_prefix("add")
        .and_then(|r| r.strip_suffix("Job"))
        .is_some_and(w)
}

/// `calleeName(ts, call)`.
fn callee_name<'a>(t: &Tree<'a>, call: usize) -> &'a str {
    let AstKind::CallExpression(c) = t.nodes[call].kind else {
        return "";
    };
    match &c.callee {
        Expression::Identifier(id) => id.name.as_str(),
        Expression::StaticMemberExpression(m) => m.property.name.as_str(),
        Expression::PrivateFieldExpression(m) => t.span_text(m.field.span),
        _ => "",
    }
}

/// `sentinelOf(ts, expr)`.
fn sentinel_of(e: Option<&Expression<'_>>) -> Option<&'static str> {
    match e? {
        Expression::NullLiteral(_) => Some("null"),
        Expression::BooleanLiteral(b) if !b.value => Some("false"),
        Expression::ArrayExpression(a) if a.elements.is_empty() => Some("[]"),
        Expression::Identifier(id) if id.name.as_str() == "undefined" => Some("undefined"),
        _ => None,
    }
}

struct Sentinel {
    s: &'static str,
    in_catch: bool,
    line: usize,
}

/// The sentinel returns of a function body, nested functions left out, each
/// marked by whether a catch between it and `f` returns it.
fn sentinel_returns(t: &Tree<'_>, f: usize) -> Vec<Sentinel> {
    let mut out = Vec::new();
    let Some(body) = t.body(f) else {
        return out;
    };
    t.walk(body.node(), |m| {
        if m != f && t.is_function_like(m) {
            return false;
        }
        if let AstKind::ReturnStatement(r) = t.nodes[m].kind {
            if let Some(s) = sentinel_of(r.argument.as_ref()) {
                let mut in_catch = false;
                let mut p = t.parent(m);
                while let Some(q) = p {
                    if q == f {
                        break;
                    }
                    if t.tag(q) == Tag::Catch {
                        in_catch = true;
                    }
                    p = t.parent(q);
                }
                out.push(Sentinel {
                    s,
                    in_catch,
                    line: t.line(m),
                });
            }
        }
        true
    });
    out
}

/// An assignment `x = ...` whose left side TypeScript sees as a bare
/// identifier: its name.
fn assigned_identifier<'a>(t: &Tree<'a>, i: usize) -> Option<&'a str> {
    let AstKind::AssignmentExpression(a) = t.nodes[i].kind else {
        return None;
    };
    if a.operator != AssignmentOperator::Assign {
        return None;
    }
    t.assign_left_text(i)?;
    match &a.left {
        AssignmentTarget::AssignmentTargetIdentifier(id) => Some(id.name.as_str()),
        _ => None,
    }
}

/// `factsForFile(ts, file, content, added)`.
#[must_use]
pub fn facts_for_file(file: &str, content: &str, added: &Added) -> Vec<Fact> {
    with_tree(file, content, ScriptKind::of(file), |t| {
        facts_for_tree(t, file, added)
    })
}

#[allow(clippy::too_many_lines)]
fn facts_for_tree(t: &Tree<'_>, file: &str, added: &Added) -> Vec<Fact> {
    let mut facts = Vec::new();
    let mut called_already: HashSet<&str> = HashSet::new();
    let mut declared: HashMap<&str, usize> = HashMap::new();
    for (s, _) in t.top_level() {
        if t.tag(s) == Tag::FunctionDecl {
            if let Some(f) = t.function(s) {
                if let (Some(id), Some(_)) = (&f.id, &f.body) {
                    declared.insert(id.name.as_str(), s);
                }
            }
        }
        if matches!(t.nodes[s].kind, AstKind::VariableDeclaration(_)) {
            for &d in &t.nodes[s].children {
                if let Some((name, Some(init))) = t.declared_function(d) {
                    if matches!(t.body(init), Some(Body::Block(_))) {
                        declared.insert(name, init);
                    }
                }
            }
        }
    }
    for n in t.preorder(0) {
        if let AstKind::TryStatement(ts) = t.nodes[n].kind {
            if let Some(fin) = &ts.finalizer {
                let fin_node = t.child_at(n, fin.span).expect("the finally block");
                let try_node = t.child_at(n, ts.block.span).expect("the try block");
                let mut read_in_finally: HashSet<&str> = HashSet::new();
                t.walk(fin_node, |m| {
                    if t.tag(m) == Tag::Identifier {
                        read_in_finally.insert(t.nodes[m].name);
                    }
                    true
                });
                let mut fills: Vec<(&str, u32)> = Vec::new();
                t.walk(try_node, |m| {
                    if let Some(name) = assigned_identifier(t, m) {
                        if read_in_finally.contains(name) {
                            fills.push((name, t.nodes[m].start));
                        }
                    }
                    if let AstKind::CallExpression(c) = t.nodes[m].kind {
                        if let Expression::StaticMemberExpression(p) = &c.callee {
                            if matches!(
                                p.property.name.as_str(),
                                "push" | "add" | "set" | "unshift"
                            ) {
                                if let Expression::Identifier(id) = &p.object {
                                    if read_in_finally.contains(id.name.as_str()) {
                                        fills.push((id.name.as_str(), t.nodes[m].start));
                                    }
                                }
                            }
                        }
                    }
                    true
                });
                let fin_line = t.line_of(fin.span.start);
                t.walk(try_node, |m| {
                    if t.is_function_like(m) {
                        return false;
                    }
                    let exit = match t.tag(m) {
                        Tag::Return => "return",
                        Tag::Throw => "throw",
                        _ => return true,
                    };
                    if added.has(t.line(m)) {
                        let mut later: Vec<&str> = Vec::new();
                        for &(name, pos) in &fills {
                            if pos > t.nodes[m].start && !later.contains(&name) {
                                later.push(name);
                            }
                        }
                        if !later.is_empty() {
                            facts.push(Fact::new(
                                "exit-before-finally",
                                file,
                                t.line(m),
                                format!(
                                    "a {exit} added inside a try whose finally (line {fin_line}) reads {}, which the try fills only after this line",
                                    later.join(", ")
                                ),
                            ));
                        }
                    }
                    true
                });
            }
        }
        if let AstKind::CallExpression(c) = t.nodes[n].kind {
            let line = t.line(n);
            if let Expression::Identifier(id) = &c.callee {
                let callee = id.name.as_str();
                if added.has(line) {
                    if let Some(&f) = declared.get(callee) {
                        let (cs, ce) = (t.nodes[n].start, t.nodes[n].end);
                        let inside = cs >= t.nodes[f].start && ce <= t.nodes[f].end;
                        let hidden = if inside || called_already.contains(callee) {
                            None
                        } else {
                            sentinel_returns(t, f).into_iter().find(|r| r.in_catch)
                        };
                        if let Some(h) = hidden {
                            called_already.insert(callee);
                            facts.push(Fact::new(
                                "sentinel-from-catch",
                                file,
                                line,
                                format!(
                                    "calls {callee}, which returns {} from a catch (line {}), so an error there arrives here as {}",
                                    h.s, h.line, h.s
                                ),
                            ));
                        }
                    }
                }
            }
            let name = callee_name(t, n);
            if is_enqueue(name)
                && added.has(line)
                && t.parent(n).is_some_and(|p| t.tag(p) == Tag::ExprStmt)
            {
                facts.push(Fact::new(
                    "unawaited-enqueue",
                    file,
                    line,
                    format!(
                        "{name}(...) is called and its promise is not awaited, returned or caught"
                    ),
                ));
            }
            if let Expression::StaticMemberExpression(m) = &c.callee {
                if m.property.name.as_str() == "emit" && added.has(line) {
                    if let Some(event) = c.arguments.first().and_then(arg_string_like) {
                        facts.push(Fact::new(
                            "new-emit",
                            file,
                            line,
                            format!("emits \"{event}\""),
                        ));
                    }
                }
            }
        }
        if t.is_function_like(n) {
            if let Some(Body::Block(_)) = t.body(n) {
                let start = t.line(n);
                let end = t.line_of(t.nodes[n].end);
                if added.any_within(start, end) {
                    let returns = sentinel_returns(t, n);
                    let mut seen: Vec<&str> = Vec::new();
                    for r in returns.iter().filter(|r| r.in_catch) {
                        if seen.contains(&r.s) {
                            continue;
                        }
                        seen.push(r.s);
                        let other = returns.iter().find(|x| !x.in_catch && x.s == r.s);
                        if let Some(o) = other {
                            facts.push(Fact::new(
                                "sentinel-from-catch",
                                file,
                                r.line,
                                format!(
                                    "returns {} from a catch (line {}) and from another branch (line {})",
                                    r.s, r.line, o.line
                                ),
                            ));
                        }
                    }
                }
            }
        }
    }
    facts
}

/// The word a repo-wide grep can find: a literal's text, the last name of
/// `A.B`, or an identifier.
fn token_of(t: &Tree<'_>, e: &Expression<'_>) -> Option<String> {
    if let Some(s) = string_like(e) {
        return Some(s);
    }
    match e {
        Expression::StaticMemberExpression(m) => Some(m.property.name.as_str().to_owned()),
        Expression::PrivateFieldExpression(m) => Some(t.span_text(m.field.span).to_owned()),
        Expression::Identifier(id) => Some(id.name.as_str().to_owned()),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct Keys {
    keys: Vec<String>,
    open: bool,
}

/// `objectKeys(ts, obj)`.
fn object_keys(e: Option<&Expression<'_>>) -> Option<Keys> {
    let Some(Expression::ObjectExpression(o)) = e else {
        return None;
    };
    let mut keys = Vec::new();
    let mut open = false;
    for p in &o.properties {
        match p {
            oxc_ast::ast::ObjectPropertyKind::SpreadProperty(_) => open = true,
            oxc_ast::ast::ObjectPropertyKind::ObjectProperty(p) if !p.computed => match &p.key {
                PropertyKey::StaticIdentifier(k) => keys.push(k.name.as_str().to_owned()),
                PropertyKey::StringLiteral(s) => keys.push(crate::js_string!(s.value)),
                _ => {}
            },
            oxc_ast::ast::ObjectPropertyKind::ObjectProperty(_) => {}
        }
    }
    Some(Keys { keys, open })
}

#[derive(Debug, Clone)]
struct Emit {
    file: String,
    line: usize,
    event: String,
    token: String,
    keys: Keys,
}

#[derive(Debug, Clone)]
struct Writer {
    file: String,
    line: usize,
    model: String,
    keys: Keys,
}

#[derive(Debug, Clone)]
struct Enqueue {
    line: usize,
    job: String,
    token: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Edges {
    emits: Vec<Emit>,
    writers: Vec<Writer>,
    enqueues: Vec<Enqueue>,
    cases: Vec<String>,
}

fn arg_expr<'b, 'a>(args: &'b [Argument<'a>], i: usize) -> Option<&'b Expression<'a>> {
    args.get(i).and_then(Argument::as_expression)
}

/// The text of argument `i` with whitespace removed, spread included.
fn arg_text(t: &Tree<'_>, args: &[Argument<'_>], i: usize) -> Option<String> {
    args.get(i).map(|a| squeeze(t.span_text(a.span())))
}

/// A fact the script returned, `None` for a kind the port does not know.
pub(crate) fn fact_of(v: &Value) -> Option<Fact> {
    const KINDS: [&str; 11] = [
        "payload-parity",
        "writer-parity",
        "status-order",
        "contract-change",
        "exit-before-finally",
        "job-without-handler",
        "mocks-subject",
        "test-without-assertion",
        "unawaited-enqueue",
        "new-emit",
        "sentinel-from-catch",
    ];
    let kind = *KINDS.iter().find(|k| v["kind"] == **k)?;
    Some(Fact {
        kind,
        file: v["file"].as_str()?.to_owned(),
        line: usize::try_from(v["line"].as_u64()?).ok()?,
        detail: v["detail"].as_str()?.to_owned(),
    })
}

fn keys_of(v: &Value) -> Option<Keys> {
    Some(Keys {
        keys: v["keys"]
            .as_array()?
            .iter()
            .map(|k| k.as_str().map(str::to_owned))
            .collect::<Option<_>>()?,
        open: v["open"].as_bool()?,
    })
}

/// The `{emits, writers, enqueues, cases}` the script's `edgesForFile`
/// returned.
pub(crate) fn edges_of(v: &Value) -> Option<Edges> {
    let list = |k: &str| v[k].as_array().cloned().unwrap_or_default();
    let s = |e: &Value, k: &str| e[k].as_str().map(str::to_owned);
    let line = |e: &Value| usize::try_from(e["line"].as_u64()?).ok();
    Some(Edges {
        emits: list("emits")
            .iter()
            .map(|e| {
                Some(Emit {
                    file: s(e, "file")?,
                    line: line(e)?,
                    event: s(e, "event")?,
                    token: s(e, "token")?,
                    keys: keys_of(e)?,
                })
            })
            .collect::<Option<_>>()?,
        writers: list("writers")
            .iter()
            .map(|e| {
                Some(Writer {
                    file: s(e, "file")?,
                    line: line(e)?,
                    model: s(e, "model")?,
                    keys: keys_of(e)?,
                })
            })
            .collect::<Option<_>>()?,
        enqueues: list("enqueues")
            .iter()
            .map(|e| {
                Some(Enqueue {
                    line: line(e)?,
                    job: s(e, "job")?,
                    token: s(e, "token")?,
                })
            })
            .collect::<Option<_>>()?,
        cases: list("cases")
            .iter()
            .map(|e| s(e, "text"))
            .collect::<Option<_>>()?,
    })
}

/// `edgesForFile(ts, file, content)`.
pub(crate) fn edges_for_file(file: &str, content: &str) -> Edges {
    with_tree(file, content, ScriptKind::of(file), |t| {
        let mut out = Edges::default();
        for n in t.preorder(0) {
            if t.tag(n) == Tag::Case {
                if let AstKind::SwitchCase(c) = t.nodes[n].kind {
                    if let Some(test) = &c.test {
                        out.cases.push(squeeze(t.span_text(test.span())));
                    }
                }
            }
            let AstKind::CallExpression(c) = t.nodes[n].kind else {
                continue;
            };
            let Expression::StaticMemberExpression(m) = &c.callee else {
                continue;
            };
            let name = m.property.name.as_str();
            let args = &c.arguments;
            let line = t.line(n);
            if name == "emit" && args.len() >= 2 {
                let k = object_keys(arg_expr(args, 1));
                let token = arg_expr(args, 0).and_then(|a| token_of(t, a));
                if let (Some(keys), Some(token)) = (k, token.filter(|s| !s.is_empty())) {
                    out.emits.push(Emit {
                        file: file.to_owned(),
                        line,
                        event: arg_text(t, args, 0).unwrap_or_default(),
                        token,
                        keys,
                    });
                }
            }
            if matches!(name, "create" | "createMany" | "upsert") {
                let model = match &m.object {
                    Expression::StaticMemberExpression(o) => {
                        Some(o.property.name.as_str().to_owned())
                    }
                    Expression::PrivateFieldExpression(o) => {
                        Some(t.span_text(o.field.span).to_owned())
                    }
                    _ => None,
                };
                if let (Some(model), Some(Expression::ObjectExpression(a0))) =
                    (model, arg_expr(args, 0))
                {
                    let key = if name == "upsert" { "create" } else { "data" };
                    let prop = a0.properties.iter().find_map(|p| match p {
                        oxc_ast::ast::ObjectPropertyKind::ObjectProperty(p)
                            if !p.computed
                                && matches!(&p.key, PropertyKey::StaticIdentifier(k) if k.name.as_str() == key) =>
                        {
                            Some(p)
                        }
                        _ => None,
                    });
                    let mut data = prop
                        .filter(|p| {
                            !p.method && !p.shorthand && p.kind == oxc_ast::ast::PropertyKind::Init
                        })
                        .map(|p| &p.value);
                    if let Some(Expression::ArrayExpression(arr)) = data {
                        data = arr.elements.first().and_then(|e| e.as_expression());
                    }
                    if let Some(keys) = object_keys(data) {
                        out.writers.push(Writer {
                            file: file.to_owned(),
                            line,
                            model,
                            keys,
                        });
                    }
                }
            }
            if name == "add" && args.len() >= 2 {
                if let Some(a0) = arg_expr(args, 0) {
                    let ok = matches!(
                        a0,
                        Expression::StaticMemberExpression(_)
                            | Expression::PrivateFieldExpression(_)
                    ) || string_like(a0).is_some();
                    if ok {
                        if let Some(token) = token_of(t, a0).filter(|s| !s.is_empty()) {
                            out.enqueues.push(Enqueue {
                                line,
                                job: arg_text(t, args, 0).unwrap_or_default(),
                                token,
                            });
                        }
                    }
                }
            }
        }
        out
    })
}

const STATUS_401: [&str; 3] = ["401", "StatusCodes.UNAUTHORIZED", "HttpStatus.UNAUTHORIZED"];
const STATUS_403: [&str; 3] = ["403", "StatusCodes.FORBIDDEN", "HttpStatus.FORBIDDEN"];

struct Answer {
    code: u16,
    pos: u32,
    line: usize,
}

/// `authAnswers(ts, sf, fn)`: 401 and 403 answers in one function body.
fn auth_answers(t: &Tree<'_>, f: usize, body: usize) -> Vec<Answer> {
    let mut out = Vec::new();
    t.walk(body, |m| {
        if m != f && t.is_function_like(m) {
            return false;
        }
        let mut code = None;
        if let AstKind::CallExpression(c) = t.nodes[m].kind {
            if let Expression::StaticMemberExpression(p) = &c.callee {
                if matches!(p.property.name.as_str(), "status" | "sendStatus") {
                    if let Some(s) = arg_text(t, &c.arguments, 0) {
                        if STATUS_401.contains(&s.as_str()) {
                            code = Some(401);
                        } else if STATUS_403.contains(&s.as_str()) {
                            code = Some(403);
                        }
                    }
                }
            }
        }
        if let AstKind::NewExpression(ne) = t.nodes[m].kind {
            if let Expression::Identifier(id) = &ne.callee {
                let s = id.name.as_str();
                if s.contains("Unauthorized") || s.contains("Unauthorised") {
                    code = Some(401);
                } else if s.contains("Forbidden") {
                    code = Some(403);
                }
            }
        }
        if let Some(code) = code {
            out.push(Answer {
                code,
                pos: t.nodes[m].start,
                line: t.line(m),
            });
        }
        true
    });
    out
}

/// `statusOrderFacts(ts, file, content, added)`.
#[must_use]
pub fn status_order_facts(file: &str, content: &str, added: &Added) -> Vec<Fact> {
    with_tree(file, content, ScriptKind::of(file), |t| {
        let mut facts = Vec::new();
        for n in t.preorder(0) {
            if !t.is_function_like(n) {
                continue;
            }
            let Some(Body::Block(b)) = t.body(n) else {
                continue;
            };
            let answers = auth_answers(t, n, b);
            for a in answers
                .iter()
                .filter(|x| x.code == 401 && added.has(x.line))
            {
                if let Some(before) = answers.iter().find(|x| x.code == 403 && x.pos < a.pos) {
                    facts.push(Fact::new(
                        "status-order",
                        file,
                        a.line,
                        format!(
                            "a 401 added here comes after the 403 at line {} in the same function, so a request that fails both checks gets the 403",
                            before.line
                        ),
                    ));
                }
            }
        }
        facts
    })
}

/// `\b(expect|assert\w*|expectTypeOf)\s*\(|\.should\b|toMatch(Inline)?Snapshot`.
fn has_assertion(text: &str) -> bool {
    let u: Vec<u16> = text.encode_utf16().collect();
    let at = |i: usize| u.get(i).copied();
    let word_at = |i: usize| at(i).is_some_and(is_word);
    let starts = |i: usize, lit: &str| {
        lit.encode_utf16()
            .enumerate()
            .all(|(k, c)| at(i + k) == Some(c))
    };
    let ws = |c: u16| char::from_u32(u32::from(c)).is_some_and(js::is_js_whitespace);
    for i in 0..u.len() {
        let boundary = i == 0 || !word_at(i - 1);
        if boundary && word_at(i) {
            let mut ends: Vec<usize> = Vec::new();
            if starts(i, "expect") {
                ends.push(i + 6);
            }
            if starts(i, "expectTypeOf") {
                ends.push(i + 12);
            }
            if starts(i, "assert") {
                let mut j = i + 6;
                ends.push(j);
                while word_at(j) {
                    j += 1;
                    ends.push(j);
                }
            }
            for e in ends {
                let mut j = e;
                while at(j).is_some_and(ws) {
                    j += 1;
                }
                if at(j) == Some(u16::from(b'(')) {
                    return true;
                }
            }
        }
        if starts(i, ".should") && !word_at(i + 7) {
            return true;
        }
        if starts(i, "toMatchSnapshot") || starts(i, "toMatchInlineSnapshot") {
            return true;
        }
    }
    false
}

/// `s.replace(/\.[^.]+$/, "")`.
fn strip_last_ext(s: &str) -> &str {
    match s.rfind('.') {
        Some(i) if i + 1 < s.len() => &s[..i],
        _ => s,
    }
}

/// `file.split("/").pop().replace(/\.(test|spec)\.[^.]+$/, "")`.
fn test_subject(file: &str) -> &str {
    let base = file.rsplit('/').next().unwrap_or(file);
    let stem = strip_last_ext(base);
    if stem.len() < base.len() {
        if let Some(s) = stem
            .strip_suffix(".test")
            .or_else(|| stem.strip_suffix(".spec"))
        {
            return s;
        }
    }
    base
}

/// `testFacts(ts, file, content, added)`.
#[must_use]
pub fn test_facts(file: &str, content: &str, added: &Added) -> Vec<Fact> {
    let subject = test_subject(file);
    with_tree(file, content, ScriptKind::of(file), |t| {
        let mut facts = Vec::new();
        for n in t.preorder(0) {
            let AstKind::CallExpression(c) = t.nodes[n].kind else {
                continue;
            };
            let line = t.line(n);
            if !added.has(line) {
                continue;
            }
            let (callee, member) = match &c.callee {
                Expression::Identifier(id) => (id.name.as_str(), None),
                Expression::StaticMemberExpression(m) => match &m.object {
                    Expression::Identifier(id) => {
                        (id.name.as_str(), Some(m.property.name.as_str()))
                    }
                    _ => ("", None),
                },
                Expression::PrivateFieldExpression(m) => match &m.object {
                    Expression::Identifier(id) => {
                        (id.name.as_str(), Some(t.span_text(m.field.span)))
                    }
                    _ => ("", None),
                },
                _ => ("", None),
            };
            let args = &c.arguments;
            let cb =
                args.last()
                    .and_then(Argument::as_expression)
                    .and_then(|e| match e {
                        Expression::ArrowFunctionExpression(_)
                        | Expression::FunctionExpression(_) => t.child_at(n, e.span()),
                        _ => None,
                    });
            let title = args.first().and_then(arg_string_like);
            if matches!(callee, "it" | "test") {
                if let (Some(cb), Some(title)) = (cb, &title) {
                    if !has_assertion(t.text(cb)) {
                        facts.push(Fact::new(
                            "test-without-assertion",
                            file,
                            line,
                            format!(
                                "the test \"{}\" asserts nothing, so it passes whatever the code does",
                                utf16_prefix(title, 80)
                            ),
                        ));
                    }
                }
            }
            if matches!(callee, "vi" | "jest") && member == Some("mock") {
                if let Some(spec) = &title {
                    let last = spec.rsplit('/').next().unwrap_or(spec);
                    if strip_last_ext(last) == subject {
                        facts.push(Fact::new(
                            "mocks-subject",
                            file,
                            line,
                            format!("mocks \"{spec}\", the module this file tests, so its assertions run against the mock"),
                        ));
                    }
                }
            }
        }
        facts
    })
}

/// One exported function's contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    pub params: usize,
    pub required: usize,
    pub sentinels: Vec<&'static str>,
    pub throws: usize,
    pub line: usize,
}

fn shape(t: &Tree<'_>, f: usize) -> Shape {
    let mut sentinels: Vec<&'static str> = Vec::new();
    let mut throws = 0;
    if let Some(body) = t.body(f) {
        t.walk(body.node(), |m| {
            if m != f && t.is_function_like(m) {
                return false;
            }
            if let AstKind::ReturnStatement(r) = t.nodes[m].kind {
                if let Some(s) = sentinel_of(r.argument.as_ref()) {
                    if !sentinels.contains(&s) {
                        sentinels.push(s);
                    }
                }
            }
            if t.tag(m) == Tag::Throw {
                throws += 1;
            }
            true
        });
    }
    let (this, params) = match t.nodes[f].kind {
        AstKind::ArrowFunctionExpression(a) => (false, &a.params),
        _ => {
            let func = t.function(f).expect("a function");
            (func.this_param.is_some(), &func.params)
        }
    };
    let n = usize::from(this) + params.items.len() + usize::from(params.rest.is_some());
    let required = usize::from(this)
        + params
            .items
            .iter()
            .filter(|p| p.initializer.is_none() && !p.optional)
            .count();
    Shape {
        params: n,
        required,
        sentinels,
        throws,
        line: t.line(f),
    }
}

/// `module.exports.name` / `exports.name` with `name` an identifier.
fn cjs_export_target(text: &str) -> Option<&str> {
    let rest = text
        .strip_prefix("module.exports.")
        .or_else(|| text.strip_prefix("exports."))?;
    let b = rest.as_bytes();
    let ok = !b.is_empty()
        && (b[0].is_ascii_alphabetic() || b[0] == b'_' || b[0] == b'$')
        && b.iter()
            .all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'$');
    ok.then_some(rest)
}

/// `Map.set`: a new key goes last, an existing one keeps its place.
fn map_set(m: &mut Vec<(String, usize)>, k: &str, v: usize) {
    match m.iter_mut().find(|(n, _)| n == k) {
        Some(slot) => slot.1 = v,
        None => m.push((k.to_owned(), v)),
    }
}

/// The assignment an expression statement holds, unparenthesized.
fn statement_assignment(t: &Tree<'_>, st: usize) -> Option<usize> {
    let AstKind::ExpressionStatement(e) = t.nodes[st].kind else {
        return None;
    };
    match &e.expression {
        Expression::AssignmentExpression(a) if a.operator == AssignmentOperator::Assign => {
            t.child_at(st, a.span)
        }
        _ => None,
    }
}

/// `exportedShapes(ts, file, content)`, in the script's map order.
#[must_use]
pub fn exported_shapes(file: &str, content: &str) -> Vec<(String, Shape)> {
    with_tree(file, content, ScriptKind::of(file), |t| {
        let mut all: HashMap<String, usize> = HashMap::new();
        let mut fns: Vec<(String, usize)> = Vec::new();
        let mut listed: Vec<String> = Vec::new();
        for (st, exported) in t.top_level() {
            if t.tag(st) == Tag::FunctionDecl {
                if let Some(id) = t.function(st).and_then(|f| f.id.as_ref()) {
                    all.insert(id.name.as_str().to_owned(), st);
                    if exported {
                        map_set(&mut fns, id.name.as_str(), st);
                    }
                }
            }
            if matches!(t.nodes[st].kind, AstKind::VariableDeclaration(_)) {
                for &d in &t.nodes[st].children {
                    if let Some((name, Some(init))) = t.declared_function(d) {
                        all.insert(name.to_owned(), init);
                        if exported {
                            map_set(&mut fns, name, init);
                        }
                    }
                }
            }
            if let Some(a) = statement_assignment(t, st) {
                let target = t.assign_left_text(a);
                if let Some(name) = target.and_then(cjs_export_target) {
                    if let Some(r) = t.assigned_function(a) {
                        map_set(&mut fns, name, r);
                    }
                }
                if target == Some("module.exports") {
                    if let AstKind::AssignmentExpression(ae) = t.nodes[a].kind {
                        if let Expression::ObjectExpression(o) = &ae.right {
                            for p in &o.properties {
                                if let oxc_ast::ast::ObjectPropertyKind::ObjectProperty(p) = p {
                                    if p.shorthand {
                                        if let PropertyKey::StaticIdentifier(k) = &p.key {
                                            let k = k.name.as_str().to_owned();
                                            if !listed.contains(&k) {
                                                listed.push(k);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        for name in listed {
            if !fns.iter().any(|(n, _)| *n == name) {
                if let Some(&f) = all.get(&name) {
                    map_set(&mut fns, &name, f);
                }
            }
        }
        fns.into_iter().map(|(k, f)| (k, shape(t, f))).collect()
    })
}

/// `contractFacts(ts, file, baseContent, headContent)`.
#[must_use]
pub fn contract_facts(file: &str, base: &str, head: &str) -> Vec<Fact> {
    let b_shapes = exported_shapes(file, base);
    let h_shapes = exported_shapes(file, head);
    let mut facts = Vec::new();
    for (name, h) in &h_shapes {
        let Some((_, b)) = b_shapes.iter().find(|(n, _)| n == name) else {
            continue;
        };
        let new_sentinels: Vec<&str> = h
            .sentinels
            .iter()
            .copied()
            .filter(|x| !b.sentinels.contains(x))
            .collect();
        let dropped: Vec<&str> = b
            .sentinels
            .iter()
            .copied()
            .filter(|x| !h.sentinels.contains(x))
            .collect();
        if !new_sentinels.is_empty() && b.throws > h.throws {
            facts.push(Fact::new(
                "contract-change",
                file,
                h.line,
                format!(
                    "{name} now returns {} where the base version threw, so callers that relied on the throw get the value instead",
                    new_sentinels.join(" or ")
                ),
            ));
        } else if h.required > b.required || h.params < b.params {
            facts.push(Fact::new(
                "contract-change",
                file,
                h.line,
                format!(
                    "{name} now takes {} parameter(s), {} required; the base version took {}, {} required",
                    h.params, h.required, b.params, b.required
                ),
            ));
        } else if h.throws > b.throws && !dropped.is_empty() {
            facts.push(Fact::new(
                "contract-change",
                file,
                h.line,
                format!(
                    "{name} now throws where the base version returned {}, so callers that checked for that value now get an exception",
                    dropped.join(" or ")
                ),
            ));
        }
    }
    facts
}

// ---------------------------------------------------------------------------
// edgeFacts

/// `new RegExp(`where\\.${k}\\b|where\\s*:\\s*\\{[^}]{0,300}\\b${k}\\b`).test(text)`
/// for a key whose only regular-expression syntax is `$`, the one an
/// identifier can hold. Any other syntax in a key is matched literally
/// (a declared difference: the script compiles it).
fn key_filtered(text: &[u16], key: &str) -> bool {
    #[derive(Clone, Copy, PartialEq)]
    enum Tok {
        Lit(u16),
        End,
    }
    let toks: Vec<Tok> = key
        .encode_utf16()
        .map(|u| {
            if u == u16::from(b'$') {
                Tok::End
            } else {
                Tok::Lit(u)
            }
        })
        .collect();
    let n = text.len();
    let word = |i: usize| i < n && is_word(text[i]);
    let boundary = |i: usize| (i > 0 && word(i - 1)) != word(i);
    let seq = |mut i: usize| -> Option<usize> {
        for &tk in &toks {
            match tk {
                Tok::End => {
                    if i != n {
                        return None;
                    }
                }
                Tok::Lit(u) => {
                    if text.get(i) != Some(&u) {
                        return None;
                    }
                    i += 1;
                }
            }
        }
        Some(i)
    };
    let lit = |i: usize, s: &str| {
        s.encode_utf16()
            .enumerate()
            .all(|(k, c)| text.get(i + k) == Some(&c))
    };
    let ws =
        |i: usize| i < n && char::from_u32(u32::from(text[i])).is_some_and(js::is_js_whitespace);
    for i in 0..n {
        if !lit(i, "where") {
            continue;
        }
        if lit(i + 5, ".") && seq(i + 6).is_some_and(boundary) {
            return true;
        }
        let mut j = i + 5;
        while ws(j) {
            j += 1;
        }
        if !lit(j, ":") {
            continue;
        }
        j += 1;
        while ws(j) {
            j += 1;
        }
        if !lit(j, "{") {
            continue;
        }
        let start = j + 1;
        for k in 0..=300 {
            let p = start + k;
            if p > n {
                break;
            }
            if k > 0 && text[p - 1] == u16::from(b'}') {
                break;
            }
            if boundary(p) && seq(p).is_some_and(boundary) {
                return true;
            }
        }
    }
    false
}

struct EdgeCache {
    edges: HashMap<String, Option<Edges>>,
}

impl EdgeCache {
    async fn of(&mut self, repo: &mut Repo<'_>, f: &str) -> Option<Edges> {
        if !self.edges.contains_key(f) {
            let e = match repo.read(f).await.filter(|c| !c.is_empty()) {
                Some(c) => repo.ts.edges_for_file(f, &c).await,
                None => None,
            };
            self.edges.insert(f.to_owned(), e);
        }
        self.edges[f].clone()
    }
}

/// `filesFor(token, self)`: `self`, then the other source files that mention
/// the token, 40 at most.
async fn files_for(repo: &Repo<'_>, token: &str, me: &str) -> Vec<String> {
    let found = repo.files_mentioning(token).await;
    std::iter::once(me.to_owned())
        .chain(
            found
                .into_iter()
                .filter(|f| f != me && source_ext(f).is_some() && !is_test(f)),
        )
        .take(MAX_EDGE_FILES)
        .collect()
}

/// The keys every one of `others` carries, in the first one's order.
fn common(others: &[&Keys]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for k in &others[0].keys {
        if !out.contains(k) {
            out.push(k.clone());
        }
    }
    out.retain(|k| others[1..].iter().all(|o| o.keys.contains(k)));
    out
}

/// `edgeFacts(ts, repo, changed)`.
pub async fn edge_facts(repo: &mut Repo<'_>, changed: &[(String, Added)]) -> Vec<Fact> {
    let mut cache = EdgeCache {
        edges: HashMap::new(),
    };
    let mut facts = Vec::new();
    for (file, added) in changed {
        let Some(mine) = cache.of(repo, file).await else {
            continue;
        };
        for e in mine
            .emits
            .iter()
            .filter(|x| added.has(x.line) && !x.keys.open)
        {
            let mut others: Vec<Emit> = Vec::new();
            for f in files_for(repo, &e.token, file).await {
                if let Some(edges) = cache.of(repo, &f).await {
                    others.extend(edges.emits.into_iter().filter(|o| {
                        o.event == e.event && !o.keys.open && !(o.file == *file && o.line == e.line)
                    }));
                }
            }
            if others.is_empty() {
                continue;
            }
            let keys: Vec<&Keys> = others.iter().map(|o| &o.keys).collect();
            let missing: Vec<String> = common(&keys)
                .into_iter()
                .filter(|k| !e.keys.keys.contains(k))
                .collect();
            if !missing.is_empty() {
                facts.push(Fact::new(
                    "payload-parity",
                    file,
                    e.line,
                    format!(
                        "emits {} without {}, which the other {} emit(s) of this event carry",
                        e.event,
                        missing.join(", "),
                        others.len()
                    ),
                ));
            }
        }
        for w in mine
            .writers
            .iter()
            .filter(|x| added.has(x.line) && !x.keys.open)
        {
            let files = files_for(repo, &w.model, file).await;
            let mut others: Vec<Writer> = Vec::new();
            for f in &files {
                if let Some(edges) = cache.of(repo, f).await {
                    others.extend(edges.writers.into_iter().filter(|o| {
                        o.model == w.model && !o.keys.open && !(o.file == *file && o.line == w.line)
                    }));
                }
            }
            if others.is_empty() {
                continue;
            }
            let keys: Vec<&Keys> = others.iter().map(|o| &o.keys).collect();
            let missing: Vec<String> = common(&keys)
                .into_iter()
                .filter(|k| !w.keys.keys.contains(k))
                .collect();
            if missing.is_empty() {
                continue;
            }
            let mut texts: Vec<Vec<u16>> = Vec::new();
            for f in &files {
                texts.push(
                    repo.read(f)
                        .await
                        .unwrap_or_default()
                        .encode_utf16()
                        .collect(),
                );
            }
            let keyed: Vec<&String> = missing
                .iter()
                .filter(|k| texts.iter().any(|t| key_filtered(t, k)))
                .collect();
            let readers = if keyed.is_empty() {
                String::new()
            } else {
                format!(
                    "; readers filter on {}",
                    keyed
                        .iter()
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            facts.push(Fact::new(
                "writer-parity",
                file,
                w.line,
                format!(
                    "creates a {} row without {}, which every other writer of this model sets{readers}",
                    w.model,
                    missing.join(", ")
                ),
            ));
        }
        for q in mine.enqueues.iter().filter(|x| added.has(x.line)) {
            let mut handled = false;
            for f in files_for(repo, &q.token, file).await {
                if cache
                    .of(repo, &f)
                    .await
                    .is_some_and(|e| e.cases.contains(&q.job))
                {
                    handled = true;
                    break;
                }
            }
            if !handled {
                facts.push(Fact::new(
                    "job-without-handler",
                    file,
                    q.line,
                    format!(
                        "enqueues {}, and no worker in the repo has a `case {}:`",
                        q.job, q.job
                    ),
                ));
            }
        }
    }
    facts
}

/// `attentionFacts(diff, repo, ts, { max })`:
/// `{facts, total, truncated, parser}`.
pub async fn attention_facts(diff: &str, repo: &mut Repo<'_>, max: usize) -> Value {
    let mut facts: Vec<Fact> = Vec::new();
    let mut changed: Vec<(String, Added)> = Vec::new();
    let bases = base_blobs(diff);
    for (file, added) in added_lines(diff) {
        if added.is_empty() || source_ext(&file).is_none() {
            continue;
        }
        let Some(content) = repo.read(&file).await.filter(|c| !c.is_empty()) else {
            continue;
        };
        // One `try` per file in the script: a throw in any step drops the
        // rest of that file's steps, and what was pushed before it stays.
        if is_test(&file) {
            facts.extend(
                repo.ts
                    .test_facts(&file, &content, &added)
                    .await
                    .unwrap_or_default(),
            );
            continue;
        }
        let Some(own) = repo.ts.own_facts(&file, &content, &added).await else {
            continue;
        };
        facts.extend(own);
        let base = match bases.get(&file) {
            Some(b) => repo.read_blob(b).await,
            None => None,
        };
        changed.push((file.clone(), added));
        if let Some(base) = base.filter(|b| !b.is_empty()) {
            facts.extend(
                repo.ts
                    .contract_facts(&file, &base, &content)
                    .await
                    .unwrap_or_default(),
            );
        }
    }
    facts.extend(edge_facts(repo, &changed).await);
    let mut seen: HashSet<(&'static str, String, usize)> = HashSet::new();
    let mut unique: Vec<Fact> = facts
        .into_iter()
        .filter(|f| seen.insert((f.kind, f.file.clone(), f.line)))
        .collect();
    unique.sort_by_key(|f| kind_rank(f.kind));
    let total = unique.len();
    json!({
        "facts": unique.iter().take(max).map(Fact::to_json).collect::<Vec<_>>(),
        "total": total,
        "truncated": total > max,
        "parser": PARSER,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_names() {
        assert!(is_enqueue("addSyncJob"));
        assert!(is_enqueue("addJob"));
        assert!(is_enqueue("enqueue"));
        assert!(!is_enqueue("addNumbers"));
        assert!(!is_enqueue("add$Job"));
    }

    #[test]
    fn subjects() {
        assert_eq!(test_subject("src/a/billing.test.ts"), "billing");
        assert_eq!(test_subject("a.test.spec.ts"), "a.test");
        assert_eq!(test_subject("x.ts"), "x.ts");
        assert_eq!(strip_last_ext("x."), "x.");
    }

    #[test]
    fn assertions() {
        assert!(has_assertion("() => { expect(x).toBe(1) }"));
        assert!(has_assertion("() => assertRefunded (1)"));
        assert!(has_assertion("x.should.equal(1)"));
        assert!(!has_assertion("() => { await refund(1); }"));
        assert!(!has_assertion("() => { reexpect(1) }"));
    }

    fn kinds_at(facts: &[Fact]) -> Vec<(&'static str, usize)> {
        facts.iter().map(|f| (f.kind, f.line)).collect()
    }

    // The expectations of `eval/replay-attention-facts.mjs`, which the script
    // meets; `tests/diff_blast_radius_ast.rs` compares the two directly.
    #[test]
    fn replay_attention_facts() {
        let upload = "export async function upload(req, res) {\n  const filesToClean = [];\n  try {\n    if (!req.ok) {\n      return res.status(403).json({});\n    }\n    req.files.forEach((f) => filesToClean.push(f.path));\n    return res.status(200).json({});\n  } finally {\n    cleanup(filesToClean);\n  }\n}";
        let f = facts_for_file("src/x.ts", upload, &Added::of(&[5]));
        assert_eq!(kinds_at(&f), [("exit-before-finally", 5)]);
        assert!(f[0].detail.contains("filesToClean"));
        assert!(facts_for_file("src/x.ts", upload, &Added::of(&[8])).is_empty());
        let hook = "export async function hook(req, res) {\n  addSyncJob(req.body);\n  await addOtherJob(req.body);\n  return enqueueThing(req.body);\n}";
        assert_eq!(
            kinds_at(&facts_for_file(
                "src/x.ts",
                hook,
                &Added::of(&[1, 2, 3, 4, 5])
            )),
            [("unawaited-enqueue", 2)]
        );
        let sentinel = "const getByVid = async (vid) => {\n  try {\n    return await db.find(vid);\n  } catch (e) {\n    return null;\n  }\n};\n\nexport async function resolve(vid) {\n  for (let i = 0; i < 3; i++) {\n    const a = await getByVid(vid);\n    if (!a) return null;\n  }\n  return null;\n}";
        assert_eq!(
            kinds_at(&facts_for_file("src/x.ts", sentinel, &Added::of(&[11]))),
            [("sentinel-from-catch", 11)]
        );
        let emit = facts_for_file(
            "src/x.ts",
            "io.to(room).emit(\"xls:done\", { id });",
            &Added::of(&[1]),
        );
        assert_eq!(emit[0].detail, "emits \"xls:done\"");
        let authz = "module.exports = (req, res, next) => {\n  if (!allowed(req)) {\n    return res.status(403).json({ code: 403 });\n  }\n  if (account.isActive === 0) {\n    return res.status(StatusCodes.UNAUTHORIZED).json({ code: 401 });\n  }\n  next();\n};";
        let s = status_order_facts("src/legacy/authorize.js", authz, &Added::of(&[6]));
        assert_eq!(kinds_at(&s), [("status-order", 6)]);
        assert!(s[0].detail.contains("403 at line 3"));
        let tests = "import { vi } from \"vitest\";\nvi.mock(\"./billing\");\nit(\"refunds the payment\", async () => {\n  await refund(1);\n});\nit(\"rejects a bad id\", () => {\n  expect(() => refund(-1)).toThrow();\n});";
        let t = test_facts(
            "src/services/billing.test.ts",
            tests,
            &Added::of(&[2, 3, 6]),
        );
        assert_eq!(
            kinds_at(&t),
            [("mocks-subject", 2), ("test-without-assertion", 3)]
        );
        let base = "export const find = async (id) => {\n  const r = await db.get(id);\n  if (!r) throw new NotFound(id);\n  return r;\n};\nexport function sum(a, b) { return a + b; }\n";
        let head = "export const find = async (id) => {\n  const r = await db.get(id);\n  if (!r) return null;\n  return r;\n};\nexport function sum(a, b, c) { return a + b + c; }\n";
        let c = contract_facts("src/x.ts", base, head);
        assert!(c[0]
            .detail
            .starts_with("find now returns null where the base version threw"));
        assert_eq!(
            c[1].detail,
            "sum now takes 3 parameter(s), 3 required; the base version took 2, 2 required"
        );
    }

    // TypeScript parses a static method named `constructor` as a constructor,
    // which is not function-like: only the method gets a fact (the script's
    // answer, TypeScript 6.0.3).
    #[test]
    fn a_static_constructor_is_a_constructor() {
        let src = "export class A {\n  static constructor() {\n    if (a) return null;\n    try { return x(); } catch { return null; }\n  }\n  m() {\n    if (a) return null;\n    try { return x(); } catch { return null; }\n  }\n}\n";
        let all = Added::of(&(1..=10).collect::<Vec<_>>());
        assert_eq!(
            kinds_at(&facts_for_file("src/a.ts", src, &all)),
            [("sentinel-from-catch", 8)]
        );
    }

    #[test]
    fn diff_parsing() {
        let diff = "diff --git a/src/a.ts b/src/a.ts\nindex 1a2b3c4..5d6e7f8 100644\n--- a/src/a.ts\n+++ b/src/a.ts\n@@ -1,2 +1,3 @@\n export function f() {\n+  addSyncJob(1);\n }\n-gone\ndiff --git a/src/n.ts b/src/n.ts\nindex 0000000..9a9a9a9\n--- /dev/null\n+++ b/src/n.ts\n";
        let lines = added_lines(diff);
        assert_eq!(lines[0].0, "src/a.ts");
        assert_eq!(lines[0].1.lines(), [2.0]);
        assert_eq!(
            base_blobs_ordered(diff),
            [("src/a.ts".to_owned(), "1a2b3c4".to_owned())]
        );
    }

    #[test]
    fn key_regex() {
        let t = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        assert!(key_filtered(&t("if (x) where.contactId = 1"), "contactId"));
        assert!(key_filtered(
            &t("findMany({ where: { a: 1, contactId } })"),
            "contactId"
        ));
        assert!(!key_filtered(&t("where.contactIds"), "contactId"));
        assert!(!key_filtered(&t("where: { } contactId"), "contactId"));
    }
}
