mod common;
use common::*;

// Differential tests for the report group: CARRY-FORWARD,
// DRAFT-COMMENTS-STRIP, MERGED-LOCATIONS-CHECK and REPORT-FIDELITY-CHECKS.
//
// The generators lean on what these regions actually do with JSON they did
// not write: `String(x)` of numbers, arrays and objects inside substring
// tests, `Number(x)` of line fields, `===` between 5 and 5.0, title tokens
// after a Unicode `toLowerCase`, the JavaScript `trim` set, and the key order
// of an object spread over keys that look like array indices.
//
// Where JavaScript throws, the Rust side returns `Err`, and the two are
// compared on whether they threw, not on the message: V8's wording is not
// part of the behavior and changes between Node releases.
//
// The JavaScript's side is read from `tests/goldens/oracle/diff_report.json`
// (see `common`): the workflow is being retired, so a run starts only the
// port. The file keeps each call's results under a SHA-256 of its inputs and a
// SHA-256 of every region's text, so a region edited without regenerating
// fails here; with the workflow gone the golden is final. Regenerate:
// `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-core --test diff_report`.

use proptest::prelude::*;
use serde_json::{json, Map, Value};

use tribunal_core::report::{
    carry_forward as cf, draft_comments_strip, merged_locations_check, report_fidelity_checks,
};

fn pick(values: Vec<Value>) -> impl Strategy<Value = Value> {
    prop::sample::select(values)
}

