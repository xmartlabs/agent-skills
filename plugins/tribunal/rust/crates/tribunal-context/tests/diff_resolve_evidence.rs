//! `skills/review-pr/scripts/resolve-evidence-line.mjs` against
//! `tribunal_context::resolve_evidence_line`: the same diff file and
//! candidates text on both sides, compared on the printed JSON as text.
mod common;
mod patchgen;
use common::*;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use proptest::prelude::*;
use proptest::sample::select;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use serde_json::{json, Value};
use tribunal_context::resolve_evidence_line::run;
use tribunal_core::profile::js_ext::json_stringify;

const SCRIPT: &str = "skills/review-pr/scripts/resolve-evidence-line.mjs";

/// How a candidate's field is built: a fixed value (`None` leaves the key
/// out), or taken from the diff around an anchor line.
#[derive(Debug, Clone)]
enum Field {
    Fixed(Option<Value>),
    /// The anchor's 1-based patch line plus an offset.
    PatchLine(i64),
    /// A guess at a file line.
    FileLine(i64),
    /// The anchor line's text (offset by `delta` lines), dressed by `how`.
    Quote(usize, usize),
    /// One of the paths the generator wrote into a header.
    DiffPath(usize),
    /// The b-side of the last header above the anchor line.
    AnchorPath,
}

#[derive(Debug, Clone)]
struct Spec {
    id: Option<Value>,
    file: Field,
    line: Field,
    evidence: Field,
    anchor: usize,
}

fn ids() -> Vec<Option<Value>> {
    vec![
        Some(json!("c1")),
        Some(json!(1)),
        Some(json!(0)),
        Some(Value::Null),
        None,
        Some(json!({"2": 1, "1": 0, "b": {"10": "x", "a": 1}})),
        Some(json!("é😀")),
        Some(json!(1.5)),
        Some(serde_json::from_str("1e21").unwrap()),
        Some(serde_json::from_str("-0.0").unwrap()),
        Some(json!([1, "two", null])),
    ]
}

fn fixed_files() -> Vec<Option<Value>> {
    vec![
        None,
        Some(Value::Null),
        Some(json!(5)),
        Some(json!(true)),
        Some(json!(["src/a.ts"])),
        Some(json!({"toString": 1})),
        Some(json!({})),
        Some(json!("")),
        Some(json!("nomatch.ts")),
        Some(json!("a.ts")),
        Some(json!("acme/widgets-api/src/a.ts")),
        Some(json!("./src/b.ts")),
        Some(json!("lib/x y.ts")),
    ]
}

fn fixed_lines() -> Vec<Option<Value>> {
    vec![
        None,
        Some(Value::Null),
        Some(json!(0)),
        Some(json!(-1)),
        Some(json!("12")),
        Some(json!(" 7 ")),
        Some(json!(2.5)),
        Some(json!(true)),
        Some(json!([5])),
        Some(json!("abc")),
        Some(serde_json::from_str("1e21").unwrap()),
        Some(json!("0x10")),
    ]
}

fn fixed_evidence() -> Vec<Option<Value>> {
    vec![
        None,
        Some(Value::Null),
        Some(json!(5)),
        Some(json!("")),
        Some(json!("   \n\t")),
        Some(json!("No relevant quotes found")),
        Some(json!("  No relevant quotes found \n")),
        Some(json!("}")),
        Some(json!("return;")),
        Some(json!("short")),
        Some(json!(
            "\n\n  const total = computeTotal(items);\n  return total;"
        )),
        Some(json!("\u{FEFF}const total = computeTotal(items);\u{3000}")),
        Some(json!("const   total =\tcomputeTotal(items);")),
        Some(json!("émoji 😀 line with astral 𝒳 chars")),
        Some(json!("not in any diff at all, anywhere")),
        Some(json!("}\nreturn null;\n{")),
    ]
}

