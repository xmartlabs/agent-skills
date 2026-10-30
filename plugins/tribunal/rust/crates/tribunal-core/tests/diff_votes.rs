mod common;
use common::*;

// Differential tests for the vote regions: each ported function and its
// JavaScript region run on the same generated inputs, and any difference
// fails. Generators lean on what each region actually coerces: truthiness,
// `===` between JSON values, `String()`/`Number()` of the wrong type, the
// JavaScript `\s`, `.` and `i`-flag rules, and UTF-16 slicing.
//
// Two exports are not functions, so the oracle cannot call them by name:
// `voteCorrections` (an array the region pushes to) and `PANEL_TIEBREAK_CAP`.
// The oracle builds its export object as `{ <exports> }`, so a
// `name: () => expr` entry reads them from inside the region.
//
// The JavaScript's side is read from `tests/goldens/oracle/diff_votes.json`
// (see `common`): the workflow is being retired, so a run starts only the
// port. The file keeps each call's results under a SHA-256 of its inputs and a
// SHA-256 of every region's text, so a region edited without regenerating
// fails here; with the workflow gone the golden is final. Regenerate:
// `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-core --test diff_votes`.
// The ignored recorded-fixtures test is not in the golden: its inputs live
// outside this repository and are not to be copied into it, so with the
// golden in place it fails asking for a regeneration.

use std::num::NonZeroUsize;
use std::path::PathBuf;

use proptest::prelude::*;
use serde_json::{json, Map, Value};

use tribunal_core::votes::{
    artifact_truth, availability_check_gate, clean_sweep, dedup_veto, evidence_resolver_degrade,
    panel_ceiling_tiebreak, vote_quotes,
};

const DRAIN_CORRECTIONS: &str = "drainVoteCorrections: () => voteCorrections.splice(0)";
const TIEBREAK_CAP: &str = "panelTiebreakCap: () => PANEL_TIEBREAK_CAP";

/// `oracle_raw` results as comparable JSON, and the same for Rust.
fn as_outcome(r: Result<Value, String>) -> Value {
    match r {
        Ok(v) => json!({ "ok": v }),
        Err(e) => json!({ "err": e }),
    }
}

fn maybe(m: &mut Map<String, Value>, key: &str, v: Option<Value>) {
    if let Some(v) = v {
        m.insert(key.to_owned(), v);
    }
}

fn pick<T: Clone + std::fmt::Debug + 'static>(v: Vec<T>) -> impl Strategy<Value = T> {
    prop::sample::select(v)
}

/// A well-formed value most of the time and an edge value otherwise, so that
/// an input built from several fields is still well-formed often enough for
/// the paths behind the checks to run.
fn mostly<T: Clone + std::fmt::Debug + 'static>(
    good: Vec<T>,
    edges: Vec<T>,
) -> impl Strategy<Value = T> {
    prop_oneof![5 => pick(good), 1 => pick(edges)]
}

// ===== ARTIFACT-TRUTH =====

fn vote() -> impl Strategy<Value = Value> {
    let verdict = pick(vec![
        None,
        Some(json!("REFUTED")),
        Some(json!("NOT_REFUTED")),
        Some(json!(7)),
        Some(Value::Null),
    ]);
    let confidence = pick(vec![
        None,
        Some(json!(0.33)),
        Some(json!(0.5)),
        Some(json!(0.505)),
        Some(json!(0.52)),
        Some(json!(1)),
        Some(json!(1.0)),
        Some(json!(0.99)),
        Some(json!("0.5")),
    ]);
    let status = pick(vec![
        None,
        Some(json!("OK")),
        Some(json!("UNAVAILABLE")),
        Some(json!("CRASHED")),
    ]);
    let reason = pick(vec![None, Some(json!("relayed why")), Some(json!(""))]);
    let extra = pick(vec![
        None,
        Some(("failureReason", json!("timeout"))),
        Some(("source", json!("relay"))),
        Some(("quoteFound", json!(false))),
    ]);
    let object = (verdict, confidence, status, reason, extra, any::<bool>()).prop_map(
        |(v, c, s, r, extra, extra_first)| {
            let mut m = Map::new();
            if extra_first {
                if let Some((k, val)) = &extra {
                    m.insert((*k).to_owned(), val.clone());
                }
            }
            maybe(&mut m, "verdict", v);
            maybe(&mut m, "confidence", c);
            maybe(&mut m, "slotStatus", s);
            maybe(&mut m, "reason", r);
            if !extra_first {
                if let Some((k, val)) = extra {
                    m.insert(k.to_owned(), val);
                }
            }
            Value::Object(m)
        },
    );
    // A vote is an object or nothing: the relay schema holds it to an object.
    prop_oneof![3 => object, 1 => Just(Value::Null)]
}

fn truth_entry() -> impl Strategy<Value = Value> {
    let failclass = mostly(
        vec![Some(json!("none"))],
        vec![
            None,
            Some(json!("unparseable")),
            Some(json!("confined")),
            Some(json!("quota")),
            Some(json!("")),
            Some(json!(5)),
        ],
    );
    let verdict = mostly(
        vec![Some(json!("REFUTED")), Some(json!("NOT_REFUTED"))],
        vec![None, Some(Value::Null), Some(json!("")), Some(json!(1))],
    );
    let confidence = pick(vec![
        None,
        Some(json!(0.99)),
        Some(json!(0.5)),
        Some(json!(0.515)),
        Some(json!(0)),
        Some(json!(1.0)),
        Some(Value::Null),
        Some(json!("0.9")),
    ]);
    let declared = pick(vec![
        None,
        Some(json!(true)),
        Some(json!(false)),
        Some(json!("yes")),
    ]);
    let reason = pick(vec![None, Some(json!("artifact why")), Some(json!(""))]);
    let object = (failclass, verdict, confidence, declared, reason).prop_map(|(f, v, c, d, r)| {
        let mut m = Map::new();
        maybe(&mut m, "failclass", f);
        maybe(&mut m, "verdict", v);
        maybe(&mut m, "confidence", c);
        maybe(&mut m, "confidenceDeclared", d);
        maybe(&mut m, "reason", r);
        Value::Object(m)
    });
    prop_oneof![
        12 => object,
        1 => Just(json!(0)),
        1 => Just(json!("x")),
        1 => Just(json!([])),
        1 => Just(Value::Null),
    ]
}

fn truth_map() -> impl Strategy<Value = Value> {
    let keys = vec![
        "0-codex",
        "0-gemini",
        "1-codex",
        "0r1-codex",
        "1r1-gemini",
        "2-codex",
        "x-codex",
        "0-claude",
    ];
    let object = prop::collection::vec((pick(keys), truth_entry()), 0..8).prop_map(|entries| {
        let mut m = Map::new();
        for (k, v) in entries {
            m.insert(k.to_owned(), v);
        }
        Value::Object(m)
    });
    prop_oneof![
        10 => object,
        1 => Just(Value::Null),
        1 => Just(json!(false)),
        1 => Just(json!("truthy but no keys")),
    ]
}

