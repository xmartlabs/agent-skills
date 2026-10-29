//! The parser half of `lib/blast-radius.mjs` (`bindingRefs`,
//! `returnsPromise`) and the syntax layer `lib/attention-facts.mjs` stands on.
//!
//! The script parses with the reviewed repo's `typescript` package
//! (`ts.createSourceFile`, syntax only). This port parses with oxc and reads
//! the result through [`Tree`], a view shaped like the TypeScript AST where
//! the two differ in a way the script can observe:
//!
//! - `a?.b()` has no `ChainExpression` above it: the call's parent is what
//!   TypeScript says it is.
//! - A method's `Function` is folded into the method node, so a class or
//!   object method is one function-like node, and a constructor or accessor
//!   is none (TypeScript's `isFunctionLike` checks in the script leave those
//!   out).
//! - Every place TypeScript has an `Identifier` has one identifier node: oxc's
//!   two nodes at one span (`{ a }`, `import { a }`) become one, and
//!   `import.meta` / `new.target` get the `meta` / `target` identifier
//!   TypeScript gives them.
//! - A function declaration under `export` starts at `export`, as a
//!   TypeScript node with that modifier does.
//! - `implements a.B` and an interface's `extends a.B` are property accesses
//!   (TypeScript reads a heritage clause as an expression), not qualified
//!   names.
//! - A class element named `constructor` is a constructor even when static,
//!   with no name node and not function-like, as TypeScript parses it.
//! - Parentheses stay (`preserve_parens`), as in TypeScript. Where oxc drops
//!   them, around an assignment target, [`Tree::assign_left_text`] says so.
//!
//! Line numbers follow TypeScript's `getLineAndCharacterOfPosition`: `\n`,
//! `\r\n`, a lone `\r`, U+2028 and U+2029 each end a line. Positions are
//! compared as UTF-8 offsets, which order the same way as TypeScript's UTF-16
//! ones.

use std::collections::HashSet;
use std::sync::Mutex;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrowFunctionBody, AssignmentOperator, BindingPattern, ChainElement, Expression,
    Function, FunctionType, ImportDeclarationSpecifier, JSXMemberExpressionObject,
    MethodDefinitionKind, ModuleExportName, PropertyKey, PropertyKind, TSTypeName, UnaryOperator,
};
use oxc_ast::AstKind;
use oxc_ast_visit::Visit;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::{GetSpan, SourceType, Span};
use tribunal_core::js;

use crate::blast_radius::{spec_points_at, utf16_prefix};

/// What the port names in `attention.parser`, where the script names
/// `typescript <version>`.
pub const PARSER: &str = "oxc_parser 0.155.0";
const TEXT_CHARS: usize = 100;

/// TypeScript's `ScriptKind` for a path, which picks JSX or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptKind {
    Ts,
    Tsx,
    Js,
    Jsx,
}

fn js_ext(file: &str) -> bool {
    file.ends_with(".js") || file.ends_with(".mjs") || file.ends_with(".cjs")
}

impl ScriptKind {
    /// `bindingRefs`, `returnsPromise`, `factsForFile` and attention-facts'
    /// `parse`:
    /// `.tsx`, `.jsx`, `.js|.mjs|.cjs`, else TS.
    #[must_use]
    pub fn of(file: &str) -> Self {
        if file.ends_with(".tsx") {
            Self::Tsx
        } else if file.ends_with(".jsx") {
            Self::Jsx
        } else if js_ext(file) {
            Self::Js
        } else {
            Self::Ts
        }
    }

    fn source_type(self, file: &str) -> SourceType {
        match self {
            Self::Ts => SourceType::ts().with_typescript_definition(file.ends_with(".d.ts")),
            Self::Tsx => SourceType::tsx(),
            // TypeScript parses every JS file with the JSX variant, as a
            // module or a script by what it contains.
            Self::Js | Self::Jsx => SourceType::unambiguous().with_jsx(true),
        }
    }
}

/// The TypeScript node kinds the script tests for. Everything else is
/// [`Tag::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Identifier,
    /// `PropertyAccessExpression`: `a.b`, `a.#b`, and `<a.b>` in JSX.
    PropertyAccess,
    Call,
    /// `import(...)`, a `CallExpression` to TypeScript.
    ImportCall,
    New,
    Await,
    Return,
    Throw,
    Arrow,
    FunctionExpr,
    FunctionDecl,
    /// `MethodDeclaration`: a class or object method, not a constructor or
    /// accessor.
    Method,
    /// `VariableDeclaration` (oxc's `VariableDeclarator`).
    VarDecl,
    ExprStmt,
    Try,
    Catch,
    Block,
    /// `CaseClause` (a `default:` is not one).
    Case,
    Other,
}

pub struct Node<'a> {
    pub kind: AstKind<'a>,
    pub tag: Tag,
    pub start: u32,
    pub end: u32,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// The identifier's text, or the property name of a property access.
    pub name: &'a str,
}

impl Node<'_> {
    #[must_use]
    pub fn span(&self) -> Span {
        Span::new(self.start, self.end)
    }
}

/// A parsed file seen as TypeScript would shape it.
pub struct Tree<'a> {
    pub src: &'a str,
    pub nodes: Vec<Node<'a>>,
    line_starts: Vec<usize>,
}

/// A function-like node's body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    Block(usize),
    Expr(usize),
}

impl Body {
    #[must_use]
    pub fn node(self) -> usize {
        match self {
            Self::Block(i) | Self::Expr(i) => i,
        }
    }
}

