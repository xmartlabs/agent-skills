mod common;
use common::*;

// Differential tests for the evidence group: TSC-ORACLE, REF-EVIDENCE-CHECK,
// LINE-CORRECTION, FETCH-PR-CONTEXT and PRIOR-REVIEWS against their regions.
//
// The oracle can only pass JSON, so two exports are small wrappers written
// in the export list itself (no commas, which the oracle splits on):
// `adjudicateFindingSet` turns its third argument into the `Set` the region
// expects, and `applyLineCorrectionState` returns the mutated candidate next
// to the boolean, so the mutation is compared and not only the return value.
//
// Where the JavaScript throws, the Rust returns `Err` and only the fact of
// the error is compared: V8's message text is not part of the contract.

use std::collections::HashSet;

use proptest::prelude::*;
use serde_json::{json, Map, Value};

use tribunal_core::evidence::js_ext::property_order_deep;
use tribunal_core::evidence::tsc_oracle::{self, DiagsByFile, TscDiagnostic};
use tribunal_core::evidence::{
    fetch_pr_context, line_correction, prior_reviews, ref_evidence_check,
};

fn same_outcome(
    what: &str,
    calls: &[Value],
    rust: &[Result<Value, String>],
    js: &[Result<Value, String>],
) {
    let as_value = |r: &Result<Value, String>| match r {
        Ok(v) => json!({"ok": v}),
        Err(_) => json!({"err": true}),
    };
    let rust: Vec<Value> = rust.iter().map(as_value).collect();
    let js: Vec<Value> = js.iter().map(as_value).collect();
    assert_same(what, calls, &rust, &js);
}

fn pick<T: Clone + std::fmt::Debug + 'static>(v: Vec<T>) -> impl Strategy<Value = T> + Clone {
    prop::sample::select(v)
}

/// Mostly a value that keeps the input on its main path, sometimes one that
/// probes a divergence point off it.
fn mostly(
    good: Vec<&'static str>,
    odd: Vec<&'static str>,
) -> impl Strategy<Value = &'static str> + Clone {
    prop_oneof![5 => pick(good), 1 => pick(odd)]
}

/// An object from optional fields, in the given order.
fn object(fields: Vec<(&str, Option<Value>)>) -> Value {
    Value::Object(
        fields
            .into_iter()
            .filter_map(|(k, v)| v.map(|v| (k.to_owned(), v)))
            .collect::<Map<String, Value>>(),
    )
}

// ---------------------------------------------------------------- TSC-ORACLE

const TSC_EXPORTS: &[&str] = &[
    "parseTscDiagnostics",
    "classifyTypecheckText",
    "filterDiagnosticsToPR",
    "groupDiagnosticsByFile",
    "GROUNDED_LINE_WINDOW",
    "adjudicateFinding",
    "resolveDiagFile",
    "adjudicateFindingSet:a=>{a[2]=a[2]&&new Set(a[2]);return adjudicateFinding(...a)}",
];

fn ws() -> impl Strategy<Value = &'static str> + Clone {
    mostly(
        vec![
            " ", "  ", "\t", " \u{A0}", "\u{FEFF}", "\u{2028}", "\r", "\u{3000}",
        ],
        vec!["", "\u{0085}"],
    )
}

fn tsc_line() -> impl Strategy<Value = String> {
    let file = mostly(
        vec![
            "src/a.ts",
            "a(b).ts",
            "x",
            "constructor",
            "10",
            "é/ü.ts",
            "src/😀.ts",
            "a(1,2)",
        ],
        vec!["a\rb", "", "a\u{2028}b"],
    );
    let digits = mostly(
        vec!["1", "42", "0012", "9999999999999999999999999"],
        vec![
            "１", "٣", "",
            "99999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999",
        ],
    );
    let code = mostly(vec!["TS2345", "TS2304"], vec!["TS", "ts1", "TS١"]);
    let message = mostly(
        vec![
            "Argument of type 'string'.",
            "",
            "trailing \u{FEFF}",
            " é ",
            "m\u{0085}",
            "x  ",
        ],
        vec!["x\r", "a\u{2028}b", " \r"],
    );
    let head = (
        file,
        digits.clone(),
        digits,
        ws(),
        mostly(vec!["error"], vec!["Error", "warning"]),
    );
    let tail = (ws(), code, ws(), message);
    let shaped = (head, tail).prop_map(|((f, l, c, w1, e), (w2, code, w3, m))| {
        format!("{f}({l},{c}):{w1}{e}{w2}{code}:{w3}{m}")
    });
    prop_oneof![
        6 => shaped,
        1 => pick(vec!["    continuation that tsc indents", "", "NO_DIAGNOSTICS", "src/a.ts(1,2) error TS1: x"]).prop_map(String::from),
    ]
}

fn tsc_text() -> impl Strategy<Value = Option<String>> {
    let body = prop::collection::vec((tsc_line(), mostly(vec!["\n"], vec!["\r\n", "\n\n"])), 0..5)
        .prop_map(|v| {
            v.into_iter()
                .map(|(l, t)| format!("{l}{t}"))
                .collect::<String>()
        });
    proptest::option::weighted(0.95, body)
}

fn diag() -> impl Strategy<Value = TscDiagnostic> {
    (
        pick(vec![
            "src/a.ts",
            "src/c.ts",
            "10",
            "2",
            "01",
            "4294967295",
            "4294967294",
            "constructor",
            "toString",
            "__proto__",
            "b",
        ]),
        pick(vec![1.0, 3.0, 8.0, 9.0, 42.0, 1e21, 47.0]),
        pick(vec!["TS2345", "TS2307"]),
        pick(vec!["m", "Cannot find name 'x'."]),
    )
        .prop_map(|(f, l, c, m)| TscDiagnostic {
            file: f.into(),
            line: l,
            code: c.into(),
            message: m.into(),
        })
}

fn pr_files() -> impl Strategy<Value = Option<Vec<String>>> {
    proptest::option::weighted(
        0.9,
        prop::collection::vec(
            pick(vec![
                "src/a.ts",
                "src/c.ts",
                "10",
                "2",
                "4294967295",
                "toString",
                "src/a/index.ts",
                "src/b/index.ts",
                "é.ts",
                "a.ts",
            ])
            .prop_map(String::from),
            0..5,
        ),
    )
}

fn diags_json(d: &[TscDiagnostic]) -> Value {
    Value::Array(d.iter().map(TscDiagnostic::to_value).collect())
}

