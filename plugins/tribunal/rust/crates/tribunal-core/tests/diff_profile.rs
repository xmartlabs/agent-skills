mod common;
use common::*;

// Differential tests for the profile group: PROFILE-LOOKUP, LENS-ROUTING,
// LENS-FOCI, HIGH-RISK-SURFACE, FILE-PARTITION and CONVENTIONS-CHANNEL.
//
// Several of these functions take or return what JSON cannot carry: risk
// rules hold a `RegExp`, and the conventions channel returns closures. Those
// go through a small composite driver that slices the same region with the
// same `loadRegion` and builds the rules (or calls the closures) on the
// JavaScript side, so both sides start from the same JSON profile.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use proptest::prelude::*;
use serde_json::{json, Map, Value};

use tribunal_core::profile::conventions_channel::conventions_channel;
use tribunal_core::profile::file_partition::{partition_files, partition_files_by_churn};
use tribunal_core::profile::high_risk_surface::{
    high_risk_surface_for, risk_path_touched, with_risk_path_surface,
};
use tribunal_core::profile::lens_foci::{foci_for, foci_label_for, select_foci};
use tribunal_core::profile::lens_routing::{lens_agent_for, AGENT_NS, REVIEWER_AGENTS};
use tribunal_core::profile::profile_lookup::{
    in_context_risk_path_rules, profile_warnings, risk_path_problems, risk_path_rules,
    validate_profile, RiskRule,
};

const COMPOSITE: &str = r#"
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
const { loadRegion } = await import(pathToFileURL(process.env.EXTRACT).href);
const { api, violations } = loadRegion(
  process.env.WORKFLOW,
  process.env.REGION,
  process.env.EXPORTS.split(","),
  process.env.DEPS ? process.env.DEPS.split(",") : [],
);
if (violations.length) {
  console.error(violations.join("\n"));
  process.exit(1);
}
const rules = (which, profile) => (profile === null ? null : api[which](profile));
const fns = {
  rulesView: (which, profile, probes) =>
    api[which](profile).map((r) => ({ focus: r.focus, hits: probes.map((p) => r.re.test(p)) })),
  selectFoci: (files, budget, profile, which) =>
    api.selectFoci(files, budget, api.fociFor(profile), api[which](profile)),
  // A budget JSON cannot carry (NaN, Infinity), sent as text.
  selectFociText: (files, budget, profile, which) =>
    api.selectFoci(files, Number(budget), api.fociFor(profile), api[which](profile)),
  lensAgentType: (profile) => typeof api.lensAgentFor(profile),
  riskPathTouched: (file, profile) => api.riskPathTouched(file, rules("riskPathRules", profile)),
  withRiskPathSurface: (findings, profile) =>
    api.withRiskPathSurface(findings, rules("riskPathRules", profile)),
  conventions: (args, profile, methodologyLine, prompt) => {
    const w = api.conventionsChannel(args, profile);
    return {
      manifest: w.manifest,
      withConventions: w.withConventions(prompt),
      reviewerPrompt: w.reviewerPrompt(methodologyLine, prompt),
    };
  },
  constants: () => ({ AGENT_NS: api.AGENT_NS, REVIEWER_AGENTS: api.REVIEWER_AGENTS }),
};
const calls = JSON.parse(readFileSync(0, "utf8"));
const out = calls.map(({ fn, args }) => {
  try {
    const v = fns[fn](...args);
    return { ok: v === undefined ? null : v };
  } catch (err) {
    return { err: String(err && err.message) };
  }
});
process.stdout.write(JSON.stringify(out));
"#;

fn composite_raw(
    region: &str,
    exports: &[&str],
    deps: &[&str],
    calls: &[Value],
) -> Vec<Result<Value, String>> {
    let oracle_dir = oracle_path().parent().unwrap().to_path_buf();
    let plugin = oracle_dir.join("../..");
    let extract: PathBuf = plugin.join("skills/review-pr/eval/extract-pure-region.mjs");
    let workflow: PathBuf = plugin.join("workflow-src/tribunal.js");
    let mut child = Command::new("node")
        .args(["--input-type=module", "-e", COMPOSITE])
        .env("EXTRACT", extract)
        .env("WORKFLOW", workflow)
        .env("REGION", region)
        .env("EXPORTS", exports.join(","))
        .env("DEPS", deps.join(","))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("node must be on PATH to run the differential tests");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(calls).unwrap().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "composite driver failed for {region}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let results: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(results.len(), calls.len());
    results
        .into_iter()
        .map(|r| match r.get("ok") {
            Some(v) => Ok(v.clone()),
            None => Err(r["err"].as_str().unwrap_or_default().to_owned()),
        })
        .collect()
}

/// A result as one comparable value, so an error on one side and a value on
/// the other is a difference like any other.
fn res(r: Result<Value, String>) -> Value {
    match r {
        Ok(v) => json!({ "ok": v }),
        Err(e) => json!({ "err": e }),
    }
}

const SYNTAX_ERROR: &str = "Invalid regular expression: /";

/// Declared difference: the reason a pattern does not compile is regress's
/// wording, not V8's. Both sides write `Invalid regular expression:
/// /<pattern>/i: <reason>`, and this replaces `<reason>` with `…` wherever
/// such a message appears (a `riskPathProblems` entry, the joined
/// `validateProfile` error, a `SyntaxError` thrown while building rules).
/// Whether a pattern compiles is still compared. Witness: pattern `(`, which
/// V8 refuses with "Unterminated group" and regress with "Unbalanced
/// parenthesis".
fn normalize_message(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(at) = rest.find(SYNTAX_ERROR) {
        let body = &rest[at + SYNTAX_ERROR.len()..];
        let Some(sep) = body.find("/i: ") else {
            break;
        };
        let reason = &body[sep + 4..];
        let end = reason.find('\n').unwrap_or(reason.len());
        let close = if reason[..end].ends_with(')') {
            ")"
        } else {
            ""
        };
        out.push_str(&rest[..at + SYNTAX_ERROR.len() + sep + 4]);
        out.push('…');
        out.push_str(close);
        rest = &reason[end..];
    }
    out.push_str(rest);
    out
}

fn normalize(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(normalize_message(s)),
        Value::Array(a) => Value::Array(a.iter().map(normalize).collect()),
        Value::Object(m) => {
            Value::Object(m.iter().map(|(k, v)| (k.clone(), normalize(v))).collect())
        }
        other => other.clone(),
    }
}

/// `assert_same` over normalized values.
fn same(what: &str, calls: &[Value], rust: &[Value], js: &[Value]) {
    let rust: Vec<Value> = rust.iter().map(normalize).collect();
    let js: Vec<Value> = js.iter().map(normalize).collect();
    assert_same(what, calls, &rust, &js);
}

fn compare(
    what: &str,
    calls: &[Value],
    rust: Vec<Result<Value, String>>,
    js: Vec<Result<Value, String>>,
) {
    let rust: Vec<Value> = rust.into_iter().map(res).collect();
    let js: Vec<Value> = js.into_iter().map(res).collect();
    same(what, calls, &rust, &js);
}

fn pick(vals: Vec<Option<Value>>) -> impl Strategy<Value = Option<Value>> {
    prop::sample::select(vals)
}

fn obj(fields: Vec<(&str, Option<Value>)>) -> Value {
    let mut m = Map::new();
    for (k, v) in fields {
        if let Some(v) = v {
            m.insert(k.to_owned(), v);
        }
    }
    Value::Object(m)
}

fn s(x: &str) -> Option<Value> {
    Some(json!(x))
}

// ---------------------------------------------------------------------------
// Patterns and the strings they are tested on
// ---------------------------------------------------------------------------

const ATOMS: &[&str] = &[
    "a", "b", "A", "é", "É", "ſ", "s", "k", "K", "\u{212A}", "ß", "σ", "ς", "_", "0", "1", "-",
    "/", ".", "^", "$", "|", "\\d", "\\D", "\\w", "\\W", "\\s", "\\S", "\\b", "\\B", "\\c", "\\cA",
    "\\c1", "\\x41", "\\x4", "\\u0041", "\\u00e9", "\\/", "\\-", "\\.", "\\t", "\\n", "\\p", "\\k",
    "\\k<n>", "\\k<a>", "\\1", "\\2", "\\0", "\\00", "\\12", "\\8", "\\400", "[a-z]", "[z-a]",
    "[\\d-z]", "[a-]", "[^a]", "[\\w\\s]", "[\\b]", "[\\k]", "[\\c1]", "[\\cA]", "[^]", "[]",
    "[A-Z]", "[ſ]", "[^k]", "{", "}", "]", "\n", "\r", "\u{2028}", " ", "😀", "x", "y",
];

const SOUP: &[&str] = &[
    "(",
    ")",
    "(?:",
    "(?=",
    "(?!",
    "(?<=",
    "(?<!",
    "(?<n>",
    "(?<a>",
    "(?<é>",
    "(?<\\u{61}>",
    "(?<1>",
    "(?<\\uZZ>",
    "(?<>",
    "(?i:",
    "(?-i:",
    "(?m:",
    "(?s:",
    "(?i-:",
    "(?ii:",
    "(?-:",
    "(?x:",
    "(?i)",
    "(?--i:",
    "*",
    "+",
    "?",
    "*?",
    "{2}",
    "{1,2}",
    "{2,1}",
    "{1,",
    "[",
    "[^",
    "\\",
];

const OPENERS: &[&str] = &[
    "(",
    "(?:",
    "(?=",
    "(?!",
    "(?<=",
    "(?<!",
    "(?<n>",
    "(?<a>",
    "(?<\\u0061>",
    "(?i:",
    "(?-i:",
    "(?m:",
    "(?s:",
    "(?is-m:",
];

const QUANTS: &[&str] = &[
    "", "", "", "", "*", "+", "?", "*?", "+?", "{2}", "{0,1}", "{1,}", "{2,1}", "{,1}", "{1,2}?",
];

fn atom() -> impl Strategy<Value = String> {
    (prop::sample::select(ATOMS), prop::sample::select(QUANTS)).prop_map(|(a, q)| format!("{a}{q}"))
}

fn pattern() -> impl Strategy<Value = String> {
    let group = (
        prop::sample::select(OPENERS),
        prop::collection::vec(atom(), 0..3),
        prop_oneof![9 => Just(")"), 1 => Just("")],
        prop::sample::select(QUANTS),
        proptest::bool::weighted(0.3),
    )
        .prop_map(|(o, inner, c, q, alt)| {
            let body = inner.concat();
            let body = if alt { format!("{body}|x") } else { body };
            format!("{o}{body}{c}{q}")
        });
    let element = prop_oneof![6 => atom(), 3 => group, 1 => prop::sample::select(SOUP).prop_map(str::to_owned)];
    prop::collection::vec(element, 1..5).prop_map(|v| v.concat())
}