fn line_starts(src: &str) -> Vec<usize> {
    let b = src.as_bytes();
    let mut out = vec![0];
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\r' => {
                if b.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                out.push(i + 1);
            }
            b'\n' => out.push(i + 1),
            0xE2 if b.get(i + 1) == Some(&0x80) && matches!(b.get(i + 2), Some(0xA8 | 0xA9)) => {
                i += 2;
                out.push(i + 1);
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Nesting the port parses at most. TypeScript's recursive parser runs out
/// of V8's stack at about this depth (the script then catches the
/// `RangeError` and the file yields nothing); oxc's would abort the process,
/// so a file nested deeper is not parsed at all, which yields the same
/// nothing.
pub const MAX_NESTING: usize = 1000;
/// The stack a parse runs on: deep enough for [`MAX_NESTING`] levels of any
/// construct in a debug build, reserved rather than committed.
const PARSE_STACK: usize = 256 << 20;

/// The deepest nesting of `(`, `[`, `{` and `${` in `src`, outside comments,
/// strings, template text and regular expressions.
///
/// A quote that does not close on its own line opens no string (a string
/// cannot span lines), and a `/` where an operand is expected starts a
/// regular expression when it closes on its line. Without both, a quote or a
/// bracket in a regular expression or in JSX text hid a closing bracket on
/// every such line, and a long but shallow file passed the limit.
#[must_use]
pub fn nesting_depth(src: &str) -> usize {
    #[derive(PartialEq)]
    enum Mode {
        Code,
        Template,
    }
    let b = src.as_bytes();
    let mut modes = vec![Mode::Code];
    // Per code level: how deep the brackets go inside it.
    let mut depth = 0usize;
    let mut max = 0usize;
    let mut opened_at: Vec<usize> = Vec::new();
    // The last significant byte in code, `a` after an operand that is not
    // a plain byte (a string, a template or a regular expression).
    let mut prev = b'\n';
    let mut i = 0;
    while i < b.len() {
        if *modes.last().expect("a mode") == Mode::Template {
            match b[i] {
                b'\\' => i += 1,
                b'`' => {
                    modes.pop();
                    prev = b'a';
                }
                b'$' if b.get(i + 1) == Some(&b'{') => {
                    i += 1;
                    modes.push(Mode::Code);
                    opened_at.push(depth);
                    depth += 1;
                    max = max.max(depth);
                    prev = b'{';
                }
                _ => {}
            }
            i += 1;
            continue;
        }
        let c = b[i];
        match c {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 1;
            }
            b'/' if b"(,=:[!&|?{};+-*%<>~^\n".contains(&prev) => {
                if let Some(end) = regex_end(b, i) {
                    i = end;
                    prev = b'a';
                } else {
                    prev = c;
                }
            }
            q @ (b'\'' | b'"') => {
                let mut j = i + 1;
                while j < b.len() && b[j] != q && b[j] != b'\n' {
                    if b[j] == b'\\' {
                        j += 1;
                    }
                    j += 1;
                }
                if j < b.len() && b[j] == q {
                    i = j;
                    prev = b'a';
                } else {
                    prev = c;
                }
            }
            b'`' => modes.push(Mode::Template),
            b'(' | b'[' | b'{' => {
                depth += 1;
                max = max.max(depth);
                prev = c;
            }
            b')' | b']' => {
                depth = depth.saturating_sub(1);
                prev = c;
            }
            b'}' => {
                if modes.len() > 1 && opened_at.last() == Some(&(depth - depth.min(1))) {
                    opened_at.pop();
                    modes.pop();
                    prev = b'a';
                } else {
                    prev = c;
                }
                depth = depth.saturating_sub(1);
            }
            b' ' | b'\t' | b'\r' => {}
            _ => prev = c,
        }
        i += 1;
    }
    max
}

/// Where a regular expression literal starting at the `/` at `i` closes,
/// when it closes on its own line.
fn regex_end(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    let mut class = false;
    while j < b.len() && b[j] != b'\n' && b[j] != b'\r' {
        match b[j] {
            b'\\' => j += 1,
            b'[' => class = true,
            b']' => class = false,
            b'/' if !class => return Some(j),
            _ => {}
        }
        j += 1;
    }
    None
}

/// Parses `content` as TypeScript would for `file` with `kind`, and hands the
/// tree to `f`. A file oxc cannot parse at all, or nested deeper than
/// [`MAX_NESTING`], is an empty program.
pub fn with_tree<R: Send>(
    file: &str,
    content: &str,
    kind: ScriptKind,
    f: impl FnOnce(&Tree<'_>) -> R + Send,
) -> R {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(PARSE_STACK)
            .spawn_scoped(scope, || parse_and(file, content, kind, f))
            .expect("a parse thread")
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e))
    })
}

/// Runs one per-file step the way the script's `try { … } catch {}` around it
/// does: a panic on that file (a parser bug, say) yields `None` and the run
/// goes on with the next file. [`with_tree`] re-raises a parse thread's panic
/// in the caller, so this is where it stops.
pub fn guarded<R>(f: impl FnOnce() -> R) -> Option<R> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).ok()
}

static PANIC_MARKERS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Test hook: while `marker` is set, parsing any file whose text contains it
/// panics, so a test can show a parser panic costs only that file. Each test
/// sets its own marker, so tests running in parallel do not interfere.
#[doc(hidden)]
pub fn set_panic_marker(marker: &str, on: bool) {
    if let Ok(mut m) = PANIC_MARKERS.lock() {
        m.retain(|x| x != marker);
        if on {
            m.push(marker.to_owned());
        }
    }
}

fn parse_and<R>(file: &str, content: &str, kind: ScriptKind, f: impl FnOnce(&Tree<'_>) -> R) -> R {
    let markers = PANIC_MARKERS.lock().map(|m| m.clone()).unwrap_or_default();
    for m in &markers {
        assert!(
            !content.contains(m.as_str()),
            "injected parser panic in {file}"
        );
    }
    let content = if nesting_depth(content) > MAX_NESTING {
        ""
    } else {
        content
    };
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, content, kind.source_type(file))
        .with_options(ParseOptions {
            preserve_parens: true,
            allow_return_outside_function: true,
            ..ParseOptions::default()
        })
        .parse();
    let mut b = Builder {
        nodes: Vec::new(),
        stack: Vec::new(),
        merged: HashSet::new(),
        idents: HashSet::new(),
        src: content,
    };
    b.visit_program(&ret.program);
    let mut nodes = b.nodes;
    let starts: Vec<u32> = nodes.iter().map(|n| n.start).collect();
    for n in &mut nodes {
        n.children.sort_by_key(|&c| starts[c]);
    }
    let tree = Tree {
        src: content,
        nodes,
        line_starts: line_starts(content),
    };
    f(&tree)
}