fn field_strategy(
    fixed: Vec<Option<Value>>,
    kinds: &'static [&'static str],
) -> BoxedStrategy<Field> {
    let mut options: Vec<BoxedStrategy<Field>> = Vec::new();
    if !fixed.is_empty() {
        options.push(select(fixed).prop_map(Field::Fixed).boxed());
    }
    for k in kinds {
        options.push(match *k {
            "patch" => prop_oneof![3 => Just(0i64), 1 => -3i64..4]
                .prop_map(Field::PatchLine)
                .boxed(),
            "file" => (1i64..40).prop_map(Field::FileLine).boxed(),
            "quote" => (0usize..3, 0usize..7)
                .prop_map(|(d, h)| Field::Quote(d, h))
                .boxed(),
            "path" => (0usize..8).prop_map(Field::DiffPath).boxed(),
            "anchor-path" => Just(Field::AnchorPath).boxed(),
            _ => unreachable!(),
        });
    }
    proptest::strategy::Union::new(options).boxed()
}

fn spec() -> BoxedStrategy<Spec> {
    (
        select(ids()),
        prop_oneof![
            1 => field_strategy(fixed_files(), &[]),
            2 => field_strategy(vec![], &["path"]),
            3 => field_strategy(vec![], &["anchor-path"]),
        ],
        prop_oneof![
            1 => field_strategy(fixed_lines(), &[]),
            4 => field_strategy(vec![], &["patch"]),
            3 => field_strategy(vec![], &["file"]),
        ],
        prop_oneof![
            1 => field_strategy(fixed_evidence(), &[]),
            4 => field_strategy(vec![], &["quote"]),
        ],
        0usize..10_000,
    )
        .prop_map(|(id, file, line, evidence, anchor)| Spec {
            id,
            file,
            line,
            evidence,
            anchor,
        })
        .boxed()
}

/// The candidate a spec stands for over `lines` (the diff split on `\n`).
fn build(spec: &Spec, lines: &[&str], paths: &[String]) -> Value {
    let n = lines.len().max(1);
    let k = spec.anchor % n;
    let resolve = |f: &Field| -> Option<Value> {
        match f {
            Field::Fixed(v) => v.clone(),
            Field::PatchLine(off) => Some(json!(k as i64 + 1 + off)),
            Field::FileLine(l) => Some(json!(l)),
            Field::AnchorPath => {
                let header = lines[..=k.min(lines.len().saturating_sub(1))]
                    .iter()
                    .rev()
                    .find(|l| l.starts_with("diff --git a/"))
                    .copied()
                    .unwrap_or("");
                Some(json!(header.rsplit(" b/").next().unwrap_or("")))
            }
            Field::DiffPath(i) => Some(match paths.get(i % paths.len().max(1)) {
                Some(p) => json!(p),
                None => json!("src/a.ts"),
            }),
            Field::Quote(delta, how) => {
                let at = |j: usize| lines.get((k + j) % n).copied().unwrap_or("");
                let line = at(*delta);
                let bare = line.strip_prefix(['+', '-', ' ']).unwrap_or(line);
                Some(json!(match how {
                    0 => line.to_owned(),
                    1 => bare.to_owned(),
                    2 => format!("  {bare}\t "),
                    3 => format!("\n  \n{bare}"),
                    4 => format!("{}\n{}", at(*delta), at(delta + 1)),
                    5 => format!("{bare}\r"),
                    _ => format!("{}\n{line}", at(delta + 1)),
                }))
            }
        }
    };
    let mut m = serde_json::Map::new();
    for (key, v) in [
        ("id", spec.id.clone()),
        ("file", resolve(&spec.file)),
        ("line", resolve(&spec.line)),
        ("evidence", resolve(&spec.evidence)),
    ] {
        if let Some(v) = v {
            m.insert(key.into(), v);
        }
    }
    Value::Object(m)
}

/// What the outputs held, so the generator is shown to reach every branch.
#[derive(Default, Debug)]
struct Seen {
    kinds: BTreeMap<String, usize>,
    corrected: usize,
    no_resolved_file: usize,
    removed_match: usize,
    not_ran: usize,
}