#[test]
fn tsc_oracle_matches_js() {
    // parseTscDiagnostics
    let texts = sample(tsc_text());
    let calls: Vec<Value> = texts
        .iter()
        .map(|t| json!({"fn": "parseTscDiagnostics", "args": [t]}))
        .collect();
    let rust: Vec<Value> = texts
        .iter()
        .map(|t| diags_json(&tsc_oracle::parse_tsc_diagnostics(t.as_deref())))
        .collect();
    let js = oracle("TSC-ORACLE", TSC_EXPORTS, &[], &calls);
    assert_same("parseTscDiagnostics", &calls, &rust, &js);
    let parsed = rust
        .iter()
        .filter(|v| v.as_array().is_some_and(|a| !a.is_empty()))
        .count();
    assert!(
        parsed > 50,
        "the generator parses too few diagnostics: {parsed}"
    );

    // classifyTypecheckText
    let line = pick(vec![
        "NO_TSCONFIG",
        " NO_TSCONFIG ",
        "xNO_TSCONFIG",
        "NO_TSCONFIGx",
        "\u{FEFF}NO_TSCONFIG",
        "\u{0085}NO_TSCONFIG",
        "DEGRADED_TIMEOUT",
        "DEGRADED",
        "a.ts(1,2): error TS2345: m",
        "): error TS",
        "): error TSx",
        "): error TS٣",
        "NO_DIAGNOSTICS",
        "\u{FEFF}NO_DIAGNOSTICS\u{0085}",
        "NO_DIAGNOSTICS\u{00A0}",
        "x",
        "",
    ]);
    let term = pick(vec![
        "\n", "\r\n", "\r", "\u{2028}", "\u{2029}", " ", "\u{0085}", "",
    ]);
    let texts = sample(proptest::option::weighted(
        0.95,
        prop::collection::vec((line, term), 0..4).prop_map(|v| {
            v.into_iter()
                .map(|(l, t)| format!("{l}{t}"))
                .collect::<String>()
        }),
    ));
    let calls: Vec<Value> = texts
        .iter()
        .map(|t| json!({"fn": "classifyTypecheckText", "args": [t]}))
        .collect();
    let rust: Vec<Value> = texts
        .iter()
        .map(|t| json!(tsc_oracle::classify_typecheck_text(t.as_deref())))
        .collect();
    let js = oracle("TSC-ORACLE", TSC_EXPORTS, &[], &calls);
    assert_same("classifyTypecheckText", &calls, &rust, &js);

    // filterDiagnosticsToPR, then groupDiagnosticsByFile on what it kept
    let cases = sample((
        proptest::option::weighted(0.95, prop::collection::vec(diag(), 0..6)),
        pr_files(),
    ));
    let calls: Vec<Value> = cases
        .iter()
        .map(|(d, f)| json!({"fn": "filterDiagnosticsToPR", "args": [d.as_deref().map(diags_json), f]}))
        .collect();
    let filtered: Vec<Vec<TscDiagnostic>> = cases
        .iter()
        .map(|(d, f)| tsc_oracle::filter_diagnostics_to_pr(d.as_deref(), f.as_deref()))
        .collect();
    let rust: Vec<Value> = filtered.iter().map(|d| diags_json(d)).collect();
    let js = oracle("TSC-ORACLE", TSC_EXPORTS, &[], &calls);
    assert_same("filterDiagnosticsToPR", &calls, &rust, &js);

    let groups: Vec<Option<Vec<TscDiagnostic>>> = filtered
        .into_iter()
        .chain(cases.iter().map(|(d, _)| d.clone().unwrap_or_default()))
        .map(Some)
        .chain([None])
        .collect();
    let calls: Vec<Value> = groups
        .iter()
        .map(|d| json!({"fn": "groupDiagnosticsByFile", "args": [d.as_deref().map(diags_json)]}))
        .collect();
    let rust: Vec<Result<Value, String>> = groups
        .iter()
        .map(|d| tsc_oracle::group_diagnostics_by_file(d.as_deref()).map(|g| g.to_value()))
        .collect();
    let js = oracle_raw("TSC-ORACLE", TSC_EXPORTS, &[], &calls);
    same_outcome("groupDiagnosticsByFile", &calls, &rust, &js);

    // adjudicateFinding: with a coverage set through the wrapper, and with none
    let by = prop::collection::vec(
        (
            pick(vec![
                "src/a.ts",
                "src/c.ts",
                "10",
                "2",
                "toString",
                "__proto__",
                "x",
            ]),
            prop::collection::vec(diag(), 0..3),
        ),
        0..4,
    )
    .prop_map(|entries| {
        let mut seen = HashSet::new();
        DiagsByFile::from_entries(
            entries
                .into_iter()
                .filter(|(k, _)| seen.insert(*k))
                .map(|(k, d)| (k.to_owned(), d))
                .collect(),
        )
    });
    let finding = (
        prop_oneof![
            6 => pick(vec!["src/a.ts", "src/c.ts", "10", "2", "toString", "constructor", "valueOf", "__proto__", "__defineGetter__", "hasOwnProperty", "x"]).prop_map(|f| Some(json!(f))),
            1 => Just(None),
            1 => pick(vec![Value::Null, json!(10), json!(["10"]), json!(2.0)]).prop_map(Some),
        ],
        pick(vec![
            None, Some(json!(0)), Some(json!(-1)), Some(json!(3)), Some(json!(3.5)), Some(json!(4)),
            Some(json!(14)), Some(json!(14.5)), Some(json!(15)), Some(json!(36)), Some(json!(37)),
            Some(json!(47)), Some(json!(48)),
            Some(json!("5")), Some(json!(1e300)), Some(Value::Null),
        ]),
        pick(vec![None, Some(json!(true)), Some(json!(false)), Some(json!(1)), Some(json!("")), Some(json!("x")), Some(Value::Null)]),
    )
        .prop_map(|(file, line, mech)| object(vec![("file", file), ("line", line), ("mechanicalClaim", mech)]));
    let covered = proptest::option::weighted(
        0.8,
        prop::collection::vec(
            pick(vec![
                "src/a.ts", "src/c.ts", "10", "toString", "valueOf", "x",
            ])
            .prop_map(String::from),
            0..4,
        ),
    );
    let cases = sample((finding, by, covered));
    let calls: Vec<Value> = cases
        .iter()
        .map(|(f, by, c)| json!({"fn": "adjudicateFindingSet", "args": [[f, by.to_value(), c]]}))
        .collect();
    let rust: Vec<Result<Value, String>> = cases
        .iter()
        .map(|(f, by, c)| {
            let set: Option<HashSet<String>> = c.clone().map(|c| c.into_iter().collect());
            tsc_oracle::adjudicate_finding(f, by, set.as_ref()).map(|a| a.to_value())
        })
        .collect();
    let js = oracle_raw("TSC-ORACLE", TSC_EXPORTS, &[], &calls);
    same_outcome("adjudicateFinding (with coverage)", &calls, &rust, &js);
    let verdicts: HashSet<String> = rust
        .iter()
        .map(|r| {
            r.as_ref()
                .map_or("err".into(), |v| v["verdict"].to_string())
        })
        .collect();
    assert_eq!(
        verdicts.len(),
        4,
        "every outcome must be generated: {verdicts:?}"
    );

    let calls: Vec<Value> = cases
        .iter()
        .map(|(f, by, _)| json!({"fn": "adjudicateFinding", "args": [f, by.to_value(), null]}))
        .collect();
    let rust: Vec<Result<Value, String>> = cases
        .iter()
        .map(|(f, by, _)| tsc_oracle::adjudicate_finding(f, by, None).map(|a| a.to_value()))
        .collect();
    let js = oracle_raw("TSC-ORACLE", TSC_EXPORTS, &[], &calls);
    same_outcome("adjudicateFinding (no coverage)", &calls, &rust, &js);

    // resolveDiagFile
    let cases = sample((
        proptest::option::weighted(
            0.9,
            pick(vec![
                "",
                "index.ts",
                "a/index.ts",
                "src/a.ts",
                "a.ts",
                "ts",
                "é.ts",
                "x",
                "src/a/index.ts.bak",
                "c.ts",
            ]),
        ),
        pr_files(),
    ));
    let calls: Vec<Value> = cases
        .iter()
        .map(|(f, p)| json!({"fn": "resolveDiagFile", "args": [f, p]}))
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|(f, p)| json!(tsc_oracle::resolve_diag_file(*f, p.as_deref())))
        .collect();
    let js = oracle("TSC-ORACLE", TSC_EXPORTS, &[], &calls);
    assert_same("resolveDiagFile", &calls, &rust, &js);
}