const PROBE_CHARS: &[char] = &[
    'a', 'A', 'b', 'B', 'é', 'É', 'ſ', 's', 'S', 'k', 'K', '\u{212A}', 'ß', 'σ', 'ς', 'Σ', '\n',
    '\r', '\u{2028}', ' ', '\u{0001}', '\u{0008}', '0', '1', '_', '/', '-', 'x', 'y', '😀', '{',
    '}', '\\', '\u{FEFF}', '\t', '\u{0}', 'n', 'm', '.',
];

fn probe() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(PROBE_CHARS), 0..7)
        .prop_map(|v| v.into_iter().collect())
}

/// Patterns whose V8 behavior was read off by hand while writing the port.
const SEED_PATTERNS: &[&str] = &[
    "(",
    ")",
    "a)",
    "*",
    "a**",
    "a{2,1}",
    "[z-a]",
    "[",
    "\\",
    "a\\",
    "(?",
    "(?a)",
    "(?<",
    "(?<a",
    "(?<a>",
    "(?<1>x)",
    "(?<a>x)(?<a>y)",
    "(?<a>x)|(?<a>y)",
    "\\k<b>(?<a>x)",
    "\\k",
    "\\k<a",
    "(?<a>x)\\k",
    "(?<a>x)\\k<b>",
    "^*",
    "$*",
    "\\b*",
    "(?=a)*",
    "(?<=a)*",
    "(?<!a)?",
    "{",
    "{1}",
    "a{1}{2}",
    "x{1,",
    "]",
    "}",
    "(?i:a)",
    "(?i-i:a)",
    "(?ii:a)",
    "(?-:a)",
    "(?x:a)",
    "(?i)",
    "(?i-:a)",
    "(?im-s:a)",
    "[\\d-z]",
    "[a-\\d]",
    "\\c",
    "[\\c]",
    "(?<$>x)",
    "(?<a\\u0062>x)",
    "(?<\\u{61}>x)",
    "(?<é>x)",
    "(?<a-b>x)",
    "a{99999999999,3}",
    "((?<a>x)|(?<a>y))(?<a>z)",
    "\\u{1}",
    "\\8",
    "\\1(a)",
    "(?<a>x)\\k<a",
    "x*?+",
    "x+??",
    "(?s:.)",
    "(?m:^)",
    "a/(",
    "(?<a>x)(?<b>y)\\k<c>(",
    "\\k(?<a>x)",
    "(?<a>x)\\k<b>)",
    "\\c1",
    "[\\c1]",
    "\\400",
    "\\12",
    "(?<a\\u{>x)",
    "(?<\\ud83d\\ude00>x)",
    "(?<😀>x)",
    "(?<𝒜>x)",
    "(?<a\u{200D}>x)",
    "(?<\u{200D}>x)",
    "[b-a\\d]",
    "[\\d-a-b]",
    "[z-a",
    "(?<a>x)(?:(?<a>y)|z)",
    "(?<a>x)|(?:(?<a>y)|(?<a>z))",
    "(?:(?<a>x)|(?<a>w))|(?<a>z)",
    "\\k<a>(?<a>x)|(?<a>y)",
    "(?=a){2,1}",
    "(?<a>x)(?<a>y)(",
    "(?<a>x)(?<a>y",
    "[\\k](?<a>x)",
    "(?<a>x)\\k<a>\\k<a",
    "(?<=\\1(a))b",
    "(?<a\\u{62}c>x)\\k<a\\u0062c>",
    "(?<\\u0030>x)",
    "(?--i:a)",
    "(?i-s-m:a)",
    "(?<a>a)(?i:\\k<a>)",
    "(?i=a)",
    "(?-:",
    "(?<\\uZZ>x)",
    "(?<\\u{110000}>x)",
    "(?<\\u{}>x)",
    "(?<\\ud835\\udc9c>x)",
    "(?<a\\>x)",
    "(?<>x)",
    "(?<a>x)\\k<1>",
    "(?<·>x)",
    "(?<a·>x)",
    "(?<ª>x)",
    "(?<\u{0300}>x)",
    "s",
    "k",
    "\u{212A}",
    "ß",
    "é",
    "[a-z]",
    "\\w",
    "\\W",
    "[^k]",
    ".",
    "^b",
    "(?m:^b)",
    "😀+",
    "(?<=a)b",
    "(?<=(a)\\1)b",
    "(a)|\\1b",
    "(?-i:a)",
    "\\u0130",
    "i",
    "\u{0131}",
    "\u{03C3}",
    "\u{03A3}",
    "(?<a>a)(?-i:\\k<a>)",
    "(a)(?-i:\\1)",
    "(?-i:[a-z])",
    "(a*)*b",
    "(?:)*x",
    "(a|)*?b",
    "(?=(a))\\1b",
    "(?!(a))\\1b",
    "(\\2a|(b))+",
    "x{0}",
    "auth|role",
    "invoice|payment",
    "^prisma/migrations/",
    "\\.sql$",
    "",
];

const SEED_PROBES: &[&str] = &[
    "a",
    "A",
    "ab",
    "aB",
    "aA",
    "aab",
    "b",
    "ba",
    "ababc",
    "aaab",
    "x",
    "",
    "k",
    "K",
    "\u{212A}",
    "s",
    "S",
    "\u{017F}",
    "SS",
    "é",
    "É",
    "\r",
    "a\nb",
    "a\u{2028}b",
    "😀😀",
    "i",
    "\u{0130}",
    "I",
    "\u{03C2}",
    "\\c1",
    "\u{0011}",
    "c",
    " 0",
    "\u{0008}",
];

fn risk_profile(pattern: &str) -> Value {
    json!({ "riskPaths": [{ "focus": "security", "pattern": pattern }] })
}

fn rules_view(rules: &[RiskRule], probes: &[String]) -> Value {
    Value::Array(
        rules
            .iter()
            .map(|r| json!({ "focus": r.focus, "hits": probes.iter().map(|p| r.re.test(p)).collect::<Vec<_>>() }))
            .collect(),
    )
}

/// `new RegExp(pattern, "i")` on both sides, through the region's own
/// compile sites: `riskPathProblems` for the refusal text, `riskPathRules`
/// for the compiled rule and what it matches.
fn check_patterns(cases: &[(String, Vec<String>)]) {
    let exports = ["riskPathProblems", "riskPathRules"];
    let calls: Vec<Value> = cases
        .iter()
        .map(|(p, _)| json!({"fn": "riskPathProblems", "args": [[{"focus": "security", "pattern": p}], "riskPaths"]}))
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|(p, _)| {
            json!(risk_path_problems(
                Some(&json!([{"focus": "security", "pattern": p}])),
                "riskPaths"
            ))
        })
        .collect();
    let js = oracle("PROFILE-LOOKUP", &exports, &[], &calls);
    same("riskPathProblems (patterns)", &calls, &rust, &js);

    let calls: Vec<Value> = cases
        .iter()
        .map(|(p, probes)| json!({"fn": "rulesView", "args": ["riskPathRules", risk_profile(p), probes]}))
        .collect();
    let rust = cases
        .iter()
        .map(|(p, probes)| risk_path_rules(&risk_profile(p)).map(|r| rules_view(&r, probes)))
        .collect();
    let js = composite_raw("PROFILE-LOOKUP", &exports, &[], &calls);
    compare("riskPathRules (patterns)", &calls, rust, js);
}

/// The places where regress parts from V8 and `js_ext` rewrites the pattern
/// or the path, each with its neighbours that need no rewrite.
const WRAPPER_SEEDS: &[&str] = &[
    // A quantified word boundary is refused; a brace that is no quantifier
    // is a literal.
    "\\b*",
    "\\B+",
    "a\\b?",
    "\\b{2}",
    "\\B{1,}",
    "\\b{0,1}x",
    "\\b{,1}",
    "\\b{",
    "\\b{a}",
    "(?:\\b)*",
    "[\\b]*",
    "\\\\b*",
    // `\k` in a class is refused only when a named group exists.
    "[\\k]",
    "[\\k](?<a>x)",
    "(?<a>x)[\\k]",
    "(?<a>x)[\\\\k]",
    "[(?<a>x)\\k]",
    // `\u{` is the letter u and a brace, outside a group name.
    "\\u{61}",
    "^\\u{2}$",
    "[\\u{61}]",
    "\\u{1}*",
    "\\u{6",
    "\\\\u{2}",
    "(?<\\u{61}>x)\\k<\\u{61}>",
    "(?<\\u{1D49C}>x)",
    // Group names by code point.
    "(?<𝒜>x)",
    "(?<𝒜>x)\\k<𝒜>",
    "(?<😀>x)",
    "\\k<𝒜>",
    // Surrogate escapes stay two units.
    "\\uD83D\\uDE00",
    "\\uD83D\\uDE00+",
    "^\\uD83D\\uDE00{2}$",
    "[\\uD83D\\uDE00]",
    "\\uD83D\\u0041",
    "[\\uDBFF\\u0132]",
    "[\\uD800-\\uDB7F]x",
    "😀",
    "[😀]",
    // Case folding of the units V8 keeps apart.
    "s",
    "\\u017F",
    "[\\u017F]",
    "[^\\u017F]",
    "\\ſ",
    "[\\ſ]",
    "(?-i:ſ)",
    "[ſ-ƀ]",
    "[a-z]",
    "[A-Z]+",
    "\\w",
    "\\W",
    "[\\w]",
    "[\\W]",
    "[\\W_]",
    "[^\\W]",
    "[^\\W_]",
    "[a-\\W]",
    "[\\W-a]",
    "[\\W-]",
    "[\\D]",
    "[\\S]",
    "(?-i:[\\W])",
    "[\\u0080-\\uFFFF]",
    "[^\\x00-\\x7F]",
    "[\\u0100-\\u017F]",
    "[\\u2100-\\u214F]",
    "[\\u1E00-\\u1FFF]",
    "[\\u0000-\\uFFFF]",
    "k",
    "[k]",
    "[^k]",
    "\\u212A",
    "i",
    "\\u0131",
    "ß",
    "\\u1E9E",
    "[ß]",
    "[\\u1E9E]",
    "\\u03F4",
    "[\\u03F4]",
    "\\u2126",
    "[\\u03A9]",
    "\\u1F80",
    "\\u1F88",
    "[\\u1F88]",
    "(.)\\1",
    "(.)(?-i:\\1)",
    "^(.)\\1$",
    // Class atoms the rewrite reads.
    "[\\W\\cA]",
    "[\\W\\c1]",
    "[\\W\\c]",
    "[\\W\\c-]",
    "[\\W\\0]",
    "[\\W\\08]",
    "[\\W\\177]",
    "[\\W\\400]",
    "[\\W\\8]",
    "[\\W\\x41]",
    "[\\W\\x4]",
    "[\\W\\u0041]",
    "[\\W\\u004]",
    "[\\W\\b]",
    "[\\W\\-]",
    "[\\W\\]]",
    "[\\W\\\\]",
    "[\\W\\t\\n]",
    "[\\W\\p]",
    "[\\W]]",
    "[\\W",
    "[\\W\\",
    "[--\\W]",
    "[a--\\W]",
    "[\\W--]",
    "[z-a\\W]",
    "[\\ca-\\W]",
    "[\\W\\uD83D\\uDE00]",
    "[^ſ\\W]",
];