/// The syntax errors oxc reports for `content` parsed as [`with_tree`]
/// parses it, and whether the error was fatal (an empty program).
#[must_use]
pub fn parse_errors(file: &str, content: &str, kind: ScriptKind) -> (Vec<String>, bool) {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, content, kind.source_type(file))
        .with_options(ParseOptions {
            preserve_parens: true,
            allow_return_outside_function: true,
            ..ParseOptions::default()
        })
        .parse();
    (
        ret.diagnostics.iter().map(ToString::to_string).collect(),
        ret.fatal_error,
    )
}

struct Builder<'a> {
    nodes: Vec<Node<'a>>,
    /// `None` for a node the view leaves out; its children go to the
    /// nearest node above.
    stack: Vec<Option<usize>>,
    /// The `Function`s a method node absorbs.
    merged: HashSet<usize>,
    idents: HashSet<(u32, u32)>,
    src: &'a str,
}

fn addr<T>(r: &T) -> usize {
    std::ptr::from_ref(r) as usize
}

impl<'a> Builder<'a> {
    fn parent(&self) -> Option<usize> {
        self.stack.iter().rev().find_map(|x| *x)
    }

    /// Is the qualified name being entered a heritage clause's name, or the
    /// left side of one?
    fn in_heritage_name(&self) -> bool {
        self.parent().is_some_and(|p| match self.nodes[p].kind {
            AstKind::TSClassImplements(_) | AstKind::TSInterfaceHeritage(_) => true,
            AstKind::TSQualifiedName(_) => self.nodes[p].tag == Tag::PropertyAccess,
            _ => false,
        })
    }

    fn push(&mut self, kind: AstKind<'a>, tag: Tag, span: Span, name: &'a str) -> usize {
        let parent = self.parent();
        let i = self.nodes.len();
        self.nodes.push(Node {
            kind,
            tag,
            start: span.start,
            end: span.end,
            parent,
            children: Vec::new(),
            name,
        });
        if let Some(p) = parent {
            self.nodes[p].children.push(i);
        }
        i
    }

    /// An identifier node, unless one already stands at this span.
    fn ident(&mut self, kind: AstKind<'a>, span: Span, name: &'a str) -> Option<usize> {
        self.idents
            .insert((span.start, span.end))
            .then(|| self.push(kind, Tag::Identifier, span, name))
    }

    fn tag_of(&mut self, kind: AstKind<'a>) -> (Tag, &'a str) {
        match kind {
            AstKind::StaticMemberExpression(m) => (Tag::PropertyAccess, m.property.name.as_str()),
            AstKind::PrivateFieldExpression(m) => {
                let s = m.field.span;
                (
                    Tag::PropertyAccess,
                    &self.src[s.start as usize..s.end as usize],
                )
            }
            AstKind::JSXMemberExpression(m) => (Tag::PropertyAccess, m.property.name.as_str()),
            // `implements a.B` and an interface's `extends a.B` are
            // expressions to TypeScript (`ExpressionWithTypeArguments` over a
            // property access) and type names to oxc.
            AstKind::TSQualifiedName(q) if self.in_heritage_name() => {
                (Tag::PropertyAccess, q.right.name.as_str())
            }
            AstKind::CallExpression(_) => (Tag::Call, ""),
            AstKind::ImportExpression(_) => (Tag::ImportCall, ""),
            AstKind::NewExpression(_) => (Tag::New, ""),
            AstKind::AwaitExpression(_) => (Tag::Await, ""),
            AstKind::ReturnStatement(_) => (Tag::Return, ""),
            AstKind::ThrowStatement(_) => (Tag::Throw, ""),
            AstKind::ArrowFunctionExpression(_) => (Tag::Arrow, ""),
            AstKind::Function(f) => match f.r#type {
                FunctionType::FunctionDeclaration | FunctionType::TSDeclareFunction => {
                    (Tag::FunctionDecl, "")
                }
                _ => (Tag::FunctionExpr, ""),
            },
            AstKind::MethodDefinition(m) => {
                self.merged.insert(addr::<Function>(&m.value));
                // TypeScript parses a class element named `constructor` (or
                // `'constructor'`) as a constructor even when it is static,
                // and gives a constructor no name node.
                let ctor = !m.computed
                    && m.kind != MethodDefinitionKind::Get
                    && m.kind != MethodDefinitionKind::Set
                    && match &m.key {
                        PropertyKey::StaticIdentifier(k) => k.name.as_str() == "constructor",
                        PropertyKey::StringLiteral(s) => s.value.as_str() == Some("constructor"),
                        _ => false,
                    };
                if ctor {
                    let k = m.key.span();
                    self.idents.insert((k.start, k.end));
                }
                let tag = if m.kind == MethodDefinitionKind::Method && !ctor {
                    Tag::Method
                } else {
                    Tag::Other
                };
                (tag, "")
            }
            AstKind::ObjectProperty(p) => {
                let mut tag = Tag::Other;
                if p.method || p.kind != PropertyKind::Init {
                    if let Expression::FunctionExpression(f) = &p.value {
                        self.merged.insert(addr::<Function>(f));
                        if p.method && p.kind == PropertyKind::Init {
                            tag = Tag::Method;
                        }
                    }
                }
                (tag, "")
            }
            AstKind::VariableDeclarator(_) => (Tag::VarDecl, ""),
            AstKind::ExpressionStatement(_) => (Tag::ExprStmt, ""),
            AstKind::TryStatement(_) => (Tag::Try, ""),
            AstKind::CatchClause(_) => (Tag::Catch, ""),
            AstKind::BlockStatement(_) | AstKind::FunctionBody(_) => (Tag::Block, ""),
            AstKind::SwitchCase(c) if c.test.is_some() => (Tag::Case, ""),
            _ => (Tag::Other, ""),
        }
    }
}

