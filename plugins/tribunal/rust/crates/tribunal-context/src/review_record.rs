//! Port of `skills/review-pr/scripts/persist-review-record.mjs`: patch the
//! run's metrics row in place (posted, verdict, agentsActual, agentCost, the
//! findings' `posted` flags and, from `--report-file`, their four
//! `*MissingFromReport` fields), under the ledger lock, then append the two
//! backstops. It carries the parts of `eval/agent-cost.mjs` the patch uses
//! (`findRunDirs`, `readRunDir`, `parseTranscript`, `classifyAgent`,
//! `toMetricsShape`) and of `lib/check-report-fidelity.mjs`
//! (`applyFidelityChecks`, over `tribunal-core`'s ports of its regions).
//!
//! Directories come from [`LedgerEnv`] and `--project-dir`, so a test points
//! the ledger and the transcript tree at temp directories.
//!
//! The transcript arithmetic is JavaScript's: a usage field that is a string
//! turns `+=` into concatenation, `||` falls back on any falsy value, and a
//! model named after an `Object.prototype` member gets the entry V8 gives it.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;

use serde_json::{Map, Number, Value};
use tribunal_core::evidence::js_ext::{object_prototype_property, utf16_cmp};
use tribunal_core::js;
use tribunal_core::report::js_ext::{strict_eq, to_js_string};
use tribunal_core::report::merged_locations_check::merged_locations_missing_from_report_text;
use tribunal_core::report::report_fidelity_checks::{
    grounded_diagnostic_missing_from_report_text, kill_reason_missing_from_report_text,
    merged_from_annotation_missing_from_report_text, unexamined_wording_in_report_text,
};

use crate::ledger::{
    locate_last_row, parse_args, with_ledger_lock, Arg, Args, LedgerEnv, RowQuery,
};
use crate::operator::{json_parse, node_fs_message, path_join, CliOutput};
use crate::post_footer::{math_round, to_fixed};

// ---------------------------------------------------------------------------
// JavaScript values for the transcript arithmetic

/// A JavaScript value as the cost sums see it.
#[derive(Debug, Clone, PartialEq)]
pub enum Js {
    Undef,
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    /// An array or object, as read from JSON.
    Obj(Value),
}

impl Js {
    fn of(v: Option<&Value>) -> Js {
        match v {
            None => Js::Undef,
            Some(Value::Null) => Js::Null,
            Some(Value::Bool(b)) => Js::Bool(*b),
            Some(Value::Number(n)) => Js::Num(n.as_f64().unwrap_or(f64::NAN)),
            Some(Value::String(s)) => Js::Str(s.clone()),
            Some(v) => Js::Obj(v.clone()),
        }
    }

    /// `v || 0`.
    fn or_zero(v: Option<&Value>) -> Js {
        if js::truthy(v) {
            Js::of(v)
        } else {
            Js::Num(0.0)
        }
    }

    fn prim(&self) -> Js {
        match self {
            Js::Obj(v) => Js::Str(to_js_string(Some(v))),
            other => other.clone(),
        }
    }

    fn text(&self) -> String {
        match self {
            Js::Undef => "undefined".into(),
            Js::Null => "null".into(),
            Js::Bool(b) => b.to_string(),
            Js::Num(x) => js::number_to_string(*x),
            Js::Str(s) => s.clone(),
            Js::Obj(v) => to_js_string(Some(v)),
        }
    }

    fn number(&self) -> f64 {
        match self.prim() {
            Js::Undef => f64::NAN,
            Js::Null => 0.0,
            Js::Bool(b) => f64::from(u8::from(b)),
            Js::Num(x) => x,
            Js::Str(s) => js::string_to_number(&s),
            Js::Obj(_) => unreachable!(),
        }
    }

    fn truthy(&self) -> bool {
        match self {
            Js::Undef | Js::Null => false,
            Js::Bool(b) => *b,
            Js::Num(x) => *x != 0.0 && !x.is_nan(),
            Js::Str(s) => !s.is_empty(),
            Js::Obj(_) => true,
        }
    }

    /// `a + b`.
    fn add(&self, b: &Js) -> Js {
        let (pa, pb) = (self.prim(), b.prim());
        if matches!(pa, Js::Str(_)) || matches!(pb, Js::Str(_)) {
            Js::Str(pa.text() + &pb.text())
        } else {
            Js::Num(pa.number() + pb.number())
        }
    }

    /// `a > b`.
    fn gt(&self, b: &Js) -> bool {
        match (self.prim(), b.prim()) {
            (Js::Str(x), Js::Str(y)) => utf16_cmp(&x, &y) == std::cmp::Ordering::Greater,
            (x, y) => x.number() > y.number(),
        }
    }

    fn finite(&self) -> Option<f64> {
        match self {
            Js::Num(x) if x.is_finite() => Some(*x),
            _ => None,
        }
    }

    /// The value `JSON.stringify` writes; `None` drops the key.
    pub fn to_json(&self) -> Option<Value> {
        Some(match self {
            Js::Undef => return None,
            Js::Null => Value::Null,
            Js::Bool(b) => Value::Bool(*b),
            Js::Num(x) => num(*x),
            Js::Str(s) => Value::String(s.clone()),
            Js::Obj(v) => v.clone(),
        })
    }
}

fn num(x: f64) -> Value {
    Number::from_f64(x).map_or(Value::Null, Value::Number)
}

// ---------------------------------------------------------------------------
// classifyAgent

fn non_space_run(s: &str) -> usize {
    s.find(js::is_js_whitespace).unwrap_or(s.len())
}