/// Paths for the wrapper seeds: each unit V8 keeps apart from its regress
/// partners, the partners, a few plain letters and digits, and astral
/// characters, including one whose lead surrogate is outside the stand-ins.
const WRAPPER_PROBES: &[&str] = &[
    "s",
    "S",
    "\u{017F}",
    "k",
    "K",
    "\u{212A}",
    "i",
    "I",
    "\u{0131}",
    "\u{0130}",
    "\u{00E5}",
    "\u{00C5}",
    "\u{212B}",
    "\u{03C9}",
    "\u{03A9}",
    "\u{2126}",
    "\u{00DF}",
    "\u{1E9E}",
    "\u{03B8}",
    "\u{0398}",
    "\u{03D1}",
    "\u{03F4}",
    "\u{1F80}",
    "\u{1F88}",
    "\u{0390}",
    "\u{1FD3}",
    "\u{FB05}",
    "\u{FB06}",
    "a",
    "z",
    "0",
    "_",
    "-",
    "]",
    "\\",
    "\u{0001}",
    "\u{0008}",
    "\u{0011}",
    "\u{007F}",
    "u",
    "uu",
    "u{61}",
    "{",
    "1",
    "6",
    "xx",
    "aa",
    "aA",
    "s\u{017F}",
    "\u{017F}\u{017F}",
    "\u{0132}",
    "😀",
    "😀😀",
    "A😀",
    "\u{10400}",
];

fn with_probes(patterns: &[&str], probes: &[&str]) -> Vec<(String, Vec<String>)> {
    patterns
        .iter()
        .map(|p| {
            (
                p.to_string(),
                probes.iter().map(|s| s.to_string()).collect(),
            )
        })
        .collect()
}

/// Tokens that put regex syntax next to itself: assertions under
/// quantifiers, escapes inside and outside classes, named groups and their
/// references, and the case-folding units.
const FUZZ_TOKENS: &[&str] = &[
    "(",
    ")",
    "[",
    "]",
    "{",
    "}",
    "?",
    "*",
    "+",
    "|",
    "^",
    "$",
    ".",
    "\\",
    "-",
    ",",
    "<",
    ">",
    "=",
    "!",
    ":",
    "/",
    "a",
    "b",
    "A",
    "0",
    "1",
    "2",
    "3",
    "8",
    "9",
    "c",
    "k",
    "u",
    "x",
    "d",
    "D",
    "w",
    "W",
    "s",
    "S",
    "B",
    "n",
    "i",
    "m",
    "_",
    "é",
    "ſ",
    "K",
    "\u{212A}",
    "😀",
    "\n",
    "\u{D7}",
    "\u{FF}",
    "\u{178}",
    "\u{1E9E}",
    "\u{2126}",
    "\u{3A9}",
    "\u{3C9}",
    "\u{1C4}",
    "\u{1C5}",
    "\u{390}",
    "\u{3B9}",
    "\u{345}",
    "\u{B5}",
    "\u{130}",
    "\u{131}",
    "\u{FB00}",
    "\u{1F88}",
    "Z",
    "z",
    "@",
    "(?<a>",
    "(?<b>",
    "\\k<a>",
    "\\k<b>",
    "(?<=",
    "(?<!",
    "(?=",
    "(?!",
    "(?:",
    "(?i:",
    "(?-i:",
    "(?m:",
    "(?s:",
    "(?ms-i:",
    "\\b",
    "\\B",
    "\\1",
    "\\2",
    "\\cA",
    "\\c",
    "\\x41",
    "\\u0041",
    "\\u{41}",
    "[^",
    "{2}",
    "{1,3}",
    "{0,}",
    "\\0",
    "\\07",
    "\\377",
    "\\400",
    "[a-z]",
    "[A-Z]",
    "[^\\W]",
    "[\\W]",
    "[^\\w]",
    "\\u017F",
    "\\u212A",
    "[\\s\\S]",
    "\\d",
    "\\s",
    "\\S",
    "\\w",
    "\\W",
    "\\D",
    "[\\b]",
    "[\\-]",
    "[a-\\u00ff]",
    "\\ud83d",
    "\\ude00",
    "\\uDBFF",
];

const FUZZ_PROBE_CHARS: &[char] = &[
    'a', 'A', 'b', 'B', 'é', 'É', 'ſ', 's', 'S', 'k', 'K', '\u{212A}', '\n', '\r', ' ', '0', '1',
    '_', 'x', '😀', '\u{FF}', '\u{178}', '\u{1E9E}', '\u{DF}', '\u{2126}', '\u{3A9}', '\u{3C9}',
    '\u{1C4}', '\u{1C5}', '\u{1C6}', '\u{390}', '\u{1FD3}', '\u{3B9}', '\u{345}', '\u{B5}',
    '\u{39C}', '\u{130}', '\u{131}', 'i', 'I', '\u{1F80}', '\u{1F88}', '[', ']', '\\', '^', '@',
    'c', 'u', 'n', 'd', 'w', '{', '}', '-', '\u{3}', '\u{1C}', '\u{132}',
];

fn fuzz_case() -> impl Strategy<Value = (String, Vec<String>)> {
    let pattern =
        prop::collection::vec(prop::sample::select(FUZZ_TOKENS), 1..12).prop_map(|v| v.concat());
    let probe = prop::collection::vec(prop::sample::select(FUZZ_PROBE_CHARS), 0..6)
        .prop_map(|v| v.into_iter().collect::<String>());
    (pattern, prop::collection::vec(probe, 8))
}

#[test]
fn profile_lookup_regex_matches_js() {
    check_patterns(&with_probes(SEED_PATTERNS, SEED_PROBES));
    check_patterns(&with_probes(WRAPPER_SEEDS, WRAPPER_PROBES));
    let generated = sample((pattern(), prop::collection::vec(probe(), 6)));
    check_patterns(&generated);
    check_patterns(&sample(fuzz_case()));
}

/// Every unit whose case folding regress and V8 disagree on, and every
/// partner regress folds it to, as a pattern (`\uXXXX`, `[\uXXXX]`,
/// `[^\uXXXX]`) and as a path, against each other.
#[test]
fn case_folding_matches_js() {
    let units: &[u32] = &[
        0x49, 0x4B, 0x53, 0x69, 0x6B, 0x73, 0xC5, 0xDF, 0xE5, 0x130, 0x131, 0x17F, 0x390, 0x398,
        0x3A9, 0x3B0, 0x3B8, 0x3C9, 0x3D1, 0x3F4, 0x1E9E, 0x1F80, 0x1F87, 0x1F88, 0x1F8F, 0x1F90,
        0x1F98, 0x1F9F, 0x1FA0, 0x1FA8, 0x1FAF, 0x1FB3, 0x1FBC, 0x1FC3, 0x1FCC, 0x1FD3, 0x1FE3,
        0x1FF3, 0x1FFC, 0x2126, 0x212A, 0x212B, 0xFB05, 0xFB06,
    ];
    let probes: Vec<String> = units
        .iter()
        .map(|&u| char::from_u32(u).unwrap().to_string())
        .collect();
    let mut cases = Vec::new();
    for &u in units {
        for p in [
            format!("\\u{u:04X}"),
            format!("[\\u{u:04X}]"),
            format!("[^\\u{u:04X}]"),
            format!("(?-i:\\u{u:04X})"),
            format!("(.)\\1\\u{u:04X}"),
        ] {
            cases.push((p, probes.clone()));
        }
    }
    let pairs: Vec<String> = probes
        .iter()
        .flat_map(|a| probes.iter().map(move |b| format!("{a}{b}")))
        .collect();
    cases.push(("^(.)\\1$".into(), pairs));
    check_patterns(&cases);
}

/// The differences this port declares, each shown on its witness: the
/// JavaScript side gives one answer and the Rust side another.
///
/// - The reason in a `SyntaxError` is regress's wording (see
///   `normalize_message`).
/// - A `FOLD_SINGLETONS` unit is matched as a stand-in, a lead surrogate in
///   U+DB80..U+DBA4. A pattern that names such a surrogate (here through a
///   range of lead surrogates) also matches the path's `ſ`, and a pattern
///   that names `ı` also matches the first half of U+F0000, whose lead
///   surrogate is U+DB80. Both need a risk path written for surrogate code
///   units, which a path regex has no use for.
/// - regress refuses a pattern nested 256 groups deep, which V8 accepts.
#[test]
fn declared_regex_differences() {
    let cases: Vec<(String, Vec<String>)> = vec![
        ("(".into(), vec![]),
        ("[\\uD800-\\uDBFF]".into(), vec!["\u{017F}".into()]),
        ("\u{0131}".into(), vec!["\u{F0000}".into()]),
        (
            format!("{}a{}", "(".repeat(256), ")".repeat(256)),
            vec!["a".into()],
        ),
    ];
    let exports = ["riskPathRules"];
    let calls: Vec<Value> = cases
        .iter()
        .map(|(p, probes)| json!({"fn": "rulesView", "args": ["riskPathRules", risk_profile(p), probes]}))
        .collect();
    let js = composite_raw("PROFILE-LOOKUP", &exports, &[], &calls);
    let rust: Vec<Result<Value, String>> = cases
        .iter()
        .map(|(p, probes)| risk_path_rules(&risk_profile(p)).map(|r| rules_view(&r, probes)))
        .collect();
    let hits = |r: &Result<Value, String>| r.as_ref().ok().map(|v| v[0]["hits"][0].clone());

    let js_reason = js[0].as_ref().unwrap_err();
    let rust_reason = rust[0].as_ref().unwrap_err();
    assert!(
        js_reason.ends_with("/(/i: Unterminated group"),
        "{js_reason}"
    );
    assert_ne!(js_reason, rust_reason);
    assert_eq!(normalize_message(js_reason), normalize_message(rust_reason));

    assert_eq!(hits(&js[1]), Some(json!(false)));
    assert_eq!(hits(&rust[1]), Some(json!(true)));
    assert_eq!(hits(&js[2]), Some(json!(false)));
    assert_eq!(hits(&rust[2]), Some(json!(true)));

    assert_eq!(hits(&js[3]), Some(json!(true)));
    assert!(rust[3].is_err());
}