impl<'a> Visit<'a> for Builder<'a> {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        let ident = match kind {
            AstKind::IdentifierName(x) => Some(x.name.as_str()),
            AstKind::IdentifierReference(x) => Some(x.name.as_str()),
            AstKind::BindingIdentifier(x) => Some(x.name.as_str()),
            AstKind::LabelIdentifier(x) => Some(x.name.as_str()),
            AstKind::JSXIdentifier(x) => Some(x.name.as_str()),
            AstKind::TSIndexSignatureName(x) => Some(x.name.as_str()),
            _ => None,
        };
        if let Some(name) = ident {
            let i = self.ident(kind, kind.span(), name);
            self.stack.push(i);
            return;
        }
        let skip = match kind {
            AstKind::ChainExpression(_) => true,
            AstKind::Function(f) => self.merged.contains(&addr::<Function>(f)),
            _ => false,
        };
        if skip {
            self.stack.push(None);
            return;
        }
        let (tag, name) = self.tag_of(kind);
        let mut span = kind.span();
        if tag == Tag::FunctionDecl {
            if let Some(p) = self.parent() {
                if matches!(
                    self.nodes[p].kind,
                    AstKind::ExportDeclaration(_) | AstKind::ExportDefaultDeclaration(_)
                ) {
                    span.start = self.nodes[p].start;
                }
            }
        }
        let i = self.push(kind, tag, span, name);
        self.stack.push(Some(i));
        let meta = match kind {
            AstKind::ImportMeta(_) => Some("meta"),
            AstKind::NewTarget(_) => Some("target"),
            _ => None,
        };
        if let Some(word) = meta {
            let end = span.end;
            let start = end - u32::try_from(word.len()).expect("short");
            self.ident(kind, Span::new(start, end), word);
        }
    }

    fn leave_node(&mut self, _kind: AstKind<'a>) {
        self.stack.pop();
    }
}

/// `ts.isStringLiteralLike(e)` and its `.text`: a string literal or a
/// template with no substitutions.
#[must_use]
pub fn string_like(e: &Expression<'_>) -> Option<String> {
    match e {
        Expression::StringLiteral(s) => Some(js_string!(s.value)),
        Expression::TemplateLiteral(t) if t.expressions.is_empty() => {
            t.quasis.first()?.value.cooked.map(|c| js_string!(c))
        }
        _ => None,
    }
}

/// [`string_like`] for a call argument; a spread is not one.
#[must_use]
pub fn arg_string_like(a: &Argument<'_>) -> Option<String> {
    a.as_expression().and_then(string_like)
}

/// A JavaScript string as Rust holds it: a lone surrogate becomes U+FFFD.
/// Called through [`js_string!`] with oxc's string value.
#[must_use]
pub fn wtf_string(utf8: Option<&str>, units: impl Iterator<Item = u16>) -> String {
    match utf8 {
        Some(t) => t.to_owned(),
        None => String::from_utf16_lossy(&units.collect::<Vec<_>>()),
    }
}

/// [`wtf_string`] of an oxc `JSStr`, whose type this crate cannot name.
#[macro_export]
macro_rules! js_string {
    ($v:expr) => {{
        let v = $v;
        $crate::ast_bindings::wtf_string(v.as_str(), v.encode_utf16())
    }};
}
pub use js_string;

fn export_name(n: &ModuleExportName<'_>) -> String {
    match n {
        ModuleExportName::IdentifierName(x) => x.name.as_str().to_owned(),
        ModuleExportName::IdentifierReference(x) => x.name.as_str().to_owned(),
        ModuleExportName::StringLiteral(s) => js_string!(s.value),
    }
}

impl<'a> Tree<'a> {
    /// 1-based line of a UTF-8 offset, by TypeScript's line breaks.
    #[must_use]
    pub fn line_of(&self, pos: u32) -> usize {
        self.line_starts.partition_point(|&s| s <= pos as usize)
    }

    /// `lineOf(n)`: the line `n.getStart(sf)` is on.
    #[must_use]
    pub fn line(&self, i: usize) -> usize {
        self.line_of(self.nodes[i].start)
    }