fn panel_slot() -> impl Strategy<Value = Value> {
    (
        pick(vec![
            None,
            Some("codex"),
            Some("gemini"),
            Some("claude"),
            Some(""),
        ]),
        any::<bool>(),
    )
        .prop_map(|(source, task)| {
            let mut m = Map::new();
            if let Some(s) = source {
                m.insert("source".into(), json!(s));
            }
            // In the workflow `task` is a function; any truthy value stands
            // in for it.
            m.insert("task".into(), if task { json!(true) } else { Value::Null });
            Value::Object(m)
        })
}

fn to_slot(v: &Value) -> artifact_truth::PanelSlot {
    artifact_truth::PanelSlot {
        source: v["source"].as_str().map(str::to_owned),
        relayed: v["task"] == json!(true),
    }
}

type TruthCase = (Vec<Value>, Vec<Value>, Value, u64, u64);

/// Most external slots find an entry under their own artifact key, so the
/// verdict, abstention and correction paths are all reached; the rest of the
/// map is decoys.
fn truth_case() -> impl Strategy<Value = TruthCase> {
    (
        prop::collection::vec(vote(), 0..4),
        prop::collection::vec(panel_slot(), 0..4),
        0u64..3,
        0u64..2,
    )
        .prop_flat_map(|(votes, panel, slot, round)| {
            let r = if round > 0 {
                format!("r{round}")
            } else {
                String::new()
            };
            let keys: Vec<String> = panel
                .iter()
                .filter_map(|p| p["source"].as_str())
                .map(|s| format!("{slot}{r}-{s}"))
                .collect();
            let n = keys.len();
            (
                Just((votes, panel, slot, round, keys)),
                prop::collection::vec((0u8..5, truth_entry()), n..=n),
                truth_map(),
            )
        })
        .prop_map(|((votes, panel, slot, round, keys), entries, base)| {
            let truth = match base {
                Value::Object(mut m) => {
                    for (k, (coin, e)) in keys.into_iter().zip(entries) {
                        if coin != 0 {
                            m.insert(k, e);
                        }
                    }
                    Value::Object(m)
                }
                other => other,
            };
            (votes, panel, truth, slot, round)
        })
}

/// A relay that echoed its artifact, off by a small confidence delta: the
/// shape where only the 0.01 drift tolerance decides whether a correction is
/// recorded.
fn echo_case() -> impl Strategy<Value = TruthCase> {
    (
        pick(vec!["codex", "gemini"]),
        pick(vec!["REFUTED", "NOT_REFUTED"]),
        pick(vec![0.5, 0.39, 0.99, 0.8]),
        pick(vec![0.0, 0.005, -0.005, 0.01, -0.01, 0.015, -0.02, 0.05, 0.09, 0.2]),
        pick(vec![Some(json!("OK")), None, Some(json!("UNAVAILABLE"))]),
        0u64..3,
        0u64..2,
    )
        .prop_map(|(source, verdict, conf, delta, status, slot, round)| {
            let r = if round > 0 { format!("r{round}") } else { String::new() };
            let mut vote = Map::new();
            vote.insert("verdict".into(), json!(verdict));
            vote.insert("confidence".into(), json!(conf + delta));
            maybe(&mut vote, "slotStatus", status);
            let truth = json!({
                format!("{slot}{r}-{source}"): {"failclass": "none", "verdict": verdict, "confidence": conf}
            });
            (
                vec![Value::Object(vote)],
                vec![json!({"source": source, "task": true})],
                truth,
                slot,
                round,
            )
        })
}

fn replay_truth_seeds() -> Vec<TruthCase> {
    let driver_truth = json!({
        "0-codex": {"failclass": "none", "verdict": "NOT_REFUTED", "confidence": 0.99, "confidenceDeclared": true},
        "4-codex": {"failclass": "none", "verdict": "REFUTED", "confidence": 0.8, "confidenceDeclared": true},
        "5-codex": {"failclass": "confined"},
    });
    let driver = json!({"source": "codex", "task": null});
    let relayed = json!({"source": "codex", "task": true});
    let mut seeds: Vec<TruthCase> = [0u64, 4, 5, 7]
        .into_iter()
        .map(|slot| {
            (
                vec![Value::Null],
                vec![driver.clone()],
                driver_truth.clone(),
                slot,
                0,
            )
        })
        .collect();
    seeds.push((vec![Value::Null], vec![relayed.clone()], driver_truth, 0, 0));
    let pr1911 = json!({
        "0-gemini": {"failclass": "unparseable", "verdict": null, "confidence": null, "reason": ""},
        "1-codex": {"failclass": "unparseable", "verdict": null, "confidence": null, "reason": ""},
        "0-codex": {"failclass": "none", "verdict": "REFUTED", "confidence": 0.39, "reason": "absent"},
    });
    for (slot, source, verdict, confidence) in [
        (0, "gemini", "NOT_REFUTED", json!(1)),
        (1, "codex", "REFUTED", json!(0.33)),
        (0, "codex", "REFUTED", json!(0.39)),
    ] {
        seeds.push((
            vec![json!({"slotId": slot, "source": source, "verdict": verdict, "confidence": confidence, "slotStatus": "OK"})],
            vec![json!({"source": source, "task": true})],
            pr1911.clone(),
            slot,
            0,
        ));
    }
    seeds
}

fn check_artifact_truth(what: &str, cases: &[TruthCase]) {
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for (votes, panel, truth, slot, round) in cases {
        calls.push(json!({"fn": "applyArtifactTruth", "args": [votes, panel, truth, slot, round]}));
        calls.push(json!({"fn": "drainVoteCorrections", "args": []}));
        let slots: Vec<_> = panel.iter().map(to_slot).collect();
        let mut corrections = Vec::new();
        let out = artifact_truth::apply_artifact_truth(
            votes,
            &slots,
            Some(truth),
            *slot,
            *round,
            &mut corrections,
        );
        rust.push(Value::Array(out));
        rust.push(Value::Array(corrections));
    }
    let js = oracle(
        "ARTIFACT-TRUTH",
        &["applyArtifactTruth", DRAIN_CORRECTIONS],
        &["ARTIFACT-KEY"],
        &calls,
    );
    assert_same(what, &calls, &rust, &js);
}

#[test]
fn artifact_truth_matches_js() {
    let mut cases = replay_truth_seeds();
    cases.extend(sample(prop_oneof![3 => truth_case(), 1 => echo_case()]));
    check_artifact_truth("applyArtifactTruth", &cases);
}

// ===== VOTE-QUOTES =====

fn ws_text() -> impl Strategy<Value = String> {
    let chars = mostly(
        vec!['a', 'x', ' '],
        vec![
            '\t', '\u{FEFF}', '\u{0085}', '\u{3000}', '\u{2028}', '\u{00A0}', '😀',
        ],
    );
    prop::collection::vec(chars, 0..5).prop_map(|v| v.into_iter().collect())
}