// ---------------------------------------------------------------------------
// Profiles
// ---------------------------------------------------------------------------

const RULE_PATTERNS: &[&str] = &[
    "auth|role",
    "invoice|payment",
    "^prisma/migrations/",
    "migration",
    "\\.sql$",
    "(?<!x)auth",
    "\u{017F}",
    "K",
    "é",
    "(",
    "",
    " ",
    "a{2,1}",
    "\\bsrc\\b",
    "(?<n>v2)/\\k<n>",
];

const PROBE_FILES: &[&str] = &[
    "prisma/migrations/2026_x.sql",
    "src/auth/login.ts",
    "src/AUTH.ts",
    "src/services/v2/payment.service.ts",
    "src/\u{017F}ervices.ts",
    "src/\u{212A}ey.ts",
    "src/key.ts",
    "README.md",
    "",
    "a\nmigration",
    "src/Role.ts",
    "src/v2/v2/x.ts",
    "src/é.ts",
];

fn rule() -> impl Strategy<Value = Value> {
    let focus = pick(vec![
        s("migration"),
        s("security"),
        s("security"),
        s("dataContract"),
        s("performance"),
        s(" "),
        s(""),
        Some(json!(5)),
        None,
    ]);
    let pattern = prop_oneof![
        8 => prop::sample::select(RULE_PATTERNS).prop_map(|p| Some(json!(p))),
        1 => pick(vec![None, Some(json!(5)), Some(Value::Null), Some(json!(["auth"]))]),
    ];
    let object = (focus, pattern).prop_map(|(f, p)| obj(vec![("focus", f), ("pattern", p)]));
    prop_oneof![
        12 => object,
        1 => pick(vec![Some(Value::Null), Some(json!(0)), Some(json!("x")), Some(json!([]))]).prop_map(Option::unwrap),
    ]
}

fn rules_field() -> impl Strategy<Value = Option<Value>> {
    prop_oneof![
        2 => Just(None),
        1 => pick(vec![s("x"), Some(json!({})), Some(Value::Null)]),
        6 => prop::collection::vec(rule(), 0..4).prop_map(|v| Some(Value::Array(v))),
    ]
}

const FOCUS_KEYS: &[&str] = &[
    "correctness",
    "dataContract",
    "security",
    "performance",
    "errorHandling",
    "migration",
    "style",
    "0",
    "12",
    "__proto__",
];

fn sentence() -> impl Strategy<Value = Value> {
    prop_oneof![
        8 => prop::sample::select(vec!["c sentence", "d sentence", "s sentence", "p sentence", "e sentence", "m sentence", "12"]).prop_map(|x| json!(x)),
        2 => pick(vec![s(""), s(" "), s("\u{FEFF}"), s("\u{0085}"), Some(json!(5)), Some(Value::Null), Some(json!(["x"])), Some(json!({"a": 1}))]).prop_map(Option::unwrap),
    ]
}

fn valid_foci() -> impl Strategy<Value = Value> {
    (
        any::<bool>(),
        any::<bool>(),
        proptest::option::of(prop::sample::select(FOCUS_KEYS)),
        sentence(),
    )
        .prop_map(|(migration, reversed, extra, extra_val)| {
            let mut keys = vec![
                "correctness",
                "dataContract",
                "security",
                "performance",
                "errorHandling",
            ];
            if migration {
                keys.push("migration");
            }
            if reversed {
                keys.reverse();
            }
            let mut m = Map::new();
            for k in keys {
                m.insert(k.into(), json!(format!("{} sentence", &k[..1])));
            }
            if let Some(k) = extra {
                m.insert(k.into(), extra_val);
            }
            Value::Object(m)
        })
}