// --------------------------------------------------------- REF-EVIDENCE-CHECK

const REF_EXPORTS: &[&str] = &["citedIdentifiers", "refEvidenceVerdict"];

// Verbatim from replay-ref-evidence.mjs.
const REF_SEEDS: &[&str] = &[
    "At HEAD the catch block checks `e instanceof RemoteOrderCountSyncError` and prints a distinct 'do NOT decrement the DB count again' message when only OpsHub failed.",
    "Every console.log/console.warn that references prodEmail passes it through `redactEmail()` imported from `./export-sanitize.ts`, which masks all but the first character and domain.",
    "An intra-batch dedupe pass (seenOrderIds Set, lines 616-635) already runs before createMany, pushing a success:false result for any duplicate orderId within the batch.",
    "existingOrderIds is computed only from the DB and never deduplicated against itself; OrderDetail.orderId is @unique in schema.prisma so skipDuplicates genuinely drops an intra-batch duplicate row.",
    "The OpsHub call can fail and BillingsHub is not involved; see the GitHub thread. TypeScript types are fine.",
    "Returning NOT_REFUTED with refutationType NO_REFUTATION, killReason other, slotStatus OK.",
];

#[test]
fn ref_evidence_check_matches_js() {
    let token = pick(vec![
        "redactEmail",
        "VendorSyncCountError",
        "export-sanitize.ts",
        "a.tsx",
        "a.ts.js",
        "foo.ts-bar",
        "schema.prisma",
        "x.prisma2",
        "refutationType",
        "GitHub",
        "BillingsHub",
        "OpsHub",
        "fooBarBa",
        "fooBarB",
        "_fooBarBaz",
        "9fooBarBaz",
        "éfooBarBaz",
        "fooBarBazé",
        "fooBarß",
        "\u{212A}elvinFooBar",
        "a/b/c.mts",
        "./x.cts",
        "-y.jsx",
        "x..ts",
        ".ts",
        "ab.TS",
        "über.ts",
        "fooßBarBaz",
        "ABCdefGHI",
        "abcDEF",
        "tsx.ts",
        "zzLongNameX",
        "aaLongNameX",
        "a_b.ts",
    ]);
    let sep = pick(vec![
        " ", "`", "(", ".", "/", "-", "_", "é", "\n", "", ",", "ſ", "😀",
    ]);
    let text = prop_oneof![
        8 => prop::collection::vec((token, sep), 0..8)
            .prop_map(|v| Some(json!(v.into_iter().map(|(t, s)| format!("{t}{s}")).collect::<String>()))),
        1 => pick(vec![Value::Null, json!(5), json!(["redactEmail", "x.ts"]), json!({"a": 1}), json!(true), json!(false), json!(0), json!("")]).prop_map(Some),
        1 => Just(None),
    ];
    let mut texts = sample(text);
    texts.extend(REF_SEEDS.iter().map(|s| Some(json!(s))));
    let calls: Vec<Value> = texts
        .iter()
        .map(|t| match t {
            Some(t) => json!({"fn": "citedIdentifiers", "args": [t]}),
            None => json!({"fn": "citedIdentifiers", "args": []}),
        })
        .collect();
    let rust: Vec<Value> = texts
        .iter()
        .map(|t| json!(ref_evidence_check::cited_identifiers(t.as_ref())))
        .collect();
    let js = oracle("REF-EVIDENCE-CHECK", REF_EXPORTS, &[], &calls);
    assert_same("citedIdentifiers", &calls, &rust, &js);

    let list = |items: Vec<Value>| {
        proptest::option::weighted(0.8, prop::collection::vec(pick(items), 0..4))
    };
    let cases = sample((
        any::<bool>(),
        list(vec![json!("seenOrderIds"), json!("newOrders"), json!("a")]),
        list(vec![
            json!("seenOrderIds"),
            Value::Null,
            json!(""),
            json!(0),
            json!(false),
            json!("x"),
        ]),
        any::<bool>(),
    ));
    let calls: Vec<Value> = cases
        .iter()
        .map(|(ran, cited, missing, null_absent)| {
            let absent = if *null_absent {
                Some(Value::Null)
            } else {
                None
            };
            let arg = object(vec![
                ("ran", Some(json!(ran))),
                ("cited", cited.clone().map(Value::Array).or(absent.clone())),
                ("missing", missing.clone().map(Value::Array).or(absent)),
            ]);
            json!({"fn": "refEvidenceVerdict", "args": [arg]})
        })
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|(ran, cited, missing, _)| {
            ref_evidence_check::ref_evidence_verdict(*ran, cited.as_deref(), missing.as_deref())
                .to_value()
        })
        .collect();
    let js = oracle("REF-EVIDENCE-CHECK", REF_EXPORTS, &[], &calls);
    assert_same("refEvidenceVerdict", &calls, &rust, &js);
}

// ------------------------------------------------------------ LINE-CORRECTION