fn contradiction() -> impl Strategy<Value = Value> {
    let file = pick(vec![
        None,
        Some(json!("src/b.ts")),
        Some(json!("")),
        Some(Value::Null),
        Some(json!(0)),
    ]);
    let line = pick(vec![
        None,
        Some(json!(10)),
        Some(json!(10.0)),
        Some(json!(1.5)),
        Some(json!("10")),
        Some(json!(-3)),
    ]);
    let quote = prop_oneof![
        24 => ws_text().prop_map(|s| Some(json!(s))),
        1 => Just(None),
        1 => Just(Some(json!(5))),
        1 => Just(Some(json!(0))),
        1 => Just(Some(json!([" "]))),
        1 => Just(Some(json!(["a", null]))),
        1 => Just(Some(json!({}))),
        1 => Just(Some(json!(false))),
    ];
    let object = (file, line, quote).prop_map(|(f, l, q)| {
        let mut m = Map::new();
        maybe(&mut m, "file", f);
        maybe(&mut m, "line", l);
        maybe(&mut m, "quote", q);
        Value::Object(m)
    });
    prop_oneof![12 => object, 1 => Just(Value::Null), 1 => Just(json!("quote as text"))]
}

fn quote_vote() -> impl Strategy<Value = Value> {
    let verdict = mostly(
        vec![Some(json!("REFUTED"))],
        vec![Some(json!("NOT_REFUTED")), Some(json!("refuted")), None],
    );
    let introduced = mostly(
        vec![Some(json!("yes")), Some(json!(""))],
        vec![Some(json!(true)), None],
    );
    let rtype = mostly(
        vec![None, Some(json!("FACTUAL"))],
        vec![Some(json!("PRE_DECIDED"))],
    );
    let contradiction = proptest::option::weighted(0.9, contradiction());
    let object = (verdict, introduced, rtype, contradiction).prop_map(|(v, i, r, c)| {
        let mut m = Map::new();
        maybe(&mut m, "verdict", v);
        maybe(&mut m, "introducedByPr", i);
        maybe(&mut m, "refutationType", r);
        maybe(&mut m, "contradiction", c);
        Value::Object(m)
    });
    prop_oneof![16 => object, 1 => Just(Value::Null), 1 => Just(json!(0)), 1 => Just(json!("v"))]
}

fn panel_state() -> impl Strategy<Value = Value> {
    let finding = pick(vec![
        None,
        Some(json!({"file": "src/a.ts"})),
        Some(json!({"file": ""})),
        Some(json!({})),
        Some(Value::Null),
    ]);
    let skipped = mostly(
        vec![None, Some(json!(false))],
        vec![Some(json!(true)), Some(json!(1)), Some(json!(""))],
    );
    let votes = prop_oneof![
        6 => prop::collection::vec(quote_vote(), 0..4).prop_map(|v| Some(Value::Array(v))),
        1 => Just(None),
        1 => Just(Some(Value::Null)),
    ];
    let object = (finding, skipped, votes).prop_map(|(f, s, v)| {
        let mut m = Map::new();
        maybe(&mut m, "finding", f);
        maybe(&mut m, "skipped", s);
        maybe(&mut m, "votes", v);
        Value::Object(m)
    });
    prop_oneof![8 => object, 1 => Just(Value::Null), 1 => Just(json!("state"))]
}

fn lookup_result() -> impl Strategy<Value = Value> {
    let index = pick(vec![
        Some(json!(0)),
        Some(json!(1)),
        Some(json!(2)),
        Some(json!(4)),
        Some(json!(9)),
        Some(json!(-1)),
        Some(json!(1.5)),
        Some(json!(2.0)),
        Some(json!("1")),
        Some(json!("01")),
        Some(json!(" 2 ")),
        Some(json!([2])),
        Some(json!(true)),
        Some(Value::Null),
        None,
    ]);
    let status = pick(vec![
        Some(json!("found")),
        Some(json!("found-shifted")),
        Some(json!("not-found")),
        Some(json!("file-not-at-ref")),
        Some(json!("file-not-at-ref")),
        Some(json!("file-not-at-ref")),
        Some(json!("malformed")),
        Some(json!("ref-not-found")),
        Some(json!("lookup-failed")),
        Some(json!(["found"])),
        Some(json!(["lookup-failed"])),
        Some(json!("not-found ")),
        Some(Value::Null),
        None,
    ]);
    let object = (index, status).prop_map(|(i, s)| {
        let mut m = Map::new();
        maybe(&mut m, "findingIndex", i);
        maybe(&mut m, "status", s);
        Value::Object(m)
    });
    prop_oneof![16 => object, 1 => Just(Value::Null), 1 => Just(json!(3)), 1 => Just(json!("found"))]
}

#[test]
fn vote_quotes_match_js() {
    let exports = ["checkableVoteQuotes", "voteQuoteVerdicts"];

    let mut states: Vec<Option<Vec<Value>>> = vec![None];
    states.extend(
        sample(prop::collection::vec(panel_state(), 0..4))
            .into_iter()
            .map(Some),
    );
    let calls: Vec<Value> = states
        .iter()
        .map(|s| json!({"fn": "checkableVoteQuotes", "args": [s]}))
        .collect();
    let rust: Vec<Value> = states
        .iter()
        .map(|s| json!(vote_quotes::checkable_vote_quotes(s.as_deref())))
        .collect();
    let js = oracle("VOTE-QUOTES", &exports, &[], &calls);
    assert_same("checkableVoteQuotes", &calls, &rust, &js);

    let lists = prop_oneof![
        10 => prop::collection::vec(lookup_result(), 0..5).prop_map(Value::Array),
        1 => Just(Value::Null),
        1 => Just(json!({"results": []})),
    ];
    let cases = sample((lists, 0usize..5));
    let calls: Vec<Value> = cases
        .iter()
        .map(|(l, n)| json!({"fn": "voteQuoteVerdicts", "args": [l, n]}))
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|(l, n)| as_outcome(vote_quotes::vote_quote_verdicts(Some(l), *n).map(|v| json!(v))))
        .collect();
    let js: Vec<Value> = oracle_raw("VOTE-QUOTES", &exports, &[], &calls)
        .into_iter()
        .map(as_outcome)
        .collect();
    assert_same("voteQuoteVerdicts", &calls, &rust, &js);
}

// ===== CLEAN-SWEEP =====