fn foci_object() -> impl Strategy<Value = Value> {
    prop_oneof![
        6 => valid_foci(),
        3 => prop::collection::vec((prop::sample::select(FOCUS_KEYS), sentence()), 0..8).prop_map(|kv| {
            Value::Object(kv.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
        }),
    ]
}

fn anchor() -> impl Strategy<Value = Value> {
    let repo = pick(vec![
        s("acme/web"),
        s("acme/web"),
        s("acme/api"),
        Some(json!(["acme/web"])),
        Some(json!(5)),
        s(""),
        s("acme"),
        None,
    ]);
    let r = pick(vec![
        s("development"),
        s("development"),
        s("-x"),
        s(""),
        Some(json!(7)),
        None,
    ]);
    let object = (repo, r).prop_map(|(a, b)| obj(vec![("repo", a), ("ref", b)]));
    prop_oneof![9 => object, 1 => pick(vec![Some(Value::Null), Some(json!("x")), Some(json!(0))]).prop_map(Option::unwrap)]
}

fn profile() -> impl Strategy<Value = Value> {
    let head = (
        pick(vec![
            Some(json!(1)),
            Some(json!(1)),
            Some(json!(1)),
            Some(json!(1.0)),
            Some(json!(2)),
            s("1"),
            Some(Value::Null),
            None,
            Some(json!(0.5)),
            Some(json!(0)),
            Some(json!(1.5)),
            Some(json!(1e21)),
        ]),
        pick(vec![
            s("Acme"),
            s("Acme"),
            s("Acme"),
            s(""),
            s(" "),
            s("\u{FEFF}"),
            Some(json!(5)),
            None,
        ]),
        pick(vec![
            s("acme/api"),
            s("acme/api"),
            s("acme/api"),
            s("acme/api"),
            s("acme/web"),
            s("acme"),
            s("acme/api\n"),
            Some(json!(["acme/api"])),
            Some(json!(7)),
            None,
            s("é/x"),
            s("a.b/c_d-e"),
        ]),
        pick(vec![
            s("backend"),
            s("backend"),
            s("frontend"),
            s("e2e"),
            s("Backend"),
            s(""),
            Some(Value::Null),
            Some(json!(["backend"])),
            None,
        ]),
        pick(vec![
            s("development"),
            s("development"),
            s("development"),
            s("-x"),
            s("release/1.0"),
            s("a b"),
            s(""),
            None,
            s("main\n"),
        ]),
        pick(vec![
            Some(json!(["staging", "uat", "production"])),
            Some(json!(["staging", "uat", "production"])),
            Some(json!([])),
            Some(json!(["staging; rm -rf ~"])),
            s("staging"),
            Some(json!([" "])),
            Some(json!(["ok", 5])),
            None,
        ]),
    );
    let foci = prop_oneof![
        8 => foci_object().prop_map(Some),
        1 => pick(vec![None, Some(Value::Null), s("x"), Some(json!([]))]),
    ];
    let tail = (
        foci,
        pick(vec![
            s("auth, payments"),
            s("auth, payments"),
            s(""),
            s(" "),
            Some(json!(5)),
            None,
        ]),
        prop_oneof![
            6 => prop::collection::vec(anchor(), 0..3).prop_map(|v| Some(Value::Array(v))),
            1 => pick(vec![None, s("x")]),
        ],
        rules_field(),
        pick(vec![
            None,
            None,
            None,
            s(".claude/review-rules.md"),
            s("../x"),
            s("a/../b"),
            s("/abs"),
            s("-x.md"),
            s(""),
            Some(Value::Null),
            Some(json!(5)),
            s("a b"),
        ]),
        pick(vec![
            None,
            None,
            None,
            Some(json!(["CLAUDE.md", ".claude/rules/*.md"])),
            Some(json!(["*.md"])),
            Some(json!(["a*/b.md"])),
            Some(json!(["dir/"])),
            Some(json!(["../x"])),
            Some(json!([5])),
            s("CLAUDE.md"),
            Some(json!([])),
            Some(json!([".claude/rules/*"])),
        ]),
    );
    let extra = (
        prop_oneof![
            3 => Just(None),
            1 => pick(vec![Some(json!({})), Some(Value::Null), Some(json!([])), s("x"), Some(json!(false)), Some(json!({"inContextRiskPaths": null}))]),
            2 => rules_field().prop_map(|r| Some(obj(vec![("inContextRiskPaths", r)]))),
        ],
        any::<bool>(),
    );
    (head, tail, extra).prop_map(|(h, t, (tribunal, index_key))| {
        let mut fields: Vec<(&str, Option<Value>)> = vec![
            ("schemaVersion", h.0),
            ("project", h.1),
            ("repo", h.2),
            ("reviewer", h.3),
            ("baseBranch", h.4),
            ("deployBranches", h.5),
            ("foci", t.0),
            ("highRiskSurface", t.1),
            ("crossRepoAnchors", t.2),
            ("riskPaths", t.3),
            ("projectRules", t.4),
            ("conventionFiles", t.5),
            ("tribunal", tribunal),
        ];
        if index_key {
            fields.push(("7", s("index keys sort first")));
        }
        obj(fields)
    })
}

const VALID_PATTERNS: &[&str] = &[
    "auth|role",
    "invoice|payment",
    "^prisma/migrations/",
    "migration",
    "\\.sql$",
    "(?<!x)auth",
    "\u{017F}",
    "K",
    "(?<n>v2)/\\k<n>",
];

fn valid_rules() -> impl Strategy<Value = Value> {
    let focus = prop::sample::select(vec![
        "migration",
        "security",
        "dataContract",
        "performance",
        "money",
    ]);
    prop::collection::vec((focus, prop::sample::select(VALID_PATTERNS)), 0..6).prop_map(|v| {
        Value::Array(
            v.into_iter()
                .map(|(f, p)| json!({"focus": f, "pattern": p}))
                .collect(),
        )
    })
}

/// A profile that validates, with at most one field taken from `profile()`.
/// Without it nearly every generated profile is refused, and the profile
/// `validateProfile` returns (key order included) is never compared.
fn mostly_valid_profile() -> impl Strategy<Value = Value> {
    let base = (
        prop::sample::select(vec!["backend", "frontend", "e2e"]),
        valid_foci(),
        valid_rules(),
        proptest::option::of(valid_rules()),
        prop::sample::select(vec![0usize, 1, 2]),
        any::<bool>(),
        any::<bool>(),
        prop::sample::select(vec![
            json!([]),
            json!(["staging", "uat", "production"]),
            json!(["release/x"]),
        ]),
    );
    (base, profile(), 0usize..24).prop_map(|(b, chaos, mutate)| {
        let (reviewer, foci, risk, in_context, extras, index_key, anchors) =
            (b.0, b.1, b.2, b.3, b.4, b.5, b.6);
        let foci = match foci {
            Value::Object(m) => Value::Object(
                m.into_iter()
                    .filter(|(k, v)| {
                        [
                            "correctness",
                            "dataContract",
                            "security",
                            "performance",
                            "errorHandling",
                            "migration",
                        ]
                        .contains(&k.as_str())
                            && v.as_str().is_some_and(|s| !s.trim().is_empty())
                    })
                    .collect(),
            ),
            other => other,
        };
        let mut m = Map::new();
        if index_key {
            m.insert("12".into(), json!({"b": 1, "3": 2}));
        }
        m.insert("schemaVersion".into(), json!(1));
        m.insert("project".into(), json!("Acme"));
        m.insert("repo".into(), json!("acme/api"));
        m.insert("reviewer".into(), json!(reviewer));
        m.insert("baseBranch".into(), json!("development"));
        m.insert("deployBranches".into(), b.7);
        m.insert("foci".into(), foci);
        m.insert("highRiskSurface".into(), json!("auth, payments"));
        let anchor_list = if anchors {
            json!([{"repo": "acme/web", "ref": "development"}])
        } else {
            json!([])
        };
        m.insert("crossRepoAnchors".into(), anchor_list);
        m.insert("riskPaths".into(), risk);
        if extras >= 1 {
            m.insert("projectRules".into(), json!(".claude/review-rules.md"));
            m.insert(
                "conventionFiles".into(),
                json!(["CLAUDE.md", ".claude/rules/*.md"]),
            );
        }
        if let Some(r) = in_context {
            m.insert("tribunal".into(), json!({"inContextRiskPaths": r}));
        }
        let keys: Vec<String> = chaos.as_object().unwrap().keys().cloned().collect();
        if let Some(k) = keys.get(mutate) {
            m.insert(k.clone(), chaos[k.as_str()].clone());
        }
        Value::Object(m)
    })
}

fn expected_repo() -> impl Strategy<Value = Value> {
    pick(vec![
        Some(Value::Null),
        Some(Value::Null),
        s("acme/api"),
        s("acme/api"),
        s("acme/api"),
        s("acme/web"),
        s(""),
        Some(json!(0)),
        Some(json!(["acme/api"])),
        s("acme/api "),
    ])
    .prop_map(Option::unwrap)
}

#[test]
fn profile_lookup_matches_js() {
    let exports = [
        "validateProfile",
        "riskPathProblems",
        "profileWarnings",
        "riskPathRules",
        "inContextRiskPathRules",
    ];
    let cases = sample((
        prop_oneof![1 => profile(), 2 => mostly_valid_profile()],
        expected_repo(),
        proptest::bool::weighted(0.1),
    ));

    // validateProfile, sometimes handed the profile as a JSON string. Only
    // valid JSON is sent: the wording of a JSON.parse failure is V8's own.
    let mut calls = vec![
        json!({"fn": "validateProfile", "args": [null, "acme/api"]}),
        json!({"fn": "validateProfile", "args": [[], null]}),
        json!({"fn": "validateProfile", "args": ["\"just a string\"", null]}),
        json!({"fn": "validateProfile", "args": ["[1]", null]}),
    ];
    for (p, e, as_text) in &cases {
        let raw = if *as_text {
            json!(p.to_string())
        } else {
            p.clone()
        };
        calls.push(json!({"fn": "validateProfile", "args": [raw, e]}));
    }
    let rust = calls
        .iter()
        .map(|c| validate_profile(&c["args"][0], &c["args"][1]))
        .collect();
    let js = oracle_raw("PROFILE-LOOKUP", &exports, &[], &calls);
    compare("validateProfile", &calls, rust, js);

    let calls: Vec<Value> = cases
        .iter()
        .map(|(p, _, _)| json!({"fn": "profileWarnings", "args": [p]}))
        .collect();
    let rust = cases
        .iter()
        .map(|(p, _, _)| profile_warnings(p).map(|w| json!(w)))
        .collect();
    let js = oracle_raw("PROFILE-LOOKUP", &exports, &[], &calls);
    compare("profileWarnings", &calls, rust, js);

    // An absent list is sent as no argument at all, which JavaScript reads as
    // `undefined`; a present `null` is sent as `null`.
    let lists: Vec<(Option<&Value>, &str)> = cases
        .iter()
        .flat_map(|(p, _, _)| {
            [
                (p.get("riskPaths"), "riskPaths"),
                (
                    p.get("tribunal").and_then(|t| t.get("inContextRiskPaths")),
                    "tribunal.inContextRiskPaths",
                ),
            ]
        })
        .collect();
    let calls: Vec<Value> = lists
        .iter()
        .map(|(l, at)| match l {
            Some(l) => json!({"fn": "riskPathProblems", "args": [l, at]}),
            None => json!({"fn": "riskPathProblems", "args": []}),
        })
        .collect();
    let rust: Vec<Value> = lists
        .iter()
        .map(|(l, at)| json!(risk_path_problems(*l, at)))
        .collect();
    let js = oracle("PROFILE-LOOKUP", &exports, &[], &calls);
    same("riskPathProblems", &calls, &rust, &js);

    let probes: Vec<String> = PROBE_FILES.iter().map(|s| s.to_string()).collect();
    for which in ["riskPathRules", "inContextRiskPathRules"] {
        let calls: Vec<Value> = cases
            .iter()
            .map(|(p, _, _)| json!({"fn": "rulesView", "args": [which, p, probes]}))
            .collect();
        let rust = cases
            .iter()
            .map(|(p, _, _)| {
                let rules = if which == "riskPathRules" {
                    risk_path_rules(p)
                } else {
                    in_context_risk_path_rules(p)
                };
                rules.map(|r| rules_view(&r, &probes))
            })
            .collect();
        let js = composite_raw("PROFILE-LOOKUP", &exports, &[], &calls);
        compare(which, &calls, rust, js);
    }
}

// ---------------------------------------------------------------------------
// LENS-ROUTING
// ---------------------------------------------------------------------------

#[test]
fn lens_routing_matches_js() {
    // Kinds that name an Object.prototype member ("constructor", "toString")
    // are left out: JavaScript returns an inherited function for them, and
    // validateProfile admits only "backend", "frontend" and "e2e" before this runs.
    let reviewer = pick(vec![
        s("backend"),
        s("frontend"),
        s("e2e"),
        s("E2E"),
        s("Backend"),
        s("backend "),
        s(""),
        Some(Value::Null),
        Some(json!(5)),
        Some(json!(1.5)),
        Some(json!(true)),
        Some(json!(["backend"])),
        Some(json!(["frontend"])),
        Some(json!(["e2e"])),
        Some(json!(["back", "end"])),
        Some(json!([])),
        Some(json!({})),
        Some(json!({"1": "x", "a": "y", "0": "z"})),
        None,
    ]);
    let profile = prop_oneof![
        8 => reviewer.prop_map(|r| obj(vec![("repo", s("acme/api")), ("reviewer", r)])),
        1 => pick(vec![Some(Value::Null), Some(json!(0)), Some(json!("")), Some(json!(false)), Some(json!("backend")), Some(json!([])), Some(json!(7))]).prop_map(Option::unwrap),
    ];
    let profiles = sample(profile);
    let exports = ["lensAgentFor"];
    let calls: Vec<Value> = profiles
        .iter()
        .map(|p| json!({"fn": "lensAgentFor", "args": [p]}))
        .collect();
    let rust = profiles
        .iter()
        .map(|p| lens_agent_for(p).map(|a| json!(a)))
        .collect();
    let js = oracle_raw("LENS-ROUTING", &exports, &["PROFILE-LOOKUP"], &calls);
    compare("lensAgentFor", &calls, rust, js);

    let calls = vec![json!({"fn": "constants", "args": []})];
    let agents: Map<String, Value> = REVIEWER_AGENTS
        .iter()
        .map(|(k, a)| (k.to_string(), json!(a)))
        .collect();
    let rust = vec![Ok(json!({"AGENT_NS": AGENT_NS, "REVIEWER_AGENTS": agents}))];
    let js = composite_raw(
        "LENS-ROUTING",
        &["AGENT_NS", "REVIEWER_AGENTS"],
        &["PROFILE-LOOKUP"],
        &calls,
    );
    compare("REVIEWER_AGENTS", &calls, rust, js);
}

// ---------------------------------------------------------------------------
// LENS-FOCI and HIGH-RISK-SURFACE
// ---------------------------------------------------------------------------

fn files() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(
        prop::sample::select(PROBE_FILES).prop_map(str::to_owned),
        0..4,
    )
}

/// A profile as selectFoci sees one: its foci are an object (validateProfile
/// refuses anything else), its rules anything the generator makes.
fn foci_profile() -> impl Strategy<Value = Value> {
    // Valid rules are weighted up so that two rules forcing one focus, the
    // case the Set in selectFoci exists for, come up often.
    let rules = prop_oneof![1 => rules_field(), 2 => valid_rules().prop_map(Some)];
    (foci_object(), rules, prop_oneof![3 => Just(None), 2 => rules_field().prop_map(|r| Some(obj(vec![("inContextRiskPaths", r)])))])
        .prop_map(|(f, r, t)| obj(vec![("foci", Some(f)), ("riskPaths", r), ("tribunal", t)]))
}

#[test]
fn lens_foci_matches_js() {
    let exports = [
        "fociFor",
        "fociLabelFor",
        "selectFoci",
        "riskPathRules",
        "inContextRiskPathRules",
    ];
    let deps = ["PROFILE-LOOKUP"];
    let cases = sample((
        foci_profile(),
        files(),
        // Around every forced count (0 to 3) and every list length (5 to 8).
        prop::sample::select(vec![
            -2.0, -0.5, 0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 100.0,
        ]),
        prop::sample::select(vec!["riskPathRules", "inContextRiskPathRules"]),
    ));

    let calls: Vec<Value> = cases
        .iter()
        .flat_map(|(p, _, _, _)| {
            [
                json!({"fn": "fociFor", "args": [p]}),
                json!({"fn": "fociLabelFor", "args": [p]}),
            ]
        })
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .flat_map(|(p, _, _, _)| {
            [
                foci_for(p).unwrap().unwrap_or(Value::Null),
                Value::Object(foci_label_for(p).unwrap()),
            ]
        })
        .collect();
    let js = oracle("LENS-FOCI", &exports, &deps, &calls);
    same("fociFor/fociLabelFor", &calls, &rust, &js);

    let calls: Vec<Value> = cases
        .iter()
        .map(|(p, f, b, w)| json!({"fn": "selectFoci", "args": [f, b, p, w]}))
        .collect();
    let rust = cases
        .iter()
        .map(|(p, f, b, w)| {
            let rules = if *w == "riskPathRules" {
                risk_path_rules(p)
            } else {
                in_context_risk_path_rules(p)
            }?;
            let foci = p["foci"].as_object().unwrap();
            Ok(json!(select_foci(f, *b, foci, &rules)))
        })
        .collect();
    let js = composite_raw("LENS-FOCI", &exports, &deps, &calls);
    compare("selectFoci", &calls, rust, js);
}