#[test]
fn line_correction_matches_js() {
    let line = pick(vec![
        None,
        Some(json!(17)),
        Some(json!("17")),
        Some(json!(" 17 ")),
        Some(json!("0x11")),
        Some(json!("1.7e1")),
        Some(Value::Null),
        Some(json!([])),
        Some(json!([17])),
        Some(json!("")),
        Some(json!(true)),
        Some(json!(17.0)),
        Some(json!(17.5)),
        Some(json!("17abc")),
        Some(json!({})),
        Some(json!("\u{FEFF}17\u{3000}")),
        Some(json!("\u{0085}17")),
        Some(json!("Infinity")),
        Some(json!("inf")),
        Some(json!("0b10001")),
        Some(json!(0)),
    ]);
    let candidate = prop_oneof![
        8 => (any::<bool>(), line, any::<bool>(), any::<bool>()).prop_map(|(file_first, line, prior, index_key)| {
            let mut fields = Vec::new();
            if file_first {
                fields.push(("file", Some(json!("src/b.ts"))));
            }
            fields.push(("line", line));
            if prior {
                fields.push(("lineCorrectedFrom", Some(json!(3))));
            }
            if index_key {
                fields.push(("0", Some(json!("index key"))));
            }
            fields.push(("title", Some(json!("t"))));
            object(fields)
        }),
        1 => pick(vec![Value::Null, json!(0), json!(""), json!("17"), json!(17), json!([17]), json!(true)]),
    ];
    let odd = vec![
        None,
        Some(json!(41)),
        Some(json!(17)),
        Some(json!(0)),
        Some(json!(1)),
        Some(json!(-1)),
        Some(json!("41")),
        Some(json!(41.5)),
        Some(json!(41.0)),
        Some(Value::Null),
        Some(json!(1e300)),
    ];
    let corrected =
        prop_oneof![3 => Just(Some(json!(41))), 1 => Just(Some(json!(1))), 2 => pick(odd.clone())];
    let patch = prop_oneof![4 => Just(Some(json!(17))), 1 => Just(Some(json!(0))), 2 => pick(odd)];
    let resolution = prop_oneof![
        8 => (corrected, patch).prop_map(|(c, p)| object(vec![("correctedLine", c), ("patchLine", p), ("matchKind", Some(json!("exact")))])),
        1 => pick(vec![Value::Null, json!(0), json!(""), json!(5), json!("str"), json!(true), json!([])]),
    ];
    let mut cases: Vec<(Value, Value)> = sample((candidate, resolution));
    // Seeds from replay-line-correction.mjs.
    cases.extend([
        (
            json!({"file": "src/b.ts", "line": 17}),
            json!({"correctedLine": 41, "patchLine": 17}),
        ),
        (
            json!({"file": "src/b.ts", "line": "17"}),
            json!({"correctedLine": 41, "patchLine": 17}),
        ),
        (
            json!({"file": "src/b.ts", "line": 20}),
            json!({"correctedLine": 41, "patchLine": 17}),
        ),
        (
            json!({"file": "src/b.ts", "line": 17}),
            json!({"correctedLine": "41", "patchLine": 17}),
        ),
        (json!({"line": 17}), Value::Null),
    ]);
    let wrapper =
        "applyLineCorrectionState:a=>{const r=applyLineCorrection(...a);return a.concat([r])}";
    let calls: Vec<Value> = cases
        .iter()
        .map(|(c, r)| json!({"fn": "applyLineCorrectionState", "args": [[c, r]]}))
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|(c, r)| {
            // The candidate the JavaScript mutates came out of JSON.parse.
            let mut c = property_order_deep(c.clone());
            let applied = line_correction::apply_line_correction(&mut c, r);
            json!([c, r, applied])
        })
        .collect();
    let js = oracle(
        "LINE-CORRECTION",
        &["applyLineCorrection", wrapper],
        &[],
        &calls,
    );
    assert_same("applyLineCorrection", &calls, &rust, &js);
    let applied = rust.iter().filter(|v| v[2] == json!(true)).count();
    assert!(applied > 20, "too few corrections applied: {applied}");
}

// ----------------------------------------------------------- FETCH-PR-CONTEXT

fn review_entry() -> impl Strategy<Value = Value> {
    let obj = (
        prop_oneof![
            3 => Just(Some(json!("CHANGES_REQUESTED"))),
            2 => pick(vec![None, Some(json!("changes_requested")), Some(json!("APPROVED")), Some(Value::Null)]),
        ],
        pick(vec![None, Some(json!("")), Some(json!("old"))]),
        any::<bool>(),
    )
        .prop_map(|(state, summary, index_key)| {
            let mut f = vec![("author", Some(json!("alice"))), ("state", state), ("summary", summary)];
            if index_key {
                f.insert(0, ("7", Some(json!("seven"))));
            }
            object(f)
        });
    prop_oneof![8 => obj, 1 => pick(vec![Value::Null, json!("x"), json!(3), json!([])])]
}

#[test]
fn fetch_pr_context_matches_js() {
    let prior = pick(vec![
        None,
        Some(Value::Null),
        Some(json!("")),
        Some(json!("str")),
        Some(json!(5)),
        Some(json!({})),
        Some(json!(true)),
    ]);
    let reviews = prop_oneof![
        3 => prop::collection::vec(review_entry(), 0..5).prop_map(|v| Some(Value::Array(v))),
        1 => prior,
    ];
    let data = prop_oneof![
        8 => (reviews, any::<bool>(), any::<bool>(), any::<bool>()).prop_map(|(r, files_first, numeric, decisions)| {
            let mut f = Vec::new();
            if files_first {
                f.push(("files", Some(json!(["src/f1.ts"]))));
            }
            f.push(("priorReviews", r));
            if numeric {
                f.push(("10", Some(json!(1))));
                f.push(("2", Some(json!(2))));
            }
            if decisions {
                f.push(("inThreadDecisions", Some(json!(["stale"]))));
            }
            f.push(("headRefOid", Some(json!("abc"))));
            object(f)
        }),
        1 => pick(vec![Value::Null, json!("ab"), json!(["x", "y"]), json!(5), json!(true), json!(false)]),
    ];
    let summary_entry = prop_oneof![
        6 => (
            prop_oneof![
                3 => pick(vec![Some(json!(0)), Some(json!(1)), Some(json!(2))]),
                2 => pick(vec![Some(json!("1")), Some(json!(1.5)), Some(json!(1.0)), Some(json!(-1)), Some(json!(1e300)), None]),
            ],
            prop_oneof![
                2 => Just(Some(json!("racy"))),
                3 => pick(vec![Some(json!(5)), Some(Value::Null), Some(json!("")), Some(json!([1, [2, null]])), Some(json!({})), Some(json!(1e21)), Some(json!(1.5e-7)), None]),
            ],
        )
            .prop_map(|(i, s)| object(vec![("index", i), ("summary", s)])),
        1 => pick(vec![Value::Null, json!("x"), json!(3), json!(true)]),
    ];
    let summaries = prop_oneof![
        4 => prop::collection::vec(summary_entry, 0..4).prop_map(|v| Some(Value::Array(v))),
        1 => pick(vec![None, Some(Value::Null), Some(json!("abc")), Some(json!(5)), Some(json!({})), Some(json!(true)), Some(json!(""))]),
    ];
    let decisions = pick(vec![
        None,
        Some(json!([{"category": "BY-DESIGN", "summary": "the guard stays"}])),
        Some(json!("x")),
        Some(Value::Null),
        Some(json!({})),
    ]);
    let answer = prop_oneof![
        6 => (decisions, summaries).prop_map(|(d, s)| object(vec![("inThreadDecisions", d), ("priorReviewSummaries", s)])),
        1 => pick(vec![Value::Null, json!(""), json!("str"), json!(5), json!([]), json!(0)]),
    ];
    let cases = sample((data, answer));
    let calls: Vec<Value> = cases
        .iter()
        .map(|(d, a)| json!({"fn": "buildPrContext", "args": [d, a]}))
        .collect();
    let rust: Vec<Result<Value, String>> = cases
        .iter()
        .map(|(d, a)| {
            fetch_pr_context::build_pr_context(
                &property_order_deep(d.clone()),
                &property_order_deep(a.clone()),
            )
        })
        .collect();
    let js = oracle_raw("FETCH-PR-CONTEXT", &["buildPrContext"], &[], &calls);
    same_outcome("buildPrContext", &calls, &rust, &js);
    let summarized = rust
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .filter(|v| {
            v["priorReviews"].as_array().is_some_and(|a| {
                a.iter()
                    .any(|r| r.get("summary").is_some_and(|s| s != "" && s != "old"))
            })
        })
        .count();
    assert!(summarized > 20, "too few summaries landed: {summarized}");
}