fn finding_line() -> impl Strategy<Value = String> {
    let label = mostly(
        vec!["FINDING:", "finding:", "Finding:"],
        vec!["FINDING", "FINDINGS:"],
    );
    let sev = mostly(
        vec!["important", "critical", "nit", "NIT", "Critical"],
        vec!["nitpick", "\u{017F}evere", "minor", "n\u{0130}t"],
    );
    let title = mostly(
        vec![" Count push races ", "Missing guard", " a:b "],
        vec![
            "",
            "   ",
            " t\u{2028}x ",
            "\u{FEFF}Padded\u{FEFF}",
            "\u{0085}x\u{0085}",
        ],
    );
    let file = mostly(
        vec![" src/a.ts", "src/services/v2/accounts.ts"],
        vec![
            "   ",
            "",
            " a|b.ts",
            " \u{0085}x.ts\u{0085}",
            "\u{FEFF}y.ts ",
        ],
    );
    let digits = mostly(
        vec!["585", "1", "007"],
        vec![
            "",
            "12abc",
            "\u{FF11}\u{FF12}",
            "99999999999999999999",
            "123456789012345678901234",
        ],
    );
    let after = mostly(
        vec![" | ", "|"],
        vec![" \u{3000}| ", " x | ", " \u{0085}| "],
    );
    let why = mostly(
        vec!["two invocations leave the partner low", "ok ", " because"],
        vec![
            "",
            "  ",
            " \u{2028}",
            "\u{2028}after",
            "why\r",
            "a\u{2029}b",
            " \u{FEFF}",
            "\u{0085}",
        ],
    );
    (
        lead_ws(),
        label,
        lead_ws(),
        sev,
        title,
        file,
        digits,
        after,
        why,
    )
        .prop_map(|(w1, label, w2, sev, title, file, digits, after, why)| {
            format!("{w1}{label}{w2}{sev} |{title}|{file}:{digits}{after}{why}")
        })
}

fn evidence_line() -> impl Strategy<Value = String> {
    let label = mostly(
        vec!["EVIDENCE:", "evidence:"],
        vec!["EVIDENCE", "\u{0130}EVIDENCE:"],
    );
    let body = mostly(
        vec![
            " const actualCount = countData",
            "await write(id)",
            " quote \u{FEFF}",
        ],
        vec![
            "",
            "   ",
            " a\u{2028}b",
            "\u{2029}tail",
            " \u{0085}",
            " x\r",
        ],
    );
    (lead_ws(), label, body).prop_map(|(w, l, b)| format!("{w}{l}{b}"))
}

fn verified_line() -> impl Strategy<Value = String> {
    let label = mostly(
        vec!["VERIFIED:", "verified:"],
        vec!["VERIFIED", "VERIFIED;"],
    );
    let file = mostly(
        vec![" src/b.ts", "src/c.ts"],
        vec!["   ", "", " x|y", "\u{0085}"],
    );
    let digits = mostly(vec!["4", "9", "0"], vec!["", "\u{0664}", "4x"]);
    let rest = mostly(
        vec![" | export function write(id: string) {", "| q", " |  q  "],
        vec![" |  ", " | a\rb", "|", " x | q", " | \u{2029}"],
    );
    (lead_ws(), label, file, digits, rest).prop_map(|(w, l, f, d, r)| format!("{w}{l}{f}:{d}{r}"))
}

/// A FINDING line, usually its EVIDENCE line, and zero to three VERIFIED
/// lines, as the sweep prompt asks for them.
fn finding_block() -> impl Strategy<Value = Vec<String>> {
    (
        finding_line(),
        proptest::option::weighted(0.8, evidence_line()),
        prop::collection::vec(verified_line(), 0..4),
    )
        .prop_map(|(f, e, v)| std::iter::once(f).chain(e).chain(v).collect())
}

fn lead_ws() -> impl Strategy<Value = &'static str> {
    mostly(
        vec!["", " "],
        vec!["\t ", "\u{FEFF}", "\u{0085}", "\u{3000}", "\u{2028}"],
    )
}

fn head_failure_line() -> impl Strategy<Value = String> {
    let lead = pick(vec!["", "", " "]);
    let class = pick(vec!["quota", "auth", "error", "QUOTA", "timeout"]);
    let head = pick(vec!["HEAD FAILURE", "HEAD FAILURE", "head failure"]);
    let reason = prop_oneof![
        3 => pick(vec![
            "ERROR: Your workspace is out of credits.".to_owned(),
            "".to_owned(),
            "  \u{FEFF}padded\u{3000} ".to_owned(),
        ]),
        // Long reasons cross the 200-unit cut. A leading emoji makes the cut
        // count UTF-16 units without landing inside the pair.
        3 => (any::<bool>(), 190usize..215).prop_map(|(emoji, n)| {
            format!("{}{}", if emoji { "😀" } else { "" }, "y".repeat(n))
        }),
    ];
    (lead, head, class, reason).prop_map(|(l, h, c, r)| format!("{l}{h} ({c}): {r}"))
}

fn other_line() -> impl Strategy<Value = String> {
    pick(vec![
        "Some preamble the model wrote.".to_owned(),
        "NO FINDINGS ABOVE NIT.".to_owned(),
        "no findings above nit".to_owned(),
        "NO FINDINGS ABOVE N\u{0130}T".to_owned(),
        "NO FINDINGS ABOVE NIT".to_owned(),
        "Command executed with exit code 1. No output was printed.".to_owned(),
        String::new(),
        "   ".to_owned(),
    ])
}

fn sweep_text() -> impl Strategy<Value = Option<String>> {
    let block = prop_oneof![
        6 => finding_block(),
        1 => evidence_line().prop_map(|l| vec![l]),
        1 => verified_line().prop_map(|l| vec![l]),
        3 => head_failure_line().prop_map(|l| vec![l]),
        3 => other_line().prop_map(|l| vec![l]),
    ];
    let terminator = || mostly(vec!["\n"], vec!["\r\n", "\r", "\u{2028}", "\u{2029}"]);
    let text = prop::collection::vec(block, 0..4).prop_flat_map(move |blocks| {
        let lines: Vec<String> = blocks.into_iter().flatten().collect();
        let n = lines.len();
        (Just(lines), prop::collection::vec(terminator(), n..=n))
    });
    let text = text.prop_map(|(lines, terms)| {
        lines
            .into_iter()
            .zip(terms)
            .map(|(l, t)| format!("{l}{t}"))
            .collect()
    });
    proptest::option::weighted(0.95, text)
}