fn finding() -> impl Strategy<Value = Value> {
    let file = pick(vec![
        s("src/auth/login.ts"),
        s("src/services/v2/payment.service.ts"),
        s("README.md"),
        s("prisma/migrations/x.sql"),
        s(""),
        Some(Value::Null),
        Some(json!(5)),
        Some(json!(["src/auth.ts"])),
        Some(json!({"path": "auth"})),
        None,
    ]);
    let flag = pick(vec![
        None,
        None,
        Some(json!(false)),
        Some(json!(true)),
        Some(json!(0)),
        Some(json!(1)),
        Some(json!("")),
        Some(json!("yes")),
        Some(Value::Null),
    ]);
    let object = (any::<bool>(), file, flag, any::<bool>(), any::<bool>()).prop_map(
        |(flag_first, file, flag, prior, index_key)| {
            let mut m = Map::new();
            if flag_first {
                if let Some(v) = &flag {
                    m.insert("touchesHighRiskSurface".into(), v.clone());
                }
            }
            m.insert("title".into(), json!("t"));
            if let Some(f) = file {
                m.insert("file".into(), f);
            }
            if prior {
                m.insert("riskPathSurface".into(), json!(false));
            }
            if !flag_first {
                if let Some(v) = flag {
                    m.insert("touchesHighRiskSurface".into(), v);
                }
            }
            if index_key {
                m.insert("3".into(), json!({"b": 1, "0": 2}));
            }
            Value::Object(m)
        },
    );
    prop_oneof![
        9 => object,
        1 => pick(vec![Some(Value::Null), Some(json!(0)), Some(json!("src/auth.ts")), Some(json!([])), Some(json!(true))]).prop_map(Option::unwrap),
    ]
}

#[test]
fn high_risk_surface_matches_js() {
    let exports = [
        "highRiskSurfaceFor",
        "riskPathTouched",
        "withRiskPathSurface",
        "riskPathRules",
    ];
    let deps = ["PROFILE-LOOKUP"];
    let rules_profile = prop_oneof![
        1 => Just(Value::Null),
        6 => rules_field().prop_map(|r| obj(vec![("riskPaths", r)])),
    ];
    let surface = pick(vec![
        s("prisma/schema, migrations, auth, payments, mass delete/update"),
        s(""),
        Some(json!(5)),
        Some(json!({"2": "b", "a": 1, "1": "c"})),
        Some(Value::Null),
        None,
    ]);
    let cases = sample((
        surface,
        rules_profile,
        proptest::option::weighted(0.9, prop::collection::vec(finding(), 0..4)),
    ));

    let calls: Vec<Value> = cases
        .iter()
        .map(|(sf, _, _)| json!({"fn": "highRiskSurfaceFor", "args": [obj(vec![("highRiskSurface", sf.clone())])]}))
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|(sf, _, _)| {
            high_risk_surface_for(&obj(vec![("highRiskSurface", sf.clone())]))
                .unwrap()
                .unwrap_or(Value::Null)
        })
        .collect();
    let js = oracle("HIGH-RISK-SURFACE", &exports, &deps, &calls);
    same("highRiskSurfaceFor", &calls, &rust, &js);

    let rules_of = |p: &Value| -> Result<Option<Vec<RiskRule>>, String> {
        if p.is_null() {
            Ok(None)
        } else {
            risk_path_rules(p).map(Some)
        }
    };

    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for (_, p, findings) in &cases {
        for f in findings.iter().flatten() {
            let file = f.get("file");
            calls.push(json!({"fn": "riskPathTouched", "args": [file, p]}));
            rust.push(rules_of(p).map(|r| json!(risk_path_touched(file, r.as_deref()))));
        }
    }
    let js = composite_raw("HIGH-RISK-SURFACE", &exports, &deps, &calls);
    compare("riskPathTouched", &calls, rust, js);

    let calls: Vec<Value> = cases
        .iter()
        .map(|(_, p, f)| json!({"fn": "withRiskPathSurface", "args": [f, p]}))
        .collect();
    let rust = cases
        .iter()
        .map(|(_, p, f)| {
            let r = rules_of(p)?;
            Ok(json!(with_risk_path_surface(f.as_deref(), r.as_deref())))
        })
        .collect();
    let js = composite_raw("HIGH-RISK-SURFACE", &exports, &deps, &calls);
    compare("withRiskPathSurface", &calls, rust, js);
}

// ---------------------------------------------------------------------------
// FILE-PARTITION
// ---------------------------------------------------------------------------

const PARTITION_FILES: &[&str] = &[
    "a.ts",
    "b.ts",
    "c.ts",
    "d.ts",
    "e.ts",
    "f.ts",
    "0",
    "12",
    "constructor",
    "__proto__",
    "src/big.ts",
];

#[test]
fn file_partition_matches_js() {
    let churn_value = prop_oneof![
        6 => prop::sample::select(vec![0.0, 1.0, 3.0, 10.0, 10.0, 25.0, 5000.0, -4.0, 2.5, 0.1, 1e-3, 7.0]).prop_map(|x| json!(x)),
        1 => pick(vec![s("7"), Some(Value::Null), Some(json!(true)), Some(json!([3])), Some(json!({}))]).prop_map(Option::unwrap),
    ];
    let churn_map = prop_oneof![
        1 => Just(None),
        1 => Just(Some(Map::new())),
        6 => prop::collection::vec((prop::sample::select(PARTITION_FILES), churn_value), 0..9)
            .prop_map(|kv| Some(kv.into_iter().map(|(k, v)| (k.to_owned(), v)).collect::<Map<_, _>>())),
    ];
    let file_list = proptest::option::weighted(
        0.95,
        prop::collection::vec(
            prop::sample::select(PARTITION_FILES).prop_map(str::to_owned),
            0..12,
        ),
    );
    let cases = sample((file_list, churn_map, 0usize..7));
    let exports = ["partitionFiles", "partitionFilesByChurn"];

    // partitionFiles with zero groups and a file to place throws in
    // JavaScript; the workflow's group count is at least 1.
    let calls: Vec<Value> = cases
        .iter()
        .map(|(f, _, g)| json!({"fn": "partitionFiles", "args": [f, (*g).max(1)]}))
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|(f, _, g)| json!(partition_files(f.as_deref(), (*g).max(1))))
        .collect();
    let js = oracle("FILE-PARTITION", &exports, &[], &calls);
    same("partitionFiles", &calls, &rust, &js);

    let calls: Vec<Value> = cases
        .iter()
        .map(|(f, c, g)| json!({"fn": "partitionFilesByChurn", "args": [f, c, g]}))
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|(f, c, g)| json!(partition_files_by_churn(f.as_deref(), c.as_ref(), *g)))
        .collect();
    let js = oracle("FILE-PARTITION", &exports, &[], &calls);
    same("partitionFilesByChurn", &calls, &rust, &js);
}

// ---------------------------------------------------------------------------
// CONVENTIONS-CHANNEL
// ---------------------------------------------------------------------------

const SHA: &str = "a3f1c2d4e5b6a7980a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f6071";

fn conventions_entry() -> impl Strategy<Value = Value> {
    let matched = pick(vec![
        None,
        None,
        s("src/controllers/x.ts"),
        s(""),
        Some(json!(0)),
        Some(json!(1)),
        Some(Value::Null),
        Some(json!(["a"])),
    ]);
    let object = (any::<bool>(), matched, any::<bool>(), any::<bool>()).prop_map(
        |(full, matched, bytes, index_key)| {
            let mut m = Map::new();
            if index_key {
                m.insert("9".into(), json!("index"));
            }
            if bytes {
                m.insert("bytes".into(), json!(10));
            }
            if full {
                m.insert("scope".into(), json!("paths"));
                m.insert("path".into(), json!(".claude/rules/x.md"));
                m.insert("sha256".into(), json!("b".repeat(64)));
            } else {
                m.insert("path".into(), json!({"1": "x", "a": "y", "0": "z"}));
            }
            if let Some(v) = matched {
                m.insert("matched".into(), v);
            }
            Value::Object(m)
        },
    );
    prop_oneof![
        12 => object,
        3 => pick(vec![Some(Value::Null), Some(json!("x")), Some(json!(5)), Some(json!([]))]).prop_map(Option::unwrap),
    ]
}