    #[must_use]
    pub fn span_text(&self, s: Span) -> &'a str {
        &self.src[s.start as usize..s.end as usize]
    }

    /// `n.getText(sf)`.
    #[must_use]
    pub fn text(&self, i: usize) -> &'a str {
        self.span_text(self.nodes[i].span())
    }

    #[must_use]
    pub fn tag(&self, i: usize) -> Tag {
        self.nodes[i].tag
    }

    #[must_use]
    pub fn parent(&self, i: usize) -> Option<usize> {
        self.nodes[i].parent
    }

    /// The direct child of `i` whose oxc node spans exactly `span` (a
    /// declaration's own span, before the view moves its start to `export`).
    #[must_use]
    pub fn child_at(&self, i: usize, span: Span) -> Option<usize> {
        self.nodes[i]
            .children
            .iter()
            .copied()
            .find(|&c| self.nodes[c].tag != Tag::Identifier && self.nodes[c].kind.span() == span)
            .or_else(|| {
                self.nodes[i]
                    .children
                    .iter()
                    .copied()
                    .find(|&c| self.nodes[c].span() == span)
            })
    }

    /// Pre-order from `root`, the order `ts.forEachChild` recursion visits;
    /// `f` returns whether to go into the node's children.
    pub fn walk(&self, root: usize, mut f: impl FnMut(usize) -> bool) {
        let mut stack = vec![root];
        while let Some(i) = stack.pop() {
            if f(i) {
                stack.extend(self.nodes[i].children.iter().rev());
            }
        }
    }

    /// Every node in pre-order.
    #[must_use]
    pub fn preorder(&self, root: usize) -> Vec<usize> {
        let mut out = Vec::new();
        self.walk(root, |i| {
            out.push(i);
            true
        });
        out
    }

    /// The script's `isFunctionLike`: a function declaration or expression,
    /// an arrow function or a method.
    #[must_use]
    pub fn is_function_like(&self, i: usize) -> bool {
        matches!(
            self.tag(i),
            Tag::FunctionDecl | Tag::FunctionExpr | Tag::Arrow | Tag::Method
        )
    }

    /// The oxc `Function` behind a declaration, an expression or a method.
    #[must_use]
    pub fn function(&self, i: usize) -> Option<&'a Function<'a>> {
        match self.nodes[i].kind {
            AstKind::Function(f) => Some(f),
            AstKind::MethodDefinition(m) => Some(&m.value),
            AstKind::ObjectProperty(p) => match &p.value {
                Expression::FunctionExpression(f) => Some(f),
                _ => None,
            },
            _ => None,
        }
    }

    /// `fn.body`.
    #[must_use]
    pub fn body(&self, i: usize) -> Option<Body> {
        if let AstKind::ArrowFunctionExpression(a) = self.nodes[i].kind {
            return match &a.body {
                ArrowFunctionBody::FunctionBody(b) => self.child_at(i, b.span).map(Body::Block),
                e => self.child_at(i, e.to_expression().span()).map(Body::Expr),
            };
        }
        if !self.is_function_like(i) {
            return None;
        }
        let b = self.function(i)?.body.as_ref()?;
        self.child_at(i, b.span).map(Body::Block)
    }

    /// The expression body of an arrow function, as a span.
    #[must_use]
    pub fn arrow_expr_body(&self, i: usize) -> Option<Span> {
        match self.nodes[i].kind {
            AstKind::ArrowFunctionExpression(a) => match &a.body {
                ArrowFunctionBody::FunctionBody(_) => None,
                e => Some(e.to_expression().span()),
            },
            _ => None,
        }
    }

    /// `n.left.getText(sf)` for an assignment, or `None` where TypeScript's
    /// left side is parenthesized (oxc keeps no node for those parentheses,
    /// so the text TypeScript reads would start with `(`).
    #[must_use]
    pub fn assign_left_text(&self, i: usize) -> Option<&'a str> {
        let AstKind::AssignmentExpression(a) = self.nodes[i].kind else {
            return None;
        };
        let l = a.left.span();
        (l.start == a.span.start).then(|| self.span_text(l))
    }

    /// Is `i` the name of the property access above it?
    #[must_use]
    pub fn is_property_name(&self, i: usize) -> bool {
        let Some(p) = self.parent(i) else {
            return false;
        };
        let name_span = match self.nodes[p].kind {
            AstKind::StaticMemberExpression(m) => m.property.span,
            AstKind::JSXMemberExpression(m) => m.property.span,
            AstKind::TSQualifiedName(q) if self.nodes[p].tag == Tag::PropertyAccess => q.right.span,
            _ => return false,
        };
        name_span == self.nodes[i].span()
    }

    /// `call.expression === node`.
    #[must_use]
    pub fn is_callee(&self, call: usize, node: usize) -> bool {
        match self.nodes[call].kind {
            AstKind::CallExpression(c) => c.callee.span() == self.nodes[node].span(),
            _ => false,
        }
    }

    /// The program's statements, with `export` and `export default` around a
    /// declaration unwrapped, and whether each was exported. TypeScript keeps
    /// those as modifiers of the declaration itself.
    #[must_use]
    pub fn top_level(&self) -> Vec<(usize, bool)> {
        let mut out = Vec::new();
        if self.nodes.is_empty() {
            return out;
        }
        for &c in &self.nodes[0].children {
            let inner = match self.nodes[c].kind {
                AstKind::ExportDeclaration(e) => self.child_at(c, e.declaration.span()),
                AstKind::ExportDefaultDeclaration(e) => match &e.declaration {
                    oxc_ast::ast::ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                        self.child_at(c, f.span)
                    }
                    _ => None,
                },
                _ => None,
            };
            match inner {
                Some(d) => out.push((d, true)),
                None => out.push((c, false)),
            }
        }
        out
    }

    /// For a `VariableDeclarator` whose name is an identifier: that name and
    /// the initializer node when it is an arrow function or a function
    /// expression (not parenthesized).
    #[must_use]
    pub fn declared_function(&self, i: usize) -> Option<(&'a str, Option<usize>)> {
        let AstKind::VariableDeclarator(v) = self.nodes[i].kind else {
            return None;
        };
        let BindingPattern::BindingIdentifier(id) = &v.id else {
            return None;
        };
        let init = match &v.init {
            Some(
                e @ (Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_)),
            ) => self.child_at(i, e.span()),
            _ => None,
        };
        Some((id.name.as_str(), init))
    }

    /// The node of an assignment's right side when it is an arrow function or
    /// a function expression.
    #[must_use]
    pub fn assigned_function(&self, i: usize) -> Option<usize> {
        let AstKind::AssignmentExpression(a) = self.nodes[i].kind else {
            return None;
        };
        match &a.right {
            e @ (Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_)) => {
                self.child_at(i, e.span())
            }
            _ => None,
        }
    }
}

/// `textOf(sf, n)`: the source text with every JavaScript whitespace
/// character removed.
#[must_use]
pub fn squeeze(s: &str) -> String {
    s.chars().filter(|&c| !js::is_js_whitespace(c)).collect()
}

/// Whether a function-like node carries `async` (TypeScript's
/// `ModifierFlags.Async`).
fn is_async(t: &Tree<'_>, i: usize) -> bool {
    match t.nodes[i].kind {
        AstKind::ArrowFunctionExpression(a) => a.r#async,
        _ => t.function(i).is_some_and(|f| f.r#async),
    }
}

// ---------------------------------------------------------------------------
// bindingRefs

/// One reference site `bindingRefs` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub line: usize,
    pub call: bool,
    pub awaited: Option<bool>,
    pub in_try: Option<bool>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    pub refs: Vec<Ref>,
    pub reexports: bool,
}

/// Is `e` a direct `require("<spec>")` whose spec points at `target`?
/// `require?.("<spec>")` is one too: TypeScript has no node for the chain.
fn require_of(e: &Expression<'_>, points: &dyn Fn(&str) -> bool) -> bool {
    let c = match e {
        Expression::CallExpression(c) => c,
        Expression::ChainExpression(ch) => match &ch.expression {
            ChainElement::CallExpression(c) => c,
            _ => return false,
        },
        _ => return false,
    };
    let Expression::Identifier(id) = &c.callee else {
        return false;
    };
    id.name.as_str() == "require"
        && c.arguments
            .first()
            .and_then(arg_string_like)
            .is_some_and(|s| points(&s))
}