#[test]
fn clean_sweep_matches_js() {
    let exports = [
        "shouldRunCleanSweep",
        "sweepFamilyFor",
        "parseSweepFindings",
        "sweepArtifactLooksReal",
    ];

    let gates = sample((
        0usize..3,
        any::<bool>(),
        any::<bool>(),
        pick(vec![None, Some(""), Some("/tmp/peer-review-wt-2101-XXXX")]),
    ));
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for (count, installed, healthy, cwd) in &gates {
        let mut input = json!({"panelEligibleCount": count, "codexInstalled": installed, "codexHealthy": healthy});
        if let Some(c) = cwd {
            input["externalCwd"] = json!(c);
        }
        let gate = clean_sweep::should_run_clean_sweep(&clean_sweep::SweepGateInput {
            panel_eligible_count: *count,
            codex_installed: *installed,
            codex_healthy: *healthy,
            external_cwd: *cwd,
        });
        calls.push(json!({"fn": "shouldRunCleanSweep", "args": [input]}));
        rust.push(json!(gate));
        calls.push(json!({"fn": "sweepFamilyFor", "args": [gate]}));
        rust.push(json!(clean_sweep::sweep_family_for(&gate)));
    }
    let js = oracle("CLEAN-SWEEP", &exports, &[], &calls);
    assert_same("shouldRunCleanSweep/sweepFamilyFor", &calls, &rust, &js);

    let mut texts: Vec<Option<String>> = vec![
        Some("I looked hard.\n\nNO FINDINGS ABOVE NIT.\n\nNits:\nnit stuff here".into()),
        Some("FINDING: important | Count push races the commit | src/services/v2/accounts.ts:585 | two invocations\nEVIDENCE: const actualCount = countData\nFINDING: nit | Padded | src/lib/helpers/textFormat.ts:41 | doubled space\nEVIDENCE: `${a}`.trim() ||".into()),
        Some("FINDING: important | Missing guard | src/a.ts:10 | a null id reaches the write\nEVIDENCE: await write(id)\nVERIFIED: src/b.ts:4 | export function write(id: string) {\nVERIFIED: src/c.ts:9 | write(req.query.id)\nFINDING: nit | Name | src/a.ts:20 | unclear\nEVIDENCE: const x = 1".into()),
        Some("HEAD FAILURE (quota): ERROR: Your workspace is out of credits.\n".into()),
        Some("The change looks good to me overall.".into()),
        Some(String::new()),
        None,
    ];
    texts.extend(sample(sweep_text()));
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for t in &texts {
        calls.push(json!({"fn": "parseSweepFindings", "args": [t]}));
        rust.push(json!(clean_sweep::parse_sweep_findings(t.as_deref())));
        calls.push(json!({"fn": "sweepArtifactLooksReal", "args": [t]}));
        rust.push(json!(clean_sweep::sweep_artifact_looks_real(t.as_deref())));
    }
    let js = oracle("CLEAN-SWEEP", &exports, &[], &calls);
    assert_same(
        "parseSweepFindings/sweepArtifactLooksReal",
        &calls,
        &rust,
        &js,
    );
}

// ===== PANEL-CEILING-TIEBREAK =====

fn summaries() -> impl Strategy<Value = Vec<Value>> {
    let one = (
        pick(vec![0, 1, 2, 4, 4, 5, 8]),
        pick(vec![
            Some(json!(10)),
            Some(json!(11)),
            Some(json!(12)),
            Some(json!(13)),
            None,
        ]),
        any::<bool>(),
        any::<bool>(),
    );
    prop::collection::vec(one, 0..9).prop_map(|items| {
        items
            .into_iter()
            .enumerate()
            .map(|(i, (priority, slot, float, distinct))| {
                let mut m = Map::new();
                // Mostly distinct ids; some repeat or are missing, which a
                // Set compares by `===` and `undefined`.
                match slot {
                    Some(_) if distinct => {
                        m.insert("slotId".into(), json!(100 + i));
                    }
                    Some(s) => {
                        m.insert("slotId".into(), s);
                    }
                    None => {}
                }
                m.insert("title".into(), json!(format!("finding {i}")));
                m.insert("severity".into(), json!("important"));
                m.insert(
                    "priority".into(),
                    if float {
                        json!(f64::from(priority))
                    } else {
                        json!(priority)
                    },
                );
                m.insert("i".into(), json!(i));
                Value::Object(m)
            })
            .collect()
    })
}

fn agent_order(ids: Vec<Value>) -> impl Strategy<Value = Value> {
    let n = ids.len();
    let permuted = Just(ids).prop_shuffle();
    let mutation = pick(vec![0u8, 0, 0, 1, 2, 3, 4, 5, 6]);
    (permuted, mutation, 0..n.max(1)).prop_map(|(mut ids, mutation, at)| {
        match mutation {
            1 if !ids.is_empty() => {
                ids.remove(at.min(ids.len() - 1));
            }
            2 if !ids.is_empty() => {
                let dup = ids[at.min(ids.len() - 1)].clone();
                ids.push(dup);
            }
            3 => ids.push(json!(99)),
            4 => {
                // The same id written as a string, or as a float: only the
                // float is the same id under `===`.
                if let Some(first) = ids.first_mut() {
                    if let Some(x) = first.as_f64() {
                        *first = if at % 2 == 0 {
                            json!(x.to_string())
                        } else {
                            json!(x + 0.0)
                        };
                    }
                }
            }
            5 => return Value::Null,
            6 => return json!("nope"),
            _ => {}
        }
        Value::Array(ids)
    })
}

#[test]
fn panel_ceiling_tiebreak_matches_js() {
    let exports = [
        "panelCeilingTiedGroup",
        "resolvePanelCeilingTiebreak",
        TIEBREAK_CAP,
    ];

    let cases = sample((summaries(), 1usize..7));
    let mut calls = vec![json!({"fn": "panelTiebreakCap", "args": []})];
    let mut rust = vec![json!(panel_ceiling_tiebreak::PANEL_TIEBREAK_CAP)];
    for (eligible, max) in &cases {
        calls.push(json!({"fn": "panelCeilingTiedGroup", "args": [eligible, max]}));
        rust.push(json!(panel_ceiling_tiebreak::panel_ceiling_tied_group(
            eligible,
            NonZeroUsize::new(*max).unwrap()
        )));
    }
    let js = oracle("PANEL-CEILING-TIEBREAK", &exports, &[], &calls);
    assert_same("panelCeilingTiedGroup", &calls, &rust, &js);

    let tied = summaries().prop_flat_map(|group| {
        let ids: Vec<Value> = group
            .iter()
            .map(|f| f.get("slotId").cloned().unwrap_or(Value::Null))
            .collect();
        (Just(group), -2i64..7, agent_order(ids))
    });
    let cases = sample(tied);
    let calls: Vec<Value> = cases
        .iter()
        .map(|(g, slots, order)| json!({"fn": "resolvePanelCeilingTiebreak", "args": [g, slots, order]}))
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|(g, slots, order)| {
            json!(panel_ceiling_tiebreak::resolve_panel_ceiling_tiebreak(
                g,
                *slots,
                Some(order)
            ))
        })
        .collect();
    let js = oracle("PANEL-CEILING-TIEBREAK", &exports, &[], &calls);
    assert_same("resolvePanelCeilingTiebreak", &calls, &rust, &js);
}

// ===== AVAILABILITY-CHECK-GATE =====

#[test]
fn availability_check_gate_matches_js() {
    let cases = sample((any::<bool>(), any::<bool>(), any::<bool>()));
    let calls: Vec<Value> = cases
        .iter()
        .map(|(a, c, g)| json!({"fn": "computeWantsAvailabilityCheck", "args": [a, c, g]}))
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|(a, c, g)| {
            json!(availability_check_gate::compute_wants_availability_check(
                *a, *c, *g
            ))
        })
        .collect();
    let js = oracle(
        "AVAILABILITY-CHECK-GATE",
        &["computeWantsAvailabilityCheck"],
        &[],
        &calls,
    );
    assert_same("computeWantsAvailabilityCheck", &calls, &rust, &js);
}

// ===== EVIDENCE-RESOLVER-DEGRADE =====