/// `\S+` then `rest`: the run is maximal because `rest` starts with a space.
fn nonspace_then(s: &str, rest: &str) -> bool {
    let n = non_space_run(s);
    n > 0 && s[n..].starts_with(rest)
}

/// `\d+` then `rest`, which does not start with a digit.
fn digits_then(s: &str, rest: &str) -> bool {
    let n = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    n > 0 && s[n..].starts_with(rest)
}

/// `.{0,40}was nearly refuted`, where `.` is one UTF-16 unit other than a
/// line terminator.
fn within_40_units(s: &str, lit: &str) -> bool {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        if s[i..].starts_with(lit) {
            return true;
        }
        units += c.len_utf16();
        if units > 40 || js::is_line_terminator(c) {
            return false;
        }
    }
    s.ends_with(lit)
}

fn after<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.strip_prefix(prefix)
}

type Pattern = (&'static str, &'static str, fn(&str) -> bool);

const PATTERNS: &[Pattern] = &[
    ("harness-audit", "audit-lens", |s| {
        s.starts_with("You are auditing a personal Claude Code skill")
    }),
    ("harness-audit", "audit-refuter", |s| {
        s.starts_with("REFUTE — do not confirm — this audit finding")
    }),
    ("harness-audit", "audit-refuter", |s| {
        s.starts_with("REFUTE this root-cause diagnosis of a harness defect")
    }),
    ("harness-audit", "audit-invariant", |s| {
        s.starts_with("A self-audit of the peer-review harness failed an invariant")
    }),
    ("harness-audit", "audit-invariant", |s| {
        s.starts_with("A harness self-audit reported:")
    }),
    ("harness-audit", "audit-fixer", |s| {
        after(s, "Propose fixes for ")
            .is_some_and(|r| digits_then(r, " confirmed defect(s) in the peer-rev"))
    }),
    ("harness-audit", "audit-battery", |s| {
        s.starts_with("Run the peer-review offline battery")
    }),
    ("harness-audit", "audit-runner", |s| {
        s.starts_with("Audit the most recent peer-review run")
    }),
    ("lens", "lens", |s| {
        after(s, "Review PR #").is_some_and(|r| nonspace_then(r, " in repo "))
    }),
    ("lens", "lens-legacy", |s| {
        s.starts_with("You are a code reviewer focusing ONLY on:")
    }),
    ("refuter", "refute-single", |s| {
        s.starts_with("REFUTE — do not confirm — this code review finding")
    }),
    ("refuter", "refute-batch", |s| {
        after(s, "You are refuting ")
            .is_some_and(|r| digits_then(r, " INDEPENDENT code-review findings on PR #"))
    }),
    ("external-heads", "heads-driver", |s| {
        s.contains("You will NOT be asked for a verdict and must not form one")
    }),
    ("external-heads", "heads-codex", |s| {
        s.starts_with("You are a model-diverse refuter (Codex/GPT slot)")
    }),
    ("external-heads", "sweep-artifact-read", |s| {
        s.starts_with("Run exactly: cat ")
    }),
    ("external-heads", "heads-gemini", |s| {
        s.starts_with("You are a model-diverse refuter (Gemini slot)")
    }),
    ("strengthen", "strengthen", |s| {
        after(s, "A code-review finding on PR #").is_some_and(|r| {
            let n = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
            n > 0
                && r[n..]
                    .strip_prefix(' ')
                    .is_some_and(|t| within_40_units(t, "was nearly refuted"))
        })
    }),
    ("defense", "defense", |s| {
        s.starts_with("DEFEND this code-review finding on PR #")
    }),
    ("adjudicator", "adjudicator", |s| {
        s.starts_with("You are the ADJUDICATOR (judge lens)")
    }),
    ("dedup", "dedup-candidates", |s| {
        s.starts_with("You are given candidate code-review findings from DIFFERENT review lenses")
    }),
    ("dedup", "panel-ceiling-tiebreak", |s| {
        s.starts_with("PANEL-CEILING TIE-BREAK. This review has more findings")
    }),
    ("bot-vetting", "bot-vetting", |s| {
        s.starts_with("Vet these automated review comments on PR #")
    }),
    ("critic", "completeness-critic", |s| {
        s.starts_with("Completeness critic for PR #")
    }),
    ("critic", "fidelity-critic", |s| {
        s.starts_with("FIDELITY CRITIC for the peer-review report on PR #")
    }),
    ("verifier", "verify-prior-finding", |s| {
        s.starts_with("A previous review of this PR raised the finding below")
    }),
    ("synthesize", "synthesize", |s| {
        s.starts_with("Synthesize the final peer review report for PR #")
    }),
    ("mechanical", "fetch-pr", |s| {
        after(s, "Fetch PR #").is_some_and(|r| nonspace_then(r, " from GitHub repo"))
    }),
    ("mechanical", "mark-run-started", |s| {
        s.starts_with("Best-effort bookkeeping —")
    }),
    ("mechanical", "context-bookkeeping", |s| {
        s.starts_with("Mechanical, best-effort. Decide whether this PR was already reviewed")
    }),
    ("mechanical", "evidence-resolver", |s| {
        s.starts_with("Run this bash EXACTLY and return its stdout verbatim")
    }),
    ("mechanical", "evidence-resolver", |s| {
        s.starts_with("Run this bash EXACTLY and return its stdout as structured data")
    }),
    ("mechanical", "check-external-heads", |s| {
        s.starts_with("Run these commands and return their output verbatim, nothing else, in this exact order")
    }),
    ("mechanical", "check-external-heads", |s| {
        s.starts_with("Run these three commands and return their output verbatim")
    }),
    ("mechanical", "check-external-heads", |s| {
        s.starts_with("Run `command -v codex") || s.starts_with("Run `command -v gemini")
    }),
    ("mechanical", "setup-worktree", |s| {
        s.starts_with("Create a detached git worktree")
    }),
    ("mechanical", "setup-head-worktree", |s| {
        s.starts_with("Run this bash EXACTLY and return the LAST line it prints, verbatim, as your entire answer")
    }),
    ("mechanical", "ref-evidence-check", |s| {
        s.starts_with("Run this bash EXACTLY and return its output verbatim, nothing else. It prints one MISSING line")
    }),
    ("mechanical", "grounded-typecheck", |s| {
        s.starts_with("Run a TypeScript typecheck for grounded verification")
    }),
    ("mechanical", "classify-artifacts", |s| {
        s.starts_with("Run one classification script and report its output EXACTLY")
    }),
    ("mechanical", "persist", |s| {
        s.starts_with("Run a sequence of scripts EXACTLY as given")
            || s.starts_with("Run one script EXACTLY as given")
    }),
    ("mechanical", "persist", |s| {
        s.starts_with("Append one JSON line to a metrics log")
    }),
    ("mechanical", "persist-parts", |s| {
        s.starts_with("Mechanical step. Run EXACTLY this bash and return its LAST line verbatim as your entire answer. The only text you compose yourself is copying each heredoc body")
    }),
    ("mechanical", "persist-parts", |s| {
        s.starts_with("Mechanical step. Run EXACTLY this bash and return its LAST line verbatim as your entire answer. Each `printf` line carries one part")
    }),
    ("mechanical", "persist-parts", |s| {
        after(s, "Mechanical step. Run EACH of the ")
            .is_some_and(|r| digits_then(r, " bash block(s) below as its own Bash call"))
    }),
    ("mechanical", "reconcile", |s| {
        s.starts_with("Run a vote-reconciliation script")
    }),
    ("mechanical", "self-audit", |s| {
        s.starts_with("Run an audit script and report its output EXACTLY")
    }),
    ("mechanical", "verify-metrics-row", |s| {
        s.starts_with("Return the last line of")
    }),
    ("mechanical", "teardown-worktree", |s| {
        s.starts_with("Remove a temporary git worktree")
            || s.starts_with("Remove temporary git worktrees")
    }),
    ("mechanical", "persist-and-verify", |s| {
        s.starts_with("Mechanical step in three parts.")
    }),
    ("mechanical", "context-relay", |s| {
        s.starts_with("Mechanical step. Run the bash below and return its relay line.")
    }),
    ("mechanical", "resolver-relay", |s| {
        s.starts_with("Mechanical step. Return the relay line of the bash below.")
    }),
    ("external-heads", "heads-driver", |s| {
        s.starts_with("You are a DRIVER, not a reviewer.")
    }),
    ("external-heads", "sweep-artifact-read", |s| {
        s.starts_with("Run exactly this bash, then report what it printed.")
    }),
];