// -------------------------------------------------------------- PRIOR-REVIEWS

const PRIOR_EXPORTS: &[&str] = &["openChangeRequests", "ownStandingReview", "postGate"];

fn raw_review() -> impl Strategy<Value = Value> {
    let author = pick(vec![
        Some(json!("alice")),
        Some(json!("Alice")),
        Some(json!(" alice ")),
        Some(json!("bob")),
        Some(json!("coderabbitai")),
        Some(json!("CodeRabbitAI")),
        Some(json!("x[bot]")),
        Some(json!("x[BOT] ")),
        Some(json!("renovate ")),
        Some(json!("ſomeone")),
        Some(json!("\u{212A}elvin")),
        Some(json!("kelvin")),
        Some(json!("ΟΔΟΣ")),
        Some(json!("οδος")),
        Some(json!("")),
        Some(json!("  ")),
        Some(json!(5)),
        Some(Value::Null),
        Some(json!(["alice"])),
        Some(json!(true)),
        None,
    ]);
    let state = pick(vec![
        Some(json!("CHANGES_REQUESTED")),
        Some(json!("changes_requested")),
        Some(json!("changes_requeſted")),
        Some(json!("APPROVED")),
        Some(json!("COMMENTED")),
        Some(json!("DISMISSED")),
        Some(json!("dismiſſed")),
        Some(json!("")),
        Some(Value::Null),
        None,
    ]);
    let at = pick(vec![
        Some(json!("2026-07-21T10:00:00Z")),
        Some(json!("2026-07-22T10:00:00Z")),
        Some(json!(" 2026-07-22T10:00:00Z")),
        Some(json!("")),
        Some(json!("\u{FF5E}")),
        Some(json!("\u{1F600}")),
        Some(json!(5)),
        Some(json!(10)),
        Some(Value::Null),
        None,
    ]);
    let summary = pick(vec![
        Some(json!("x")),
        Some(json!(" y ")),
        Some(Value::Null),
        Some(json!(5)),
        None,
    ]);
    prop_oneof![
        9 => (author, state, at, summary).prop_map(|(a, s, t, m)| object(vec![("author", a), ("state", s), ("at", t), ("summary", m)])),
        1 => pick(vec![Value::Null, json!("str"), json!(5), json!([])]),
    ]
}

fn viewer() -> impl Strategy<Value = Option<Value>> {
    pick(vec![
        Some(json!("alice")),
        Some(json!("ALICE")),
        Some(json!(" Alice ")),
        Some(json!("")),
        Some(Value::Null),
        None,
        Some(json!(5)),
        Some(json!("kelvin")),
        Some(json!("ΟΔΟΣ")),
        Some(json!("bob")),
        Some(json!("ſomeone")),
    ])
}

#[test]
fn prior_reviews_matches_js() {
    // The dense lists put one author's reviews on timestamps where UTF-16 order
    // and code-point order disagree, and on ties.
    let dense = (
        pick(vec!["alice", "Alice", " alice "]),
        pick(vec!["CHANGES_REQUESTED", "APPROVED", "COMMENTED"]),
        pick(vec!["\u{FF5E}", "\u{1F600}", "2026-07-21T10:00:00Z"]),
        pick(vec!["first", "second"]),
    )
        .prop_map(|(a, s, t, m)| json!({"author": a, "state": s, "at": t, "summary": m}));
    let raw = prop_oneof![
        6 => prop::collection::vec(raw_review(), 0..6).prop_map(Value::Array),
        3 => prop::collection::vec(dense, 1..5).prop_map(Value::Array),
        1 => pick(vec![Value::Null, json!({}), json!("x")]),
    ];
    let cases = sample((raw, viewer(), any::<bool>()));

    let calls: Vec<Value> = cases
        .iter()
        .map(|(r, _, _)| json!({"fn": "openChangeRequests", "args": [r]}))
        .collect();
    let opens: Vec<Vec<prior_reviews::Review>> = cases
        .iter()
        .map(|(r, _, _)| prior_reviews::open_change_requests(r))
        .collect();
    let rust: Vec<Value> = opens
        .iter()
        .map(|o| Value::Array(o.iter().map(|r| r.to_value()).collect()))
        .collect();
    let js = oracle("PRIOR-REVIEWS", PRIOR_EXPORTS, &[], &calls);
    assert_same("openChangeRequests", &calls, &rust, &js);
    assert!(opens.iter().filter(|o| !o.is_empty()).count() > 100);

    let calls: Vec<Value> = cases
        .iter()
        .map(|(r, v, _)| json!({"fn": "ownStandingReview", "args": [r, v]}))
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|(r, v, _)| {
            json!(prior_reviews::own_standing_review(r, v.as_ref()).map(|r| r.to_value()))
        })
        .collect();
    let js = oracle("PRIOR-REVIEWS", PRIOR_EXPORTS, &[], &calls);
    assert_same("ownStandingReview", &calls, &rust, &js);

    // postGate gets openChangeRequests' output, as in the workflow, and in a
    // third of the cases no raw list at all.
    let calls: Vec<Value> = cases
        .iter()
        .zip(&rust_open_values(&opens))
        .map(|((r, v, with_all), open)| {
            let mut args = vec![open.clone(), v.clone().unwrap_or(Value::Null)];
            if *with_all {
                args.push(r.clone());
            }
            json!({"fn": "postGate", "args": args})
        })
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .zip(&opens)
        .map(|((r, v, with_all), open)| {
            prior_reviews::post_gate(Some(open), v.as_ref(), with_all.then_some(r)).to_value()
        })
        .collect();
    let js = oracle("PRIOR-REVIEWS", PRIOR_EXPORTS, &[], &calls);
    assert_same("postGate", &calls, &rust, &js);
    let statuses: HashSet<String> = rust.iter().map(|g| g["status"].to_string()).collect();
    assert_eq!(
        statuses.len(),
        4,
        "every gate status must be generated: {statuses:?}"
    );
}

fn rust_open_values(opens: &[Vec<prior_reviews::Review>]) -> Vec<Value> {
    opens
        .iter()
        .map(|o| Value::Array(o.iter().map(|r| r.to_value()).collect()))
        .collect()
}