fn maybe(values: Vec<Value>) -> impl Strategy<Value = Option<Value>> {
    proptest::option::weighted(0.8, pick(values))
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

/// The oracle's result shape for a call that may throw.
fn outcome(r: Result<Value, String>) -> Value {
    match r {
        Ok(v) => json!({"ok": v}),
        Err(_) => json!({"threw": true}),
    }
}

fn compare_raw(region: &str, exports: &[&str], calls: &[Value], rust: Vec<Result<Value, String>>) {
    let js: Vec<Value> = oracle_raw(region, exports, &[], calls)
        .into_iter()
        .map(outcome)
        .collect();
    let rust: Vec<Value> = rust.into_iter().map(outcome).collect();
    assert_same(region, calls, &rust, &js);
}

// --- DRAFT-COMMENTS-STRIP ----------------------------------------------------

fn draft_body() -> impl Strategy<Value = String> {
    let line = prop::sample::select(vec![
        "### Draft comments",
        "### Draft comments (ready to paste)",
        "### Draft commentsé",
        "### Draft comments_x",
        "### Draft commentsX",
        "### Draft comments1",
        "### Draft comments\r",
        "### Draft comments\u{2028}x",
        "### Draft commentary",
        "#### Draft comments",
        " ### Draft comments",
        "### draft comments",
        "### 🔵 Nits",
        "### Nits",
        "###Nits",
        "### ",
        "###",
        "#### sub",
        "",
        "",
        " ",
        "\t",
        "\u{FEFF}",
        "\u{0085}",
        "\u{3000}",
        "\u{00A0}",
        "\r",
        "- one",
        "**B** `y.ts:2`",
    ]);
    let sep = prop::sample::select(vec!["\n", "\n", "\r\n", "\u{2028}"]);
    prop::collection::vec((line, sep), 0..9)
        .prop_map(|parts| parts.into_iter().map(|(l, s)| format!("{l}{s}")).collect())
}

const DRAFT_SEEDS: &[&str] = &[
    "### 🔴 Blocking\n\n**A** `x.ts:1`\n",
    "### 🔴 Blocking\n\n**A** `x.ts:1`\n\n### Draft comments (ready to paste)\n\n- one\n- two\n",
    "### 🔴 Blocking\n\n**A** `x.ts:1`\n\n### Draft comments\n\n- one\n\n### 🔵 Nits\n\n**B** `y.ts:2`\n",
    "### Draft comments\n\n- one\n\n### 🔵 Nits\n\n**B**\n",
    "### 🔴 Blocking\n\n### Draft comments\n\n### 🔵 Nits\n\n**B**\n",
    "### 🔴 Blocking\n\n### Draft comments\n- one\n",
    "",
    "### 🔴 Blocking\n\n### Draft commentary\n\n- keep me\n",
    "### Draft comments\n\n#### sub\n\n- one\n\n### 🔵 Nits\n\n**B**\n",
    "### A\n\n### Draft comments\n\n- one\n\n### B\n\n### Draft comments\n\n- two\n",
    "### 🔴 Blocking\n\n**Negative amount accepted** `src/invoice.ts:88`\nNo positive check before create.\n\n### Draft comments (ready to paste)\n\n- `src/invoice.ts:88` — also at: src/bulk-invoice.ts:210\n- the duplicate above was dismissed as not-triggered\n",
];

#[test]
fn draft_comments_strip_matches_js() {
    let mut bodies: Vec<String> = DRAFT_SEEDS.iter().map(|s| (*s).to_owned()).collect();
    bodies.extend(sample(draft_body()));
    let calls: Vec<Value> = bodies
        .iter()
        .map(|b| json!({"fn": "stripDraftCommentsFromBody", "args": [b]}))
        .collect();
    let rust: Vec<Value> = bodies
        .iter()
        .map(|b| json!(draft_comments_strip::strip_draft_comments_from_body(b)))
        .collect();
    let js = oracle(
        "DRAFT-COMMENTS-STRIP",
        &["stripDraftCommentsFromBody"],
        &[],
        &calls,
    );
    assert_same("stripDraftCommentsFromBody", &calls, &rust, &js);
}

// --- shared report text --------------------------------------------------------

/// Fragments that hold both the JavaScript rendering of a value and the
/// rendering a naive port would produce, so a wrong `String(x)` misses one and
/// hits the other.
fn report_text() -> impl Strategy<Value = String> {
    let fragment = prop::sample::select(vec![
        "a.ts:5",
        "a.ts:12",
        "a.ts:3",
        "a.ts:3.0",
        "a.ts:1e+21",
        "a.ts:1000000000000000000000",
        "a.ts:1e-7",
        "a.ts:0.0000001",
        "a.ts:0.5",
        "a.ts:-4",
        "b.sh:12",
        "a.ts:true",
        "a.ts:1,2",
        "a,b:5",
        "a.ts",
        "b.sh",
        "undefined",
        "undefined:5",
        "null",
        "null:12",
        "[object Object]",
        "7",
        "1e+21",
        "true",
        "a,b",
        "killed: out-of-scope",
        "uncategorized",
        "not-triggered",
        "(found by: lensA, lensB)",
        "(found by:",
        "lensA",
        "lensB",
        "TS2345: x",
        "é",
    ]);
    prop::collection::vec(fragment, 0..5).prop_map(|f| f.join(" "))
}

// --- MERGED-LOCATIONS-CHECK --------------------------------------------------

fn location() -> impl Strategy<Value = Value> {
    let file = maybe(vec![
        json!("a.ts"),
        json!("a.ts"),
        json!("b.sh"),
        json!(""),
        Value::Null,
        json!(0),
        json!(7),
        json!(["a", "b"]),
        json!({}),
        json!(true),
    ]);
    let line = maybe(vec![
        json!(0),
        json!(5),
        json!(12),
        json!(3.0),
        json!(1e21),
        json!(1e-7),
        json!(0.5),
        json!(-4),
        json!("12"),
        json!(""),
        json!(true),
        json!([1, 2]),
        json!([]),
        json!({}),
        Value::Null,
    ]);
    let object = (file, line).prop_map(|(f, l)| obj(vec![("file", f), ("line", l)]));
    prop_oneof![
        12 => object,
        1 => Just(Value::Null),
        1 => Just(json!("x")),
        1 => Just(json!(5)),
    ]
}

fn merged_locations() -> impl Strategy<Value = Value> {
    prop_oneof![
        12 => prop::collection::vec(location(), 0..5).prop_map(Value::Array),
        1 => Just(Value::Null),
        1 => Just(json!("")),
        1 => Just(json!("abc")),
        1 => Just(json!(5)),
        1 => Just(json!({})),
    ]
}

#[test]
fn merged_locations_check_matches_js() {
    let cases = sample((merged_locations(), report_text()));
    let mut calls: Vec<Value> = vec![
        json!({"fn": "mergedLocationsMissingFromReportText", "args": [[{"file": "a.sh", "line": 5}, {"file": "b.sh", "line": 12}], "also at: b.sh:12"]}),
        json!({"fn": "mergedLocationsMissingFromReportText", "args": [[{"file": "a.sh", "line": 5}, {"file": "b.sh", "line": 12}, {"file": "c.sh", "line": 9}], "### Blocking\n1. dup exit-0 bug — a.sh:5 — also at: b.sh:12"]}),
        json!({"fn": "mergedLocationsMissingFromReportText", "args": [[], "empty"]}),
    ];
    calls.extend(
        cases
            .iter()
            .map(|(l, t)| json!({"fn": "mergedLocationsMissingFromReportText", "args": [l, t]})),
    );
    let rust = calls
        .iter()
        .map(|c| {
            merged_locations_check::merged_locations_missing_from_report_text(
                Some(&c["args"][0]),
                c["args"][1].as_str().unwrap(),
            )
            .map(Value::Array)
        })
        .collect();
    compare_raw(
        "MERGED-LOCATIONS-CHECK",
        &["mergedLocationsMissingFromReportText"],
        &calls,
        rust,
    );
}

// --- REPORT-FIDELITY-CHECKS --------------------------------------------------

fn kill_reason() -> impl Strategy<Value = Value> {
    pick(vec![
        Value::Null,
        json!(""),
        json!(0),
        json!(false),
        json!("out-of-scope"),
        json!("out-of-scope"),
        json!("uncategorized"),
        json!("not-triggered"),
        json!(7),
        json!(1e21),
        json!(true),
        json!([]),
        json!(["a", "b"]),
        json!([null]),
        json!({}),
        json!("é"),
    ])
}

fn merged_from() -> impl Strategy<Value = Value> {
    let lens = pick(vec![
        json!("lensA"),
        json!("lensB"),
        json!("lensC"),
        Value::Null,
        json!(7),
        json!(["lensA"]),
        json!({}),
        json!(""),
        json!(true),
    ]);
    prop_oneof![
        8 => prop::collection::vec(lens, 0..4).prop_map(Value::Array),
        1 => Just(Value::Null),
        1 => Just(json!("lensA,lensB")),
        1 => Just(json!({"0": "lensA", "1": "lensB", "length": 2})),
    ]
}

fn grounded() -> impl Strategy<Value = Value> {
    let verdict = maybe(vec![
        json!("confirmed"),
        json!("confirmed"),
        json!("Confirmed"),
        json!("refuted"),
        Value::Null,
        json!(true),
    ]);
    let diagnostic = maybe(vec![
        json!("TS2345: x"),
        json!("TS9999: y"),
        json!(""),
        Value::Null,
        json!(0),
        json!(7),
        json!({}),
        json!(["TS2345: x"]),
        json!(1e21),
    ]);
    let object =
        (verdict, diagnostic).prop_map(|(v, d)| obj(vec![("verdict", v), ("diagnostic", d)]));
    prop_oneof![
        8 => object,
        1 => Just(Value::Null),
        1 => Just(json!("confirmed")),
        1 => Just(json!(["confirmed"])),
        1 => Just(json!(0)),
    ]
}

#[test]
fn report_fidelity_checks_matches_js() {
    let cases = sample((kill_reason(), merged_from(), grounded(), report_text()));
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for (k, m, g, t) in &cases {
        calls.push(json!({"fn": "killReasonMissingFromReportText", "args": [k, t]}));
        rust.push(json!(
            report_fidelity_checks::kill_reason_missing_from_report_text(Some(k), t)
        ));
        calls.push(json!({"fn": "mergedFromAnnotationMissingFromReportText", "args": [m, t]}));
        rust.push(json!(
            report_fidelity_checks::merged_from_annotation_missing_from_report_text(Some(m), t)
        ));
        calls.push(json!({"fn": "groundedDiagnosticMissingFromReportText", "args": [g, t]}));
        rust.push(json!(
            report_fidelity_checks::grounded_diagnostic_missing_from_report_text(Some(g), t)
        ));
    }
    let js = oracle(
        "REPORT-FIDELITY-CHECKS",
        &[
            "killReasonMissingFromReportText",
            "mergedFromAnnotationMissingFromReportText",
            "groundedDiagnosticMissingFromReportText",
        ],
        &[],
        &calls,
    );
    assert_same("REPORT-FIDELITY-CHECKS", &calls, &rust, &js);
}

fn gated_finding() -> impl Strategy<Value = Value> {
    let note = maybe(vec![
        json!("Downgraded"),
        json!(""),
        Value::Null,
        json!(0),
        json!(true),
    ]);
    let votes = maybe(vec![
        json!([{}]),
        json!([{}, {}]),
        json!([]),
        json!("v"),
        json!({"length": 1}),
        Value::Null,
    ]);
    let file = maybe(vec![
        json!("a.ts"),
        json!("a.ts"),
        json!(""),
        json!(7),
        json!(["a.ts"]),
    ]);
    let line = maybe(vec![
        json!(62),
        json!(62.0),
        json!(-0.0),
        json!(1e21),
        json!(0.5),
        json!("62"),
        Value::Null,
        json!(true),
    ]);
    let object = (note, votes, file, line).prop_map(|(n, v, f, l)| {
        obj(vec![
            ("verifiedByGateNote", n),
            ("votes", v),
            ("file", f),
            ("line", l),
        ])
    });
    let demoted = pick(vec![json!(62), json!(62.0), json!(-0.0), json!(1e21)]).prop_map(
        |l| json!({"verifiedByGateNote": "Downgraded", "votes": [{}], "file": "a.ts", "line": l}),
    );
    prop_oneof![
        12 => demoted,
        12 => object,
        1 => Just(Value::Null),
        1 => Just(json!("a.ts:62")),
        1 => Just(json!([{"file": "a.ts", "line": 62}])),
        1 => Just(json!(0)),
    ]
}

fn block_report() -> impl Strategy<Value = String> {
    let line = prop::sample::select(vec![
        "### Nits",
        "#",
        "# a.ts:62",
        "####### a.ts:62",
        "#a.ts:62",
        "**Demoted**",
        "**Demoted** \r",
        "**Demoted** — `a.ts:62`",
        "***",
        "**",
        "**Note:** body",
        "1. Demoted — a.ts:62 — why",
        "12. a.ts:62",
        "12.a.ts:62",
        "1.  a.ts:62",
        "```",
        "  ```ts",
        "`a.ts:62`",
        "a.ts:62",
        "a.ts:0",
        "a.ts:1e+21",
        "a.ts:620",
        "`b.ts:9`",
        "",
        " ",
        "\r",
        "é",
        "NOT ADJUDICATED",
        "— NOT ADJUDICATED: no refuter examined this claim",
        "not adjudicated",
        "No Refuter Examined",
        "NO REFUTER EXAMINED",
        "no refuter exam\u{0130}ned",
        "no  refuter examined",
        "\u{2028}NOT ADJUDICATED",
    ]);
    let sep = prop::sample::select(vec!["\n", "\n", "\r\n"]);
    prop::collection::vec((line, sep), 0..12).prop_map(|parts| {
        parts
            .into_iter()
            .map(|(l, s)| format!("{l}{s}"))
            .collect::<String>()
    })
}

#[test]
fn unexamined_wording_matches_js() {
    let cases = sample((gated_finding(), block_report()));
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for (f, t) in &cases {
        calls.push(json!({"fn": "unexaminedWordingInReportText", "args": [f, t]}));
        rust.push(json!(
            report_fidelity_checks::unexamined_wording_in_report_text(f, t)
        ));
    }
    let hits = rust
        .iter()
        .filter(|r| r.as_array().is_some_and(|a| !a.is_empty()))
        .count();
    assert!(hits > 20, "only {hits} cases found the marker");
    let js = oracle(
        "REPORT-FIDELITY-CHECKS",
        &["unexaminedWordingInReportText"],
        &[],
        &calls,
    );
    assert_same("REPORT-FIDELITY-CHECKS", &calls, &rust, &js);
}

// --- CARRY-FORWARD -----------------------------------------------------------

fn title() -> impl Strategy<Value = Value> {
    let word = prop::sample::select(vec![
        "the",
        "with",
        "into",
        "listener",
        "Failed",
        "LISTENER",
        "row",
        "x_y",
        "ab",
        "abc",
        "a1b",
        "42",
        "\u{212A}ey",
        "\u{0130}nit",
        "stra\u{00DF}e",
        "\u{00C9}COLE",
        "\u{03A3}test",
        "error",
        "handler",
        "retry",
        "job",
        "worker",
        "__init__",
        "0x1E",
        "é",
    ]);
    let sep = prop::sample::select(vec![" ", " ", "-", "'", "é", "\u{2028}", "/", "", "\t"]);
    let text = prop::collection::vec((word, sep), 0..7).prop_map(|parts| {
        Value::String(parts.into_iter().map(|(w, s)| format!("{w}{s}")).collect())
    });
    prop_oneof![
        10 => text,
        1 => Just(Value::Null),
        1 => Just(json!(0)),
        1 => Just(json!(12345)),
        1 => Just(json!({})),
        1 => Just(json!(["Worker failed", "retry job"])),
        1 => Just(json!(true)),
    ]
}

fn line_field() -> impl Strategy<Value = Option<Value>> {
    maybe(vec![
        Value::Null,
        json!(0),
        json!(10),
        json!(50),
        json!(51),
        json!(90),
        json!(-5),
        json!(45.5),
        json!(1e21),
        json!("30"),
        json!(" 30 "),
        json!("0x1E"),
        json!("1e1"),
        json!(""),
        json!(true),
        json!(false),
        json!([20]),
        json!([[20]]),
        json!([1, 2]),
        json!({}),
        json!("Infinity"),
        json!("12abc"),
        json!(".5"),
        json!("5."),
        json!("+7"),
        json!("\u{FEFF}20"),
        json!("\u{0085}20"),
        json!("0b11"),
        json!("0o17"),
        json!("1_000"),
        json!("inf"),
        json!("NaN"),
        json!("-0x10"),
        json!("1e400"),
    ])
}

fn file_field() -> impl Strategy<Value = Option<Value>> {
    maybe(vec![
        json!("a.ts"),
        json!("a.ts"),
        json!("a.ts"),
        json!("b.ts"),
        json!(""),
        Value::Null,
        json!(0),
        json!(5),
        json!(5.0),
        json!(false),
        json!({}),
        json!([]),
    ])
}

fn finding_like() -> impl Strategy<Value = Value> {
    let object = (
        file_field(),
        line_field(),
        proptest::option::weighted(0.9, title()),
    )
        .prop_map(|(f, l, t)| obj(vec![("file", f), ("line", l), ("title", t)]));
    prop_oneof![
        12 => object,
        1 => Just(Value::Null),
        1 => Just(json!(0)),
        1 => Just(json!("x")),
        1 => Just(json!([])),
        1 => Just(json!({})),
    ]
}

/// Single-token titles: comparing a title against one of these on the same
/// file asks whether that token is in the title's set, which is how
/// `cfTokens` (a `Set`, invisible through JSON) is checked token by token.
const PROBES: &[&str] = &[
    "listener", "failed", "row", "x_y", "abc", "a1b", "key", "nit", "cole", "stra", "test",
    "error", "handler", "retry", "job", "worker", "__init__", "0x1e", "object", "true", "12345",
    "with", "init", "42",
];

fn severity() -> impl Strategy<Value = Option<Value>> {
    maybe(vec![
        json!("critical"),
        json!("important"),
        json!("nit"),
        json!("Critical"),
        Value::Null,
        json!(3),
    ])
}

fn truthy_flag() -> impl Strategy<Value = Option<Value>> {
    maybe(vec![
        json!(true),
        json!(false),
        json!(1),
        json!(0),
        json!(""),
        json!("no"),
        json!([]),
        Value::Null,
    ])
}

fn override_field() -> impl Strategy<Value = Option<Value>> {
    let direction = maybe(vec![
        json!("reinstated"),
        json!("rejected"),
        json!("withdrawn"),
        json!("dropped"),
        json!("Reinstated"),
        json!(1),
        Value::Null,
    ]);
    let object = direction.prop_map(|d| obj(vec![("direction", d), ("by", Some(json!("alice")))]));
    proptest::option::weighted(
        0.6,
        prop_oneof![
            6 => object,
            1 => Just(Value::Null),
            1 => Just(json!("reinstated")),
            1 => Just(json!(0)),
        ],
    )
}

fn verification() -> impl Strategy<Value = Option<Value>> {
    let verdict = maybe(vec![
        json!("FIXED"),
        json!("STILL_OPEN"),
        json!("CANNOT_TELL"),
        json!("fixed"),
        Value::Null,
    ]);
    let object = verdict.prop_map(|v| obj(vec![("verdict", v), ("quote", Some(json!("q")))]));
    proptest::option::weighted(
        0.7,
        prop_oneof![
            6 => object,
            1 => Just(Value::Null),
            1 => Just(json!("FIXED")),
        ],
    )
}

/// A prior finding as a baseline row holds it, plus keys that look like array
/// indices (spread moves them first) and keys the region overwrites.
fn prior_object() -> impl Strategy<Value = Value> {
    let file = maybe(vec![
        json!("a.ts"),
        json!("b.ts"),
        json!("c.ts"),
        json!("d.ts"),
        json!("e.ts"),
        json!("a.ts"),
        json!("b.ts"),
        json!("c.ts"),
        json!("d.ts"),
        json!("a.ts"),
        json!("b.ts"),
        json!("c.ts"),
        json!("d.ts"),
        json!(5),
        json!(""),
        json!("constructor"),
        json!("toString"),
        json!("__proto__"),
        json!("hasOwnProperty"),
        json!("valueOf"),
        json!("__defineGetter__"),
        json!([]),
        json!(["a.ts"]),
    ]);
    (
        any::<bool>(),
        file,
        line_field(),
        title(),
        severity(),
        truthy_flag(),
        override_field(),
        verification(),
        prop::sample::select(vec![0u8, 0, 0, 1, 2, 3]),
    )
        .prop_map(
            |(index_keys, file, line, title, sev, blocking, oo, ver, extra)| {
                let mut m = Map::new();
                m.insert("title".into(), title);
                if index_keys {
                    m.insert("10".into(), json!("ten"));
                    m.insert("2".into(), json!({"b": 1, "1": 2}));
                    m.insert("01".into(), json!("not an index"));
                }
                for (k, v) in [
                    ("file", file),
                    ("line", line),
                    ("severity", sev),
                    ("blocking", blocking),
                    ("operatorOverride", oo),
                    ("verification", ver),
                ] {
                    if let Some(v) = v {
                        m.insert(k.into(), v);
                    }
                }
                match extra {
                    1 => {
                        m.insert("carryReason".into(), json!("old"));
                    }
                    2 => {
                        m.insert("matchedTitle".into(), json!("old"));
                    }
                    3 => {
                        m.insert("escalatedByImportHeuristic".into(), json!(false));
                    }
                    _ => {}
                }
                Value::Object(m)
            },
        )
}

fn prior_item() -> impl Strategy<Value = Value> {
    prop_oneof![
        60 => prior_object(),
        1 => Just(Value::Null),
        1 => Just(json!("ab")),
        1 => Just(json!(7)),
        1 => Just(json!([1, 2])),
    ]
}

fn prior_findings() -> impl Strategy<Value = Value> {
    prop_oneof![
        20 => prop::collection::vec(prior_item(), 0..6).prop_map(Value::Array),
        1 => Just(Value::Null),
        1 => Just(json!("ab")),
        1 => Just(json!(5)),
        1 => Just(json!({})),
    ]
}

/// Each file lands in one bucket (or none), so every bucket is reachable;
/// a list is now and then missing, `null`, a string (searched as a substring)
/// or a number (which throws).
fn scope() -> impl Strategy<Value = Value> {
    const FILES: [&str; 10] = [
        "a.ts",
        "b.ts",
        "c.ts",
        "d.ts",
        "constructor",
        "toString",
        "__proto__",
        "hasOwnProperty",
        "valueOf",
        "__defineGetter__",
    ];
    const LISTS: [&str; 4] = ["unchanged", "gone", "changed", "added"];
    let buckets = prop::collection::vec(0usize..5, FILES.len());
    let extras = prop::collection::vec(
        (0usize..4, pick(vec![json!(5.0), json!({}), json!("e.ts")])),
        0..3,
    );
    let mut shapes = vec![0u8; 24];
    shapes.extend([1, 2, 3, 4, 5]);
    let shape = prop::collection::vec(prop::sample::select(shapes), 4);
    let object = (buckets, extras, shape).prop_map(|(buckets, extras, shape)| {
        let mut lists: Vec<Vec<Value>> = vec![Vec::new(); 4];
        for (file, bucket) in FILES.iter().zip(buckets) {
            if bucket < 4 {
                lists[bucket].push(json!(file));
            }
        }
        for (i, v) in extras {
            lists[i].push(v);
        }
        let mut m = Map::new();
        for ((name, list), shape) in LISTS.iter().zip(lists).zip(shape) {
            let v = match shape {
                0 => Value::Array(list),
                1 => continue,
                2 => Value::Null,
                3 => json!("a.ts b.ts"),
                4 => json!(5),
                _ => json!(""),
            };
            m.insert((*name).to_owned(), v);
        }
        Value::Object(m)
    });
    prop_oneof![10 => object, 1 => Just(Value::Null), 1 => Just(json!("x"))]
}

fn adjacency() -> impl Strategy<Value = Option<Map<String, Value>>> {
    let key = prop::sample::select(vec![
        "a.ts",
        "b.ts",
        "c.ts",
        "5",
        "constructor",
        "toString",
        "__proto__",
    ]);
    let related = pick(vec![
        json!([]),
        json!([{"file": "b.ts", "matchConfidence": "path"}]),
        json!([{"file": "b.ts", "matchConfidence": "path"}]),
        json!([{"file": "c.ts"}, {"file": {"1": 1, "a": 2}}, {"x": 1}]),
        json!([null]),
        json!([5, "s"]),
        json!("abc"),
        json!({"length": 1}),
        json!({"length": 0}),
        json!({"length": "0"}),
        json!(0),
        Value::Null,
        json!(true),
    ]);
    proptest::option::weighted(
        0.8,
        prop::collection::vec((key, related), 0..4)
            .prop_map(|kv| kv.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()),
    )
}

fn current_findings() -> impl Strategy<Value = Vec<Value>> {
    let file = maybe(vec![
        json!("a.ts"),
        json!("b.ts"),
        json!("c.ts"),
        json!(5),
        json!("constructor"),
    ]);
    let object = (file, line_field(), proptest::option::weighted(0.8, title()))
        .prop_map(|(f, l, t)| obj(vec![("file", f), ("line", l), ("title", t)]));
    prop::collection::vec(object, 0..4)
}

fn grade_input() -> impl Strategy<Value = Value> {
    let verdict = maybe(vec![
        json!("FIXED"),
        json!("FIXED"),
        json!("STILL_OPEN"),
        json!("STILL_OPEN"),
        json!("CANNOT_TELL"),
        json!("PROBABLY_FINE"),
        json!("fixed"),
        json!(5),
        Value::Null,
    ]);
    let text = prop::sample::select(vec![
        'a', 'a', 'a', 'a', 'é', '😀', ' ', '\t', '\u{FEFF}', '\u{0085}', '\u{3000}', '\u{00A0}',
        '\n',
    ]);
    let edge =
        prop::collection::vec(text, 9..15).prop_map(|v| Value::String(v.into_iter().collect()));
    let quote = proptest::option::weighted(
        0.8,
        prop_oneof![
            6 => edge,
            1 => Just(json!("if (job && isLastRetry(job)) {")),
            1 => Just(json!(123456789012_i64)),
            1 => Just(json!(12345678901_i64)),
            1 => Just(json!(1e21)),
            1 => Just(json!({})),
            1 => Just(json!(["abcdef", "ghijk"])),
            1 => Just(json!("")),
            1 => Just(Value::Null),
        ],
    );
    let reason = maybe(vec![
        json!("looks handled"),
        json!("  padded  "),
        json!(""),
        json!(" "),
        json!("\u{FEFF}"),
        json!(0),
        json!(7),
        json!({}),
    ]);
    let object = (verdict, quote, reason)
        .prop_map(|(v, q, r)| obj(vec![("verdict", v), ("quote", q), ("reason", r)]));
    prop_oneof![
        12 => object,
        1 => Just(Value::Null),
        1 => Just(json!(0)),
        1 => Just(json!("FIXED")),
        1 => Just(json!([])),
    ]
}

fn fallback_args() -> impl Strategy<Value = Vec<Value>> {
    let v = || {
        pick(vec![
            Value::Null,
            json!(""),
            json!(0),
            json!(false),
            json!(true),
            json!(3),
            json!(2.5),
            json!(1e21),
            json!("91c5e0458392"),
            json!("a diff was empty"),
            json!("tribunal-row"),
            json!([]),
            json!(["a", 1]),
            json!({"b": 1, "3": 2}),
        ])
    };
    (v(), v(), v(), v(), v(), 0usize..6).prop_map(|(a, b, c, d, e, n)| {
        // A shorter argument list leaves the tail `undefined`.
        vec![a, b, c, d, e].into_iter().take(n).collect()
    })
}

fn verdict_finding() -> impl Strategy<Value = Value> {
    let object = (
        severity(),
        truthy_flag(),
        truthy_flag(),
        maybe(vec![json!("Downgraded"), json!(""), Value::Null, json!(0)]),
        maybe(vec![
            json!(false),
            json!(true),
            json!("false"),
            json!(0),
            Value::Null,
        ]),
    )
        .prop_map(|(s, b, h, note, adj)| {
            obj(vec![
                ("severity", s),
                ("isBehaviorChanging", b),
                ("touchesHighRiskSurface", h),
                ("verifiedByGateNote", note),
                ("adjudicated", adj),
            ])
        });
    prop_oneof![10 => object, 1 => Just(Value::Null), 1 => Just(json!("critical"))]
}

fn verdict_list() -> impl Strategy<Value = Option<Vec<Value>>> {
    proptest::option::weighted(0.9, prop::collection::vec(verdict_finding(), 0..4))
}

fn severities() -> impl Strategy<Value = Option<Vec<Value>>> {
    let sev = pick(vec![
        json!("critical"),
        json!("important"),
        json!("nit"),
        Value::Null,
        json!(0),
        json!("Critical"),
        json!(["critical"]),
    ]);
    proptest::option::weighted(0.9, prop::collection::vec(sev, 0..3))
}

const CARRY_FORWARD_EXPORTS: &[&str] = &[
    "classifyPriorFindings",
    "cfSameFinding",
    "cfTokens",
    "incrementalFallbackFor",
    "verdictFor",
    "gradePriorVerification",
    "unresolvedPriorBlockersFrom",
    "countsAsBlocker",
    "priorGrade",
];

fn opt_arg(args: &Value, i: usize) -> Option<&Value> {
    args.as_array().and_then(|a| a.get(i))
}

fn list_arg(v: &Option<Vec<Value>>) -> Value {
    v.as_ref().map_or(Value::Null, |l| Value::Array(l.clone()))
}

/// The replay's PR #1968 golden, the PR #2012 adjacency case and its own
/// hand-written matcher and grading cases, as seeds.
fn carry_forward_seeds() -> Vec<Value> {
    let prior = json!([
        {"file": "src/services/v2/contactResults.ts", "title": "Redundant prisma.account lookup re-fetches data already in scope on every update/merge job outcome", "severity": "nit", "blocking": false},
        {"file": "src/controllers/v2/account.controller.ts", "title": "Enqueue-failure + partner redelivery creates a duplicate ContactResults row that is stranded in 'error' forever", "severity": "important", "blocking": true},
        {"file": "src/jobs/profiles_worker.ts", "title": "Worker's 'failed' listener permanently flips contactResults row to error on any retryable failure", "severity": "critical", "blocking": true},
    ]);
    let scope = json!({
        "changed": ["src/controllers/v2/account.controller.test.ts", "src/jobs/profiles_worker.ts"],
        "added": ["src/jobs/retryState.test.ts", "src/jobs/retryState.ts"],
        "unchanged": ["src/controllers/v2/account.controller.ts", "src/controllers/v2/contactResults.ts", "src/services/v2/contactResults.ts", "src/jobs/queue.ts", "src/routes/v2/contactResults.routes.ts"],
        "gone": [],
    });
    let current = json!([{"file": "src/jobs/profiles_worker.ts", "title": "outcome.contactName ?? undefined collapses an intentional null into 'not provided', triggering an unintended DB lookup", "severity": "nit"}]);
    let reraise = json!({"file": "src/jobs/profiles_worker.ts", "title": "The 'failed' listener still flips the contactResults row to error on a retryable failure"});
    let scope_2012 = json!({"changed": ["src/services/v2/accounts.ts"], "added": [], "unchanged": ["src/controllers/v2/auth.controller.ts"], "gone": []});
    let prior_2012 = json!([{"file": "src/controllers/v2/auth.controller.ts", "title": "Account-level skip pushes success:false rows for orders never attempted", "severity": "important", "blocking": true}]);
    let adjacency_2012 = json!({"src/controllers/v2/auth.controller.ts": [{"file": "src/services/v2/accounts.ts", "matchConfidence": "path"}]});
    let reinstated = json!({"file": "src/controllers/v2/account.controller.ts", "severity": "important", "blocking": false, "survived": false, "operatorOverride": {"direction": "reinstated", "by": "alice"}});
    vec![
        json!({"fn": "classifyPriorFindings", "args": [scope, prior, current]}),
        json!({"fn": "classifyPriorFindings", "args": [Value::Null, prior, current]}),
        json!({"fn": "classifyPriorFindings", "args": [scope, prior, []]}),
        json!({"fn": "classifyPriorFindings", "args": [scope, [reraise], current]}),
        json!({"fn": "classifyPriorFindings", "args": [scope_2012, prior_2012, [], adjacency_2012]}),
        json!({"fn": "classifyPriorFindings", "args": [scope_2012, prior_2012, [], {}]}),
        json!({"fn": "cfSameFinding", "args": [prior[2], current[0]]}),
        json!({"fn": "cfSameFinding", "args": [prior[2], reraise]}),
        json!({"fn": "cfSameFinding", "args": [{"file": "a.ts", "line": 100, "title": "completely unrelated words here"}, {"file": "a.ts", "line": 120, "title": "nothing in common at all"}]}),
        json!({"fn": "gradePriorVerification", "args": [{"verdict": "FIXED", "quote": "if (job && isLastRetry(job)) {", "reason": "the flip is now gated on the final attempt"}]}),
        json!({"fn": "gradePriorVerification", "args": [{"verdict": "FIXED", "reason": "looks handled"}]}),
        json!({"fn": "gradePriorVerification", "args": [{"verdict": "PROBABLY_FINE", "quote": "some real looking code here"}]}),
        json!({"fn": "gradePriorVerification", "args": [Value::Null]}),
        json!({"fn": "countsAsBlocker", "args": [reinstated]}),
        json!({"fn": "priorGrade", "args": [reinstated]}),
        json!({"fn": "verdictFor", "args": [[], ["critical"]]}),
        json!({"fn": "verdictFor", "args": [Value::Null, ["critical"]]}),
        json!({"fn": "incrementalFallbackFor", "args": ["91c5e0458392", false, 0, "a diff was empty"]}),
        json!({"fn": "incrementalFallbackFor", "args": ["abc", false, 3, "ignored"]}),
        json!({"fn": "unresolvedPriorBlockersFrom", "args": [[reinstated], [{"severity": "critical", "blocking": true, "verification": {"verdict": "FIXED"}}]]}),
    ]
}

fn run_carry_forward(call: &Value) -> Result<Value, String> {
    let args = &call["args"];
    let a = |i| opt_arg(args, i);
    let slice = |i| -> Option<&[Value]> {
        match a(i) {
            Some(Value::Array(l)) => Some(l.as_slice()),
            _ => None,
        }
    };
    Ok(match call["fn"].as_str().unwrap() {
        "classifyPriorFindings" => cf::classify_prior_findings(
            a(0),
            a(1),
            slice(2).unwrap_or(&[]),
            a(3).and_then(Value::as_object),
        )?
        .to_json(),
        "cfSameFinding" => json!(cf::cf_same_finding(a(0).unwrap(), a(1).unwrap())),
        "incrementalFallbackFor" => {
            cf::incremental_fallback_for(a(0), a(1), a(2), a(3), a(4)).unwrap_or(Value::Null)
        }
        "verdictFor" => json!(cf::verdict_for(slice(0), slice(1))),
        "gradePriorVerification" => cf::grade_prior_verification(a(0)),
        "unresolvedPriorBlockersFrom" => Value::Array(cf::unresolved_prior_blockers_from(
            slice(0).unwrap_or(&[]),
            slice(1).unwrap_or(&[]),
        )),
        "countsAsBlocker" => json!(cf::counts_as_blocker(a(0).unwrap())),
        "priorGrade" => cf::prior_grade(a(0).unwrap()),
        other => panic!("no Rust port wired for {other}"),
    })
}

#[test]
fn carry_forward_matches_js() {
    let mut calls = carry_forward_seeds();

    let classify = sample((scope(), prior_findings(), current_findings(), adjacency()));
    for (i, (s, p, mut c, adj)) in classify.into_iter().enumerate() {
        // Every other case re-raises some priors, so `stillOpen` and the
        // `matchedTitle` overwrite are exercised: a same-title copy, a copy
        // with only a nearby line, and a copy with no title at all.
        if i % 2 == 0 {
            for (j, prior) in p.as_array().into_iter().flatten().take(3).enumerate() {
                let file = prior.get("file").cloned();
                let reraise = match j {
                    0 => obj(vec![("file", file), ("title", prior.get("title").cloned())]),
                    1 => obj(vec![
                        ("file", file),
                        ("line", Some(json!(30))),
                        ("title", Some(json!("zzz"))),
                    ]),
                    _ => obj(vec![("file", file), ("line", prior.get("line").cloned())]),
                };
                c.push(reraise);
            }
        }
        let mut args = vec![s, p, Value::Array(c)];
        if let Some(adj) = adj {
            args.push(Value::Object(adj));
        }
        calls.push(json!({"fn": "classifyPriorFindings", "args": args}));
    }
    for (a, b) in sample((finding_like(), finding_like())) {
        calls.push(json!({"fn": "cfSameFinding", "args": [a, b]}));
    }
    for t in sample(title()) {
        for probe in PROBES {
            calls.push(json!({"fn": "cfSameFinding", "args": [{"file": "a.ts", "title": t}, {"file": "a.ts", "title": probe}]}));
        }
    }
    // The line window's edges, with titles that share nothing, in the
    // spellings `Number()` accepts.
    for (base, delta, form) in sample((
        prop::sample::select(vec![-3i64, 0, 1, 5, 100]),
        prop::sample::select(vec![38i64, 39, 40, 41, -40, -41]),
        0u8..4,
    )) {
        let other = base + delta;
        let other = match form {
            0 => json!(other),
            1 => json!(format!(" {other} ")),
            2 if other >= 0 => json!(format!("0x{other:x}")),
            _ => json!([other]),
        };
        calls.push(json!({"fn": "cfSameFinding", "args": [
            {"file": "a.ts", "line": base, "title": "alpha beta"},
            {"file": "a.ts", "line": other, "title": "gamma delta"},
        ]}));
    }
    for args in sample(fallback_args()) {
        calls.push(json!({"fn": "incrementalFallbackFor", "args": args}));
    }
    for raw in sample(grade_input()) {
        calls.push(json!({"fn": "gradePriorVerification", "args": [raw]}));
    }
    for f in sample(prior_item()) {
        calls.push(json!({"fn": "countsAsBlocker", "args": [f]}));
        calls.push(json!({"fn": "priorGrade", "args": [f]}));
    }
    for f in [Value::Null, json!(0), json!(""), json!(false)] {
        calls.push(json!({"fn": "countsAsBlocker", "args": [f]}));
        calls.push(json!({"fn": "priorGrade", "args": [f]}));
    }
    // Elements are spreads of prior findings, so objects: see
    // `unresolved_prior_blockers_from`.
    let objects = || prop::collection::vec(prior_object(), 0..4);
    for (c, r) in sample((objects(), objects())) {
        calls.push(json!({"fn": "unresolvedPriorBlockersFrom", "args": [c, r]}));
    }
    for (s, p) in sample((verdict_list(), severities())) {
        calls.push(json!({"fn": "verdictFor", "args": [list_arg(&s), list_arg(&p)]}));
    }

    let rust = calls.iter().map(run_carry_forward).collect();
    compare_raw("CARRY-FORWARD", CARRY_FORWARD_EXPORTS, &calls, rust);
}

/// An own `__proto__` key (what `JSON.parse` makes of one) in a prior finding
/// and in the adjacency map, `-0` as a line, and nested arrays as `String()`
/// inputs.
#[test]
fn edge_values_match_js() {
    let proto_prior: Value =
        serde_json::from_str(r#"{"__proto__": {"1": 1, "a": 2}, "file": "a.ts", "title": "t"}"#)
            .unwrap();
    let proto_adj: Value =
        serde_json::from_str(r#"{"__proto__": [{"file": "z.ts"}], "a.ts": []}"#).unwrap();
    let scope = json!({"unchanged": ["a.ts", "__proto__"], "gone": [], "changed": [], "added": []});
    let cf_calls = vec![
        json!({"fn": "classifyPriorFindings", "args": [scope, [proto_prior], []]}),
        json!({"fn": "classifyPriorFindings", "args": [scope, [{"file": "__proto__"}], [], proto_adj]}),
        json!({"fn": "classifyPriorFindings", "args": [scope, [{"file": "__proto__"}], []]}),
        json!({"fn": "cfSameFinding", "args": [{"file": "a.ts", "line": -0.0, "title": "x"}, {"file": "a.ts", "line": 5, "title": "y"}]}),
        json!({"fn": "cfSameFinding", "args": [{"file": "a.ts", "line": [[null]], "title": [[null], "worker"]}, {"file": "a.ts", "line": 5, "title": "worker"}]}),
        json!({"fn": "incrementalFallbackFor", "args": ["sha", false, [[1, null], [2]]]}),
        json!({"fn": "gradePriorVerification", "args": [{"verdict": "FIXED", "quote": [[null, "abcdef"], ["ghijkl"]]}]}),
    ];
    let rust = cf_calls.iter().map(run_carry_forward).collect();
    compare_raw("CARRY-FORWARD", CARRY_FORWARD_EXPORTS, &cf_calls, rust);

    let ml_calls = vec![
        json!({"fn": "mergedLocationsMissingFromReportText", "args": [[{}, {"file": "a.ts", "line": -0.0}, {"file": [[null], "b"], "line": [[1]]}], "a.ts ,b:1"]}),
        json!({"fn": "mergedLocationsMissingFromReportText", "args": [[{}, "x", 5, []], "undefined"]}),
    ];
    let rust = ml_calls
        .iter()
        .map(|c| {
            merged_locations_check::merged_locations_missing_from_report_text(
                Some(&c["args"][0]),
                c["args"][1].as_str().unwrap(),
            )
            .map(Value::Array)
        })
        .collect();
    compare_raw(
        "MERGED-LOCATIONS-CHECK",
        &["mergedLocationsMissingFromReportText"],
        &ml_calls,
        rust,
    );

    let bodies = [
        "x\n\u{0085}\n### Draft comments\u{0085}\n- y\n### B",
        "x\n\u{180E}\n### Draft comments\u{00B5}\n- y",
        "x\n\u{200B}\n### Draft comments\u{0661}\n- y",
        "### Draft comments\n### Draft comments\n### B",
        "\n### Draft comments",
    ];
    let calls: Vec<Value> = bodies
        .iter()
        .map(|b| json!({"fn": "stripDraftCommentsFromBody", "args": [b]}))
        .collect();
    let rust: Vec<Value> = bodies
        .iter()
        .map(|b| json!(draft_comments_strip::strip_draft_comments_from_body(b)))
        .collect();
    let js = oracle(
        "DRAFT-COMMENTS-STRIP",
        &["stripDraftCommentsFromBody"],
        &[],
        &calls,
    );
    assert_same("stripDraftCommentsFromBody", &calls, &rust, &js);
}

/// A bit string (leading bit set) spelled in radix `2^k`.
fn bits_in_radix(bits: &[bool], k: usize) -> String {
    let pad = (k - bits.len() % k) % k;
    let padded: Vec<bool> = std::iter::repeat_n(false, pad)
        .chain(bits.iter().copied())
        .collect();
    padded
        .chunks(k)
        .map(|c| {
            let d = c.iter().fold(0u32, |acc, b| acc * 2 + u32::from(*b));
            std::char::from_digit(d, 1 << k).unwrap()
        })
        .collect()
}

/// Inputs aimed at single constructs: `Number()` of a non-decimal literal
/// wider than 128 bits (one correctly rounded value, whatever the radix), and
/// a scope list that is the empty string (`"" || []`) next to a truthy file
/// whose `String()` is empty.
#[test]
fn carry_forward_adversarial_cases_match_js() {
    let far = |a: Value, b: Value| {
        json!({"fn": "cfSameFinding", "args": [
            {"file": "a.ts", "line": a, "title": "alpha beta"},
            {"file": "a.ts", "line": b, "title": "gamma delta"},
        ]})
    };
    let mut calls = vec![
        far(
            json!("0xec45f62abad404ee17337f543c45c87f25e"),
            json!(1.2863941359478576e42),
        ),
        far(
            json!("0xcae9e420f77d1432cc0e75e2199e33296ff"),
            json!(1.104766924054022e42),
        ),
        far(
            json!("0o6412604542666712754150261430374425343363242"),
            json!(5.547465193575593e38),
        ),
        far(json!("0b100110010101011111101000010100010011110010011011111001001000010001101011000100010001010001111110011001111000101000101011011111010"), json!(4.076566502741029e38)),
        far(json!(format!("0x{}", "f".repeat(300))), json!(1e300)),
        far(json!(format!("0x1{}", "0".repeat(256))), json!(1e300)),
        far(json!("1e99999999999999999999"), json!(5)),
        far(json!("0e99999999999999999999"), json!(5)),
    ];
    let bits = prop::collection::vec(any::<bool>(), 120..300).prop_map(|mut b| {
        b.insert(0, true);
        b
    });
    for b in sample(bits) {
        let hex = json!(format!("0x{}", bits_in_radix(&b, 4)));
        let oct = json!(format!("0o{}", bits_in_radix(&b, 3)));
        let bin = json!(format!("0b{}", bits_in_radix(&b, 1)));
        calls.push(far(hex.clone(), oct.clone()));
        calls.push(far(hex, bin.clone()));
        calls.push(far(oct, bin));
    }
    for file in [json!([]), json!([null]), json!([""]), json!([[]])] {
        for list in ["unchanged", "gone", "changed", "added"] {
            let mut scope = json!({"unchanged": [], "gone": [], "changed": [], "added": []});
            scope[list] = json!("");
            calls.push(json!({"fn": "classifyPriorFindings", "args": [
                scope,
                [{"file": file, "title": "t"}],
                [],
            ]}));
        }
    }
    let rust = calls.iter().map(run_carry_forward).collect();
    compare_raw("CARRY-FORWARD", CARRY_FORWARD_EXPORTS, &calls, rust);
}