/// Ports `stripRuntimeFrame`: drops the Workflow runtime's opening line,
/// `/^\s*\[Workflow harness[^\]\n]*\][^\n]*\n/`, and returns the rest.
#[must_use]
pub fn strip_runtime_frame(s: &str) -> &str {
    let Some(rest) = s
        .trim_start_matches(js::is_js_whitespace)
        .strip_prefix("[Workflow harness")
    else {
        return s;
    };
    let Some(close) = rest.find([']', '\n']) else {
        return s;
    };
    if !rest[close..].starts_with(']') {
        return s;
    }
    let tail = &rest[close + 1..];
    tail.find('\n').map_or(s, |nl| &tail[nl + 1..])
}

/// Ports `classifyAgent`: `(kind, name)` for a transcript's prompt head.
#[must_use]
pub fn classify_agent(prompt_head: &str) -> (&'static str, &'static str) {
    let mut s = js::trim(strip_runtime_frame(prompt_head));
    while let Some(rest) = s
        .strip_prefix("METHODOLOGY FILE: ")
        .or_else(|| s.strip_prefix("CONVENTIONS FILE: "))
    {
        let n = non_space_run(rest);
        if n == 0 {
            break;
        }
        let rest = &rest[n..];
        s = rest.trim_start_matches(js::is_js_whitespace);
    }
    PATTERNS
        .iter()
        .find(|(_, _, test)| test(s))
        .map_or(("unclassified", "unclassified"), |(k, n, _)| (k, n))
}

// ---------------------------------------------------------------------------
// parseTranscript

/// What `parseTranscript` returns, the fields the row keeps.
#[derive(Debug, Clone, PartialEq)]
pub struct Transcript {
    pub turns: f64,
    pub output_tokens: Js,
    pub cache_read: Js,
    pub cache_creation: Js,
    pub fresh_input: Js,
    pub input_tokens: Js,
    pub tool_uses: f64,
    pub tool_turns: f64,
    pub prompt_head: String,
}

/// A `Map`/`Set` key: SameValueZero for primitives, identity for objects
/// (each line parses a fresh one).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Str(String),
    Num(u64),
    Bool(bool),
    Fresh(usize),
}

fn key_of(v: &Value, fresh: &mut usize) -> Key {
    match v {
        Value::String(s) => Key::Str(s.clone()),
        Value::Number(n) => {
            let x = n.as_f64().unwrap_or(f64::NAN);
            Key::Num(if x == 0.0 { 0 } else { x.to_bits() })
        }
        Value::Bool(b) => Key::Bool(*b),
        _ => {
            *fresh += 1;
            Key::Fresh(*fresh)
        }
    }
}