// --------------------------------------------------------- adversarial cases
//
// Hand-written inputs aimed at points the generators above do not reach or
// reach without discriminating: every Object.prototype member against a
// coverage set that names it, property keys built from non-string files,
// whitespace and line terminators outside the generated sets, and value
// coercions in the line, context and review paths.

fn d(file: &str, line: f64) -> TscDiagnostic {
    TscDiagnostic {
        file: file.into(),
        line,
        code: "TS2345".into(),
        message: "m".into(),
    }
}

#[test]
fn adversarial_tsc_oracle_cases() {
    let texts: Vec<Option<String>> = [
        "a(1,2): error TS1: x(3,4): error TS2: y",
        "a\u{000B}(1,2):\u{000B}error\u{000C}TS1:\u{1680}m\u{180E}",
        "a(1,2):\u{180E}error TS1: m",
        "a(1,2): error TS1:\u{205F}\u{202F}m\u{2000}",
        "a(1,2): error TS12345678901234567890: m",
        "((1,2)(3,4): error TS1: m",
        "a(9007199254740993,1): error TS1: m",
        "a(1,2): error TS1:  ",
        "a(1,2): error TS1:\u{2028}m",
        "a(1,2): error TS1: m\u{2029}",
        "a(1,2): error TS1: m\u{0085}",
        "\u{FEFF}a(1,2): error TS1: \u{FEFF}m\u{FEFF}",
        "a(1,2):\terror\tTS1:\tm\n\nb(3,4): error TS2: n\r\nc(5,6): error TS3: o",
        "a(1,2): error TS1:m",
        "a(1,2) : error TS1: m",
        "a(-1,2): error TS1: m",
        "a(1.5,2): error TS1: m",
        "a(1,2): error  TS1 : m",
    ]
    .iter()
    .map(|s| Some((*s).to_owned()))
    .collect();
    let calls: Vec<Value> = texts
        .iter()
        .map(|t| json!({"fn": "parseTscDiagnostics", "args": [t]}))
        .collect();
    let rust: Vec<Value> = texts
        .iter()
        .map(|t| diags_json(&tsc_oracle::parse_tsc_diagnostics(t.as_deref())))
        .collect();
    let js = oracle("TSC-ORACLE", TSC_EXPORTS, &[], &calls);
    assert_same("parseTscDiagnostics (adversarial)", &calls, &rust, &js);

    let texts = [
        "\u{180E}NO_TSCONFIG",
        "NO_TSCONFIG\u{000B}",
        "\u{000C}NO_DIAGNOSTICS\u{1680}",
        "xNO_TSCONFIG\nNO_TSCONFIG",
        "NO_DIAGNOSTICS\u{2029}x",
        "x\u{2029}NO_DIAGNOSTICS",
        "x \u{2029} NO_DIAGNOSTICS \u{2028} y",
        "NO_TSCONFIGNO_TSCONFIG",
        "NO_TSCONFIG_",
        "\u{FF09}: error TS1",
        "): error TS1",
        "): error  TS1",
        "):\terror TS1",
        "x): error TS9 DEGRADED_",
        "NO_DIAGNOSTICS\n): error TS1",
        "\u{205F}\u{3000}NO_TSCONFIG\u{202F}\r\n",
        "a NO_TSCONFIG\nNO_DIAGNOSTICS",
        "NO_DIAGNOSTICS\u{180E}",
        "no_tsconfig",
    ];
    let calls: Vec<Value> = texts
        .iter()
        .map(|t| json!({"fn": "classifyTypecheckText", "args": [t]}))
        .collect();
    let rust: Vec<Value> = texts
        .iter()
        .map(|t| json!(tsc_oracle::classify_typecheck_text(Some(t))))
        .collect();
    let js = oracle("TSC-ORACLE", TSC_EXPORTS, &[], &calls);
    assert_same("classifyTypecheckText (adversarial)", &calls, &rust, &js);

    // Every Object.prototype member, and index-like keys, through the group.
    let proto = [
        "constructor",
        "__defineGetter__",
        "__defineSetter__",
        "hasOwnProperty",
        "__lookupGetter__",
        "__lookupSetter__",
        "isPrototypeOf",
        "propertyIsEnumerable",
        "toString",
        "valueOf",
        "__proto__",
        "toLocaleString",
        "prototype",
        "length",
        "hasOwn",
        "toJSON",
    ];
    let mut groups: Vec<Vec<TscDiagnostic>> =
        proto.iter().map(|f| vec![d("a", 1.0), d(f, 2.0)]).collect();
    groups.push(
        [
            "4294967294",
            "0",
            "b",
            "00",
            "1",
            "4294967295",
            "-1",
            "1.0",
            "9",
            "a",
        ]
        .iter()
        .map(|f| d(f, 1.0))
        .collect(),
    );
    let calls: Vec<Value> = groups
        .iter()
        .map(|g| json!({"fn": "groupDiagnosticsByFile", "args": [diags_json(g)]}))
        .collect();
    let rust: Vec<Result<Value, String>> = groups
        .iter()
        .map(|g| tsc_oracle::group_diagnostics_by_file(Some(g)).map(|g| g.to_value()))
        .collect();
    let js = oracle_raw("TSC-ORACLE", TSC_EXPORTS, &[], &calls);
    same_outcome("groupDiagnosticsByFile (adversarial)", &calls, &rust, &js);

    // adjudicateFinding: each prototype member named by the coverage set, so
    // its `.length` decides the refute; files that are not strings, read as
    // property keys; and a coverage set holding the key's string form.
    let by = DiagsByFile::from_entries(vec![
        ("undefined".into(), vec![d("undefined", 10.0)]),
        ("null".into(), vec![d("null", 10.0)]),
        ("".into(), vec![d("", 10.0)]),
        ("true".into(), vec![d("true", 10.0)]),
        ("a,b".into(), vec![d("a,b", 10.0)]),
        ("[object Object]".into(), vec![d("[object Object]", 10.0)]),
        ("1e+21".into(), vec![d("1e+21", 10.0)]),
        ("0.5".into(), vec![d("0.5", 10.0)]),
        (
            "src/a.ts".into(),
            vec![d("src/a.ts", 1e21), d("src/a.ts", 10.0)],
        ),
    ]);
    let mut covered: Vec<String> = proto.iter().map(|s| (*s).to_owned()).collect();
    covered.extend(
        [
            "undefined",
            "null",
            "",
            "true",
            "a,b",
            "1e+21",
            "0.5",
            "src/b.ts",
        ]
        .map(String::from),
    );
    let files: Vec<Option<Value>> = proto
        .iter()
        .map(|f| Some(json!(f)))
        .chain([
            None,
            Some(Value::Null),
            Some(json!("")),
            Some(json!(true)),
            Some(json!(["a", "b"])),
            Some(json!({})),
            Some(json!(1e21)),
            Some(json!(0.5)),
            Some(json!([])),
            Some(json!([null])),
            Some(json!("src/a.ts")),
            Some(json!("src/b.ts")),
        ])
        .collect();
    let lines = [
        None,
        Some(json!(10)),
        Some(json!(15.000001)),
        Some(json!(1e21)),
        Some(json!(-0.0)),
        Some(json!(5e-324)),
    ];
    let mut cases: Vec<(Value, Option<Vec<String>>)> = Vec::new();
    for f in &files {
        for l in &lines {
            for cov in [Some(covered.clone()), Some(Vec::new()), None] {
                cases.push((
                    object(vec![
                        ("file", f.clone()),
                        ("line", l.clone()),
                        ("mechanicalClaim", Some(json!(true))),
                    ]),
                    cov,
                ));
            }
        }
    }
    let calls: Vec<Value> = cases
        .iter()
        .map(|(f, c)| json!({"fn": "adjudicateFindingSet", "args": [[f, by.to_value(), c]]}))
        .collect();
    let rust: Vec<Result<Value, String>> = cases
        .iter()
        .map(|(f, c)| {
            let set: Option<HashSet<String>> = c.clone().map(|c| c.into_iter().collect());
            tsc_oracle::adjudicate_finding(f, &by, set.as_ref()).map(|a| a.to_value())
        })
        .collect();
    let js = oracle_raw("TSC-ORACLE", TSC_EXPORTS, &[], &calls);
    same_outcome("adjudicateFinding (adversarial)", &calls, &rust, &js);

    // resolveDiagFile against an empty PR path, duplicates and substrings.
    let pr: Vec<Vec<String>> = vec![
        vec!["".into()],
        vec!["".into(), "src/a.ts".into()],
        vec!["src/a.ts".into(), "src/a.ts".into()],
        vec!["src/😀.ts".into(), "x/😀.ts".into()],
        vec!["a.ts".into(), "src/a.tsx".into()],
        vec!["src/a.ts".into()],
    ];
    let files = [
        "a.ts",
        "😀.ts",
        "src/a.ts",
        ".ts",
        "src/a.ts/x",
        "s",
        "\u{FEFF}a.ts",
    ];
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for p in &pr {
        for f in files {
            calls.push(json!({"fn": "resolveDiagFile", "args": [f, p]}));
            rust.push(json!(tsc_oracle::resolve_diag_file(Some(f), Some(p))));
        }
    }
    let js = oracle("TSC-ORACLE", TSC_EXPORTS, &[], &calls);
    assert_same("resolveDiagFile (adversarial)", &calls, &rust, &js);
}