#[test]
fn conventions_channel_matches_js() {
    let path = pick(vec![
        None,
        s(""),
        s("/tmp/c.md"),
        s("/tmp/c.md"),
        s("/tmp/c.md"),
        s("/tmp/c.md"),
        s("/tmp/c.md"),
        s("/tmp/c.md"),
        s("/tmp/tribunal-conventions-acme-api-12.md"),
        s("rel.md"),
        s("/tmp/x$(id).md"),
        s("/tmp/é.md"),
        s("/"),
        s("/a"),
        s("//"),
        s("/tmp/c.md\n"),
        s("/tmp/a b.md"),
        Some(json!(5)),
        Some(Value::Null),
    ]);
    let sha = pick(vec![
        None,
        s(SHA),
        s(SHA),
        s(SHA),
        s(SHA),
        s(SHA),
        s(SHA),
        s(SHA),
        Some(json!(SHA.to_uppercase())),
        Some(json!(SHA[..63].to_owned())),
        Some(json!(format!("{SHA}0"))),
        Some(json!(format!("{SHA}\n"))),
        s(""),
        Some(Value::Null),
        Some(json!(5)),
    ]);
    let list = |inner: BoxedStrategy<Value>| {
        prop_oneof![
            1 => Just(None),
            1 => pick(vec![Some(Value::Null), s("x")]),
            8 => prop::collection::vec(inner, 0..4).prop_map(|v| Some(Value::Array(v))),
        ]
    };
    let missing_item = pick(vec![
        s("AGENTS.md"),
        Some(json!(5)),
        Some(json!({"2": "x", "b": "y"})),
    ])
    .prop_map(Option::unwrap);
    let conventions = prop_oneof![
        1 => pick(vec![None, Some(Value::Null), Some(json!([])), s("str"), Some(json!(5))]),
        8 => (
            pick(vec![s("main"), s("main"), Some(json!(5)), Some(Value::Null), None]),
            list(conventions_entry().boxed()),
            list(conventions_entry().boxed()),
            list(missing_item.boxed()),
        )
            .prop_map(|(r, f, sk, mi)| Some(obj(vec![("ref", r), ("files", f), ("skipped", sk), ("missing", mi)]))),
    ];
    let profile = pick(vec![
        None,
        Some(json!([])),
        Some(json!(["CLAUDE.md"])),
        s("x"),
        Some(Value::Null),
        Some(json!([5])),
    ])
    .prop_map(|cf| obj(vec![("conventionFiles", cf)]));
    let cases = sample((
        (path, sha, conventions).prop_map(|(p, h, c)| {
            obj(vec![
                ("conventionsPath", p),
                ("conventionsSha256", h),
                ("conventions", c),
            ])
        }),
        profile,
        prop::sample::select(vec!["Review PR #12.", "", "REFUTE \u{2014} x\n"]),
    ));
    let exports = ["conventionsChannel"];
    let methodology = "METHODOLOGY FILE: /tmp/tribunal-methodology-acme-api-12.md";

    // The region's own return value, minus checkBash, which is not ported.
    let calls: Vec<Value> = cases
        .iter()
        .map(|(a, p, _)| json!({"fn": "conventionsChannel", "args": [a, p]}))
        .collect();
    let rust = cases
        .iter()
        .map(|(a, p, _)| conventions_channel(a, p).map(|w| w.manifest.unwrap_or(Value::Null)))
        .collect();
    let js = oracle_raw("CONVENTIONS-CHANNEL", &exports, &[], &calls)
        .into_iter()
        .map(|r| r.map(|v| v["manifest"].clone()))
        .collect();
    compare("conventionsChannel", &calls, rust, js);

    let calls: Vec<Value> = cases
        .iter()
        .map(|(a, p, pr)| json!({"fn": "conventions", "args": [a, p, methodology, pr]}))
        .collect();
    let rust = cases
        .iter()
        .map(|(a, p, pr)| {
            conventions_channel(a, p).map(|w| {
                json!({
                    "manifest": w.manifest.clone().unwrap_or(Value::Null),
                    "withConventions": w.with_conventions(pr),
                    "reviewerPrompt": w.reviewer_prompt(methodology, pr),
                })
            })
        })
        .collect();
    let js = composite_raw("CONVENTIONS-CHANNEL", &exports, &[], &calls);
    compare("conventionsChannel prompts", &calls, rust, js);
}

// ---------------------------------------------------------------------------
// Receivers the generators above never send
// ---------------------------------------------------------------------------

/// Profiles whose `foci` is not an object, and receivers that are not objects
/// at all. `Object.entries` walks a string's code units and an array's
/// indices, and a property read on `null` throws, so none of these is the
/// empty object the obvious port returns.
#[test]
fn odd_receivers_match_js() {
    let foci_profiles = vec![
        json!({"foci": "ab"}),
        json!({"foci": "aé"}),
        json!({"foci": ["x", "y", "x"]}),
        json!({"foci": [null, 5, ["a", "b"], {}]}),
        json!({"foci": 5}),
        json!({"foci": true}),
        json!({"foci": ""}),
        json!({"foci": null}),
        json!({}),
        json!(null),
        json!("x"),
        json!(5),
        json!([]),
    ];
    let exports = ["fociFor", "fociLabelFor", "selectFoci", "riskPathRules"];
    let deps = ["PROFILE-LOOKUP"];
    let calls: Vec<Value> = foci_profiles
        .iter()
        .flat_map(|p| {
            [
                json!({"fn": "fociFor", "args": [p]}),
                json!({"fn": "fociLabelFor", "args": [p]}),
            ]
        })
        .collect();
    let rust: Vec<Result<Value, String>> = foci_profiles
        .iter()
        .flat_map(|p| {
            [
                foci_for(p).map(|v| v.unwrap_or(Value::Null)),
                foci_label_for(p).map(Value::Object),
            ]
        })
        .collect();
    let js = oracle_raw("LENS-FOCI", &exports, &deps, &calls);
    compare("fociFor/fociLabelFor (odd foci)", &calls, rust, js);

    let surface_profiles = [json!(null), json!("x"), json!([]), json!({})];
    let calls: Vec<Value> = surface_profiles
        .iter()
        .map(|p| json!({"fn": "highRiskSurfaceFor", "args": [p]}))
        .collect();
    let rust = surface_profiles
        .iter()
        .map(|p| high_risk_surface_for(p).map(|v| v.unwrap_or(Value::Null)))
        .collect();
    let js = oracle_raw("HIGH-RISK-SURFACE", &["highRiskSurfaceFor"], &deps, &calls);
    compare("highRiskSurfaceFor (odd receivers)", &calls, rust, js);

    let receivers = [json!(null), json!("x"), json!(0), json!([]), json!(true)];
    let exports = ["profileWarnings", "riskPathRules", "inContextRiskPathRules"];
    let calls: Vec<Value> = receivers
        .iter()
        .map(|p| json!({"fn": "profileWarnings", "args": [p]}))
        .collect();
    let rust = receivers
        .iter()
        .map(|p| profile_warnings(p).map(|w| json!(w)))
        .collect();
    let js = oracle_raw("PROFILE-LOOKUP", &exports, &[], &calls);
    compare("profileWarnings (odd receivers)", &calls, rust, js);
    for which in ["riskPathRules", "inContextRiskPathRules"] {
        let calls: Vec<Value> = receivers
            .iter()
            .map(|p| json!({"fn": "rulesView", "args": [which, p, ["auth"]]}))
            .collect();
        let rust = receivers
            .iter()
            .map(|p| {
                let rules = if which == "riskPathRules" {
                    risk_path_rules(p)
                } else {
                    in_context_risk_path_rules(p)
                };
                rules.map(|r| rules_view(&r, &["auth".to_owned()]))
            })
            .collect();
        let js = composite_raw("PROFILE-LOOKUP", &exports, &[], &calls);
        compare(which, &calls, rust, js);
    }

    let pairs = [
        (json!(null), json!({})),
        (json!({}), json!(null)),
        (
            json!({"conventionsPath": "/tmp/c.md", "conventionsSha256": SHA}),
            json!(null),
        ),
        (json!("x"), json!("y")),
        (json!([]), json!(0)),
    ];
    let calls: Vec<Value> = pairs
        .iter()
        .map(|(a, p)| json!({"fn": "conventionsChannel", "args": [a, p]}))
        .collect();
    let rust = pairs
        .iter()
        .map(|(a, p)| conventions_channel(a, p).map(|w| w.manifest.unwrap_or(Value::Null)))
        .collect();
    let js = oracle_raw("CONVENTIONS-CHANNEL", &["conventionsChannel"], &[], &calls)
        .into_iter()
        .map(|r| r.map(|v| v["manifest"].clone()))
        .collect();
    compare("conventionsChannel (odd receivers)", &calls, rust, js);
}

/// Long paths through quantifiers. The hand-written matcher this port used
/// before regress recursed once per iteration and overflowed the stack on a
/// path of a few thousand characters. The lengths stay at a few thousand:
/// several of these patterns backtrack quadratically in both engines, and an
/// unoptimized regress takes minutes at 20,000.
#[test]
fn long_paths_match_js() {
    let long = |n: usize| format!("src/{}x.ts", "a".repeat(n));
    let probes: Vec<String> = vec![
        long(2000),
        long(5000),
        format!("{}/b", "a/".repeat(2500)),
        format!("{}\nX", "a".repeat(2000)),
        "a".repeat(3000),
    ];
    let cases: Vec<(String, Vec<String>)> = [
        ".*x",
        ".*?x",
        "a+x",
        "a+?x",
        "[a-z]*x\\.ts$",
        "[^/]*$",
        "^src/a{0,100000}x",
        "^src/a{2,}?x",
        "^(?:src/)?[a-z]*x",
        "^[^x]*\\.ts$",
        "(?<=a*)x",
        "(?<=^src/[a-z]*)x",
        "a*$",
        "^a{3000}$",
        "^a{2999}$",
        "^a{3001}",
        "^(?:a/)*b$",
        "\\w*\\s",
    ]
    .iter()
    .map(|p| (p.to_string(), probes.clone()))
    .collect();
    check_patterns(&cases);
}

// ---------------------------------------------------------------------------
// JavaScript semantics the generators reach only by chance
// ---------------------------------------------------------------------------

fn forcing_profile(foci: Value) -> Value {
    json!({
        "foci": foci,
        "riskPaths": [
            {"focus": "security", "pattern": "auth"},
            {"focus": "dataContract", "pattern": "invoice"},
            {"focus": "migration", "pattern": "^prisma/"},
            {"focus": "security", "pattern": "role"}
        ]
    })
}

fn sentences() -> Value {
    json!({
        "correctness": "c", "dataContract": "d", "security": "s",
        "performance": "p", "errorHandling": "e", "migration": "m"
    })
}

/// `Math.max(budget, forced)` and `slice(0, end)` on budgets JSON cannot
/// carry (`NaN`, `Infinity`), on fractions and on each side of the forced
/// count and of the list length.
#[test]
fn select_foci_budgets_match_js() {
    let file_sets: Vec<Vec<String>> = vec![
        vec![],
        vec!["src/auth.ts".into()],
        vec!["src/auth.ts".into(), "src/invoice.ts".into()],
        vec![
            "src/role.ts".into(),
            "src/invoice.ts".into(),
            "prisma/x.sql".into(),
        ],
    ];
    let budgets = [
        "NaN",
        "Infinity",
        "-Infinity",
        "-0",
        "0",
        "0.5",
        "0.9999",
        "1",
        "1.0000001",
        "1.5",
        "2",
        "2.5",
        "3",
        "3.5",
        "4",
        "5",
        "6",
        "7",
        "8",
        "9",
        "-1",
        "1e21",
    ];
    let p = forcing_profile(sentences());
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for files in &file_sets {
        for b in budgets {
            calls.push(json!({"fn": "selectFociText", "args": [files, b, p, "riskPathRules"]}));
            let rules = risk_path_rules(&p).unwrap();
            let budget: f64 = b.parse().unwrap();
            rust.push(Ok(json!(select_foci(
                files,
                budget,
                p["foci"].as_object().unwrap(),
                &rules
            ))));
        }
    }
    let exports = ["fociFor", "selectFoci", "riskPathRules"];
    let js = composite_raw("LENS-FOCI", &exports, &["PROFILE-LOOKUP"], &calls);
    compare("selectFoci (budgets)", &calls, rust, js);
}