#[test]
fn evidence_resolver_degrade_matches_js() {
    let parsed = pick(vec![
        Value::Null,
        json!(false),
        json!(0),
        json!(""),
        json!("text"),
        json!([]),
        json!({}),
        json!({"ran": false}),
        json!({"ran": true}),
        json!({"ran": "false"}),
        json!({"ran": false, "reason": "invalid-diff"}),
        json!({"ran": false, "reason": "diff-unreadable"}),
        json!({"reason": ""}),
        json!({"reason": 0}),
        json!({"reason": 5.5}),
        json!({"reason": ["a", "b"]}),
        json!({"reason": {}}),
        json!({"reason": true}),
    ]);
    let relay = pick(vec![
        None,
        Some(String::new()),
        Some("checksum mismatch: the script printed a, the copy is b".to_owned()),
    ]);
    let cases = sample((parsed, 0usize..2000, relay, any::<bool>()));
    let calls: Vec<Value> = cases
        .iter()
        .map(|(p, n, r, omit)| {
            // An omitted third argument takes the `= null` default.
            let args = if r.is_none() && *omit {
                json!([p, n])
            } else {
                json!([p, n, r])
            };
            json!({"fn": "evidenceResolverFailure", "args": args})
        })
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|(p, n, r, _)| {
            json!(evidence_resolver_degrade::evidence_resolver_failure(
                Some(p),
                *n,
                r.as_deref()
            ))
        })
        .collect();
    let js = oracle(
        "EVIDENCE-RESOLVER-DEGRADE",
        &["evidenceResolverFailure"],
        &[],
        &calls,
    );
    assert_same("evidenceResolverFailure", &calls, &rust, &js);
}

// ===== DEDUP-VETO =====

fn candidate() -> impl Strategy<Value = Value> {
    // A third of the pairs have no usable line, so the title tokens decide.
    let line = prop_oneof![3 => Just(None), 5 => pick(vec![
        Some(json!(10)),
        Some(json!(50)),
        Some(json!(51)),
        Some(json!(400)),
        Some(json!(0)),
        Some(json!(-5)),
        Some(json!(30.5)),
        Some(json!("12")),
        Some(json!(" 50 ")),
        Some(json!("0x20")),
        Some(json!("abc")),
        Some(json!("")),
        Some(json!([30])),
        Some(json!(true)),
        Some(Value::Null),
    ])];
    let word = pick(vec![
        "merge",
        "Transaction",
        "missing",
        "guard",
        "the",
        "and",
        "ab",
        "\u{212A}ey",
        "key",
        "KEY",
        "x_y",
        "a1b",
        "\u{0130}nvoice",
        "invoice",
        "\u{017F}tate",
        "state",
        "Stra\u{00DF}e",
        "into",
        "404",
    ]);
    let sep = pick(vec![" ", " ", "-", ": ", "\u{00A0}", "é"]);
    let words = prop::collection::vec((word, sep), 0..6).prop_map(|ws| {
        ws.into_iter()
            .map(|(w, s)| format!("{w}{s}"))
            .collect::<String>()
    });
    let title = prop_oneof![
        8 => words.prop_map(|s| Some(json!(s))),
        1 => Just(None),
        1 => Just(Some(Value::Null)),
        1 => Just(Some(json!(12345))),
        1 => Just(Some(json!(["merge transaction", "guard"]))),
        1 => Just(Some(json!({}))),
    ];
    (line, title).prop_map(|(l, t)| {
        let mut m = Map::new();
        m.insert("file".into(), json!("src/a.ts"));
        maybe(&mut m, "line", l);
        maybe(&mut m, "title", t);
        Value::Object(m)
    })
}

/// Title pairs that agree only once U+212A and U+0130 are lowercased the
/// Unicode way, and pairs that sit at the 0.4 overlap threshold.
fn dedup_seeds() -> Vec<(Value, Value)> {
    [
        ("\u{212A}ey rotation skipped", "key rotation"),
        ("\u{0130}nvoice total drifts", "invoice total"),
        ("\u{017F}tate machine", "state machine"),
        ("Stra\u{00DF}e lookup", "strasse lookup"),
        (
            "alpha beta gamma delta epsilon",
            "alpha beta zeta theta iota",
        ),
        (
            "alpha beta gamma delta epsilon",
            "alpha zeta theta iota kappa",
        ),
        ("the and of", "the and of"),
        (
            "one two three four five six seven",
            "one two three eight nine ten eleven",
        ),
    ]
    .into_iter()
    .map(|(a, b)| (json!({"title": a}), json!({"line": 900, "title": b})))
    .collect()
}

#[test]
fn dedup_veto_matches_js() {
    let mut cases = dedup_seeds();
    cases.extend(sample((candidate(), candidate())));
    let calls: Vec<Value> = cases
        .iter()
        .map(|(a, b)| json!({"fn": "mergeableWithin", "args": [a, b]}))
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|(a, b)| json!(dedup_veto::mergeable_within(a, b)))
        .collect();
    let js = oracle("DEDUP-VETO", &["mergeableWithin"], &[], &calls);
    assert_same("mergeableWithin", &calls, &rust, &js);
}

// ===== Adversarial edges =====
//
// Hand-picked inputs aimed at the JavaScript coercions the generators above
// reach rarely or not at all.

#[test]
fn artifact_truth_edges_match_js() {
    let ok = json!({"failclass": "none", "verdict": "REFUTED", "confidence": 0.9});
    let entries = [
        Some(ok.clone()),
        None,
        Some(json!({"failclass": []})),
        Some(json!({"failclass": ["a", null, ["b"]]})),
        Some(json!({"failclass": {"x": 1}})),
        Some(json!({"failclass": 1.5})),
        Some(json!({"failclass": 1e21})),
        Some(json!({"failclass": true})),
        Some(json!({"failclass": "none", "verdict": 0})),
        Some(json!({"failclass": "none", "verdict": {"v": 1}})),
        Some(json!({"failclass": "none", "verdict": "REFUTED", "confidence": 1e-7})),
        Some(json!({"failclass": "none", "verdict": "REFUTED", "confidence": 1e21})),
        Some(json!({"failclass": "none", "verdict": "REFUTED", "confidence": -0.0})),
        Some(json!({"failclass": "none", "verdict": "REFUTED", "reason": 7})),
        Some(json!({"failclass": "none", "verdict": "REFUTED", "reason": ["r"]})),
    ];
    // A vote of the wrong type: a string or an array spreads its indices into
    // the result; a number or `true` spreads nothing.
    let votes = vec![
        json!("ab"),
        json!(["x", {"k": 1}]),
        json!(5),
        json!(true),
        json!({"verdict": {"v": 1}, "confidence": 0.9, "slotStatus": "OK"}),
        json!({"verdict": "REFUTED", "confidence": "0.9", "slotStatus": "OK"}),
        json!({"0": "zero", "verdict": "REFUTED", "failureReason": "x"}),
    ];
    let mut cases = Vec::new();
    for entry in &entries {
        for relayed in [true, false] {
            let truth = match entry {
                Some(e) => json!({ "2r1-codex": e }),
                None => json!({}),
            };
            let panel = vec![json!({"source": "codex", "task": relayed}); votes.len()];
            cases.push((votes.clone(), panel, truth, 2, 1));
        }
    }
    check_artifact_truth("applyArtifactTruth (edges)", &cases);
}