#[test]
fn adversarial_ref_evidence_cases() {
    let texts = [
        "fooBar.ts and fooBarBaz.tsx and a.ts.ts and x.d.ts",
        "Foo.BarBaz.ts aB.ts camelCaseName\u{2028}otherCamelName",
        "fooBarBaz\tfooBarBaz fooBarBaz",
        "refutationTypeX xrefutationType refutationType.ts",
        "über.tsx überCamelCase CamelÜberCase",
        "a/b-c_d.prisma A.PRISMA a.prisma_ a.prisma.",
        "1fooBarBaz _fooBarBaz fooBarBaz_ fooBar_Baz",
        "ABCDEFGHIJ aBCDEFGH abcdefgH abcdefgHi",
        "-./x.ts ../y.js //z.jsx",
        "fooBarBaz\u{FEFF}quxQuuxCorge",
        "zzz.ts aaa.ts ZZZ.ts _a.ts 0a.ts",
        "aaaaaaaB",
        "aaaaaaB",
    ];
    let calls: Vec<Value> = texts
        .iter()
        .map(|t| json!({"fn": "citedIdentifiers", "args": [t]}))
        .collect();
    let rust: Vec<Value> = texts
        .iter()
        .map(|t| json!(ref_evidence_check::cited_identifiers(Some(&json!(t)))))
        .collect();
    let js = oracle("REF-EVIDENCE-CHECK", REF_EXPORTS, &[], &calls);
    assert_same("citedIdentifiers (adversarial)", &calls, &rust, &js);
}

#[test]
fn adversarial_line_correction_cases() {
    let lines = [
        json!([[17]]),
        json!(["0x11"]),
        json!([" 17 "]),
        json!([17, 1]),
        json!([null]),
        json!([[]]),
        json!("+17"),
        json!("-0x11"),
        json!("0X11"),
        json!("0o21"),
        json!("17."),
        json!(".17e2"),
        json!("1_7"),
        json!("\u{180E}17"),
        json!("\u{000B}17\u{000C}"),
        json!("\u{2028}17\u{2029}"),
        json!("+Infinity"),
        json!("infinity"),
        json!("0x"),
        json!("1e"),
        json!(-0.0),
        json!("-0"),
        json!(false),
    ];
    let resolutions = [
        json!({"correctedLine": 41, "patchLine": 17}),
        json!({"correctedLine": 41, "patchLine": 0}),
        json!({"correctedLine": 41, "patchLine": -0.0}),
        json!({"correctedLine": 1, "patchLine": 1e300}),
        json!({"correctedLine": 41, "patchLine": "17"}),
        json!({"patchLine": 17, "correctedLine": 1.0}),
    ];
    let wrapper =
        "applyLineCorrectionState:a=>{const r=applyLineCorrection(...a);return a.concat([r])}";
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for l in &lines {
        for r in &resolutions {
            let c = json!({"line": l, "1": "index", "lineCorrectedFrom": null});
            calls.push(json!({"fn": "applyLineCorrectionState", "args": [[c, r]]}));
            let mut c = property_order_deep(c);
            let applied = line_correction::apply_line_correction(&mut c, r);
            rust.push(json!([c, r, applied]));
        }
    }
    let js = oracle(
        "LINE-CORRECTION",
        &["applyLineCorrection", wrapper],
        &[],
        &calls,
    );
    assert_same("applyLineCorrection (adversarial)", &calls, &rust, &js);
}

#[test]
fn adversarial_fetch_pr_context_cases() {
    let data = json!({"priorReviews": [
        {"state": "CHANGES_REQUESTED", "summary": "old"},
        {"summary": "s", "state": "CHANGES_REQUESTED", "10": "x"},
        {"state": "CHANGES_REQUESTED"},
    ], "inThreadDecisions": 1, "3": true});
    let answers = [
        json!({"priorReviewSummaries": [{"index": -0.0, "summary": "neg zero"}]}),
        json!({"priorReviewSummaries": [{"index": 0.0, "summary": 0}, {"index": 0, "summary": "later"}]}),
        json!({"priorReviewSummaries": [{"index": 2, "summary": [[]]}, {"index": 1, "summary": [null, 1.5]}]}),
        json!({"priorReviewSummaries": "😀x", "inThreadDecisions": []}),
        json!({"priorReviewSummaries": [[], "x", 0, false], "inThreadDecisions": [null]}),
        json!({"priorReviewSummaries": [{"index": 9007199254740993_u64, "summary": "big"}]}),
        json!({"priorReviewSummaries": [{"index": 1, "summary": true}, {"index": 2, "summary": {}}]}),
    ];
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for a in &answers {
        calls.push(json!({"fn": "buildPrContext", "args": [data, a]}));
        rust.push(fetch_pr_context::build_pr_context(
            &property_order_deep(data.clone()),
            &property_order_deep(a.clone()),
        ));
    }
    let js = oracle_raw("FETCH-PR-CONTEXT", &["buildPrContext"], &[], &calls);
    same_outcome("buildPrContext (adversarial)", &calls, &rust, &js);
}