impl Seen {
    fn add(&mut self, out: &Value) {
        let Some(results) = out["results"].as_array() else {
            self.not_ran += 1;
            return;
        };
        for r in results {
            *self
                .kinds
                .entry(r["matchKind"].as_str().unwrap_or("?").to_owned())
                .or_default() += 1;
            self.corrected += usize::from(r.get("correctedLine").is_some());
            self.no_resolved_file +=
                usize::from(r.get("resolvedFile").is_none() && r["matchKind"] != "error");
            self.removed_match += usize::from(r["matchedLineKind"] == "removed");
        }
    }

    fn kind(&self, k: &str) -> usize {
        self.kinds.get(k).copied().unwrap_or(0)
    }
}

enum Candidates<'a> {
    File(&'a str),
    Stdin(&'a str),
}

/// Runs both sides and fails on any difference. A script that throws out of
/// the batch exits non-zero; the port must return an error there.
fn check_case(dir: &Path, diff: &[u8], candidates: Candidates, seen: &mut Seen) {
    std::fs::create_dir_all(dir).unwrap();
    let diff_path = dir.join("diff.patch");
    std::fs::write(&diff_path, diff).unwrap();
    let rec = recordings(dir, vec![]);
    let (js, text) = match candidates {
        Candidates::File(text) => {
            let p = dir.join("candidates.json");
            std::fs::write(&p, text).unwrap();
            let args = [diff_path.to_str().unwrap(), p.to_str().unwrap()];
            (run_script(SCRIPT, &args, dir, &rec, None), text)
        }
        Candidates::Stdin(text) => {
            let args = [diff_path.to_str().unwrap(), "-"];
            (run_script(SCRIPT, &args, dir, &rec, Some(text)), text)
        }
    };
    let rust = run(&diff_path, text);
    let ctx = || format!("case {}: {text}", dir.display());
    match rust {
        Ok(v) => {
            assert_eq!(js.code, Some(0), "{}: script failed: {}", ctx(), js.stderr);
            assert_eq!(json_stringify(Some(&v)), js.stdout, "{}", ctx());
            seen.add(&v);
        }
        Err(e) => {
            assert_eq!(
                js.code,
                Some(1),
                "{}: port failed ({e}), script printed {}",
                ctx(),
                js.stdout
            );
            assert!(js.stderr.contains(&e), "{}: {e} vs {}", ctx(), js.stderr);
            *seen.kinds.entry("crash".into()).or_default() += 1;
        }
    }
}

#[test]
fn resolve_evidence_matches_js_on_generated_candidates() {
    let root = scratch("resolve-evidence-gen");
    let strategy = (
        patchgen::diff(),
        proptest::collection::vec(spec(), 0..7),
        0usize..20,
    );
    let mut runner = TestRunner::deterministic();
    let mut seen = Seen::default();
    for i in 0..300 {
        let ((diff, paths), specs, mode) = strategy.new_tree(&mut runner).unwrap().current();
        let lines: Vec<&str> = diff.split('\n').collect();
        let mut cands: Vec<Value> = specs.iter().map(|s| build(s, &lines, &paths)).collect();
        if mode == 0 {
            cands.push(Value::Null);
        }
        let text = Value::Array(cands).to_string();
        let input = if mode == 1 {
            Candidates::Stdin(&text)
        } else {
            Candidates::File(&text)
        };
        check_case(&root.join(i.to_string()), diff.as_bytes(), input, &mut seen);
    }
    eprintln!("{seen:?}");
    for k in [
        "exact",
        "shifted",
        "elsewhere",
        "ambiguous",
        "not-found",
        "low-specificity",
        "skipped",
        "error",
        "crash",
    ] {
        assert!(seen.kind(k) >= 5, "{k} reached too rarely: {seen:?}");
    }
    assert!(seen.corrected >= 20 && seen.removed_match >= 5, "{seen:?}");
    assert!(seen.not_ran >= 10, "{seen:?}");
}

/// A candidate with no `file` resolves only when the diff's first path ends
/// in "undefined" (`key.endsWith(undefined)`); otherwise `file.endsWith`
/// throws inside the per-candidate `try`. A resolved one carries no
/// `resolvedFile` key at all.
#[test]
fn resolve_evidence_matches_js_when_the_file_is_missing() {
    let quote = "const total = computeTotal(items);";
    let body = format!("@@ -1 +1,9 @@\n{}", format!("+{quote}\n").repeat(1));
    let first_undefined = format!(
        "diff --git a/src/undefined b/src/undefined\n{body}diff --git a/src/a.ts b/src/a.ts\n{body}{}",
        " filler line that pads the diff past two hundred characters\n".repeat(4)
    );
    let first_other = format!(
        "diff --git a/src/a.ts b/src/a.ts\n{body}diff --git a/src/undefined b/src/undefined\n{body}{}",
        " filler line that pads the diff past two hundred characters\n".repeat(4)
    );
    let cands = json!([
        {"id": "no-file", "line": 1, "evidence": quote},
        {"id": "no-file-far", "line": 30, "evidence": quote},
        {"id": "null-file", "file": null, "line": 1, "evidence": quote},
    ])
    .to_string();
    let root = scratch("resolve-evidence-missing-file");
    let mut seen = Seen::default();
    check_case(
        &root.join("first-undefined"),
        first_undefined.as_bytes(),
        Candidates::File(&cands),
        &mut seen,
    );
    check_case(
        &root.join("first-other"),
        first_other.as_bytes(),
        Candidates::File(&cands),
        &mut seen,
    );
    assert!(
        seen.no_resolved_file >= 2 && seen.kind("error") >= 3,
        "{seen:?}"
    );
}

/// Declared difference: a number literal beyond the double range reads as
/// Infinity in `JSON.parse` and as null here (a `Value` cannot hold it). In a
/// candidate that only changes `file`, where `String(Infinity)` is
/// "Infinity": the script reports `file.endsWith is not a function`, the port
/// the null message. `line`, `id` and `evidence` read the same either way.
#[test]
fn an_infinite_file_is_a_declared_difference() {
    let dir = scratch("resolve-evidence-infinity");
    let diff = format!(
        "diff --git a/src/a.ts b/src/a.ts\n@@ -1 +1,9 @@\n{}",
        "+const total = computeTotal(items);\n".repeat(8)
    );
    let patch = dir.join("d.patch");
    std::fs::write(&patch, &diff).unwrap();
    let cands = r#"[{"id":1e400,"file":1e400,"line":1e400,"evidence":"const total = computeTotal(items);"}]"#;
    let rec = recordings(&dir, vec![]);
    let js = run_script(
        SCRIPT,
        &[patch.to_str().unwrap(), "-"],
        &dir,
        &rec,
        Some(cands),
    );
    let rust = json_stringify(Some(&run(&patch, cands).unwrap()));
    let js_says = r#"{"ran":true,"results":[{"id":null,"matchKind":"error","error":"file.endsWith is not a function"}]}"#;
    assert_eq!(js.stdout, js_says);
    assert_eq!(
        rust,
        js_says.replace(
            "file.endsWith is not a function",
            "Cannot read properties of null (reading 'endsWith')"
        )
    );
}

/// Patches from this repository's history, with candidates quoting their
/// real lines.
#[test]
fn resolve_evidence_matches_js_on_repo_history() {
    let root = scratch("resolve-evidence-history");
    let patches = patchgen::repo_patches(25);
    let strategy = proptest::collection::vec(spec(), 1..12);
    let mut runner = TestRunner::deterministic();
    let mut seen = Seen::default();
    for (sha, diff) in &patches {
        let lines: Vec<&str> = diff.split('\n').collect();
        let paths: Vec<String> = lines
            .iter()
            .filter_map(|l| l.strip_prefix("+++ b/"))
            .map(str::to_owned)
            .collect();
        let specs = strategy.new_tree(&mut runner).unwrap().current();
        let cands: Vec<Value> = specs.iter().map(|s| build(s, &lines, &paths)).collect();
        let text = Value::Array(cands).to_string();
        check_case(
            &root.join(sha),
            diff.as_bytes(),
            Candidates::File(&text),
            &mut seen,
        );
    }
    assert!(
        seen.kind("exact") + seen.kind("shifted") + seen.kind("elsewhere") >= 5,
        "{seen:?}"
    );
}

/// The fixtures of `replay-line-correction.mjs`, every case through the batch.
#[test]
fn resolve_evidence_matches_js_on_the_line_correction_replay() {
    let diff = [
        "diff --git a/src/a.ts b/src/a.ts",
        "index 1111111..2222222 100644",
        "--- a/src/a.ts",
        "+++ b/src/a.ts",
        "@@ -1,3 +1,4 @@",
        " export function first() {",
        "+  const guard = requireAccount(accountId);",
        "   return loadEverything();",
        " }",
        "diff --git a/src/b.ts b/src/b.ts",
        "index 3333333..4444444 100644",
        "--- a/src/b.ts",
        "+++ b/src/b.ts",
        "@@ -40,4 +40,5 @@ export async function second() {",
        "   const rows = await prisma.invoice.findMany({",
        "-    where: { accountId },",
        "+    where: { accountId, deletedAt: null },",
        "+    orderBy: { createdAt: 'desc' },",
        "   });",
        "   return rows;",
        "diff --git a/src/c.ts b/src/c.ts",
        "--- a/src/c.ts",
        "+++ b/src/c.ts",
        "@@ -20,2 +22,3 @@",
        "   const total = sumInvoices(rows);",
        "+  const total2 = sumInvoices(rows);",
        "   return total;",
        "@@ -24,1 +26,1 @@",
        "-const legacyTotal = 0;",
        "+const total2 = sumInvoices(rows);",
    ]
    .join("\n");
    let mut long = vec![
        "diff --git a/src/d.ts b/src/d.ts".to_owned(),
        "--- a/src/d.ts".to_owned(),
        "+++ b/src/d.ts".to_owned(),
        "@@ -100,30 +200,31 @@".to_owned(),
        "+const marker = computeSettlementTotals(invoices);".to_owned(),
    ];
    long.extend((0..30).map(|k| format!(" const filler{k} = {k};")));
    let long = long.join("\n");
    let w = "where: { accountId, deletedAt: null },";
    let cands = json!([
        {"id": 0, "file": "src/b.ts", "line": 17, "evidence": w},
        {"id": 1, "file": "src/b.ts", "line": 18, "evidence": format!("{w}\n    orderBy: {{ createdAt: 'desc' }},")},
        {"id": 2, "file": "src/b.ts", "line": 17, "evidence": format!("+    {w}")},
        {"id": 3, "file": "src/b.ts", "line": 15, "evidence": "const rows = await prisma.invoice.findMany({"},
        {"id": 4, "file": "src/b.ts", "line": 41, "evidence": w},
        {"id": 5, "file": "src/a.ts", "line": 17, "evidence": w},
        {"id": 6, "file": "src/b.ts", "line": 16, "evidence": "where: { accountId },"},
        {"id": 7, "file": "src/a.ts", "line": 9, "evidence": w},
        {"id": 8, "file": "src/b.ts", "line": 19, "evidence": w},
        {"id": 9, "file": "src/a.ts", "line": 9, "evidence": "const guard = requireAccount(accountId);"},
        {"id": 10, "file": "src/a.ts", "line": 9, "evidence": "}"},
        {"id": 11, "file": "src/b.ts", "line": 14, "evidence": "@@ -40,4 +40,5 @@ export async function second() {"},
        {"id": 12, "file": "src/b.ts", "evidence": w},
        {"id": 13, "file": "src/b.ts", "line": 17},
        {"id": 14, "file": "src/c.ts", "line": 26, "evidence": "const total2 = sumInvoices(rows);"},
        {"id": 15, "file": "src/c.ts", "line": 30, "evidence": "const total2 = sumInvoices(rows);"},
        {"id": 16, "file": "src/d.ts", "line": 18 + 31, "evidence": "const marker = computeSettlementTotals(invoices);"},
        {"id": 17, "file": "src/d.ts", "line": 17 + 31, "evidence": "const marker = computeSettlementTotals(invoices);"},
    ]);
    let root = scratch("resolve-evidence-replay");
    let mut seen = Seen::default();
    let both = format!("{diff}\n{long}");
    check_case(
        &root.join("both"),
        both.as_bytes(),
        Candidates::File(&cands.to_string()),
        &mut seen,
    );
    assert!(seen.corrected >= 5, "{seen:?}");
}

#[test]
fn resolve_evidence_matches_js_on_unusable_inputs() {
    let root = scratch("resolve-evidence-edges");
    let mut seen = Seen::default();
    let usable = format!(
        "diff --git a/src/a.ts b/src/a.ts\n@@ -1 +1,9 @@\n{}",
        "+const total = computeTotal(items);\n".repeat(8)
    );
    let cases: Vec<(&str, String, &str)> = vec![
        ("short", "diff --git a/x b/x\n+1".into(), "[]"),
        ("short-any", "x".into(), "{\"not\": \"an array\"}"),
        ("banner", format!("Output too large\n{usable}"), "[]"),
        ("not-json", usable.clone(), "not json"),
        ("bom-json", usable.clone(), "\u{FEFF}[]"),
        ("object", usable.clone(), "{\"a\": 1}"),
        ("string", usable.clone(), "\"abc\""),
        ("null-item", usable.clone(), "[{\"id\": 1}, null]"),
        ("empty", usable.clone(), "[]"),
        ("primitives", usable.clone(), "[0, \"\", false, true, 5, \"abc\", [], {}]"),
        ("dup-keys", usable.clone(), "[{\"id\": 1, \"id\": 2, \"evidence\": \"const total = computeTotal(items);\", \"file\": \"src/a.ts\"}]"),
        ("deep-id", usable.clone(), "[{\"id\": {\"1\": [{\"0\": 1, \"z\": 2, \"3\": 3}]}, \"file\": \"a.ts\"}]"),
        ("big-line", usable.clone(), "[{\"file\": \"src/a.ts\", \"line\": 1e400, \"evidence\": \"const total = computeTotal(items);\"}]"),
    ];
    for (name, diff, cands) in cases {
        check_case(
            &root.join(name),
            diff.as_bytes(),
            Candidates::File(cands),
            &mut seen,
        );
    }

    let dir = root.join("unreadable");
    std::fs::create_dir_all(&dir).unwrap();
    let rec = recordings(&dir, vec![]);
    let missing = dir.join("missing.patch");
    let js = run_script(
        SCRIPT,
        &[missing.to_str().unwrap(), "-"],
        &dir,
        &rec,
        Some("[]"),
    );
    let rust = run(&missing, "[]").unwrap();
    assert_eq!(json_stringify(Some(&rust)), js.stdout);
    assert_eq!(rust, json!({"ran": false, "reason": "diff-unreadable"}));
}

/// The fixtures `replay-evidence-resolver.mjs` replays, one candidate each.
#[test]
#[ignore = "needs TRIBUNAL_HARNESS_DATA"]
fn resolve_evidence_matches_js_on_recorded_fixtures() {
    let data = std::env::var("TRIBUNAL_HARNESS_DATA").expect("TRIBUNAL_HARNESS_DATA is not set");
    let dir = PathBuf::from(data).join("tribunal/fixtures/evidence-resolver");
    let root = scratch("resolve-evidence-recorded");
    let mut seen = Seen::default();
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let fixture: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        let cands = match (&fixture["batchFindings"], &fixture["finding"]) {
            (Value::Array(b), _) => Value::Array(b.clone()),
            (_, f) => json!([f]),
        };
        let name = p.file_stem().unwrap().to_string_lossy().into_owned();
        let diff = fixture["diff"].as_str().unwrap_or("");
        check_case(
            &root.join(name),
            diff.as_bytes(),
            Candidates::File(&cands.to_string()),
            &mut seen,
        );
    }
    assert!(
        !seen.kinds.is_empty() || seen.not_ran > 0,
        "no fixtures in {}",
        dir.display()
    );
}