/// `new Set` and `includes` compare a primitive by value and an object by
/// identity; `Object.fromEntries` keys a sentence by its `ToString`.
#[test]
fn foci_identity_matches_js() {
    let foci_sets = vec![
        json!({"security": {"a": 1}, "dataContract": {"a": 1}, "correctness": "c", "performance": "p", "errorHandling": "e"}),
        json!({"security": "x", "correctness": "x", "dataContract": "d", "performance": "p", "errorHandling": "e"}),
        json!({"security": 1, "dataContract": 1.0, "correctness": 1, "performance": "p", "errorHandling": "e"}),
        json!({"security": true, "correctness": true, "dataContract": "d", "performance": "p", "errorHandling": "e"}),
        json!({"security": ["a"], "correctness": ["a"], "dataContract": "d", "performance": "p", "errorHandling": "e"}),
        json!({"migration": "m", "security": "m", "correctness": "m", "dataContract": "d", "performance": "p", "errorHandling": "e"}),
        json!({"security": "s"}),
        json!({"security": "", "dataContract": 0, "migration": null, "correctness": "c"}),
        json!({"correctness": 5, "security": "5", "performance": "10", "dataContract": "2", "errorHandling": [1, 2], "migration": null, "x": {"k": 1}, "12": "12", "01": "01", "__proto__": "p"}),
    ];
    let files: Vec<String> = vec![
        "src/auth.ts".into(),
        "src/invoice.ts".into(),
        "prisma/x.sql".into(),
    ];
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for foci in foci_sets {
        let p = forcing_profile(foci);
        for budget in [0.0, 2.0, 9.0] {
            calls.push(json!({"fn": "selectFoci", "args": [files, budget, p, "riskPathRules"]}));
            let rules = risk_path_rules(&p).unwrap();
            rust.push(Ok(json!(select_foci(
                &files,
                budget,
                p["foci"].as_object().unwrap(),
                &rules
            ))));
        }
        calls.push(json!({"fn": "fociLabelFor", "args": [p]}));
        rust.push(foci_label_for(&p).map(Value::Object));
    }
    let exports = ["fociFor", "fociLabelFor", "selectFoci", "riskPathRules"];
    let deps = ["PROFILE-LOOKUP"];
    let (select, labels): (Vec<_>, Vec<_>) = calls
        .iter()
        .cloned()
        .zip(rust)
        .partition(|(c, _)| c["fn"] == "selectFoci");
    let (calls, rust): (Vec<Value>, Vec<_>) = select.into_iter().unzip();
    let js = composite_raw("LENS-FOCI", &exports, &deps, &calls);
    compare("selectFoci (identity)", &calls, rust, js);
    let (calls, rust): (Vec<Value>, Vec<_>) = labels.into_iter().unzip();
    let js = oracle_raw("LENS-FOCI", &exports, &deps, &calls);
    compare("fociLabelFor (keys)", &calls, rust, js);
}

/// `RegExp.prototype.test` reads a non-string path through `ToString`, and
/// the spread in `withRiskPathSurface` keeps an existing key where it was.
#[test]
fn risk_path_surface_coercions_match_js() {
    let p = json!({"riskPaths": [
        {"focus": "security", "pattern": "object"},
        {"focus": "security", "pattern": "^5$"},
        {"focus": "security", "pattern": "^a,b$"},
        {"focus": "security", "pattern": "^1e\\+21$"},
        {"focus": "security", "pattern": "^true$"},
        {"focus": "security", "pattern": "^,x$"},
        {"focus": "security", "pattern": "^$"},
        {"focus": "dataContract", "pattern": "auth"}
    ]});
    let files = vec![
        json!({"path": "x"}),
        json!(5),
        json!(["a", "b"]),
        json!(1e21),
        json!(true),
        json!([null, "x"]),
        json!([]),
        json!(0),
        json!(""),
        json!(-0.0),
        json!([["a"], "b"]),
    ];
    let rules = risk_path_rules(&p).unwrap();
    let calls: Vec<Value> = files
        .iter()
        .map(|f| json!({"fn": "riskPathTouched", "args": [f, p]}))
        .collect();
    let rust = files
        .iter()
        .map(|f| Ok(json!(risk_path_touched(Some(f), Some(&rules)))))
        .collect();
    let exports = ["riskPathTouched", "withRiskPathSurface", "riskPathRules"];
    let deps = ["PROFILE-LOOKUP"];
    let js = composite_raw("HIGH-RISK-SURFACE", &exports, &deps, &calls);
    compare("riskPathTouched (ToString)", &calls, rust, js);

    let findings = [json!([
        {"riskPathSurface": "old", "file": "src/auth.ts", "2": "x", "touchesHighRiskSurface": 0},
        {"file": "src/auth.ts", "1": "a", "0": "b"},
        {"touchesHighRiskSurface": "", "z": 1, "file": "src/auth.ts"},
        "src/auth.ts",
        {"file": ["src/auth.ts"]},
        {"file": {"path": "src/auth.ts"}},
        null,
        0
    ])];
    let calls: Vec<Value> = findings
        .iter()
        .map(|f| json!({"fn": "withRiskPathSurface", "args": [f, p]}))
        .collect();
    let rust = findings
        .iter()
        .map(|f| {
            Ok(json!(with_risk_path_surface(
                f.as_array().map(Vec::as_slice),
                Some(&rules)
            )))
        })
        .collect();
    let js = composite_raw("HIGH-RISK-SURFACE", &exports, &deps, &calls);
    compare("withRiskPathSurface (spread)", &calls, rust, js);
}

/// Duplicate files, keys `Object.prototype` defines, `-0`, one known churn
/// among unknown ones, and ties between group totals.
#[test]
fn partition_edges_match_js() {
    let files: Vec<String> = [
        "a.ts",
        "a.ts",
        "toString",
        "valueOf",
        "hasOwnProperty",
        "__proto__",
        "b.ts",
        "c.ts",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let maps = vec![
        json!({"a.ts": 5}),
        json!({"b.ts": 5}),
        json!({"toString": 3, "__proto__": 7}),
        json!({"a.ts": -0.0, "b.ts": 0}),
        json!({"b.ts": 4, "c.ts": 4, "a.ts": 2}),
        json!({"valueOf": 1, "hasOwnProperty": 1, "a.ts": 1, "b.ts": 1, "c.ts": 1}),
        json!({"constructor": 9}),
    ];
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for m in &maps {
        for groups in 0..5usize {
            calls.push(json!({"fn": "partitionFilesByChurn", "args": [files, m, groups]}));
            rust.push(json!(partition_files_by_churn(
                Some(&files),
                m.as_object(),
                groups
            )));
        }
    }
    let js = oracle(
        "FILE-PARTITION",
        &["partitionFiles", "partitionFilesByChurn"],
        &[],
        &calls,
    );
    same("partitionFilesByChurn (edges)", &calls, &rust, &js);
}

/// `${r.focus}` in a warning is `ToString`, and a profile handed over as
/// JSON text keeps the first position and the last value of a repeated key.
#[test]
fn profile_coercions_match_js() {
    let focuses = vec![
        json!({"focus": null}),
        json!({"focus": [1, [2, 3]]}),
        json!({"focus": {}}),
        json!({"focus": 1.5}),
        json!({"focus": 1e21}),
        json!({"focus": true}),
        json!({"focus": -0.0}),
        json!({}),
        json!("x"),
        json!(5),
    ];
    let p = json!({"riskPaths": focuses, "tribunal": {"inContextRiskPaths": [{"focus": [null]}]}});
    let calls = vec![json!({"fn": "profileWarnings", "args": [p]})];
    let rust = vec![profile_warnings(&p).map(|w| json!(w))];
    let exports = ["validateProfile", "profileWarnings"];
    let js = oracle_raw("PROFILE-LOOKUP", &exports, &[], &calls);
    compare("profileWarnings (ToString)", &calls, rust, js);

    let valid = r#""schemaVersion":1,"project":"Acme","reviewer":"backend","baseBranch":"development","deployBranches":[],"foci":{"correctness":"c","dataContract":"d","security":"s","performance":"p","errorHandling":"e"},"highRiskSurface":"auth","crossRepoAnchors":[]"#;
    let texts = [
        format!(r#"{{"repo":"x/y",{valid},"repo":"acme/api"}}"#),
        format!(r#"{{"__proto__":{{"a":1}},"repo":"acme/api",{valid}}}"#),
        format!(r#"{{"b":1,"10":2,"repo":"acme/api",{valid},"2":3,"b":4}}"#),
        format!(
            r#"{{"repo":"acme/api",{valid},"foci":{{"security":"s","correctness":"c","dataContract":"d","performance":"p","errorHandling":"e","security":"s2"}}}}"#
        ),
    ];
    let calls: Vec<Value> = texts
        .iter()
        .map(|t| json!({"fn": "validateProfile", "args": [t, "acme/api"]}))
        .collect();
    let rust = texts
        .iter()
        .map(|t| validate_profile(&json!(t), &json!("acme/api")))
        .collect();
    let js = oracle_raw("PROFILE-LOOKUP", &exports, &[], &calls);
    compare("validateProfile (JSON text)", &calls, rust, js);
}

/// Declared: a reviewer kind that names an `Object.prototype` member finds an
/// inherited function or object in `REVIEWER_AGENTS`, which JavaScript
/// returns as the agent; the port refuses the kind. `validateProfile` admits
/// only `backend` and `frontend` before `lensAgentFor` runs, so no profile
/// the workflow accepts reaches this.
#[test]
fn declared_lens_routing_difference() {
    let kinds = [
        json!("constructor"),
        json!("toString"),
        json!("__proto__"),
        json!("hasOwnProperty"),
        json!(["valueOf"]),
    ];
    let calls: Vec<Value> = kinds
        .iter()
        .map(|k| json!({"fn": "lensAgentType", "args": [{"reviewer": k}]}))
        .collect();
    let js = composite_raw(
        "LENS-ROUTING",
        &["lensAgentFor"],
        &["PROFILE-LOOKUP"],
        &calls,
    );
    for (k, j) in kinds.iter().zip(js) {
        let t = j.unwrap();
        assert!(t == json!("function") || t == json!("object"), "{k}: {t}");
        assert!(lens_agent_for(&json!({"reviewer": k})).is_err());
    }
}