#[test]
fn adversarial_prior_reviews_cases() {
    let rv =
        |a: Value, s: &str, t: Value| json!({"author": a, "state": s, "at": t, "summary": "s"});
    let lists = vec![
        json!([
            rv(json!("x[bot]\u{0085}"), "CHANGES_REQUESTED", json!("t1")),
            rv(
                json!("github-act\u{0131}ons"),
                "CHANGES_REQUESTED",
                json!("t1")
            ),
            rv(json!("GITHUB-ACTIONS"), "CHANGES_REQUESTED", json!("t1")),
            rv(json!("dependabot[bot]"), "CHANGES_REQUESTED", json!("t1")),
            rv(json!("[bot]"), "CHANGES_REQUESTED", json!("t1")),
            rv(json!("renovate\u{FEFF}"), "CHANGES_REQUESTED", json!("t1")),
            rv(json!("renovate\u{180E}"), "CHANGES_REQUESTED", json!("t1")),
        ]),
        json!([
            rv(
                json!("alice"),
                "CHANGES_REQUESTED",
                json!("2026-07-21\u{FEFF}")
            ),
            rv(json!("ALICE"), "APPROVED", json!("\u{180E}2026")),
            rv(json!("\u{0130}van"), "CHANGES_REQUESTED", json!("a")),
            rv(json!("i\u{0307}van"), "COMMENTED", json!("b")),
            rv(json!("ΑΣ"), "CHANGES_REQUESTED", json!("a")),
            rv(json!("ας"), "COMMENTED", json!("b")),
            rv(json!(["al", "ice"]), "CHANGES_REQUESTED", json!(["2026"])),
            rv(json!("al,ice"), "APPROVED", json!(2027)),
        ]),
        json!([
            rv(json!("bob"), "changes_requested", json!("b")),
            rv(json!("bob"), "commented", json!("a")),
            rv(json!("bob"), "Dismissed", json!("z")),
            rv(json!("bob"), "APPROVED", json!(1e21)),
            rv(json!("bob"), "\u{0131}", json!("c")),
        ]),
    ];
    let viewers = [
        json!("\u{0130}van"),
        json!("i\u{0307}van"),
        json!("ας"),
        json!("ΑΣ"),
        json!("al,ice"),
        json!(["al", "ice"]),
        json!("bob\u{FEFF}"),
        json!("renovate"),
    ];
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for l in &lists {
        calls.push(json!({"fn": "openChangeRequests", "args": [l]}));
        rust.push(Value::Array(
            prior_reviews::open_change_requests(l)
                .iter()
                .map(|r| r.to_value())
                .collect(),
        ));
        for v in &viewers {
            calls.push(json!({"fn": "ownStandingReview", "args": [l, v]}));
            rust.push(json!(
                prior_reviews::own_standing_review(l, Some(v)).map(|r| r.to_value())
            ));
            let open = prior_reviews::open_change_requests(l);
            let open_v = Value::Array(open.iter().map(|r| r.to_value()).collect());
            calls.push(json!({"fn": "postGate", "args": [open_v, v, l]}));
            rust.push(prior_reviews::post_gate(Some(&open), Some(v), Some(l)).to_value());
        }
    }
    let js = oracle("PRIOR-REVIEWS", PRIOR_EXPORTS, &[], &calls);
    assert_same("prior reviews (adversarial)", &calls, &rust, &js);
}

/// Strings drawn piece by piece from a small alphabet, so the regex ports meet
/// shapes no token generator writes: half-formed heads, repeated groups and
/// terminators in every position.
fn char_soup(pieces: Vec<&'static str>, max: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(pick(pieces), 0..max).prop_map(|v| v.concat())
}

#[test]
fn char_level_regex_fuzz() {
    let texts = sample(char_soup(
        vec![
            "a",
            "(",
            ")",
            ",",
            ":",
            "1",
            "0",
            " ",
            "error",
            "TS",
            "TS1",
            "\r",
            "\u{2028}",
            "\t",
            "\u{FEFF}",
            "\u{180E}",
            "é",
            "😀",
            "\n",
            "e",
            "x(1,2): error TS1: ",
        ],
        24,
    ));
    let calls: Vec<Value> = texts
        .iter()
        .map(|t| json!({"fn": "parseTscDiagnostics", "args": [t]}))
        .collect();
    let rust: Vec<Value> = texts
        .iter()
        .map(|t| diags_json(&tsc_oracle::parse_tsc_diagnostics(Some(t))))
        .collect();
    let js = oracle("TSC-ORACLE", TSC_EXPORTS, &[], &calls);
    assert_same("parseTscDiagnostics (char soup)", &calls, &rust, &js);
    assert!(rust.iter().filter(|v| v != &&json!([])).count() > 30);

    let texts = sample(char_soup(
        vec![
            "NO_TSCONFIG",
            "NO_DIAGNOSTICS",
            " ",
            "\n",
            "\r",
            "\u{2028}",
            "\u{2029}",
            "x",
            "\u{0085}",
            "\u{180E}",
            "\u{FEFF}",
            "): error TS",
            "1",
            "DEGRADED_",
            "NO_",
            "\u{000B}",
        ],
        8,
    ));
    let calls: Vec<Value> = texts
        .iter()
        .map(|t| json!({"fn": "classifyTypecheckText", "args": [t]}))
        .collect();
    let rust: Vec<Value> = texts
        .iter()
        .map(|t| json!(tsc_oracle::classify_typecheck_text(Some(t))))
        .collect();
    let js = oracle("TSC-ORACLE", TSC_EXPORTS, &[], &calls);
    assert_same("classifyTypecheckText (char soup)", &calls, &rust, &js);
    let kinds: HashSet<String> = rust.iter().map(|v| v.to_string()).collect();
    assert_eq!(kinds.len(), 4, "{kinds:?}");

    let texts = sample(char_soup(
        vec![
            "a", "B", "z", "Q", "9", "_", ".", "/", "-", " ", "é", "ſ", "\u{212A}", "ts", "tsx",
            "prisma", ".ts", "fooBar", "Baz", "😀", "\n",
        ],
        20,
    ));
    let calls: Vec<Value> = texts
        .iter()
        .map(|t| json!({"fn": "citedIdentifiers", "args": [t]}))
        .collect();
    let rust: Vec<Value> = texts
        .iter()
        .map(|t| json!(ref_evidence_check::cited_identifiers(Some(&json!(t)))))
        .collect();
    let js = oracle("REF-EVIDENCE-CHECK", REF_EXPORTS, &[], &calls);
    assert_same("citedIdentifiers (char soup)", &calls, &rust, &js);
    assert!(rust.iter().filter(|v| v != &&json!([])).count() > 100);
}