fn first_user_text(rec: &Value) -> String {
    let content = js::get(rec, "message")
        .filter(|m| js::truthy(Some(m)))
        .and_then(|m| js::get(m, "content"));
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| match js::get(x, "text") {
                Some(Value::String(t)) if js::truthy(Some(x)) => t.clone(),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_space = false;
    for c in s.chars() {
        if js::is_js_whitespace(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

/// `s.slice(0, n)` in UTF-16 units; a pair the cut splits leaves U+FFFD.
fn utf16_slice(s: &str, n: usize) -> String {
    let mut out = String::new();
    let mut units = 0;
    for c in s.chars() {
        let u = c.len_utf16();
        if units + u > n {
            if units < n {
                out.push('\u{FFFD}');
            }
            break;
        }
        out.push(c);
        units += u;
    }
    out
}

/// Ports `parseTranscript`.
#[must_use]
pub fn parse_transcript(text: &str) -> Transcript {
    let mut out = Transcript {
        turns: 0.0,
        output_tokens: Js::Num(0.0),
        cache_read: Js::Num(0.0),
        cache_creation: Js::Num(0.0),
        fresh_input: Js::Num(0.0),
        input_tokens: Js::Num(0.0),
        tool_uses: 0.0,
        tool_turns: 0.0,
        prompt_head: String::new(),
    };
    let mut fresh = 0usize;
    let mut seen: std::collections::HashSet<Key> = std::collections::HashSet::new();
    let mut output_by_message: HashMap<Key, Js> = HashMap::new();
    let mut tool_blocks: Vec<(Key, usize)> = Vec::new();
    for (i, line) in text.split('\n').enumerate() {
        let line_index = i + 1;
        if js::trim(line).is_empty() {
            continue;
        }
        let Ok(rec) = json_parse(line) else {
            continue;
        };
        if out.prompt_head.is_empty()
            && js::get(&rec, "type") == Some(&Value::String("user".into()))
        {
            let t = collapse_whitespace(strip_runtime_frame(&first_user_text(&rec)));
            let t = js::trim(&t);
            if !t.is_empty() {
                out.prompt_head = utf16_slice(t, 400);
            }
        }
        let message = js::get(&rec, "message");

        if let Some(m) = message {
            if js::get(m, "role") == Some(&Value::String("assistant".into())) {
                if let Some(Value::Array(content)) = js::get(m, "content") {
                    let blocks = content
                        .iter()
                        .filter(|b| js::get(b, "type") == Some(&Value::String("tool_use".into())))
                        .count();
                    if blocks > 0 {
                        let id = js::get(m, "id").filter(|id| js::truthy(Some(id)));
                        let key = match id {
                            Some(id) => key_of(id, &mut fresh),
                            None => Key::Str(format!("__line{line_index}")),
                        };
                        match tool_blocks.iter_mut().find(|(k, _)| *k == key) {
                            Some((_, n)) => *n += blocks,
                            None => tool_blocks.push((key, blocks)),
                        }
                    }
                }
            }
        }

        let usage = message
            .filter(|m| js::truthy(Some(m)))
            .and_then(|m| js::get(m, "usage"))
            .filter(|u| js::truthy(Some(u)))
            .or_else(|| js::get(&rec, "usage").filter(|u| js::truthy(Some(u))));
        let Some(u) = usage else {
            continue;
        };
        let message_id = match message {
            Some(m) if js::truthy(Some(m)) => js::get(m, "id").filter(|id| js::truthy(Some(id))),
            _ => None,
        };
        let message_key = message_id.map(|id| key_of(id, &mut fresh));
        let first_line = match &message_key {
            None => true,
            Some(k) => seen.insert(k.clone()),
        };

        let line_output = Js::or_zero(js::get(u, "output_tokens"));
        match &message_key {
            None => out.output_tokens = out.output_tokens.add(&line_output),
            Some(k) => {
                let prior = output_by_message
                    .get(k)
                    .filter(|p| p.truthy())
                    .cloned()
                    .unwrap_or(Js::Num(0.0));
                if line_output.gt(&prior) {
                    out.output_tokens = out
                        .output_tokens
                        .add(&Js::Num(line_output.number() - prior.number()));
                    output_by_message.insert(k.clone(), line_output.clone());
                }
            }
        }
        if first_line {
            out.turns += 1.0;
            out.cache_read = out
                .cache_read
                .add(&Js::or_zero(js::get(u, "cache_read_input_tokens")));
            out.cache_creation = out
                .cache_creation
                .add(&Js::or_zero(js::get(u, "cache_creation_input_tokens")));
            out.fresh_input = out
                .fresh_input
                .add(&Js::or_zero(js::get(u, "input_tokens")));
        }
    }
    out.input_tokens = out
        .cache_read
        .add(&out.cache_creation)
        .add(&out.fresh_input);
    #[allow(clippy::cast_precision_loss)]
    {
        out.tool_turns = tool_blocks.len() as f64;
        out.tool_uses = tool_blocks.iter().map(|(_, n)| *n).sum::<usize>() as f64;
    }
    out
}

/// Ports `agentIdsFromJournal`.
#[must_use]
pub fn agent_ids_from_journal(text: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for line in text.split('\n') {
        if js::trim(line).is_empty() {
            continue;
        }
        let Ok(rec) = json_parse(line) else {
            continue;
        };
        if let Some(Value::String(id)) = js::get(&rec, "agentId") {
            if !id.is_empty() && !ids.contains(id) {
                ids.push(id.clone());
            }
        }
    }
    ids
}

// ---------------------------------------------------------------------------
// readRunDir / toMetricsShape

#[derive(Debug, Clone, PartialEq)]
struct Totals {
    agents: Js,
    turns: Js,
    output_tokens: Js,
    input_tokens: Js,
    cache_read: Js,
    cache_creation: Js,
    fresh_input: Js,
    tool_uses: Js,
    tool_turns: Js,
}

impl Totals {
    fn zero() -> Self {
        Self::filled(&Js::Num(0.0))
    }

    /// `Object.prototype` before anything writes to it.
    fn undefined() -> Self {
        Self::filled(&Js::Undef)
    }

    fn filled(v: &Js) -> Self {
        Self {
            agents: v.clone(),
            turns: v.clone(),
            output_tokens: v.clone(),
            input_tokens: v.clone(),
            cache_read: v.clone(),
            cache_creation: v.clone(),
            fresh_input: v.clone(),
            tool_uses: v.clone(),
            tool_turns: v.clone(),
        }
    }

    fn add(&mut self, a: &Transcript) {
        self.agents = Js::Num(self.agents.number() + 1.0);
        self.turns = self.turns.add(&Js::Num(a.turns));
        self.output_tokens = self.output_tokens.add(&a.output_tokens);
        self.input_tokens = self.input_tokens.add(&a.input_tokens);
        self.cache_read = self.cache_read.add(&a.cache_read);
        self.cache_creation = self.cache_creation.add(&a.cache_creation);
        self.fresh_input = self.fresh_input.add(&a.fresh_input);
        self.tool_uses = self.tool_uses.add(&Js::Num(a.tool_uses));
        self.tool_turns = self.tool_turns.add(&Js::Num(a.tool_turns));
    }
}

/// What `readRunDir` returns, reduced to what `toMetricsShape` reads.
#[derive(Debug, Clone, PartialEq)]
pub struct RunCost {
    run_id: String,
    missing_transcripts: usize,
    totals: Totals,
    by_kind: Vec<(String, Totals)>,
    by_model: Vec<(String, Totals)>,
}

fn exists(env: &LedgerEnv, p: &str) -> bool {
    std::fs::metadata(env.fs_path(p)).is_ok()
}

/// `readFileSync(p, "utf8")`, with Node's message on failure.
fn read_utf8(env: &LedgerEnv, p: &str) -> Result<String, String> {
    let bytes = std::fs::read(env.fs_path(p)).map_err(|e| {
        if e.kind() == std::io::ErrorKind::IsADirectory {
            node_fs_message(&e, "read", None)
        } else {
            node_fs_message(&e, "open", Some(p))
        }
    })?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn model_of(env: &LedgerEnv, meta: &str) -> String {
    if !exists(env, meta) {
        return "unknown".into();
    }
    let model = read_utf8(env, meta)
        .ok()
        .and_then(|t| json_parse(&t).ok())
        .and_then(|v| {
            js::get(&v, "model")
                .filter(|m| js::truthy(Some(m)))
                .cloned()
        });
    model.map_or_else(|| "unknown".into(), |m| to_js_string(Some(&m)))
}

/// `byModel[key] = byModel[key] || emptyTotals()`. `__proto__` reaches
/// `Object.prototype` itself (`proto`), whose fields every function inherits:
/// a key naming an `Object.prototype` method starts from what `proto` holds
/// at that moment, where a plain key starts from zeros.
fn bucket<'a>(
    list: &'a mut Vec<(String, Totals)>,
    proto: &'a mut Totals,
    key: &str,
) -> &'a mut Totals {
    if key == "__proto__" {
        return proto;
    }
    let i = match list.iter().position(|(k, _)| k == key) {
        Some(i) => i,
        None => {
            let start = if object_prototype_property(key).is_some() {
                proto.clone()
            } else {
                Totals::zero()
            };
            list.push((key.to_owned(), start));
            list.len() - 1
        }
    };
    &mut list[i].1
}

/// Ports `readRunDir`. `dir` is the path as `findRunDirs` built it.
///
/// # Errors
/// A journal that is gone or unreadable, or a transcript that cannot be read.
pub fn read_run_dir(env: &LedgerEnv, dir: &str) -> Result<RunCost, String> {
    let journal = path_join(&[dir, "journal.jsonl"]);
    if !exists(env, &journal) {
        return Err(format!("no journal.jsonl in {dir}"));
    }
    let ids = agent_ids_from_journal(&read_utf8(env, &journal)?);
    let mut missing = 0;
    let mut totals = Totals::zero();
    let mut by_kind: Vec<(String, Totals)> = Vec::new();
    let mut by_model: Vec<(String, Totals)> = Vec::new();
    let mut proto = Totals::undefined();
    for id in &ids {
        let t_path = path_join(&[dir, &format!("agent-{id}.jsonl")]);
        if !exists(env, &t_path) {
            missing += 1;
            continue;
        }
        let parsed = parse_transcript(&read_utf8(env, &t_path)?);
        let model = model_of(env, &path_join(&[dir, &format!("agent-{id}.meta.json")]));
        let (kind, _) = classify_agent(&parsed.prompt_head);
        totals.add(&parsed);
        bucket(&mut by_kind, &mut proto, kind).add(&parsed);
        bucket(&mut by_model, &mut proto, &model).add(&parsed);
    }
    Ok(RunCost {
        run_id: dir.rsplit('/').next().unwrap_or(dir).to_owned(),
        missing_transcripts: missing,
        totals,
        by_kind,
        by_model,
    })
}

fn input_units(t: &Totals) -> Value {
    match (
        t.fresh_input.finite(),
        t.cache_creation.finite(),
        t.cache_read.finite(),
    ) {
        (Some(f), Some(c), Some(r)) => {
            num(math_round((f * 1.0 + c * 1.25 + r * 0.1) * 100.0) / 100.0)
        }
        _ => Value::Null,
    }
}

fn per_turn(t: &Totals) -> Value {
    if t.tool_turns.truthy() {
        num(js::string_to_number(&to_fixed(
            t.tool_uses.number() / t.tool_turns.number(),
            2,
        )))
    } else {
        Value::Null
    }
}

fn put(m: &mut Map<String, Value>, k: &str, v: &Js) {
    if let Some(v) = v.to_json() {
        m.insert(k.to_owned(), v);
    }
}

fn pick(t: &Totals) -> Value {
    let mut m = Map::new();
    put(&mut m, "agents", &t.agents);
    put(&mut m, "turns", &t.turns);
    put(&mut m, "inputTokens", &t.input_tokens);
    put(&mut m, "outputTokens", &t.output_tokens);
    m.insert("inputUnits".into(), input_units(t));
    put(&mut m, "toolUses", &t.tool_uses);
    put(&mut m, "toolTurns", &t.tool_turns);
    m.insert("toolUsesPerTurn".into(), per_turn(t));
    Value::Object(m)
}

/// Ports `toMetricsShape`.
#[must_use]
pub fn to_metrics_shape(run: &RunCost) -> Value {
    let t = &run.totals;
    let mut m = Map::new();
    m.insert("runId".into(), Value::String(run.run_id.clone()));
    put(&mut m, "turns", &t.turns);
    put(&mut m, "inputTokens", &t.input_tokens);
    put(&mut m, "outputTokens", &t.output_tokens);
    m.insert("inputUnits".into(), input_units(t));
    m.insert("outputExact".into(), Value::Bool(false));
    put(&mut m, "cacheRead", &t.cache_read);
    put(&mut m, "cacheCreation", &t.cache_creation);
    put(&mut m, "agents", &t.agents);
    m.insert("missingTranscripts".into(), run.missing_transcripts.into());
    put(&mut m, "toolUses", &t.tool_uses);
    put(&mut m, "toolTurns", &t.tool_turns);
    m.insert("toolUsesPerTurn".into(), per_turn(t));
    let group = |list: &[(String, Totals)]| {
        Value::Object(list.iter().map(|(k, v)| (k.clone(), pick(v))).collect())
    };
    m.insert("byKind".into(), group(&run.by_kind));
    m.insert("byModel".into(), group(&run.by_model));
    let u = run
        .by_kind
        .iter()
        .find(|(k, _)| k == "unclassified")
        .map(|(_, t)| t);
    let mut un = Map::new();
    match u {
        Some(u) => {
            put(&mut un, "agents", &u.agents);
            put(&mut un, "inputTokens", &u.input_tokens);
        }
        None => {
            un.insert("agents".into(), 0.into());
            un.insert("inputTokens".into(), 0.into());
        }
    }
    m.insert("unclassified".into(), Value::Object(un));
    Value::Object(m)
}

fn sorted_dirs(env: &LedgerEnv, dir: &str) -> Option<Vec<String>> {
    let mut names: Vec<Vec<u8>> = std::fs::read_dir(env.fs_path(dir))
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().into_encoded_bytes())
        .collect();
    names.sort();
    Some(
        names
            .into_iter()
            .map(|n| String::from_utf8_lossy(&n).into_owned())
            .collect(),
    )
}

/// Ports `findRunDirs`: every `wf_*` directory with a journal, sessions and
/// runs in byte order of their names, as libuv's scandir lists them.
#[must_use]
pub fn find_run_dirs(env: &LedgerEnv, project_dir: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Some(sessions) = sorted_dirs(env, project_dir) else {
        return out;
    };
    for s in sessions {
        let wf_root = path_join(&[project_dir, &s, "subagents", "workflows"]);
        if !exists(env, &wf_root) {
            continue;
        }
        let Some(runs) = sorted_dirs(env, &wf_root) else {
            continue;
        };
        for r in runs.into_iter().filter(|r| r.starts_with("wf_")) {
            let dir = path_join(&[&wf_root, &r]);
            if exists(env, &path_join(&[&dir, "journal.jsonl"])) {
                out.push(dir);
            }
        }
    }
    out
}

/// Ports `findRunDir`: the first run directory whose path ends in `run_id`.
#[must_use]
pub fn find_run_dir(env: &LedgerEnv, project_dir: &str, run_id: &str) -> Option<String> {
    find_run_dirs(env, project_dir)
        .into_iter()
        .find(|d| d.ends_with(run_id))
}

// ---------------------------------------------------------------------------
// The row, as a JavaScript object to assign into

/// `obj.key = v` on a value from `JSON.parse`. An array keeps the property
/// beside its elements, where `JSON.stringify` does not see it.
struct Target<'a> {
    value: &'a mut Value,
    overlay: &'a mut Map<String, Value>,
}

fn set_error(v: &Value, key: &str) -> String {
    match v {
        Value::Null => format!("Cannot set properties of null (setting '{key}')"),
        Value::Bool(b) => format!("Cannot create property '{key}' on boolean '{b}'"),
        Value::Number(n) => format!(
            "Cannot create property '{key}' on number '{}'",
            js::number_to_string(n.as_f64().unwrap_or(f64::NAN))
        ),
        Value::String(s) => format!("Cannot create property '{key}' on string '{s}'"),
        _ => unreachable!(),
    }
}

impl Target<'_> {
    fn set(&mut self, key: &str, v: Value) -> Result<(), String> {
        match &mut *self.value {
            Value::Object(m) => {
                m.insert(key.to_owned(), v);
                Ok(())
            }
            Value::Array(_) => {
                self.overlay.insert(key.to_owned(), v);
                Ok(())
            }
            other => Err(set_error(other, key)),
        }
    }

    fn get(&self, key: &str) -> Option<&Value> {
        match &*self.value {
            Value::Array(_) => self.overlay.get(key),
            v => js::get(v, key),
        }
    }
}