/// `bindingRefs(ts, importer, content, target, name, aliases)`: the
/// references `importer` makes to `name` exported by `target`, or `None`
/// when it binds nothing from `target`.
#[must_use]
pub fn binding_refs(
    importer: &str,
    content: &str,
    target: &str,
    name: &str,
    aliases: &[(String, String)],
) -> Option<Bound> {
    with_tree(importer, content, ScriptKind::of(importer), |t| {
        binding_refs_in(t, importer, content, target, name, aliases)
    })
}

#[allow(clippy::too_many_lines)]
fn binding_refs_in(
    t: &Tree<'_>,
    importer: &str,
    content: &str,
    target: &str,
    name: &str,
    aliases: &[(String, String)],
) -> Option<Bound> {
    let points = |spec: &str| spec_points_at(spec, importer, target, aliases);
    let mut locals: HashSet<String> = HashSet::new();
    let mut modules: HashSet<String> = HashSet::new();
    let mut declared: HashSet<u32> = HashSet::new();
    let mut reexports = false;
    let mut direct: Vec<usize> = Vec::new();
    for i in t.preorder(0) {
        match t.nodes[i].kind {
            AstKind::ImportDeclaration(d) => {
                let Some(specs) = &d.specifiers else { continue };
                if !points(&js_string!(d.source.value)) {
                    continue;
                }
                for s in specs {
                    match s {
                        ImportDeclarationSpecifier::ImportNamespaceSpecifier(n) => {
                            modules.insert(n.local.name.as_str().to_owned());
                        }
                        ImportDeclarationSpecifier::ImportSpecifier(s) => {
                            if export_name(&s.imported) == name {
                                locals.insert(s.local.name.as_str().to_owned());
                                declared.insert(s.local.span.start);
                            }
                        }
                        ImportDeclarationSpecifier::ImportDefaultSpecifier(_) => {}
                    }
                }
            }
            AstKind::ExportAllDeclaration(e) => {
                if e.exported.is_none() && points(&js_string!(e.source.value)) {
                    reexports = true;
                }
            }
            AstKind::ExportFromDeclaration(e) => {
                if points(&js_string!(e.source.value))
                    && e.specifiers.iter().any(|s| export_name(&s.local) == name)
                {
                    reexports = true;
                }
            }
            AstKind::VariableDeclarator(v) => {
                if !v.init.as_ref().is_some_and(|e| require_of(e, &points)) {
                    continue;
                }
                match &v.id {
                    BindingPattern::BindingIdentifier(b) => {
                        modules.insert(b.name.as_str().to_owned());
                    }
                    BindingPattern::ObjectPattern(p) => {
                        for prop in &p.properties {
                            let bound = match &prop.value {
                                BindingPattern::BindingIdentifier(b) => Some(b),
                                BindingPattern::AssignmentPattern(a) => match &a.left {
                                    BindingPattern::BindingIdentifier(b) => Some(b),
                                    _ => None,
                                },
                                _ => None,
                            };
                            let Some(b) = bound else { continue };
                            let key = match (&prop.key, prop.shorthand || prop.computed) {
                                (PropertyKey::StaticIdentifier(k), false) => k.name.as_str(),
                                _ => b.name.as_str(),
                            };
                            if key == name {
                                locals.insert(b.name.as_str().to_owned());
                                declared.insert(b.span.start);
                            }
                        }
                        if let Some(r) = &p.rest {
                            if let BindingPattern::BindingIdentifier(b) = &r.argument {
                                if b.name.as_str() == name {
                                    locals.insert(b.name.as_str().to_owned());
                                    declared.insert(b.span.start);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            AstKind::StaticMemberExpression(m)
                if m.property.name.as_str() == name && require_of(&m.object, &points) =>
            {
                direct.push(i);
            }
            _ => {}
        }
    }
    if locals.is_empty() && modules.is_empty() && direct.is_empty() && !reexports {
        return None;
    }
    let mut sites = direct;
    for i in t.preorder(0) {
        let n = &t.nodes[i];
        if n.tag == Tag::Identifier
            && locals.contains(n.name)
            && !declared.contains(&n.start)
            && !t.is_property_name(i)
        {
            sites.push(i);
        }
        if n.tag == Tag::PropertyAccess && n.name == name {
            let object = match n.kind {
                AstKind::StaticMemberExpression(m) => match &m.object {
                    Expression::Identifier(id) => Some(id.name.as_str()),
                    _ => None,
                },
                AstKind::JSXMemberExpression(m) => match &m.object {
                    JSXMemberExpressionObject::IdentifierReference(id) => Some(id.name.as_str()),
                    _ => None,
                },
                AstKind::TSQualifiedName(q) => match &q.left {
                    TSTypeName::IdentifierReference(id) => Some(id.name.as_str()),
                    _ => None,
                },
                _ => None,
            };
            if object.is_some_and(|o| modules.contains(o)) {
                sites.push(i);
            }
        }
    }
    let lines: Vec<&str> = content.split('\n').collect();
    let refs = sites
        .into_iter()
        .map(|node| {
            let parent = t.parent(node);
            let call = parent.is_some_and(|p| t.tag(p) == Tag::Call && t.is_callee(p, node));
            let site = if call { parent.expect("a call") } else { node };
            let awaited = call.then(|| {
                t.parent(site).is_some_and(|p| match t.tag(p) {
                    Tag::Await | Tag::Return | Tag::VarDecl | Tag::Call | Tag::ImportCall => true,
                    Tag::Arrow => t.arrow_expr_body(p) == Some(t.nodes[site].span()),
                    Tag::PropertyAccess => {
                        matches!(t.nodes[p].name, "then" | "catch" | "finally")
                    }
                    _ => false,
                })
            });
            let mut in_try = false;
            let mut m = t.parent(site);
            while let Some(q) = m {
                if t.is_function_like(q) {
                    break;
                }
                if let AstKind::TryStatement(ts) = t.nodes[q].kind {
                    let b = ts.block.span;
                    let s = t.nodes[site].span();
                    if b.start <= s.start && s.end <= b.end {
                        in_try = true;
                    }
                }
                m = t.parent(q);
            }
            let line = t.line(node);
            let text = lines.get(line - 1).copied().unwrap_or("");
            Ref {
                line,
                call,
                awaited,
                in_try: call.then_some(in_try),
                text: utf16_prefix(js::trim(text), TEXT_CHARS).to_owned(),
            }
        })
        .collect();
    Some(Bound { refs, reexports })
}

// ---------------------------------------------------------------------------
// returnsPromise

/// `plain(e)`: a returned value that is plainly not a promise.
fn plain(e: Option<&Expression<'_>>) -> bool {
    let Some(e) = e else { return true };
    match e {
        Expression::ArrowFunctionExpression(_)
        | Expression::FunctionExpression(_)
        | Expression::ObjectExpression(_)
        | Expression::ArrayExpression(_)
        | Expression::NumericLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::StringLiteral(_)
        | Expression::RegExpLiteral(_)
        | Expression::TemplateLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::BinaryExpression(_)
        | Expression::LogicalExpression(_)
        | Expression::AssignmentExpression(_)
        | Expression::SequenceExpression(_)
        | Expression::PrivateInExpression(_) => true,
        Expression::UnaryExpression(u) => matches!(
            u.operator,
            UnaryOperator::UnaryNegation
                | UnaryOperator::UnaryPlus
                | UnaryOperator::LogicalNot
                | UnaryOperator::BitwiseNot
        ),
        Expression::UpdateExpression(u) => u.prefix,
        _ => false,
    }
}

/// `/^Promise\b/`.
fn promise_type(text: &str) -> bool {
    text.strip_prefix("Promise").is_some_and(|r| {
        r.bytes()
            .next()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == b'_'))
    })
}

/// `returnsPromise(ts, file, content, name)`: `Some(true)` for a promise,
/// `Some(false)` for plainly none, `None` when unknown or not found.
#[must_use]
pub fn returns_promise(file: &str, content: &str, name: &str) -> Option<bool> {
    with_tree(file, content, ScriptKind::of(file), |t| {
        returns_promise_in(t, name)
    })
}

fn returns_promise_in(t: &Tree<'_>, name: &str) -> Option<bool> {
    let suffix = format!(".{name}");
    let mut found: Option<usize> = None;
    t.walk(0, |i| {
        if found.is_some() {
            return false;
        }
        let n = &t.nodes[i];
        match n.kind {
            AstKind::Function(f) if n.tag == Tag::FunctionDecl => {
                if f.id.as_ref().is_some_and(|id| id.name.as_str() == name) {
                    found = Some(i);
                }
            }
            AstKind::VariableDeclarator(_) => {
                if let Some((id, Some(init))) = t.declared_function(i) {
                    if id == name {
                        found = Some(init);
                    }
                }
            }
            AstKind::AssignmentExpression(a) if a.operator == AssignmentOperator::Assign => {
                let cjs = t.assign_left_text(i).is_some_and(|l| {
                    (l.starts_with("exports.") || l.starts_with("module.exports."))
                        && l.ends_with(&suffix)
                });
                if cjs {
                    found = t.assigned_function(i);
                }
            }
            _ => {}
        }
        found.is_none()
    });
    let f = found?;
    if is_async(t, f) {
        return Some(true);
    }
    let ret_type = match t.nodes[f].kind {
        AstKind::ArrowFunctionExpression(a) => a.return_type.as_ref(),
        _ => t.function(f).and_then(|x| x.return_type.as_ref()),
    };
    if let Some(rt) = ret_type {
        return Some(promise_type(t.span_text(rt.type_annotation.span())));
    }
    let body = match t.nodes[f].kind {
        AstKind::ArrowFunctionExpression(a) => match &a.body {
            ArrowFunctionBody::FunctionBody(_) => t.body(f),
            e => {
                return if plain(Some(e.to_expression())) {
                    Some(false)
                } else {
                    None
                }
            }
        },
        _ => t.body(f),
    };
    let Some(Body::Block(b)) = body else {
        // No body and no return type (an overload or a `declare`): the
        // script's `ts.isBlock(fn.body)` throws on `undefined`, and its caller
        // reads the throw as not known to be synchronous.
        return None;
    };
    let mut all_plain = true;
    t.walk(b, |m| {
        if m != b && matches!(t.tag(m), Tag::Arrow | Tag::FunctionExpr | Tag::FunctionDecl) {
            return false;
        }
        if let AstKind::ReturnStatement(r) = t.nodes[m].kind {
            all_plain &= plain(r.argument.as_ref());
        }
        true
    });
    if all_plain {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_follow_typescript() {
        let src = "a\r\nb\rc\u{2028}d\ne";
        let starts = line_starts(src);
        assert_eq!(starts.len(), 5);
    }

    #[test]
    fn nesting_counts_brackets_outside_strings_and_comments() {
        assert_eq!(nesting_depth("f(a[b{c}])"), 3);
        assert_eq!(nesting_depth("'((((' // ((((\n/* [[[[ */ x"), 0);
        assert_eq!(nesting_depth("`a ${ f(`b ${ g() }`) } c`"), 4);
        assert_eq!(nesting_depth("\"\\\"((\" (x)"), 1);
        // Quotes and brackets in regular expressions and JSX text hide
        // nothing, however many lines repeat them.
        assert_eq!(nesting_depth(&"s = s.replace(/'/g, '');\n".repeat(50)), 1);
        assert_eq!(
            nesting_depth(&"if (/[\"']/.test(s)) { n++; }\n".repeat(50)),
            2
        );
        assert_eq!(nesting_depth(&"x = /[([{]/;\ny = /\\(/;\n".repeat(50)), 0);
        assert_eq!(nesting_depth(&"<p>(you can't)</p>\n".repeat(50)), 1);
        assert_eq!(nesting_depth("a = (b) / (c) / (d)"), 1);
        assert_eq!(nesting_depth("'a\\\n(b' (c)"), 1);
    }

    // Shapes where TypeScript's tree differs from oxc's, with the answers the
    // script gives (TypeScript 6.0.3); `tests/diff_blast_radius_ast.rs`
    // compares the two directly.
    #[test]
    fn typescript_shapes() {
        let lines = |r: Option<Bound>| r.map(|b| b.refs.iter().map(|x| x.line).collect::<Vec<_>>());
        let heritage = "import * as ns from './b';\nclass X implements ns.Foo {}\ninterface I extends ns.Foo {}\nclass Z implements y.Foo {}\n";
        assert_eq!(
            lines(binding_refs("src/a.ts", heritage, "src/b.ts", "Foo", &[])),
            Some(vec![2, 3])
        );
        let named = "import { Foo } from './b';\nclass Z implements y.Foo {}\n";
        assert_eq!(
            lines(binding_refs("src/a.ts", named, "src/b.ts", "Foo", &[])),
            Some(vec![])
        );
        let optional = "const svc = require?.('./b');\nsvc.Foo();\n";
        assert_eq!(
            lines(binding_refs("src/a.js", optional, "src/b.ts", "Foo", &[])),
            Some(vec![2])
        );
        let ctor = "import { constructor } from './b';\nclass X { constructor() {} }\nclass Z { static constructor() {} }\nconst o = { constructor() {} };\n";
        assert_eq!(
            lines(binding_refs(
                "src/a.ts",
                ctor,
                "src/b.ts",
                "constructor",
                &[]
            )),
            Some(vec![4])
        );
        // The script throws on an overload with neither body nor return
        // type, and its caller reads that as not known to be synchronous.
        assert_eq!(
            returns_promise(
                "x.ts",
                "export function f(a: string);\nexport function f(a: any) { return a; }",
                "f"
            ),
            None
        );
    }

    // The expectations of `eval/replay-binding-graph.mjs`, which the script
    // meets; `tests/diff_blast_radius_ast.rs` compares the two directly.
    const ALIASED: &str = "import { getAccount as fetchAccount } from '@services/auth';\n\nexport async function handle(req) {\n  const a = await fetchAccount(req.id);\n  try {\n    fetchAccount(req.other);\n  } catch (e) {}\n  return a;\n}\n";

    fn aliases() -> Vec<(String, String)> {
        vec![("@services/*".into(), "src/services/*".into())]
    }

    #[test]
    fn replay_binding_graph_import_forms() {
        let r = binding_refs(
            "src/controllers/aliased.ts",
            ALIASED,
            "src/services/auth.ts",
            "getAccount",
            &aliases(),
        )
        .unwrap();
        let at = |l: usize| r.refs.iter().find(|x| x.line == l).cloned();
        assert_eq!(
            at(4).map(|x| (x.call, x.awaited, x.in_try)),
            Some((true, Some(true), Some(false)))
        );
        assert_eq!(
            at(6).map(|x| (x.call, x.awaited, x.in_try)),
            Some((true, Some(false), Some(true)))
        );
        assert!(at(1).is_none());
        let ns = "import * as authSvc from '../services/auth';\n\nexport function poke(id) {\n  authSvc.getAccount(id);\n}\n";
        let r = binding_refs(
            "src/controllers/namespace.ts",
            ns,
            "src/services/auth.ts",
            "getAccount",
            &[],
        )
        .unwrap();
        assert_eq!(
            r.refs.iter().map(|x| (x.line, x.call)).collect::<Vec<_>>(),
            [(4, true)]
        );
        let de = "const {\n  getAccount\n} = require('../services/auth');\n\nexports.run = (id) => getAccount(id);\n";
        let r = binding_refs(
            "src/legacy/destructured.js",
            de,
            "src/services/auth.ts",
            "getAccount",
            &[],
        )
        .unwrap();
        assert_eq!(
            r.refs
                .iter()
                .map(|x| (x.line, x.call, x.awaited))
                .collect::<Vec<_>>(),
            [(5, true, Some(true))]
        );
        let module = "const svc = require('../services/auth');\n\nexports.go = (id) => {\n  svc.getAccount(id);\n};\n";
        let r = binding_refs(
            "src/legacy/module.js",
            module,
            "src/services/auth.ts",
            "getAccount",
            &[],
        )
        .unwrap();
        assert_eq!(
            r.refs
                .iter()
                .map(|x| (x.line, x.awaited))
                .collect::<Vec<_>>(),
            [(4, Some(false))]
        );
        let collision = "import { getAccount } from './elsewhere';\n\nexport const unrelated = () => getAccount(9);\n";
        assert_eq!(
            binding_refs(
                "src/other/collision.ts",
                collision,
                "src/services/auth.ts",
                "getAccount",
                &[]
            ),
            None
        );
        let barrel = binding_refs(
            "src/services/index.ts",
            "export { getAccount } from './auth';\n",
            "src/services/auth.ts",
            "getAccount",
            &[],
        )
        .unwrap();
        assert!(barrel.reexports && barrel.refs.is_empty());
        let then = "import { getAccount } from './auth';\ngetAccount(1)?.then(f);\nconst x = a?.b(getAccount(2));\n";
        let r = binding_refs(
            "src/services/t.ts",
            then,
            "src/services/auth.ts",
            "getAccount",
            &[],
        )
        .unwrap();
        assert_eq!(
            r.refs.iter().map(|x| x.awaited).collect::<Vec<_>>(),
            [Some(true), Some(true)]
        );
    }

    #[test]
    fn replay_binding_graph_returns_promise() {
        let rp = |src: &str| returns_promise("x.ts", src, "f");
        assert_eq!(rp("export const f = async () => 1;"), Some(true));
        assert_eq!(
            rp("export function f(): Promise<number> { return g(); }"),
            Some(true)
        );
        assert_eq!(
            rp("export const f = (e): boolean => e.status === 404;"),
            Some(false)
        );
        assert_eq!(
            rp("export const f = (roles) => async (req) => roles;"),
            Some(false)
        );
        assert_eq!(
            rp("export const f = (input) => queue.add('x', input);"),
            None
        );
        assert_eq!(rp("export function f(x) { cache.delete(x); }"), Some(false));
        assert_eq!(
            returns_promise("x.js", "exports.f = async (x) => x;", "f"),
            Some(true)
        );
        assert_eq!(rp("export const g = 1;"), None);
    }

    #[test]
    fn promise_word() {
        assert!(promise_type("Promise<void>"));
        assert!(promise_type("Promise"));
        assert!(!promise_type("PromiseLike<void>"));
        assert!(!promise_type("(Promise<void>)"));
    }
}
