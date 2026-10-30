mod common;
use common::*;

// Differential tests for the lens group: each ported function and its
// JavaScript region run on the same generated inputs plus seeds from the
// replays, and any difference fails.
//
// Several region functions return a `Map` or a `Set`, which `JSON.stringify`
// writes as `{}`. The oracle builds its export list into the region's own
// scope (`return { <exports> }`), so an export can be a `name:arrow` wrapper
// that spreads the collection into an array; the region code runs unchanged.
//
// The JavaScript's side is read from `tests/goldens/oracle/diff_lens.json`
// (see `common`): the workflow is being retired, so a run starts only the
// port. The file keeps each call's results under a SHA-256 of its inputs and a
// SHA-256 of every region's text, so a region edited without regenerating
// fails here; with the workflow gone the golden is final. Regenerate:
// `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-core --test diff_lens`.

use proptest::prelude::*;
use serde_json::{json, Map, Value};

use tribunal_core::lens::{
    dependents_block, diff_sectioning, diff_usable, lens_budget, lens_prompt, lens_yield,
    out_of_diff,
};

fn pick(vals: Vec<Value>) -> impl Strategy<Value = Value> {
    prop::sample::select(vals)
}

/// `None` leaves the key out, which JavaScript reads as undefined.
fn maybe(p: f64, vals: Vec<Value>) -> impl Strategy<Value = Option<Value>> {
    prop::option::weighted(p, pick(vals))
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

fn strings(v: &[&str]) -> Vec<Value> {
    v.iter().map(|s| json!(s)).collect()
}

fn js_results(region: &str, exports: &[&str], calls: &[Value]) -> Vec<Value> {
    oracle(region, exports, &[], calls)
}

fn as_result(r: Result<Value, String>) -> Value {
    match r {
        Ok(v) => json!({ "ok": v }),
        Err(e) => json!({ "err": e }),
    }
}

/// Fails unless some output of `fn_name` satisfies `pred`: a branch the
/// generated inputs never reach is a branch the comparison never checked.
fn reach(
    calls: &[Value],
    outputs: &[Value],
    fn_name: &str,
    label: &str,
    pred: impl Fn(&Value) -> bool,
) {
    let hit = calls
        .iter()
        .zip(outputs)
        .any(|(c, o)| c["fn"] == fn_name && pred(o));
    assert!(hit, "{fn_name}: no generated case reaches `{label}`");
}

fn text_of(v: &Value) -> &str {
    v["ok"].as_str().unwrap_or("")
}

// ---------------------------------------------------------------- DIFF-SECTIONING

fn diff_line() -> impl Strategy<Value = String> {
    let long_quoted = format!("diff --git \"a/{}😀😀\" b/x", "x".repeat(105));
    let long_quoted_even = format!("diff --git \"a/{}😀😀\" b/x", "x".repeat(104));
    prop::sample::select(vec![
        "diff --git a/src/a.ts b/src/a.ts".to_owned(),
        "diff --git a/src/b.ts b/src/b.ts".to_owned(),
        "diff --git a/src/old.ts b/src/new.ts".to_owned(),
        "diff --git a/a b/c b/d".to_owned(),
        "diff --git a/x b/".to_owned(),
        "diff --git a/ b/y".to_owned(),
        "diff --git a/x b/ b/".to_owned(),
        "diff --git \"a/s p.ts\" \"b/s p.ts\"".to_owned(),
        "diff --git a/é😀.ts b/é😀.ts".to_owned(),
        "diff --git a/x\u{2028}y b/x\u{2028}y".to_owned(),
        "diff --git a/q\u{85} b/q\u{85}".to_owned(),
        long_quoted,
        long_quoted_even,
        "diffx --git a/q b/q".to_owned(),
        " diff --git a/q b/q".to_owned(),
        "+ diff --git a/fake.ts b/fake.ts".to_owned(),
        "index 1111111..2222222 100644".to_owned(),
        "--- a/src/a.ts".to_owned(),
        "+++ b/src/a.ts".to_owned(),
        "@@ -1,3 +1,4 @@".to_owned(),
        "@@ -10 +11,2 @@ fn x()".to_owned(),
        "@@ -1,x +2 @@".to_owned(),
        "@@ -5 +007 @@".to_owned(),
        "@@ -1 +9007199254740991 @@".to_owned(),
        "@@ -1 +2,3@@".to_owned(),
        "@@ -1 +٣ @@".to_owned(),
        "+const a = 1;".to_owned(),
        "+const b = 2;".to_owned(),
        "-old".to_owned(),
        " ctx".to_owned(),
        "\\ No newline at end of file".to_owned(),
        "+++y".to_owned(),
        "+é😀".to_owned(),
        String::new(),
    ])
}

fn diff_text() -> impl Strategy<Value = Option<String>> {
    let body =
        prop::collection::vec((diff_line(), prop::bool::weighted(0.15)), 0..14).prop_map(|parts| {
            parts
                .into_iter()
                .map(|(l, cr)| if cr { format!("{l}\r") } else { l })
                .collect::<Vec<_>>()
                .join("\n")
        });
    prop::option::weighted(0.95, body)
}

fn fence_text() -> impl Strategy<Value = Option<String>> {
    let piece = prop::sample::select(vec![
        "```",
        "```json",
        "```JSON",
        "``",
        "\n",
        "\r\n",
        "{\"a\":1}",
        "a ``` b",
        " ",
        "\u{FEFF}",
        "\u{85}",
        "\u{3000}",
        "é😀",
        "x",
    ]);
    prop::option::weighted(
        0.95,
        prop::collection::vec(piece, 0..8).prop_map(|v| v.concat()),
    )
}

fn prose() -> impl Strategy<Value = Value> {
    let token = prop::sample::select(vec![
        "src/a.ts",
        "a.tsx.b",
        "orders_worker.ts",
        "x.ts.bak",
        "y.json",
        "a.tsé",
        "a.tsx_",
        "schema.prisma",
        "file.mts",
        ".ts",
        "a..ts",
        "made-up.ts",
        "src/jobs/orders_worker.ts",
        "./b.ts",
        "path/to/c.sql",
        "b.ts",
        "x.TS",
        "q.jsx",
        "r.js-",
        "s.cts9",
        " ",
        ", ",
        "é",
        "😀",
        "(",
        "\n",
    ]);
    let text = prop::collection::vec(token, 0..7).prop_map(|v| json!(v.concat()));
    prop_oneof![
        8 => text,
        1 => pick(vec![json!(5), json!(true), json!(0), json!(["a.ts", "b.ts"]), json!({"a": 1}), Value::Null]),
    ]
}

fn known_files() -> impl Strategy<Value = Option<Vec<String>>> {
    let pool = prop::sample::select(vec![
        "src/a.ts",
        "src/jobs/orders_worker.ts",
        "prisma/schema.prisma",
        "a.tsx",
        "y.json",
        "b.ts",
        "",
        "x.ts",
        "path/to/c.sql",
        "src/b.ts",
    ]);
    prop::option::weighted(
        0.9,
        prop::collection::vec(pool, 0..6).prop_map(|v| v.into_iter().map(String::from).collect()),
    )
}

fn anchor_file() -> Vec<Value> {
    strings(&[
        "./b.ts",
        " src/x.ts ",
        "/etc/passwd",
        "../up.ts",
        "src/../../up.ts",
        "src/has space.ts",
        "",
        "a.ts",
        "src/é.ts",
        "\u{FEFF}c.ts",
        "x.ts\n",
        "b.ts",
        "././d.ts",
        "src/a.ts",
        "e.ts",
        "f.ts",
        "g.ts",
        ".\\h.ts",
        "x.ts\u{85}",
    ])
    .into_iter()
    .chain([json!(7), Value::Null])
    .collect()
}

fn ds_anchor() -> impl Strategy<Value = Value> {
    let object = (
        maybe(0.9, anchor_file()),
        maybe(0.5, vec![json!(1), json!("2")]),
    )
        .prop_map(|(f, l)| obj(vec![("file", f), ("line", l)]));
    prop_oneof![8 => object, 1 => pick(vec![Value::Null, json!("b.ts"), json!(0)])]
}

fn ds_finding() -> impl Strategy<Value = Value> {
    let verified_by = prop_oneof![
        5 => prop::collection::vec(ds_anchor(), 0..7).prop_map(Value::Array),
        1 => pick(vec![json!("x.ts"), Value::Null]),
    ];
    (
        maybe(
            0.9,
            strings(&["a.ts", "src/a.ts", "b.ts", "orders_worker.ts", "y.json"])
                .into_iter()
                .chain([json!(5)])
                .collect(),
        ),
        prop::option::weighted(0.9, prose()),
        prop::option::weighted(0.8, prose()),
        prop::option::weighted(0.7, verified_by),
    )
        .prop_map(|(file, d, e, v)| {
            obj(vec![
                ("file", file),
                ("description", d),
                ("evidence", e),
                ("verifiedBy", v),
            ])
        })
}

fn run_value() -> impl Strategy<Value = Value> {
    let n = || {
        pick(vec![
            json!(2),
            json!(10),
            json!(12),
            json!("5"),
            json!(" 3 "),
            Value::Null,
            json!([11]),
            json!(2.5),
        ])
    };
    prop_oneof![
        6 => (n(), prop::option::weighted(0.85, n())).prop_map(|(a, b)| match b {
            Some(b) => json!([a, b]),
            None => json!([a]),
        }),
        1 => pick(vec![json!("12"), json!("1"), json!([[5], [12]])]),
    ]
}

fn section_entry() -> impl Strategy<Value = Value> {
    let added = prop_oneof![
        5 => prop::collection::vec(run_value(), 0..4).prop_map(Value::Array),
        1 => pick(vec![json!("x"), Value::Null]),
    ];
    let section = prop_oneof![
        6 => prop::option::weighted(0.9, added).prop_map(|a| obj(vec![("added", a)])),
        1 => pick(vec![Value::Null, json!("x"), json!(0)]),
    ];
    (
        pick(strings(&["src/a.ts", "src/b.ts", "src/c.ts"])),
        section,
    )
        .prop_map(|(f, s)| json!([f, s]))
}

fn location() -> impl Strategy<Value = Value> {
    let line = vec![
        json!(11),
        json!(3),
        json!(2),
        json!(12),
        json!("11"),
        json!(" 2 "),
        Value::Null,
        json!("abc"),
        json!(2.5),
        json!(true),
        json!([11]),
        json!("0xB"),
        json!(""),
    ];
    let object = (
        maybe(
            0.9,
            strings(&["src/a.ts", "src/b.ts", "src/c.ts", "src/d.ts"])
                .into_iter()
                .chain([json!(1)])
                .collect(),
        ),
        maybe(0.9, line),
    )
        .prop_map(|(f, l)| obj(vec![("file", f), ("line", l)]));
    prop_oneof![8 => object, 1 => pick(vec![Value::Null, json!("src/a.ts")])]
}

fn evidence_resolution() -> impl Strategy<Value = Value> {
    // Mostly a resolution that vetoes on its own, so the one field that varies
    // (often `ambiguousCount` around its threshold) decides the outcome.
    let object = (
        maybe(
            0.95,
            vec![
                json!(true),
                json!(true),
                json!(true),
                json!(1),
                json!(0),
                json!(false),
            ],
        ),
        maybe(
            0.95,
            strings(&["added", "added", "added", "context", "ADDED"]),
        ),
        maybe(
            0.5,
            strings(&["", "src/a.ts", "src/a.ts", "src/b.ts"])
                .into_iter()
                .chain([json!(5)])
                .collect(),
        ),
        maybe(
            0.9,
            vec![
                json!(1),
                json!(2),
                json!(2),
                json!(3),
                json!("2"),
                json!(" 2 "),
                json!("1"),
                Value::Null,
                json!([2]),
                json!("x"),
                json!(true),
                json!(1.5),
            ],
        ),
    )
        .prop_map(|(l, k, f, a)| {
            obj(vec![
                ("evidenceLocated", l),
                ("matchedLineKind", k),
                ("resolvedFile", f),
                ("ambiguousCount", a),
            ])
        });
    prop_oneof![6 => object, 1 => pick(vec![Value::Null, json!("x")])]
}

fn located_finding() -> impl Strategy<Value = Value> {
    (
        location(),
        prop::option::weighted(0.7, evidence_resolution()),
        prop::option::weighted(
            0.5,
            prop_oneof![
                4 => prop::collection::vec(location(), 0..4).prop_map(Value::Array),
                1 => Just(Value::Null),
            ],
        ),
    )
        .prop_map(|(loc, r, m)| {
            let mut f = match loc {
                Value::Object(o) => o,
                _ => Map::new(),
            };
            if let Some(r) = r {
                f.insert("evidenceResolution".into(), r);
            }
            if let Some(m) = m {
                f.insert("mergedLocations".into(), m);
            }
            Value::Object(f)
        })
}

fn sections_map(entries: &Value) -> Map<String, Value> {
    let mut m = Map::new();
    for e in entries.as_array().unwrap() {
        m.insert(e[0].as_str().unwrap().to_owned(), e[1].clone());
    }
    m
}

const DS_EXPORTS: &[&str] = &[
    "stripCodeFence",
    "splitJ:(d) => ((r) => [[...r.sections], r.unparsed.map((s) => s.toWellFormed())])(splitDiffByFile(d))",
    "addedLinesJ:(s) => [...addedLines(s)]",
    "addedRanges",
    "citedFiles",
    "verifiedFiles",
    "externalScopeLines",
    "anyLocJ:(m, f) => anyLocationAdded(new Map(m), f)",
    "VERIFIED_FILES_CAP_J:() => VERIFIED_FILES_CAP",
];

fn opt_str(s: &Option<String>) -> Value {
    s.as_deref().map_or(Value::Null, |s| json!(s))
}

#[test]
fn diff_sectioning_matches_js() {
    let mut calls = Vec::new();
    let mut rust = Vec::new();

    let mut fences = sample(fence_text());
    fences.extend([
        Some(
            "```json\n{\"ran\":true,\"results\":[{\"id\":0,\"matchKind\":\"not-found\"}]}\n```"
                .to_owned(),
        ),
        Some("\n\n  ```json\n{\"a\":1}\n```  \n".to_owned()),
        Some("```json\n{\"s\":\"a ``` b\"}\n```".to_owned()),
        Some("```json".to_owned()),
        Some("````".to_owned()),
        Some("`````".to_owned()),
        Some("```\n````".to_owned()),
        Some("```\nbody````\n".to_owned()),
        Some("```js\r\n{}\r\n```\r\n".to_owned()),
        None,
    ]);
    for t in &fences {
        calls.push(json!({"fn": "stripCodeFence", "args": [opt_str(t)]}));
        rust.push(json!(diff_sectioning::strip_code_fence(t.as_deref())));
    }

    let diffs = sample(diff_text());
    for d in &diffs {
        calls.push(json!({"fn": "splitJ", "args": [opt_str(d)]}));
        let s = diff_sectioning::split_diff_by_file(d.as_deref());
        let pairs: Vec<Value> = s.sections.iter().map(|(p, t)| json!([p, t])).collect();
        rust.push(json!([pairs, s.unparsed]));
        calls.push(json!({"fn": "addedLinesJ", "args": [opt_str(d)]}));
        rust.push(json!(diff_sectioning::added_lines(d.as_deref())));
        calls.push(json!({"fn": "addedRanges", "args": [opt_str(d)]}));
        rust.push(json!(diff_sectioning::added_ranges(d.as_deref())));
    }

    let pr2185 = json!({
        "file": "src/services/v2/syncResults.ts", "line": 429,
        "description": "After a normal CREATE_REMOTE_RECORD job, GET /sync-results remains empty.",
        "evidence": "prisma.syncResults.findMany({",
        "verifiedBy": [
            {"file": "src/jobs/sync_worker.ts", "line": 65, "quote": "case SyncJobNames.CREATE_REMOTE_RECORD:"},
            {"file": "src/jobs/services/remote.job.ts", "line": 18, "quote": "x"}
        ]
    });
    let partner = json!({
        "file": "src/controllers/v2/partner.ts",
        "description": "The worker's 'failed' listener in orders_worker.ts flips the row; see schema.prisma.",
        "evidence": "if (isFinalAttempt) { ... }",
        "verifiedBy": [{"file": "src/jobs/orders_worker.ts", "line": 3}]
    });
    let known_seed: Vec<String> = [
        "src/controllers/v2/partner.ts",
        "src/jobs/orders_worker.ts",
        "prisma/schema.prisma",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let mut findings: Vec<(Value, Option<Vec<String>>)> = sample((ds_finding(), known_files()));
    findings.push((pr2185, Some(vec!["src/services/v2/syncResults.ts".into()])));
    findings.push((partner, Some(known_seed)));
    for (f, known) in &findings {
        let k = known.as_ref().map_or(Value::Null, |k| json!(k));
        let ks = known.as_deref();
        calls.push(json!({"fn": "citedFiles", "args": [f, k]}));
        rust.push(json!(diff_sectioning::cited_files(f, ks)));
        calls.push(json!({"fn": "verifiedFiles", "args": [f]}));
        rust.push(json!(diff_sectioning::verified_files(f)));
        calls.push(json!({"fn": "externalScopeLines", "args": [f, k]}));
        rust.push(json!(diff_sectioning::external_scope_lines(f, ks)));
    }

    let mut located = sample((
        prop::collection::vec(section_entry(), 0..4),
        located_finding(),
    ));
    located.push((
        vec![json!(["src/a.ts", {"added": [[2, 2], [10, 12]]}])],
        json!({"file": "src/a.ts", "line": 3, "mergedLocations": [{"file": "src/a.ts", "line": 3}, {"file": "src/a.ts", "line": 2}]}),
    ));
    // Radix literals past 128 bits, where Number() rounds once to nearest-even
    // and a digit-by-digit fold rounds at every step. Each bound is the value
    // node gives, so a fold that lands one ulp off misses the run.
    let radix_lines = [
        (
            format!("0x1{}8{}1", "0".repeat(13), "0".repeat(19)),
            8.711228593176027e40,
        ),
        (
            format!("0b1{}1{}1", "0".repeat(52), "0".repeat(80)),
            2.1778071482940066e40,
        ),
        (
            format!("0o1{}4{}1", "0".repeat(17), "0".repeat(30)),
            1.7840596158824502e44,
        ),
        (
            format!("0x1{}8{}", "0".repeat(13), "0".repeat(20)),
            8.711228593176025e40,
        ),
        (
            format!("0b{}{}1", "1".repeat(54), "0".repeat(80)),
            4.3556142965880123e40,
        ),
        (
            format!("0x1{}7{}", "0".repeat(13), "f".repeat(30)),
            9.578097130411805e52,
        ),
        (format!("0o{}1", "7".repeat(40)), 1.0633823966279327e37),
        (format!("0X{}", "f".repeat(300)), f64::MAX),
    ];
    // StringToNumber's grammar edges. A bound of NaN stands for a string node
    // reads as NaN or Infinity: only the all-finite run is tried for it.
    let decimal_lines: [(&str, f64); 30] = [
        ("5.", 5.0),
        ("1.e5", 100_000.0),
        ("+.5", 0.5),
        ("-.5e-3", -0.0005),
        ("1_0", f64::NAN),
        (" \u{2028}7\u{FEFF} ", 7.0),
        ("Infinity", f64::NAN),
        ("-Infinity", f64::NAN),
        ("+Infinity", f64::NAN),
        ("infinity", f64::NAN),
        ("1e1000", f64::NAN),
        ("0b", f64::NAN),
        ("0o8", f64::NAN),
        ("00x1", f64::NAN),
        ("0.0000001e7", 1.0),
        ("1e-400", 0.0),
        ("0x1p3", f64::NAN),
        ("12abc", f64::NAN),
        ("\u{a0}12\u{3000}", 12.0),
        ("1,5", f64::NAN),
        ("0012.50", 12.5),
        ("9007199254740993", 9_007_199_254_740_992.0),
        ("1e+2", 100.0),
        ("1e2.5", f64::NAN),
        (".", f64::NAN),
        ("+", f64::NAN),
        ("0X1f", 31.0),
        ("0B11", 3.0),
        ("0O17", 15.0),
        ("-0x10", f64::NAN),
    ];
    for (line, bound) in decimal_lines {
        if !bound.is_nan() {
            located.push((
                vec![json!(["src/a.ts", {"added": [[bound, bound]]}])],
                json!({"file": "src/a.ts", "line": line}),
            ));
        }
        located.push((
            vec![json!(["src/a.ts", {"added": [[-1e308, 1e308]]}])],
            json!({"file": "src/a.ts", "line": line}),
        ));
    }
    for (line, bound) in &radix_lines {
        located.push((
            vec![json!(["src/a.ts", {"added": [[bound, bound]]}])],
            json!({"file": "src/a.ts", "line": line}),
        ));
        located.push((
            vec![json!(["src/a.ts", {"added": [[bound, 1e308]]}])],
            json!({"file": "src/a.ts", "line": line}),
        ));
    }
    for (entries, f) in &located {
        let entries = Value::Array(entries.clone());
        calls.push(json!({"fn": "anyLocJ", "args": [entries, f]}));
        rust.push(json!(diff_sectioning::any_location_added(
            &sections_map(&entries),
            f
        )));
    }

    calls.push(json!({"fn": "VERIFIED_FILES_CAP_J", "args": []}));
    rust.push(json!(diff_sectioning::VERIFIED_FILES_CAP));

    let js = js_results("DIFF-SECTIONING", DS_EXPORTS, &calls);
    assert_same("DIFF-SECTIONING", &calls, &rust, &js);
    let arr_len = |v: &Value| v.as_array().map_or(0, Vec::len);
    reach(&calls, &rust, "splitJ", "an unparsed header", |v| {
        arr_len(&v[1]) > 0
    });
    reach(
        &calls,
        &rust,
        "splitJ",
        "a path split across two headers",
        |v| {
            v[0].as_array().is_some_and(|ps| {
                ps.iter().any(|p| {
                    p[1].as_str()
                        .is_some_and(|t| t.matches("diff --git ").count() > 1)
                })
            })
        },
    );
    reach(
        &calls,
        &rust,
        "addedRanges",
        "a run longer than one line",
        |v| {
            v.as_array()
                .is_some_and(|rs| rs.iter().any(|r| r[0] != r[1]))
        },
    );
    reach(&calls, &rust, "citedFiles", "a cited file", |v| {
        arr_len(v) > 0
    });
    reach(&calls, &rust, "verifiedFiles", "the anchor cap", |v| {
        arr_len(v) == 4
    });
    reach(
        &calls,
        &rust,
        "externalScopeLines",
        "both scope lines",
        |v| arr_len(v) == 3,
    );
    reach(&calls, &rust, "anyLocJ", "an added location", |v| {
        *v == json!(true)
    });
    reach(&calls, &rust, "anyLocJ", "no added location", |v| {
        *v == json!(false)
    });
}

// ---------------------------------------------------------------- DIFF-USABLE

fn usable_diff() -> impl Strategy<Value = Option<String>> {
    // Each banner line is enough to reject a diff, so they are rare: the
    // threshold cases need diffs that get past them.
    let banner = prop::sample::select(vec![
        "  Output too large",
        "\u{FEFF}Full diff available at: https://x",
        "\u{85}Output too large",
        "Preview (first 2KB)",
    ]);
    let benign = prop::sample::select(vec![
        "diff --git a/a.ts b/a.ts",
        "diff --git a/a.ts b/a.ts",
        "x Output too large",
        "Full diff available at",
        "Preview (first",
        "+const x = 1;",
        "😀😀😀😀😀😀😀😀😀😀",
        "context line here",
        " diff --git a/x b/x",
        "diff --git a/b.ts",
        "\t",
        "",
        "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    ]);
    let line = prop_oneof![40 => benign, 1 => banner];
    let term = prop::sample::select(vec![
        "\n", "\n", "\n", "\r", "\r\n", "\u{2028}", "\u{2029}", "\u{85}", " ",
    ]);
    prop::option::weighted(
        0.95,
        prop::collection::vec((line, term), 0..40)
            .prop_map(|v| v.into_iter().map(|(l, t)| format!("{l}{t}")).collect()),
    )
}

fn churn_count() -> impl Strategy<Value = Option<f64>> {
    prop::option::weighted(
        0.85,
        prop::sample::select(vec![
            0.0, 1.0, 10.0, 24.0, 25.0, 49.0, 50.0, 100.0, 1000.0, 2.5, -10.0,
        ]),
    )
}

#[test]
fn diff_usable_matches_js() {
    let cases = sample((usable_diff(), churn_count(), churn_count()));
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for (d, a, del) in &cases {
        calls.push(json!({"fn": "diffIsUsable", "args": [opt_str(d), a, del]}));
        rust.push(json!(diff_usable::diff_is_usable(d.as_deref(), *a, *del)));
    }
    let js = js_results("DIFF-USABLE", &["diffIsUsable"], &calls);
    assert_same("diffIsUsable", &calls, &rust, &js);
    let usable = rust.iter().filter(|v| **v == json!(true)).count();
    assert!(
        usable > cases.len() / 20 && usable < cases.len() * 19 / 20,
        "the generator should reach both outcomes, got {usable} usable of {}",
        cases.len()
    );
}

// ---------------------------------------------------------------- OUT-OF-DIFF

// The PR's files: the changed helper nearly always, so a caller of it can be
// tied back; "" (which every file ends with) only rarely.
fn pr_files() -> impl Strategy<Value = Vec<String>> {
    let extra = prop_oneof![
        20 => prop::sample::select(vec![
            "src/helpers/client.test.ts",
            "src/a.ts",
            "é😀.ts",
            "helpers/client.ts",
        ]),
        1 => Just(""),
    ];
    (
        prop::bool::weighted(0.85),
        prop::collection::vec(extra, 0..3),
    )
        .prop_map(|(helper, extra)| {
            let mut v: Vec<String> = Vec::new();
            if helper {
                v.push("src/helpers/client.ts".into());
            }
            v.extend(extra.into_iter().map(String::from));
            v
        })
}

// Out-of-diff paths dominate, next to paths that are a suffix, a prefix or
// only a substring of a PR file, so every clause of fileInDiff decides some
// case.
fn candidate_file() -> Vec<Value> {
    strings(&[
        "src/services/accounts.ts",
        "src/services/accounts.ts",
        "services/accounts.ts",
        "accounts.ts",
        "src/other.ts",
        "src/helpers",
        "helpers/client",
        "elpers/cli",
        "src/helpers/client.ts",
        "client.ts",
        "x/src/helpers/client.ts",
        "",
        "src/services/accounts.tsx",
        "é😀.ts",
    ])
    .into_iter()
    .chain([Value::Null])
    .collect()
}

fn ood_anchor() -> impl Strategy<Value = Value> {
    let object = (
        maybe(
            0.9,
            candidate_file().into_iter().chain([json!(5)]).collect(),
        ),
        maybe(
            0.8,
            vec![
                json!(80),
                json!(3.0),
                json!(3.5),
                json!("80"),
                json!(-1),
                Value::Null,
            ],
        ),
    )
        .prop_map(|(f, l)| obj(vec![("file", f), ("line", l), ("quote", Some(json!("q")))]));
    prop_oneof![6 => object, 1 => pick(vec![Value::Null, json!("src/helpers/client.ts")])]
}

fn blast_radius() -> impl Strategy<Value = Option<Value>> {
    let symbols = pick(vec![
        json!(["isNotFound"]),
        json!(["isNotFound", "f"]),
        json!(["f"]),
        json!("fg"),
        json!([5, "isNotFound"]),
        json!([]),
    ]);
    let changed_entry = prop_oneof![
        8 => (
            maybe(0.95, strings(&["src/helpers/client.ts", "src/helpers/client.ts", "src/x.ts", "", "helpers/client"])),
            prop::option::weighted(0.95, symbols),
        )
            .prop_map(|(f, s)| obj(vec![("file", f), ("symbols", s)])),
        1 => Just(Value::Null),
        1 => Just(json!("ab")),
    ];
    let changed = prop_oneof![
        8 => prop::collection::vec(changed_entry, 1..3).prop_map(Value::Array),
        1 => pick(vec![json!("fg"), Value::Null, json!([])]),
    ];
    let caller = prop_oneof![
        8 => (
            maybe(0.95, candidate_file().into_iter().chain([json!(5)]).collect()),
            maybe(0.95, vec![json!("isNotFound"), json!("isNotFound"), json!("isNotFound -> x"), json!("f"), json!("g"), json!(5), json!("")]),
        )
            .prop_map(|(f, v)| obj(vec![("file", f), ("via", v), ("line", Some(json!(1)))])),
        1 => Just(Value::Null),
    ];
    let callers = prop_oneof![
        8 => prop::collection::vec(caller, 1..4).prop_map(Value::Array),
        1 => pick(vec![json!("x"), Value::Null, json!([])]),
    ];
    prop::option::weighted(
        0.85,
        prop_oneof![
            8 => (prop::option::weighted(0.95, changed), prop::option::weighted(0.95, callers))
                .prop_map(|(ch, ca)| obj(vec![("changed", ch), ("callers", ca)])),
            1 => pick(vec![Value::Null, json!(0), json!("x")]),
        ],
    )
}

fn ood_candidate() -> impl Strategy<Value = Value> {
    (
        any::<bool>(),
        maybe(0.9, candidate_file()),
        maybe(
            0.7,
            vec![json!(344), Value::Null, json!(3.5), json!("12"), json!(0)],
        ),
        prop::option::weighted(
            0.6,
            prop_oneof![
                5 => prop::collection::vec(ood_anchor(), 0..3).prop_map(Value::Array),
                1 => pick(vec![json!("x"), Value::Null]),
            ],
        ),
        any::<bool>(),
    )
        .prop_map(|(title, file, line, vb, reanchored)| {
            obj(vec![
                ("title", title.then(|| json!("caller now throws"))),
                ("file", file),
                ("line", line),
                ("severity", Some(json!("important"))),
                ("verifiedBy", vb),
                ("reanchored", reanchored.then(|| json!("earlier"))),
            ])
        })
}

/// The #2117 shape, kept coherent so the dependents path is reached: a
/// candidate filed at an unchanged caller, no anchor in the diff, and a relay
/// whose caller entry names that file in some suffix relation and a `via`
/// whose root is a changed symbol.
fn dependents_scenario() -> impl Strategy<Value = (Value, Vec<String>, Option<Value>)> {
    let accounts = || {
        pick(strings(&[
            "src/services/accounts.ts",
            "services/accounts.ts",
            "accounts.ts",
            "x/src/services/accounts.ts",
            "src/services/accounts.tsx",
            "src/other.ts",
        ]))
    };
    (
        accounts(),
        accounts(),
        pick(strings(&[
            "isNotFound",
            "isNotFound -> wrap",
            "other",
            "isNotFound->x",
        ])),
        pick(strings(&[
            "src/helpers/client.ts",
            "helpers/client.ts",
            "src/x.ts",
            "",
        ])),
        maybe(0.5, vec![json!(344), Value::Null]),
        prop::option::weighted(
            0.3,
            prop::collection::vec(ood_anchor(), 0..2).prop_map(Value::Array),
        ),
    )
        .prop_map(|(file, caller, via, owner, line, vb)| {
            let c = obj(vec![
                ("title", Some(json!("caller now throws"))),
                ("file", Some(file)),
                ("line", line),
                ("verifiedBy", vb),
            ]);
            let br = json!({
                "changed": [{"file": owner, "symbols": ["isNotFound"]}],
                "callers": [{"file": caller, "line": 1, "via": via, "call": true}],
            });
            (c, vec!["src/helpers/client.ts".to_owned()], Some(br))
        })
}

#[test]
fn out_of_diff_matches_js() {
    let exports = ["fileInDiff", "reanchorOutOfDiff"];
    let mut cases = sample((ood_candidate(), pr_files(), blast_radius()));
    cases.extend(sample(dependents_scenario()));
    let files: Vec<String> = ["src/helpers/client.ts", "src/helpers/client.test.ts"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let base = json!({"title": "caller now throws", "file": "src/services/accounts.ts", "line": 344, "severity": "important"});
    let br = json!({
        "changed": [{"file": "src/helpers/client.ts", "symbols": ["isNotFound"]}],
        "callers": [{"file": "src/services/accounts.ts", "line": 344, "via": "isNotFound", "call": true, "awaited": false, "text": "if (isNotFound(e))"}]
    });
    cases.push((base.clone(), files.clone(), Some(br.clone())));
    cases.push((base, files.clone(), None));
    // Array-index keys, which every JavaScript object enumerates first and in
    // ascending order whatever order the JSON text had them in.
    let indexed = json!({
        "title": "t", "10": "ten", "file": "src/services/accounts.ts", "2": "two",
        "01": "lead", "4294967295": "big", "4294967294": "max", "-1": "neg", "line": 3,
        "verifiedBy": [{"quote": "q", "1": "x", "file": "src/helpers/client.ts", "0": "y"}]
    });
    cases.push((indexed.clone(), files.clone(), None));
    let mut no_anchor = indexed.clone();
    no_anchor["verifiedBy"] = json!([]);
    cases.push((no_anchor, files.clone(), Some(br)));
    let mut in_diff = indexed;
    in_diff["file"] = json!("src/helpers/client.ts");
    cases.push((in_diff, files, None));

    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for (file, files) in &sample((maybe(0.95, candidate_file()), pr_files())) {
        let f = file.as_ref().and_then(Value::as_str);
        calls.push(json!({"fn": "fileInDiff", "args": [file, files]}));
        rust.push(as_result(Ok(json!(out_of_diff::file_in_diff(f, files)))));
    }
    for (c, files, br) in &cases {
        let file = match &c["file"] {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        };
        calls.push(json!({"fn": "fileInDiff", "args": [c.get("file").cloned().unwrap_or(Value::Null), files]}));
        rust.push(as_result(Ok(json!(out_of_diff::file_in_diff(file, files)))));
        calls.push(json!({"fn": "reanchorOutOfDiff", "args": [c, files, br]}));
        rust.push(as_result(
            out_of_diff::reanchor_out_of_diff(c, files, br.as_ref())
                .map(|r| r.unwrap_or(Value::Null)),
        ));
    }
    let js: Vec<Value> = oracle_raw("OUT-OF-DIFF", &exports, &[], &calls)
        .into_iter()
        .map(as_result)
        .collect();
    assert_same("OUT-OF-DIFF", &calls, &rust, &js);
    let via = |v: &Value| v["ok"]["reanchored"]["via"].clone();
    reach(
        &calls,
        &rust,
        "reanchorOutOfDiff",
        "re-anchored on verifiedBy",
        |v| via(v) == "verifiedBy",
    );
    reach(
        &calls,
        &rust,
        "reanchorOutOfDiff",
        "re-anchored on dependents",
        |v| via(v) == "dependents",
    );
    reach(
        &calls,
        &rust,
        "reanchorOutOfDiff",
        "kept in the diff",
        |v| v["ok"].is_object() && via(v).is_null(),
    );
    reach(&calls, &rust, "reanchorOutOfDiff", "dropped", |v| {
        v.get("ok") == Some(&Value::Null)
    });
    reach(
        &calls,
        &rust,
        "reanchorOutOfDiff",
        "a null changed entry",
        |v| v.get("err").is_some(),
    );
    reach(&calls, &rust, "fileInDiff", "out of the diff", |v| {
        v["ok"] == json!(false)
    });
}

// ---------------------------------------------------------------- LENS-PROMPT

fn prompt_text() -> impl Strategy<Value = Value> {
    let piece = prop::sample::select(vec![
        "line", "\n", "\r\n", "${x}", "`code`", "é😀", "\u{2028}", "$&", "  ", "<diff>", "",
    ]);
    prop::collection::vec(piece, 0..5).prop_map(|v| json!(v.concat()))
}

fn lens_params() -> impl Strategy<Value = Value> {
    let files = prop_oneof![
        4 => prop::collection::vec(
            pick(vec![json!("src/a.ts"), json!("é😀.ts"), Value::Null, json!(5), json!(["x", "y"]), json!({})]),
            0..4,
        )
        .prop_map(Value::Array),
        2 => pick(vec![Value::Null, json!(""), json!(0)]),
        1 => pick(vec![json!("a.ts"), json!({}), json!(1)]),
    ];
    (
        maybe(
            0.95,
            vec![
                json!(1),
                json!(1951),
                json!("12"),
                json!(1.5),
                json!(1e21),
                Value::Null,
                json!(-0.0),
            ],
        ),
        maybe(0.95, strings(&["o/r", "acme/widgets-api"])),
        prop::option::weighted(0.95, prompt_text()),
        maybe(
            0.8,
            vec![Value::Null, json!(""), json!("ABC-1"), json!(0), json!(7)],
        ),
        prop::option::weighted(0.9, prompt_text()),
        prop::option::weighted(
            0.7,
            prop_oneof![3 => prompt_text(), 1 => pick(vec![json!(0), json!(true), Value::Null])],
        ),
        maybe(0.95, strings(&["HRS", "money, auth or data integrity"])),
        prop::option::weighted(0.95, prompt_text()),
        prop::option::weighted(0.8, files),
    )
        .prop_map(|(pr, repo, diff, jira, ctx, deps, hrs, focus, files)| {
            obj(vec![
                ("prNumber", pr),
                ("repo", repo),
                ("diffBlock", diff),
                ("jiraTicket", jira),
                ("contextBlock", ctx),
                ("dependents", deps),
                ("highRiskSurface", hrs),
                ("focus", focus),
                ("files", files),
            ])
        })
}

#[test]
fn lens_prompt_matches_js() {
    let mut params = sample(lens_params());
    params.push(json!({
        "prNumber": 1, "repo": "o/r", "diffBlock": "DIFF", "jiraTicket": null, "contextBlock": "",
        "dependents": "<dependents>\nDEP\n</dependents>", "highRiskSurface": "HRS", "focus": "FOCUS-SENTENCE", "files": null
    }));
    let calls: Vec<Value> = params
        .iter()
        .map(|p| json!({"fn": "lensPrompt", "args": [p]}))
        .collect();
    // Declared difference: the port's `notReached` paragraph asks for each
    // gap's kind. Each port prompt carries it once; with the Workflow's
    // paragraph put back, the prompt is the Workflow's.
    let rust: Vec<Value> = params
        .iter()
        .map(|p| {
            as_result(lens_prompt::lens_prompt(p).map(|text| {
                assert_eq!(text.matches(lens_prompt::NOT_REACHED_PARAGRAPH).count(), 1);
                Value::String(text.replace(
                    lens_prompt::NOT_REACHED_PARAGRAPH,
                    lens_prompt::WORKFLOW_NOT_REACHED_PARAGRAPH,
                ))
            }))
        })
        .collect();
    let js: Vec<Value> = oracle_raw("LENS-PROMPT", &["lensPrompt"], &[], &calls)
        .into_iter()
        .map(as_result)
        .collect();
    assert_same("lensPrompt", &calls, &rust, &js);
}

// ---------------------------------------------------------------- LENS-BUDGET

fn not_reached_item() -> impl Strategy<Value = Value> {
    pick(vec![
        json!("  src/a.ts  "),
        json!("src/a.ts"),
        json!(""),
        json!("   "),
        Value::Null,
        json!("\u{FEFF}x\u{FEFF}"),
        json!("\u{85}y"),
        json!(5),
        json!(1.5e-7),
        json!(true),
        json!([1, null, 2]),
        json!({"a": 1}),
        json!(1e21),
        json!(0.1),
        json!(-0.0),
        json!("x"),
    ])
}

fn lens_result() -> impl Strategy<Value = Value> {
    prop_oneof![
        4 => prop::collection::vec(not_reached_item(), 0..4)
            .prop_map(|v| json!({"findings": [], "notReached": v})),
        1 => Just(json!({"findings": []})),
        1 => pick(vec![Value::Null, json!(7), json!({"notReached": "src/a.ts"}), json!({"notReached": {"a": 1}}), json!("s")]),
    ]
}

fn not_checked_list() -> impl Strategy<Value = Value> {
    prop_oneof![
        4 => prop::collection::vec(not_reached_item(), 0..4).prop_map(Value::Array),
        2 => pick(vec![Value::Null, json!([]), json!("str"), json!(0), json!(true), json!({}), json!(7), json!("")]),
    ]
}

#[test]
fn lens_budget_matches_js() {
    let exports = ["collectNotReached", "mergeNotChecked"];
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    let results = sample((
        prop_oneof![
            6 => prop::collection::vec(lens_result(), 0..5).prop_map(Value::Array),
            1 => pick(vec![Value::Null, json!("x"), json!({})]),
        ],
        0usize..6,
    ));
    for (r, n) in &results {
        calls.push(json!({"fn": "collectNotReached", "args": [r, n]}));
        rust.push(json!(lens_budget::collect_not_reached(r, *n)));
    }
    let lists = sample(prop::collection::vec(not_checked_list(), 0..5));
    for l in &lists {
        calls.push(json!({"fn": "mergeNotChecked", "args": l}));
        rust.push(json!(lens_budget::merge_not_checked(l)));
    }
    // String(n) over the whole double range, through the one export that
    // stringifies a bare number: shortest digits and the 1e-7 / 1e21 cutovers.
    use prop::num::f64 as f;
    let mut numbers = sample(f::POSITIVE | f::NEGATIVE | f::NORMAL | f::SUBNORMAL | f::ZERO);
    numbers.extend([
        1e21,
        999_999_999_999_999_900_000.0,
        1e-7,
        f64::from_bits(1e-7_f64.to_bits() - 1),
        f64::from_bits(1e-6_f64.to_bits() + 1),
        f64::from_bits(1e21_f64.to_bits() - 1),
        5e-324,
        f64::MAX,
        9_007_199_254_740_992.0,
        123_456_789.123,
        0.1 + 0.2,
        -1.5e-300,
        1e300,
    ]);
    for x in &numbers {
        let l = vec![json!([x])];
        calls.push(json!({"fn": "mergeNotChecked", "args": l}));
        rust.push(json!(lens_budget::merge_not_checked(&l)));
    }
    let js = js_results("LENS-BUDGET", &exports, &calls);
    assert_same("LENS-BUDGET", &calls, &rust, &js);
    reach(&calls, &rust, "collectNotReached", "a surface", |v| {
        v.as_array().is_some_and(|a| !a.is_empty())
    });
    reach(&calls, &rust, "mergeNotChecked", "a null entry", |v| {
        v.as_array().is_some_and(|a| a.contains(&json!("null")))
    });
}

/// Declared difference: an entry with a string `surface`, which only the
/// port's lenses write, is `surface (kind)` in the port and `[object Object]`
/// in the Workflow. Any other object reads the same on both sides.
#[test]
fn declared_difference_a_kinded_entry_reads_as_its_surface() {
    let results = json!([{"findings": [], "notReached": [
        {"surface": "callers of f", "file": "src/a.ts", "kind": "budget"},
        {"a": 1}
    ]}]);
    let calls = vec![json!({"fn": "collectNotReached", "args": [results, 1]})];
    let js = js_results("LENS-BUDGET", &["collectNotReached"], &calls);
    assert_eq!(js[0], json!(["[object Object]"]));
    assert_eq!(
        lens_budget::collect_not_reached(&results, 1),
        ["callers of f (budget)", "[object Object]"]
    );
}

// ---------------------------------------------------------------- LENS-YIELD

fn lens_label() -> Vec<Value> {
    vec![
        json!("correctness"),
        json!("dataContract"),
        json!("lens1"),
        json!("lens2"),
        json!(""),
        Value::Null,
        json!(3),
        json!(3.0),
        json!(true),
        json!({}),
        json!(["correctness"]),
    ]
}

#[test]
fn lens_yield_matches_js() {
    let plan_entry = prop_oneof![
        8 => (
            maybe(0.85, lens_label()),
            maybe(0.8, vec![Value::Null, json!([]), json!(["a.ts", "b.ts"]), json!(["c.ts"]), json!("ab")]),
        )
            .prop_map(|(l, f)| obj(vec![("lens", l), ("files", f)])),
        1 => pick(vec![Value::Null, json!("s"), json!(0)]),
    ];
    let result = prop_oneof![
        3 => Just(json!({"findings": []})),
        2 => prop::collection::vec(Just(json!("x.ts")), 0..3).prop_map(|v| json!({"findings": [], "notReached": v})),
        2 => pick(vec![Value::Null, json!(0), json!("s"), json!({"notReached": "x"}), json!(true)]),
    ];
    let candidate = prop_oneof![
        6 => maybe(0.9, lens_label()).prop_map(|l| obj(vec![("lens", l)])),
        1 => pick(vec![Value::Null, json!(7)]),
    ];
    let reported = prop_oneof![
        6 => (
            maybe(0.9, lens_label()),
            prop::option::weighted(0.7, prop_oneof![
                3 => prop::collection::vec(pick(lens_label()), 0..3).prop_map(Value::Array),
                1 => pick(vec![json!("nope"), Value::Null]),
            ]),
            maybe(0.9, vec![json!(true), json!(false), json!(1), json!(0), json!("x"), Value::Null]),
        )
            .prop_map(|(l, m, s)| obj(vec![("lens", l), ("mergedFrom", m), ("survived", s)])),
        1 => pick(vec![Value::Null, json!(5)]),
    ];
    let list = |s: BoxedStrategy<Value>| {
        prop_oneof![
            6 => prop::collection::vec(s, 0..5).prop_map(Value::Array),
            1 => pick(vec![Value::Null, json!("not-an-array")]),
        ]
    };
    let mut cases = sample((
        list(plan_entry.boxed()),
        list(result.boxed()),
        list(candidate.boxed()),
        list(reported.boxed()),
    ));
    cases.push((
        json!([{"lens": "correctness", "files": ["a.ts", "b.ts"]}, {"lens": "correctness", "files": ["c.ts"]}]),
        json!([{"findings": [{}]}, null]),
        json!([{"lens": "correctness"}, {"lens": "correctness"}]),
        json!([{"lens": "correctness", "mergedFrom": ["dataContract", "correctness"], "survived": true}]),
    ));
    cases.push((
        json!([{"lens": {"b": 1, "7": [{"z": 0, "3": 1}], "0": 2}}, {"lens": ["x", {"q": 1, "1": 2}]}]),
        json!([]),
        json!([]),
        json!([]),
    ));
    let calls: Vec<Value> = cases
        .iter()
        .map(|(p, r, c, f)| json!({"fn": "buildLensYield", "args": [p, r, c, f]}))
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|(p, r, c, f)| json!(lens_yield::build_lens_yield(p, r, c, f)))
        .collect();
    let js = js_results("LENS-YIELD", &["buildLensYield"], &calls);
    assert_same("buildLensYield", &calls, &rust, &js);
    let any_row =
        |rows: &Value, f: &dyn Fn(&Value) -> bool| rows.as_array().is_some_and(|r| r.iter().any(f));
    for (label, field) in [
        ("files counted", "files"),
        ("a crash", "crashed"),
        ("a candidate", "candidates"),
        ("a survivor", "survivedToReport"),
        ("a notReached entry", "notReached"),
    ] {
        reach(&calls, &rust, "buildLensYield", label, |v| {
            any_row(v, &|r| r[field].as_u64().is_some_and(|n| n > 0))
        });
    }
    reach(&calls, &rust, "buildLensYield", "a repeated label", |v| {
        any_row(v, &|r| r["dispatches"].as_u64().is_some_and(|n| n > 1))
    });
}

// ---------------------------------------------------------------- DEPENDENTS-BLOCK

fn any_text() -> Vec<Value> {
    vec![
        json!("src/a.ts"),
        json!("é😀.ts"),
        json!(""),
        json!("a `b` c"),
        json!("x\r\ny"),
        Value::Null,
        json!(5),
        json!(1.5),
        json!(["a", null]),
        json!({"a": 1}),
        json!(true),
    ]
}

fn fact() -> impl Strategy<Value = Value> {
    let kind = vec![
        json!("exit-before-finally"),
        json!("unawaited-enqueue"),
        json!("sentinel-from-catch"),
        json!("new-emit"),
        json!("payload-parity"),
        json!("writer-parity"),
        json!("job-without-handler"),
        json!("status-order"),
        json!("test-without-assertion"),
        json!("mocks-subject"),
        json!("contract-change"),
        json!("toString"),
        json!("__proto__"),
        json!("constructor"),
        json!("hasOwnProperty"),
        json!("unknown"),
        json!(["new-emit"]),
        json!(5),
        Value::Null,
    ];
    prop_oneof![
        10 => (maybe(0.9, any_text()), maybe(0.9, vec![json!(2), json!(1e21), json!("7")]), maybe(0.9, kind), maybe(0.9, any_text()))
            .prop_map(|(f, l, k, d)| obj(vec![("kind", k), ("file", f), ("line", l), ("detail", d)])),
        1 => pick(vec![Value::Null, json!("s"), json!(3)]),
    ]
}

fn attention() -> impl Strategy<Value = Value> {
    prop_oneof![
        6 => (
            prop::option::weighted(0.9, prop_oneof![
                4 => prop::collection::vec(fact(), 0..4).prop_map(Value::Array),
                1 => pick(vec![json!("x"), Value::Null]),
            ]),
            maybe(0.6, vec![json!(true), json!(false), json!(1), json!("")]),
            maybe(0.6, vec![json!(30), json!(2.5), json!("9"), Value::Null]),
        )
            .prop_map(|(f, t, n)| obj(vec![("facts", f), ("total", n), ("truncated", t)])),
        1 => pick(vec![Value::Null, json!("x"), json!(0)]),
    ]
}

fn dependents_caller() -> impl Strategy<Value = Value> {
    let flag = || {
        maybe(
            0.8,
            vec![
                json!(true),
                json!(false),
                json!(1),
                json!(0),
                json!("true"),
                Value::Null,
            ],
        )
    };
    prop_oneof![
        10 => (
            maybe(0.9, any_text()),
            maybe(0.9, vec![json!(4), json!(1.5), json!("4"), Value::Null]),
            maybe(0.9, vec![json!("f"), json!("f -> g"), json!("g -> f"), json!("webhookQueue -> addWebhookJob"), json!(""), Value::Null, json!(5), json!(["f"])]),
            flag(),
            flag(),
            flag(),
            maybe(0.9, any_text()),
        )
            .prop_map(|(file, line, via, call, awaited, in_try, text)| {
                obj(vec![
                    ("file", file),
                    ("line", line),
                    ("via", via),
                    ("call", call),
                    ("awaited", awaited),
                    ("inTry", in_try),
                    ("text", text),
                ])
            }),
        1 => pick(vec![Value::Null, json!("s"), json!(2)]),
    ]
}

fn dependents_br() -> impl Strategy<Value = Option<Value>> {
    let symbols = pick(vec![
        json!(["f", "g"]),
        json!(["webhookQueue"]),
        json!("fg"),
        json!([5, "f"]),
        json!([]),
    ]);
    let changed_entry = prop_oneof![
        8 => (
            maybe(0.9, vec![json!("src/s.ts"), json!("src/jobs/queue.ts"), json!(""), json!(5), Value::Null]),
            prop::option::weighted(0.9, symbols),
        )
            .prop_map(|(f, s)| obj(vec![("file", f), ("symbols", s)])),
        1 => Just(Value::Null),
        1 => Just(json!("ab")),
    ];
    let changed = prop_oneof![
        6 => prop::collection::vec(changed_entry, 0..3).prop_map(Value::Array),
        1 => pick(vec![json!("fg"), Value::Null]),
    ];
    let callers = prop_oneof![
        6 => prop::collection::vec(dependents_caller(), 0..4).prop_map(Value::Array),
        1 => pick(vec![json!("x"), Value::Null, json!([])]),
    ];
    prop::option::weighted(
        0.85,
        prop_oneof![
            8 => (
                prop::option::weighted(0.9, changed),
                prop::option::weighted(0.9, callers),
                maybe(0.5, vec![json!(true), json!(false), json!(1)]),
                maybe(0.6, vec![json!(99), json!(0), json!("7")]),
                prop::option::weighted(0.5, attention()),
            )
                .prop_map(|(ch, ca, tr, to, at)| {
                    obj(vec![("changed", ch), ("callers", ca), ("total", to), ("truncated", tr), ("attention", at)])
                }),
            1 => pick(vec![Value::Null, json!(0), json!("x"), json!(false)]),
        ],
    )
}

#[test]
fn dependents_block_matches_js() {
    let exports = ["attentionBlock", "dependentsBlock"];
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    let mut brs = sample(dependents_br());
    brs.push(Some(json!({
        "changed": [{"file": "src/jobs/queue.ts", "symbols": ["webhookQueue"]}],
        "callers": [
            {"file": "src/controllers/webhook.ts", "line": 4, "via": "webhookQueue -> addWebhookJob", "call": true, "awaited": false, "text": "addWebhookJob(req.body);"},
            {"file": "src/scripts/report.ts", "line": 4, "via": "webhookQueue", "call": false, "text": "webhookQueue"}
        ],
        "total": 2,
        "attention": {"facts": [{"kind": "unawaited-enqueue", "file": "src/a.ts", "line": 2, "detail": "addSyncJob(...) is called and its promise is not awaited, returned or caught"}], "total": 1, "truncated": false}
    })));
    brs.push(Some(json!({"changed": [{"file": "src/s.ts", "symbols": ["f"]}], "callers": [
        {"file": "a.ts", "line": 1, "via": "f", "call": true, "awaited": false, "inTry": false, "text": "f()"},
        {"file": "b.ts", "line": 2, "via": "f", "call": true, "awaited": true, "inTry": true, "text": "await f()"}
    ], "truncated": true, "total": 99})));
    for br in &brs {
        calls.push(json!({"fn": "dependentsBlock", "args": [br]}));
        rust.push(as_result(
            dependents_block::dependents_block(br.as_ref()).map(Value::String),
        ));
    }
    let ats = sample(prop::option::weighted(0.9, attention()));
    for at in &ats {
        calls.push(json!({"fn": "attentionBlock", "args": [at]}));
        rust.push(as_result(
            dependents_block::attention_block(at.as_ref()).map(Value::String),
        ));
    }
    let js: Vec<Value> = oracle_raw("DEPENDENTS-BLOCK", &exports, &[], &calls)
        .into_iter()
        .map(as_result)
        .collect();
    assert_same("DEPENDENTS-BLOCK", &calls, &rust, &js);
    for needle in [
        " (changed in ",
        ", inside a try",
        ", no try around it",
        " [reference] ",
        "[call, NOT awaited",
        "[call, awaited or returned",
        "This shows the first ",
        "</dependents>\n\n<attention>",
    ] {
        reach(&calls, &rust, "dependentsBlock", needle, |v| {
            text_of(v).contains(needle)
        });
    }
    for needle in [
        "[native code]",
        "Question: [object Object]",
        "is this line correct?",
        "This shows the first ",
    ] {
        reach(&calls, &rust, "attentionBlock", needle, |v| {
            text_of(v).contains(needle)
        });
    }
    for prop in ["symbols", "via", "file"] {
        let msg = format!("(reading '{prop}')");
        reach(&calls, &rust, "dependentsBlock", &msg, |v| {
            v["err"].as_str().is_some_and(|e| e.contains(&msg))
        });
    }
}