#[test]
fn vote_quotes_edges_match_js() {
    let exports = ["checkableVoteQuotes", "voteQuoteVerdicts"];
    let vote = |c: Value| json!({"verdict": "REFUTED", "introducedByPr": "y", "contradiction": c});
    let states = vec![
        json!({"finding": {"file": "f.ts"}, "votes": [
            vote(json!({"quote": 1e21, "line": 1e21})),
            vote(json!({"quote": [[" "], null], "line": -0.0})),
            vote(json!({"quote": [{}], "line": 3, "file": []})),
            vote(json!({"quote": "q", "line": 18446744073709551615u64, "file": 0})),
            vote(json!({"quote": "\u{180E}", "line": 2})),
            vote(json!({"quote": "\u{200B}", "line": 2})),
            vote(json!({"quote": true, "file": {"a": 1}})),
            vote(json!("text")),
            vote(json!([{"quote": "q"}])),
            json!({"verdict": "REFUTED", "introducedByPr": "", "contradiction": {"quote": "q"}}),
            json!({"verdict": "REFUTED", "introducedByPr": ["y"], "contradiction": {"quote": "q"}}),
        ]}),
        json!({"finding": "f.ts", "votes": [vote(json!({"quote": "q"}))]}),
        json!({"finding": [], "skipped": 0, "votes": [vote(json!({"quote": "q"}))]}),
        json!({"skipped": "", "votes": []}),
    ];
    let calls = vec![json!({"fn": "checkableVoteQuotes", "args": [states]})];
    let rust = vec![json!(vote_quotes::checkable_vote_quotes(Some(&states)))];
    let js = oracle("VOTE-QUOTES", &exports, &[], &calls);
    assert_same("checkableVoteQuotes (edges)", &calls, &rust, &js);

    let indices = [
        json!(-0.0),
        json!("1e0"),
        json!("0x1"),
        json!(" 1"),
        json!(1e-7),
        json!([]),
        json!([["1"]]),
        json!({}),
        json!(false),
        json!(""),
        json!("-0"),
        json!(4294967295u64),
        json!(2.0000000000000004),
    ];
    let statuses = [
        json!(["found", "x"]),
        json!(["not-found"]),
        json!({}),
        json!("found\n"),
        json!("not-found\n"),
        json!("FOUND"),
        json!(1),
    ];
    let mut cases = Vec::new();
    for i in &indices {
        for s in &statuses {
            cases.push((json!([{"findingIndex": i, "status": s}]), 5usize));
        }
    }
    cases.push((json!([0, {"status": "lookup-failed"}, null]), 2));
    cases.push((json!([{"status": "found"}, null]), 2));
    cases.push((json!([false, "", {"status": "file-not-at-ref"}]), 2));
    cases.push((json!([{"findingIndex": 0, "status": "file-not-at-ref"}]), 2));
    cases.push((json!([{"findingIndex": 0, "status": "found"}]), 0));
    let calls: Vec<Value> = cases
        .iter()
        .map(|(l, n)| json!({"fn": "voteQuoteVerdicts", "args": [l, n]}))
        .collect();
    let rust: Vec<Value> = cases
        .iter()
        .map(|(l, n)| as_outcome(vote_quotes::vote_quote_verdicts(Some(l), *n).map(|v| json!(v))))
        .collect();
    let js: Vec<Value> = oracle_raw("VOTE-QUOTES", &exports, &[], &calls)
        .into_iter()
        .map(as_outcome)
        .collect();
    assert_same("voteQuoteVerdicts (edges)", &calls, &rust, &js);
}

#[test]
fn clean_sweep_edges_match_js() {
    let exports = ["parseSweepFindings", "sweepArtifactLooksReal"];
    let texts = [
        "FINDING:nit|t|f:1|w",
        "FINDING: critical | t | a.ts:12 |",
        "FINDING: critical | t | a.ts:12 | ",
        "FINDING: critical | t | a.ts:12 |\u{2028}",
        "FINDING: critical | t | a.ts:12 | \u{2028} x",
        "FINDING: critical | t | a.ts:12 | x\u{2028}",
        "FINDING: critical | t | C:\\x.ts:12 | w",
        "FINDING: critical | t | a.ts:12:5 | w",
        "FINDING: critical | a:b | a.ts:12 | w | more | pipes",
        "FINDING: critical | | a.ts:12 | w",
        "FINDING: critical |\u{2028}| a.ts:12 | w",
        "FINDING: critical | t |\u{2028}:12 | w",
        "FINDING: critical | t | :12 | w",
        "FINDING: critical | t | a.ts:\u{0661}2 | w",
        "FINDING: critical | t | a.ts:12\u{2029}| w",
        "FINDING: critical\u{FEFF}|\u{FEFF}t\u{FEFF}|\u{FEFF}a.ts:12\u{FEFF}|\u{FEFF}w\u{FEFF}",
        "FINDING: critical\u{0085}| t | a.ts:12 | w",
        "FINDING: criticalx | t | a.ts:12 | w",
        "FINDING: nit | t | a.ts:00012 | w\nEVIDENCE:\nVERIFIED: b.ts:1 | q",
        "FINDING: nit | t | a.ts:1 | w\nEVIDENCE: \u{2028}\nVERIFIED: b.ts:1 | q",
        "FINDING: nit | t | a.ts:1 | w\nEVIDENCE:\u{3000}x\u{3000}\nVERIFIED:\tb.ts\t:\t1 | q",
        "FINDING: nit | t | a.ts:1 | w\nEVIDENCE: e\nVERIFIED: b.ts:1|q\nVERIFIED: |:1 | q",
        "FINDING: nit | t | a.ts:1 | w\nVERIFIED: b.ts:1 | q\nFINDING: nit | u | c.ts:2 | v",
        "FINDING: nit | t | a.ts:1 | w\nFINDING: nit | u | c.ts:2 | v\nEVIDENCE: e",
        "x\rHEAD FAILURE (auth): bad token\r\nmore",
        "x\u{2028}HEAD FAILURE (error): \u{2029}",
        "HEAD FAILURE (quota): ",
        "HEAD FAILURE (quota):",
        " HEAD FAILURE (quota): x",
        "FINDING: nit | t | a.ts:1 | w\nHEAD FAILURE (quota): later",
        "no findings above nit\u{0130}",
        "NO FINDINGS ABOVE\u{00A0}NIT",
        "nO fInDiNgS aBoVe NiT",
        "\u{FEFF}\u{3000}",
        "\u{0085}",
        "FINDING:",
    ];
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for t in texts {
        calls.push(json!({"fn": "parseSweepFindings", "args": [t]}));
        rust.push(json!(clean_sweep::parse_sweep_findings(Some(t))));
        calls.push(json!({"fn": "sweepArtifactLooksReal", "args": [t]}));
        rust.push(json!(clean_sweep::sweep_artifact_looks_real(Some(t))));
    }
    let js = oracle("CLEAN-SWEEP", &exports, &[], &calls);
    assert_same("parseSweepFindings (edges)", &calls, &rust, &js);
}