/// `f.key = v` on one finding: an array element keeps nothing visible.
fn set_on(v: &mut Value, key: &str, val: Value) -> Result<(), String> {
    match v {
        Value::Object(m) => {
            m.insert(key.to_owned(), val);
            Ok(())
        }
        Value::Array(_) => Ok(()),
        other => Err(set_error(other, key)),
    }
}

/// The message V8 gives `mergedLocationsMissingFromReportText`'s TypeError.
fn merged_locations_error(v: Option<&Value>, core: String) -> String {
    match v {
        Some(Value::String(_)) | Some(Value::Null) | None => core,
        Some(_) if core.starts_with("Cannot read") => core,
        Some(_) => "(mergedLocations || []).slice is not a function".into(),
    }
}

/// Ports `applyFidelityChecks` on the findings array.
///
/// # Errors
/// The TypeError the merged-locations check throws on a non-array list or a
/// `null` location.
pub fn apply_fidelity_checks(findings: &mut [Value], report_text: &str) -> Result<(), String> {
    for f in findings.iter_mut() {
        let unexamined = unexamined_wording_in_report_text(f, report_text);
        let Value::Object(m) = f else {
            continue;
        };
        let locations = m.get("mergedLocations");
        let merged = merged_locations_missing_from_report_text(locations, report_text)
            .map_err(|e| merged_locations_error(locations, e))?;
        let refuted = match m.get("votes") {
            Some(Value::Array(votes)) => votes.iter().find(|v| {
                js::truthy(Some(v))
                    && strict_eq(
                        js::get(v, "verdict"),
                        Some(&Value::String("REFUTED".into())),
                    )
            }),
            _ => None,
        };
        let kill = match refuted {
            Some(v) => kill_reason_missing_from_report_text(js::get(v, "killReason"), report_text),
            None => Vec::new(),
        };
        let merged_from = merged_from_annotation_missing_from_report_text(
            m.get("mergedFrom").filter(|v| js::truthy(Some(v))),
            report_text,
        );
        let grounded = grounded_diagnostic_missing_from_report_text(
            m.get("grounded").filter(|v| js::truthy(Some(v))),
            report_text,
        );
        m.insert(
            "mergedLocationsMissingFromReport".into(),
            Value::Array(merged),
        );
        m.insert("killReasonMissingFromReport".into(), Value::Array(kill));
        m.insert(
            "mergedFromAnnotationMissingFromReport".into(),
            Value::Array(merged_from),
        );
        m.insert(
            "groundedDiagnosticMissingFromReport".into(),
            Value::Array(grounded),
        );
        m.insert("unexaminedWordingInReport".into(), Value::Array(unexamined));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// main

struct Patched {
    row: Value,
    overlay: Map<String, Value>,
    agent_count: Option<f64>,
    agent_cost: Option<Value>,
    fidelity: Option<usize>,
}

impl Patched {
    fn get(&self, key: &str) -> Option<&Value> {
        match &self.row {
            Value::Array(_) => self.overlay.get(key),
            v => js::get(v, key),
        }
    }
}

const USAGE: &str = "usage: --pr <n> --posted <true|false> --verdict <string> [--repo <owner/name>] [--head-sha] [--run-id] [--agent-count] [--project-dir] [--report-file]\n";

fn patch(args: &Args, env: &LedgerEnv, pr: &str, stderr: &mut String) -> Result<Patched, String> {
    let metrics = env.metrics_path();
    let q = RowQuery {
        pr: Some(Value::String(pr.to_owned())),
        head_sha: args.get("head-sha").map(Arg::value),
        repo: match args.get("repo") {
            Some(Arg::Value(r)) => Some(r.clone()),
            _ => None,
        },
    };
    let located = locate_last_row(&metrics, &q)?;
    let mut lines = located.lines;
    let mut row = located.row;
    let mut overlay = Map::new();
    let mut last = Target {
        value: &mut row,
        overlay: &mut overlay,
    };

    let posted = args
        .get("posted")
        .map(|p| matches!(p, Arg::Flag) || p.text() == "true");
    if let Some(p) = posted {
        last.set("posted", Value::Bool(p))?;
    }
    if let Some(v) = args.get("verdict") {
        last.set("verdict", v.value())?;
    }
    if let Some(p) = posted {
        if let Some(Value::Array(findings)) = last.value.get_mut("findings") {
            for f in findings.iter_mut() {
                set_on(f, "posted", Value::Bool(p))?;
            }
        }
    }

    let agent_count = args.get("agent-count").map(Arg::number);
    if let Some(n) = agent_count {
        last.set("agentsActual", num(n))?;
    }

    let mut agent_cost = None;
    if let Some(run_id) = args.get("run-id").filter(|a| a.truthy()) {
        let project_dir = args.get("project-dir").filter(|a| a.truthy()).map_or_else(
            || Some(env.default_project_dir()),
            |a| match a {
                Arg::Value(d) => Some(d.clone()),
                Arg::Flag => None,
            },
        );
        let dir = project_dir
            .as_deref()
            .and_then(|d| find_run_dir(env, d, &run_id.text()));
        match dir {
            None => stderr.push_str(&format!(
                "warning: no run directory found for --run-id {} under {}; agentCost left untouched\n",
                run_id.text(),
                project_dir.as_deref().unwrap_or("true")
            )),
            Some(dir) => {
                let cost = to_metrics_shape(&read_run_dir(env, &dir)?);
                last.set("agentCost", cost.clone())?;
                agent_cost = Some(cost);
            }
        }
    }

    let mut fidelity = None;
    if let Some(Arg::Value(report)) = args.get("report-file").filter(|a| a.truthy()) {
        let has_findings = matches!(last.get("findings"), Some(Value::Array(f)) if !f.is_empty());
        if has_findings {
            match read_utf8(env, report) {
                Err(e) => stderr.push_str(&format!(
                    "warning: could not read --report-file {report} ({e}); *MissingFromReport left untouched\n"
                )),
                Ok(text) => {
                    if let Some(Value::Array(findings)) = last.value.get_mut("findings") {
                        apply_fidelity_checks(findings, &text)?;
                        fidelity = Some(findings.len());
                    }
                }
            }
        }
    }

    lines[located.line_index] = js::stringify(&row, "");
    crate::operator::write_file(&metrics, &(lines.join("\n") + "\n"))?;
    Ok(Patched {
        row,
        overlay,
        agent_count,
        agent_cost,
        fidelity,
    })
}

fn append(env: &LedgerEnv, path: &str, line: &str) -> Result<(), String> {
    OpenOptions::new()
        .append(true)
        .create(true)
        .open(env.fs_path(path))
        .map_err(|e| node_fs_message(&e, "open", Some(path)))?
        .write_all(line.as_bytes())
        .map_err(|e| node_fs_message(&e, "write", None))
}

fn insert_opt(m: &mut Map<String, Value>, k: &str, v: Option<&Value>) {
    if let Some(v) = v {
        m.insert(k.to_owned(), v.clone());
    }
}

/// The script's command line: `node persist-review-record.mjs --pr <n>
/// [--repo <o/r>] --posted <bool> --verdict <s> [--head-sha <sha>]
/// [--run-id <wf_…>] [--agent-count <n>] [--project-dir <path>]
/// [--report-file <path>]`. Prints the one-line summary.
#[must_use]
pub fn run(argv: &[String], env: &LedgerEnv) -> CliOutput {
    let args = parse_args(argv);
    let Some(pr) = args.get("pr").filter(|a| a.truthy()).map(Arg::text) else {
        return CliOutput {
            stdout: String::new(),
            stderr: USAGE.into(),
            code: 2,
        };
    };
    let mut stderr = String::new();
    let patched = match with_ledger_lock(&env.metrics_path(), env.lock_wait, || {
        patch(&args, env, &pr, &mut stderr)
    }) {
        Ok(Ok(p)) => p,
        Ok(Err(e)) | Err(e) => {
            stderr.push_str(&e);
            stderr.push('\n');
            return CliOutput {
                stdout: String::new(),
                stderr,
                code: 1,
            };
        }
    };

    let (pr_v, sha_v) = (patched.get("pr"), patched.get("headSha"));
    if let Some(n) = patched.agent_count {
        let mut m = Map::new();
        insert_opt(&mut m, "pr", pr_v);
        insert_opt(&mut m, "headSha", sha_v);
        m.insert("agentCount".into(), num(n));
        m.insert("ts".into(), Value::Null);
        let line = js::stringify(&Value::Object(m), "") + "\n";
        if let Err(e) = append(env, &env.agent_counts_path(), &line) {
            stderr.push_str(&format!(
                "warning: failed to append agent-counts.jsonl: {e}\n"
            ));
        }
    }
    if let Some(Value::Object(cost)) = &patched.agent_cost {
        let mut m = Map::new();
        insert_opt(&mut m, "pr", pr_v);
        insert_opt(&mut m, "headSha", sha_v);
        for (k, v) in cost {
            m.insert(k.clone(), v.clone());
        }
        let line = js::stringify(&Value::Object(m), "") + "\n";
        if let Err(e) = append(env, &env.agent_costs_path(), &line) {
            stderr.push_str(&format!(
                "warning: failed to append agent-costs.jsonl: {e}\n"
            ));
        }
    }

    let mut summary = Map::new();
    insert_opt(&mut summary, "pr", pr_v);
    insert_opt(&mut summary, "headSha", sha_v);
    insert_opt(&mut summary, "posted", patched.get("posted"));
    insert_opt(&mut summary, "verdict", patched.get("verdict"));
    insert_opt(&mut summary, "agentsActual", patched.get("agentsActual"));
    summary.insert(
        "agentCostTurns".into(),
        patched
            .agent_cost
            .as_ref()
            .and_then(|c| js::get(c, "turns"))
            .cloned()
            .unwrap_or(Value::Null),
    );
    summary.insert(
        "fidelityRecomputed".into(),
        patched.fidelity.map_or(Value::Null, Value::from),
    );
    CliOutput {
        stdout: js::stringify(&Value::Object(summary), "") + "\n",
        stderr,
        code: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_concatenate_and_falsy_values_fall_back_to_zero() {
        let t = parse_transcript(
            "{\"usage\":{\"output_tokens\":\"5\",\"input_tokens\":2}}\n{\"usage\":{\"output_tokens\":3,\"input_tokens\":0}}",
        );
        assert_eq!(t.output_tokens, Js::Str("053".into()));
        assert_eq!(t.fresh_input, Js::Num(2.0));
        assert_eq!(t.turns, 2.0);
    }

    #[test]
    fn prefixes_are_stripped_before_classifying() {
        assert_eq!(
            classify_agent("METHODOLOGY FILE: /a CONVENTIONS FILE: /b Review PR #12 in repo o/r"),
            ("lens", "lens")
        );
        assert_eq!(
            classify_agent("A code-review finding on PR #3 x was nearly refuted"),
            ("strengthen", "strengthen")
        );
        assert_eq!(
            classify_agent("Fetch PR # from GitHub repo"),
            ("unclassified", "unclassified")
        );
    }
}