#[test]
fn panel_tiebreak_edges_match_js() {
    let exports = ["panelCeilingTiedGroup", "resolvePanelCeilingTiebreak"];
    let s = |id: Value, p: Value, i: Value| json!({"slotId": id, "priority": p, "i": i});
    let groups = vec![
        vec![
            s(json!(1), json!(-0.0), json!(0)),
            s(json!(2), json!(0), json!(1)),
            s(json!(3), json!(0), json!(2)),
        ],
        vec![
            s(json!(1), json!(3), json!(5)),
            s(json!(2), json!(3), json!(5)),
            s(json!(3), json!(3), json!(1)),
            s(json!(4), json!(1), json!(0)),
        ],
        vec![
            s(json!(1), json!(0.1), json!(0)),
            s(json!(2), json!(0.30000000000000004), json!(1)),
            s(json!(3), json!(0.3), json!(2)),
        ],
    ];
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for g in &groups {
        for max in 1..g.len() {
            calls.push(json!({"fn": "panelCeilingTiedGroup", "args": [g, max]}));
            rust.push(json!(panel_ceiling_tiebreak::panel_ceiling_tied_group(
                g,
                NonZeroUsize::new(max).unwrap()
            )));
        }
    }
    let tied = vec![
        json!({"slotId": 1}),
        json!({"slotId": -0.0}),
        json!({"slotId": "1"}),
        json!({}),
        json!({"slotId": null}),
        json!({"slotId": [1]}),
    ];
    let orders = [
        json!([1, 0, "1", null]),
        json!([1, -0.0, "1", null]),
        json!([1.0, 0, "1"]),
        json!([[1], 1, 0, "1"]),
        json!({"0": 1}),
    ];
    for o in &orders {
        for slots in [-9i64, -1, 0, 2, 9] {
            calls.push(json!({"fn": "resolvePanelCeilingTiebreak", "args": [tied, slots, o]}));
            rust.push(json!(
                panel_ceiling_tiebreak::resolve_panel_ceiling_tiebreak(&tied, slots, Some(o))
            ));
        }
    }
    let js = oracle("PANEL-CEILING-TIEBREAK", &exports, &[], &calls);
    assert_same("panel tiebreak (edges)", &calls, &rust, &js);
}

#[test]
fn small_regions_edges_match_js() {
    let parsed = [
        json!({"reason": 1e21}),
        json!({"reason": -0.0}),
        json!({"reason": [[]]}),
        json!({"reason": [null]}),
        json!({"reason": [1e-7, {}]}),
        json!({"ran": 0}),
        json!({"ran": [false]}),
        json!(true),
        json!(-0.0),
    ];
    let calls: Vec<Value> = parsed
        .iter()
        .map(|p| json!({"fn": "evidenceResolverFailure", "args": [p, 3, "r"]}))
        .collect();
    let rust: Vec<Value> = parsed
        .iter()
        .map(|p| {
            json!(evidence_resolver_degrade::evidence_resolver_failure(
                Some(p),
                3,
                Some("r")
            ))
        })
        .collect();
    let js = oracle(
        "EVIDENCE-RESOLVER-DEGRADE",
        &["evidenceResolverFailure"],
        &[],
        &calls,
    );
    assert_same("evidenceResolverFailure (edges)", &calls, &rust, &js);

    let lines = [
        json!("Infinity"),
        json!("-Infinity"),
        json!("+12"),
        json!("1."),
        json!(".5e1"),
        json!("1e1"),
        json!("0b101"),
        json!("0o17"),
        json!("0X1f"),
        json!("-0x10"),
        json!("0x"),
        json!("5e"),
        json!("\u{00A0}12\u{FEFF}"),
        json!("\u{0085}12"),
        json!("\u{2028}12\u{3000}"),
        json!("1_0"),
        json!("infinity"),
        json!("12px"),
        json!([[30]]),
        json!([]),
        json!([null]),
        json!([1, 2]),
        json!({"a": 1}),
        json!(false),
        json!(1e300),
        json!(-0.0),
    ];
    let titles = [
        json!("\u{212A}ELVIN \u{212A}ey"),
        json!("ǅungla \u{0130}\u{0130}x"),
        json!("ÀBC abc"),
        json!("ΣΑΣ σας abc"),
        json!("\u{FB00}ix"),
        json!("ab_ cd_"),
        json!(1.5e300),
        json!([["abc"], "def"]),
    ];
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for la in &lines {
        for lb in [json!(10), json!("30"), json!(1e300)] {
            let a = json!({"line": la, "title": "zzz"});
            let b = json!({"line": lb, "title": "yyy"});
            calls.push(json!({"fn": "mergeableWithin", "args": [a, b]}));
            rust.push(json!(dedup_veto::mergeable_within(&a, &b)));
        }
    }
    for ta in &titles {
        for tb in &titles {
            let a = json!({"title": ta});
            let b = json!({"title": tb});
            calls.push(json!({"fn": "mergeableWithin", "args": [a, b]}));
            rust.push(json!(dedup_veto::mergeable_within(&a, &b)));
        }
    }
    let js = oracle("DEDUP-VETO", &["mergeableWithin"], &[], &calls);
    assert_same("mergeableWithin (edges)", &calls, &rust, &js);
}

// ===== Recorded fixtures =====

fn harness_fixtures() -> PathBuf {
    let root = std::env::var("TRIBUNAL_HARNESS_DATA").expect("TRIBUNAL_HARNESS_DATA is not set");
    PathBuf::from(root).join("tribunal/fixtures")
}

fn read_json(path: PathBuf) -> Value {
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

/// The PR #1911 votes and artifacts `replay-vote-derivation.mjs` replays, and
/// every same-file candidate pair of the dedup fixture.
#[test]
#[ignore = "needs TRIBUNAL_HARNESS_DATA"]
fn votes_match_js_on_recorded_fixtures() {
    let dir = harness_fixtures();
    let truth = read_json(dir.join("vote-derivation/classified.json"))["entries"].clone();
    let recorded = read_json(dir.join("vote-derivation/recorded-votes.json"));
    let votes = recorded.as_array().expect("recorded votes are an array");
    assert!(!votes.is_empty());
    let cases: Vec<TruthCase> = votes
        .iter()
        .map(|v| {
            (
                vec![v.clone()],
                vec![json!({"source": v["source"], "task": true})],
                truth.clone(),
                v["slotId"].as_u64().unwrap(),
                0,
            )
        })
        .collect();
    check_artifact_truth("applyArtifactTruth (recorded)", &cases);

    let fixture = read_json(dir.join("dedup/pr-1911-clusters.json"));
    let candidates = fixture["candidates"].as_array().unwrap();
    let mut calls = Vec::new();
    let mut rust = Vec::new();
    for a in candidates {
        for b in candidates {
            if a["file"] == b["file"] {
                calls.push(json!({"fn": "mergeableWithin", "args": [a, b]}));
                rust.push(json!(dedup_veto::mergeable_within(a, b)));
            }
        }
    }
    assert!(!calls.is_empty());
    let js = oracle("DEDUP-VETO", &["mergeableWithin"], &[], &calls);
    assert_same("mergeableWithin (recorded)", &calls, &rust, &js);
}
