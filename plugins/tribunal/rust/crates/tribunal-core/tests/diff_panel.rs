mod common;
use common::*;

// Differential tests for the panel group: each ported function and its
// JavaScript region run on the same generated inputs, plus the hand-written
// cases of the region's replays as seeds.
//
// The generators lean on what these regions read loosely: JSON values of the
// wrong type where the JavaScript checks truthiness or `typeof`, the `trim`
// whitespace set, `\W` splitting on non-ASCII letters, Unicode lower-casing
// (U+212A, U+0130, final sigma), the 0.4 token-overlap and the 0.45 / 0.1
// thresholds, number-to-string in messages, and object key order.
//
// Out of scope by schema (see the module docs): non-string paths, where
// `samePath` throws, and severities named after `Object.prototype` members.
// Integer-like object keys are not generated: `JSON.parse` reorders them in
// every JavaScript object, which is not a property of these regions.
//
// The JavaScript's side is read from `tests/goldens/oracle/diff_panel.json`
// (see `common`): the workflow is being retired, so a run starts only the
// port. The file keeps each call's results under a SHA-256 of its inputs and a
// SHA-256 of every region's text, so a region edited without regenerating
// fails here; with the workflow gone the golden is final. Regenerate:
// `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-core --test diff_panel`.

use proptest::prelude::*;
use serde_json::{json, Map, Value};

use tribunal_core::panel::cost_breaker::{self, CostInputs};
use tribunal_core::panel::debate_degrade;
use tribunal_core::panel::panel_gates::{self as gates, DiscardedVote, Tally};
use tribunal_core::panel::spent_counter::SpentCounter;
use tribunal_core::panel::strengthen_merge;

type Field = BoxedStrategy<Option<Value>>;

/// One of `items`, each repeated by its weight.
fn pick(items: Vec<(u32, Option<Value>)>) -> Field {
    let pool: Vec<Option<Value>> = items
        .into_iter()
        .flat_map(|(w, v)| std::iter::repeat_n(v, w as usize))
        .collect();
    prop::sample::select(pool).boxed()
}

fn s(x: &str) -> Option<Value> {
    Some(json!(x))
}

/// An object with the given fields in order, leaving out the absent ones.
fn object_of(fields: Vec<(&'static str, Field)>) -> BoxedStrategy<Value> {
    let (names, strategies): (Vec<&str>, Vec<Field>) = fields.into_iter().unzip();
    strategies
        .prop_map(move |vals| {
            let mut m = Map::new();
            for (n, v) in names.iter().zip(vals) {
                if let Some(v) = v {
                    m.insert((*n).to_owned(), v);
                }
            }
            Value::Object(m)
        })
        .boxed()
}

/// The same, with the key order shuffled.
fn shuffled_object(fields: Vec<(&'static str, Field)>) -> BoxedStrategy<Value> {
    let (names, strategies): (Vec<&str>, Vec<Field>) = fields.into_iter().unzip();
    strategies
        .prop_map(move |vals| names.iter().copied().zip(vals).collect::<Vec<_>>())
        .prop_shuffle()
        .prop_map(|pairs| {
            let mut m = Map::new();
            for (n, v) in pairs {
                if let Some(v) = v {
                    m.insert(n.to_owned(), v);
                }
            }
            Value::Object(m)
        })
        .boxed()
}

const PATHS: [&str; 9] = [
    "src/jobs/worker.ts",
    "worker.ts",
    "src/jobs/profiles_worker.ts",
    "jobs/worker.ts",
    "src/jobs/queue.ts",
    "scripts/invoices/other-file.ts",
    "scripts/invoices/bulk-late-fee-application.ts",
    "src/a.ts",
    "",
];

const SUMMARIES: [&str; 9] = [
    "ledger flush timing deferred by the author",
    "BY-DESIGN: retryUntilRedisReady is not added to the accounts, orders, catalog_search or worker.ts workers; each accounts scheduler registration already has its own try/catch",
    "Ledger Flush deferred",
    "no auth checks on these CLI scripts, by design",
    "Fa\u{00E7}ade \u{039F}\u{0394}\u{039F}\u{03A3} caching deferred \u{2014} \u{0130}stanbul rollout",
    "per-batch ledger flushing",
    "\u{212A}elvin startup retry deferred",
    "retry_during startup scheduler",
    "",
];

const QUESTIONED: [&str; 22] = [
    "ledger flush timing deferred by the author",
    "LEDGER FLUSH",
    "  ledger flush timing  ",
    "ledger flush alpha beta gamma",
    "ledger alpha beta gamma delta",
    "flush",
    "ledger\u{00B7}flush\u{00B7}timing",
    "ade fa",
    "fa\u{00E7}ade caching",
    "\u{039F}\u{0394}\u{039F}\u{03A3}",
    "\u{03BF}\u{03B4}\u{03BF}\u{03C2} caching",
    "kelvin startup",
    "\u{212A}elvin",
    "startup retry",
    "retryUntilRedisReady",
    "deferred by the author",
    "per batch ledger flushing scheduled",
    "BY-DESIGN: retryUntilRedisReady is not added to the accounts, orders, catalog_search or worker.ts workers; each accounts scheduler registration already has its own try/catch",
    "no auth checks",
    "x",
    "\u{0130}stanbul",
    "registration reminders scheduler startup",
];

const TITLES: [&str; 8] = [
    "Accounts schedulers bypass the new startup retry",
    "Invoice totals rounded twice",
    "ledgerFlush timing is wrong",
    "LEDGERFlush TIMING deferred",
    "\u{212A}elvin startup retry",
    "a1B2c3D4 scheduler",
    "Fa\u{00E7}ade caching \u{039F}\u{0394}\u{039F}\u{03A3} deferred",
    "",
];

const DESCRIPTIONS: [&str; 6] = [
    "signupRemindersQueue schedulers are registered at boot without retryUntilRedisReady",
    "per-batch ledger flushing duplicates rows",
    "retry_during startup scheduler",
    "redisStartupRetry flushLedger",
    "\u{03A3}\u{038A}\u{03A3}\u{03A5}\u{03A6}\u{039F}\u{03A3} \u{0130}stanbul",
    "",
];

fn from_pool(pool: &[&str], weight: u32) -> Vec<(u32, Option<Value>)> {
    pool.iter().map(|x| (weight, s(x))).collect()
}

fn vote() -> BoxedStrategy<Value> {
    let mut dq = vec![
        (4, None),
        (2, s("")),
        (2, s("  \u{FEFF} ")),
        (1, Some(json!(5))),
        (1, Some(json!(true))),
        (1, Some(Value::Null)),
    ];
    dq.extend(from_pool(&QUESTIONED, 1));
    dq.extend(from_pool(&SUMMARIES, 1));
    object_of(vec![
        (
            "source",
            pick(vec![
                (4, s("claude")),
                (3, s("codex")),
                (2, s("gemini")),
                (1, None),
                (1, s("")),
                (1, Some(Value::Null)),
            ]),
        ),
        (
            "verdict",
            pick(vec![
                (6, s("REFUTED")),
                (5, s("NOT_REFUTED")),
                (1, None),
                (1, s("refuted")),
                (1, Some(Value::Null)),
            ]),
        ),
        (
            "confidence",
            pick(
                [
                    0.0, 0.1, 0.3, 0.45, 0.5, 0.55, 0.59999, 0.6, 0.7, 0.8, 0.85, 0.9, 0.96, 1.0,
                    1.5, -0.2,
                ]
                .iter()
                .map(|c| (1, Some(json!(c))))
                .chain([(2, None), (1, Some(Value::Null)), (1, s("0.9"))])
                .collect(),
            ),
        ),
        (
            "slotStatus",
            pick(vec![
                (8, s("OK")),
                (2, s("UNAVAILABLE")),
                (1, None),
                (1, s("unavailable")),
            ]),
        ),
        (
            "failureReason",
            pick(vec![
                (4, None),
                (1, s("timeout")),
                (1, s("quota")),
                (1, s("")),
            ]),
        ),
        (
            "refutationType",
            pick(vec![
                (3, None),
                (4, s("PRE_DECIDED")),
                (2, s("TECHNICALLY_WRONG")),
                (1, s("NO_REFUTATION")),
            ]),
        ),
        ("decisionQuestioned", pick(dq)),
        (
            "introducedByPr",
            pick(vec![
                (3, None),
                (2, s("yes")),
                (3, s("no")),
                (1, s("unsure")),
                (1, Some(Value::Null)),
            ]),
        ),
        (
            "killReason",
            pick(vec![
                (2, None),
                (3, s("not-triggered")),
                (2, s("incorrect-premise")),
                (1, s("pre-existing")),
                (1, s("style-not-bug")),
            ]),
        ),
        (
            "contradiction",
            pick(vec![
                (2, None),
                (1, Some(Value::Null)),
                (
                    4,
                    Some(json!({"file": "src/a.ts", "line": 10, "quote": "if (!x) return;"})),
                ),
                (1, Some(json!({"quote": " \u{3000}\u{00A0} "}))),
                (1, Some(json!({"quote": "\u{0085}"}))),
                (1, Some(json!({"quote": "\u{FEFF}"}))),
                (1, Some(json!({"quote": ""}))),
                (1, s("str")),
                (1, Some(json!({"quote": 5}))),
                (1, Some(json!({"quote": 0}))),
                (1, Some(json!({"file": "a.ts"}))),
            ]),
        ),
        (
            "quoteFound",
            pick(vec![
                (3, None),
                (1, Some(json!(true))),
                (2, Some(json!(false))),
                (1, Some(Value::Null)),
            ]),
        ),
        (
            "coreStands",
            pick(vec![
                (3, None),
                (2, Some(json!(true))),
                (2, Some(json!(false))),
                (1, Some(json!(1))),
                (1, Some(json!(0))),
                (1, s("yes")),
            ]),
        ),
        (
            "proposedSeverity",
            pick(vec![
                (2, None),
                (3, s("nit")),
                (2, s("important")),
                (1, s("critical")),
                (1, s("NIT")),
                (1, Some(json!(["nit"]))),
            ]),
        ),
        (
            "intendedBy",
            pick(vec![
                (4, None),
                (
                    3,
                    Some(
                        json!({"file": "CLAUDE.md", "rule": "Never hardcode HTTP status numbers"}),
                    ),
                ),
                (1, Some(json!({"file": "", "rule": "x is by design"}))),
                (1, Some(json!({"rule": "  "}))),
                (1, Some(json!({"rule": 5}))),
                (1, Some(json!({"rule": "\u{FEFF}"}))),
                (1, Some(json!({"rule": "\u{0085}"}))),
                (1, s("rule")),
                (1, Some(Value::Null)),
            ]),
        ),
    ])
}

fn votes() -> impl Strategy<Value = Vec<Value>> {
    let entry = prop_oneof![
        12 => vote(),
        2 => Just(Value::Null),
        1 => prop::sample::select(vec![json!(false), json!(0), json!("")]),
        1 => Just(json!("x")),
    ];
    prop::collection::vec(entry, 0..6)
}

fn panel() -> impl Strategy<Value = Vec<Value>> {
    let slot = prop_oneof![
        8 => object_of(vec![(
            "source",
            pick(vec![(3, s("claude")), (2, s("codex")), (1, s("gemini")), (1, s("")), (1, None)]),
        )]),
        1 => Just(Value::Null),
        1 => Just(json!("s")),
    ];
    prop::collection::vec(slot, 0..6)
}

fn finding() -> BoxedStrategy<Value> {
    let mut file: Vec<(u32, Option<Value>)> = from_pool(&PATHS, 2);
    file.push((2, None));
    let mut title = from_pool(&TITLES, 1);
    title.extend([(1, None), (1, Some(json!(5))), (1, Some(Value::Null))]);
    let mut description = from_pool(&DESCRIPTIONS, 1);
    description.push((1, None));
    object_of(vec![
        ("file", pick(file)),
        ("title", pick(title)),
        ("description", pick(description)),
        (
            "severity",
            pick(vec![
                (3, s("important")),
                (2, s("critical")),
                (2, s("nit")),
                (1, None),
                (1, Some(json!(["critical"]))),
                (1, Some(json!(1))),
            ]),
        ),
        (
            "touchesHighRiskSurface",
            pick(vec![
                (3, Some(json!(true))),
                (2, Some(json!(false))),
                (2, None),
                (1, Some(Value::Null)),
                (1, Some(json!(0))),
                (1, Some(json!(1))),
            ]),
        ),
    ])
}

fn path_list(len: std::ops::Range<usize>) -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(prop::sample::select(PATHS.to_vec()), len)
        .prop_map(|v| v.into_iter().map(str::to_owned).collect())
}

fn decisions() -> impl Strategy<Value = Value> {
    let mut summary = from_pool(&SUMMARIES, 2);
    summary.extend([(1, None), (1, Some(json!(5)))]);
    let files = prop_oneof![
        2 => Just(None),
        1 => Just(Some(json!([]))),
        1 => Just(Some(Value::Null)),
        6 => path_list(1..4).prop_map(|v| Some(json!(v))),
    ]
    .boxed();
    let item = prop_oneof![
        10 => object_of(vec![
            ("category", pick(vec![(1, s("DEFERRED")), (1, s("BY-DESIGN"))])),
            ("summary", pick(summary)),
            ("source", pick(vec![(3, s("bob, 2026-10-16")), (1, s("")), (1, None)])),
            ("files", files),
        ]),
        1 => Just(Value::Null),
    ];
    prop_oneof![
        1 => prop::sample::select(vec![Value::Null, json!({}), json!("d")]),
        2 => Just(json!([])),
        7 => prop::collection::vec(item, 1..4).prop_map(Value::Array),
    ]
}

#[derive(Debug, Clone)]
struct PanelCase {
    votes: Vec<Value>,
    panel: Vec<Value>,
    finding: Value,
    decisions: Value,
    decisions_given: Option<bool>,
    pr_files: Vec<String>,
    claude_r0: usize,
    line_added: bool,
}

fn panel_case() -> impl Strategy<Value = PanelCase> {
    (
        votes(),
        panel(),
        finding(),
        decisions(),
        prop::sample::select(vec![None, None, None, Some(true), Some(false), Some(false)]),
        prop_oneof![
            1 => path_list(0..4),
            2 => Just(PATHS.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>()),
        ],
        0usize..3,
        any::<bool>(),
    )
        .prop_map(
            |(
                votes,
                panel,
                finding,
                decisions,
                decisions_given,
                pr_files,
                claude_r0,
                line_added,
            )| {
                PanelCase {
                    votes,
                    panel,
                    finding,
                    decisions,
                    decisions_given,
                    pr_files,
                    claude_r0,
                    line_added,
                }
            },
        )
}

fn ok_vote(source: &str, verdict: &str, confidence: f64, extra: Value) -> Value {
    let mut m =
        json!({"source": source, "verdict": verdict, "confidence": confidence, "slotStatus": "OK"});
    for (k, v) in extra.as_object().unwrap() {
        m[k] = v.clone();
    }
    m
}

/// The goldens of replay-gates.mjs and replay-vote-quotes.mjs, as cases.
fn panel_seeds() -> Vec<PanelCase> {
    let base = |votes: Vec<Value>| PanelCase {
        panel: votes
            .iter()
            .map(|v| json!({"source": v.get("source")}))
            .collect(),
        votes,
        finding: json!({"touchesHighRiskSurface": false}),
        decisions: json!([]),
        decisions_given: None,
        pr_files: vec![],
        claude_r0: 1,
        line_added: false,
    };
    let none = json!({});
    let ledger = json!([{
        "category": "DEFERRED",
        "summary": "ledger flush timing deferred by the author",
        "source": "thread",
        "files": ["scripts/invoices/other-file.ts"],
    }]);
    let b4_summary = SUMMARIES[1];
    let b4 = json!([{
        "category": "BY-DESIGN",
        "summary": b4_summary,
        "source": "bob, 2026-10-16",
        "files": ["src/jobs/profiles_worker.ts", "src/jobs/orders_worker.ts", "src/jobs/worker.ts"],
    }]);
    let b4_pr: Vec<String> = [
        "src/jobs/queue.ts",
        "src/jobs/profiles_worker.ts",
        "src/jobs/orders_worker.ts",
        "src/jobs/worker.ts",
    ]
    .map(str::to_owned)
    .to_vec();
    let b4_finding = json!({
        "file": "src/jobs/queue.ts",
        "title": "Accounts schedulers bypass the new startup retry",
        "description": "signupRemindersQueue schedulers are registered at boot without retryUntilRedisReady",
    });
    let v2 = |verdict: &str, c: f64, extra: Value| {
        let mut e = json!({"introducedByPr": "yes", "proposedSeverity": "important"});
        for (k, v) in extra.as_object().unwrap() {
            e[k] = v.clone();
        }
        ok_vote("claude", verdict, c, e)
    };
    let quote = json!({"file": "a.ts", "line": 3, "quote": "if (!x) return;"});
    let rule = json!({"file": "CLAUDE.md", "rule": "Never hardcode HTTP status numbers"});

    let mut out = vec![
        base(vec![
            ok_vote("claude", "NOT_REFUTED", 0.85, json!({"coreStands": true})),
            ok_vote("codex", "REFUTED", 0.85, none.clone()),
        ]),
        base(vec![
            ok_vote("claude", "REFUTED", 0.9, none.clone()),
            ok_vote("codex", "REFUTED", 0.96, none.clone()),
        ]),
        PanelCase {
            claude_r0: 2,
            ..base(vec![
                ok_vote("claude", "NOT_REFUTED", 0.8, json!({"coreStands": true})),
                ok_vote("claude", "REFUTED", 0.8, none.clone()),
            ])
        },
        PanelCase {
            panel: vec![json!({"source": "claude"}), json!({"source": "codex"})],
            ..base(vec![Value::Null, Value::Null])
        },
        base(vec![
            ok_vote("claude", "NOT_REFUTED", 0.8, none.clone()),
            ok_vote("gemini", "REFUTED", 1.0, none.clone()),
        ]),
        base(vec![
            ok_vote("claude", "REFUTED", 0.9, none.clone()),
            json!({"source": "codex", "slotStatus": "UNAVAILABLE", "failureReason": "timeout"}),
        ]),
        base(vec![
            ok_vote(
                "claude",
                "REFUTED",
                0.9,
                json!({"coreStands": true, "refutationType": "PRE_DECIDED", "decisionQuestioned": "DEFERRED: this exact defect, per the thread"}),
            ),
            ok_vote("codex", "REFUTED", 0.9, none.clone()),
        ]),
        base(vec![
            ok_vote(
                "claude",
                "REFUTED",
                0.9,
                json!({"coreStands": true, "refutationType": "PRE_DECIDED"}),
            ),
            ok_vote("codex", "REFUTED", 0.9, none.clone()),
        ]),
    ];
    let keep = || ok_vote("codex", "NOT_REFUTED", 0.9, json!({}));
    let cited = json!({"refutationType": "PRE_DECIDED", "decisionQuestioned": "ledger flush timing deferred by the author"});
    for (file, vote_extra) in [
        (
            "scripts/invoices/bulk-late-fee-application.ts",
            json!({"refutationType": "PRE_DECIDED"}),
        ),
        (
            "scripts/invoices/bulk-late-fee-application.ts",
            cited.clone(),
        ),
        ("scripts/invoices/other-file.ts", cited.clone()),
    ] {
        out.push(PanelCase {
            panel: vec![],
            finding: json!({"file": file}),
            decisions: ledger.clone(),
            ..base(vec![ok_vote("claude", "REFUTED", 0.9, vote_extra), keep()])
        });
    }
    out.push(PanelCase {
        panel: vec![],
        finding: json!({"file": "src/thing.ts"}),
        decisions: ledger.clone(),
        ..base(vec![
            ok_vote(
                "claude",
                "REFUTED",
                0.9,
                json!({"refutationType": "PRE_DECIDED"}),
            ),
            ok_vote(
                "claude",
                "REFUTED",
                0.8,
                json!({"refutationType": "PRE_DECIDED"}),
            ),
        ])
    });
    let b4_vote = ok_vote(
        "claude",
        "REFUTED",
        0.8,
        json!({"refutationType": "PRE_DECIDED", "decisionQuestioned": b4_summary}),
    );
    for pr_files in [b4_pr.clone(), b4_pr[..2].to_vec(), b4_pr[1..].to_vec()] {
        out.push(PanelCase {
            panel: vec![],
            finding: b4_finding.clone(),
            decisions: b4.clone(),
            pr_files,
            ..base(vec![b4_vote.clone(), keep()])
        });
    }
    out.push(PanelCase {
        panel: vec![],
        finding: json!({"file": "src/jobs/queue.ts", "title": "Invoice totals rounded twice", "description": ""}),
        decisions: b4.clone(),
        pr_files: b4_pr.clone(),
        ..base(vec![b4_vote.clone(), keep()])
    });
    let invented = v2(
        "REFUTED",
        0.9,
        json!({"refutationType": "PRE_DECIDED", "decisionQuestioned": "deferred by the author"}),
    );
    for (votes, given) in [
        (
            vec![
                v2("REFUTED", 0.9, json!({"killReason": "pre-existing"})),
                v2("NOT_REFUTED", 0.6, json!({})),
            ],
            None,
        ),
        (
            vec![
                v2("REFUTED", 0.9, json!({"killReason": "incorrect-premise"})),
                v2("NOT_REFUTED", 0.6, json!({})),
            ],
            None,
        ),
        (
            vec![
                v2(
                    "REFUTED",
                    0.9,
                    json!({"killReason": "not-triggered", "contradiction": quote}),
                ),
                v2("NOT_REFUTED", 0.6, json!({})),
            ],
            None,
        ),
        (
            vec![
                v2(
                    "REFUTED",
                    0.9,
                    json!({"killReason": "not-triggered", "contradiction": quote, "quoteFound": false}),
                ),
                v2("NOT_REFUTED", 0.6, json!({})),
            ],
            None,
        ),
        (
            vec![invented.clone(), v2("NOT_REFUTED", 0.6, json!({}))],
            Some(false),
        ),
        (
            vec![invented.clone(), v2("NOT_REFUTED", 0.6, json!({}))],
            Some(true),
        ),
        (
            vec![
                v2("NOT_REFUTED", 0.8, json!({"introducedByPr": "no"})),
                v2("NOT_REFUTED", 0.8, json!({"introducedByPr": "no"})),
                v2("NOT_REFUTED", 0.8, json!({})),
            ],
            None,
        ),
        (
            vec![
                v2("NOT_REFUTED", 0.8, json!({"intendedBy": rule})),
                v2("NOT_REFUTED", 0.8, json!({"intendedBy": rule})),
            ],
            None,
        ),
        (
            vec![
                v2("NOT_REFUTED", 0.8, json!({"proposedSeverity": "nit"})),
                v2("NOT_REFUTED", 0.8, json!({"proposedSeverity": "nit"})),
            ],
            None,
        ),
        (
            vec![
                v2("NOT_REFUTED", 0.8, json!({"proposedSeverity": "nit"})),
                v2("NOT_REFUTED", 0.8, json!({})),
            ],
            None,
        ),
    ] {
        out.push(PanelCase {
            panel: vec![],
            finding: json!({"file": "a.ts", "severity": "important"}),
            decisions_given: given,
            ..base(votes)
        });
    }
    out
}

fn panel_exports() -> Vec<&'static str> {
    vec![
        "tallyVotes",
        // See `exact_number`.
        "tallyVotesExact:(...a)=>{const t=tallyVotes(...a);t.refuteFraction=String(t.refuteFraction);return t}",
        "placeSurvivor",
        "factualRefutationFails",
        "contestedPreDecision",
        "preDecidedCoversFile",
        "samePath",
        "shouldEscalate",
        "needsRefine",
        "shouldDebate",
        "VERDICT_SPLIT",
        "UNDER_CONFIDENT",
        // The oracle returns an object literal of its exports, so a constant
        // is read through a comma-free arrow function.
        "tKill:()=>T_KILL",
        "delta:()=>DELTA",
        "geminiCap:()=>GEMINI_CONFIDENCE_CAP",
        "floor:()=>ESCALATE_CONFIDENCE_FLOOR",
    ]
}

/// Collects calls of one export and their Rust results, then compares them
/// with the JavaScript region in one oracle run.
struct Batch {
    name: &'static str,
    calls: Vec<Value>,
    rust: Vec<Value>,
    fix: fn(&mut Value),
}

/// serde_json's default float parser (no `float_roundtrip`) misreads some
/// 17-digit numbers: `1699999.9000000001`, which JavaScript computes exactly as
/// Rust does, comes back as `1699999.9`. A computed float is therefore sent by
/// the JavaScript side as `String(x)` and read here with the correctly rounded
/// `str::parse`.
fn exact_number(v: &mut Value) {
    if let Value::String(s) = v {
        let x: f64 = s.parse().unwrap_or_else(|_| panic!("not a number: {s}"));
        *v = json!(x);
    }
}

impl Batch {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            calls: Vec::new(),
            rust: Vec::new(),
            fix: |_| {},
        }
    }

    fn fixing(mut self, fix: fn(&mut Value)) -> Self {
        self.fix = fix;
        self
    }

    fn push(&mut self, args: Vec<Value>, rust: Value) {
        self.calls.push(json!({"fn": self.name, "args": args}));
        self.rust.push(rust);
    }

    fn check(self, region: &str, exports: &[&str]) {
        let mut js = oracle(region, exports, &[], &self.calls);
        js.iter_mut().for_each(self.fix);
        assert_same(self.name, &self.calls, &self.rust, &js);
    }
}

fn opt_str(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str)
}

#[test]
fn panel_gates_matches_js() {
    let mut cases = sample(panel_case());
    cases.extend(panel_seeds());

    let mut tally =
        Batch::new("tallyVotesExact").fixing(|t| exact_number(&mut t["refuteFraction"]));
    let mut escalate = Batch::new("shouldEscalate");
    let mut refine = Batch::new("needsRefine");
    let mut debate = Batch::new("shouldDebate");
    let mut split = Batch::new("VERDICT_SPLIT");
    let mut under = Batch::new("UNDER_CONFIDENT");
    let mut contested = Batch::new("contestedPreDecision");
    let mut place = Batch::new("placeSurvivor");
    let mut factual = Batch::new("factualRefutationFails");
    let mut covers = Batch::new("preDecidedCoversFile");
    let mut same = Batch::new("samePath");
    let mut hits = [0usize; 7];
    let mut contested_null = 0;
    // Per-branch floors: each discard reason, each placement, and a falsy
    // non-boolean operand returned by shouldEscalate.
    let mut branch: std::collections::BTreeMap<String, usize> = Default::default();

    for c in &cases {
        let ff = opt_str(c.finding.get("file"));
        let t: Tally = gates::tally_votes(
            &c.votes,
            &c.panel,
            ff,
            &c.decisions,
            c.decisions_given.unwrap_or(true),
        );
        let mut args = vec![
            json!(c.votes),
            json!(c.panel),
            json!(ff),
            c.decisions.clone(),
        ];
        if let Some(g) = c.decisions_given {
            args.push(json!(g));
        }
        tally.push(args, json!(t));
        hits[0] += usize::from(!t.discarded_votes.is_empty());
        for d in &t.discarded_votes {
            *branch.entry(format!("discard {}", d.reason)).or_default() += 1;
        }
        if let Some(e) = gates::should_escalate(&t, &c.finding, c.claude_r0) {
            if !e.is_boolean() {
                *branch.entry("escalate non-boolean".into()).or_default() += 1;
            }
        } else {
            *branch.entry("escalate undefined".into()).or_default() += 1;
        }
        hits[1] += usize::from(t.borderline);
        hits[2] += usize::from(t.pre_decided);

        let tj = json!(t);
        escalate.push(
            vec![tj.clone(), c.finding.clone(), json!(c.claude_r0)],
            gates::should_escalate(&t, &c.finding, c.claude_r0).unwrap_or(Value::Null),
        );
        refine.push(vec![tj.clone()], json!(gates::needs_refine(&t)));
        debate.push(vec![tj.clone()], json!(gates::should_debate(&t)));
        split.push(vec![tj.clone()], json!(gates::verdict_split(&t)));
        under.push(vec![tj.clone()], json!(gates::under_confident(&t)));

        // The tally's own discards, then every cited vote as if discarded, so
        // the decision match is reached far more often than the tally alone
        // would reach it.
        let as_discarded: Vec<DiscardedVote> = c
            .votes
            .iter()
            .filter(|v| v.is_object())
            .map(|v| DiscardedVote {
                source: v
                    .get("source")
                    .filter(|s| tribunal_core::js::truthy(Some(s)))
                    .cloned()
                    .unwrap_or(json!("unknown")),
                reason: "citation-does-not-cover-finding",
                decision_questioned: v
                    .get("decisionQuestioned")
                    .filter(|d| tribunal_core::js::truthy(Some(d)))
                    .cloned()
                    .unwrap_or(Value::Null),
            })
            .collect();
        // And one citing each decision's own summary, which always matches it.
        let quoting: Vec<DiscardedVote> = c
            .decisions
            .as_array()
            .into_iter()
            .flatten()
            .map(|d| DiscardedVote {
                source: json!("claude"),
                reason: "citation-does-not-cover-finding",
                decision_questioned: d.get("summary").cloned().unwrap_or(Value::Null),
            })
            .collect();
        for discarded in [&t.discarded_votes, &as_discarded, &quoting] {
            let r = gates::contested_pre_decision(discarded, &c.finding, &c.decisions, &c.pr_files);
            hits[3] += usize::from(r.is_some());
            contested_null += usize::from(r.is_none() && !discarded.is_empty());
            contested.push(
                vec![
                    json!(discarded),
                    c.finding.clone(),
                    c.decisions.clone(),
                    json!(c.pr_files),
                ],
                json!(r),
            );
        }

        for vs in [&c.votes, &t.valid] {
            let p = gates::place_survivor(vs, &c.finding, c.line_added);
            hits[4] += usize::from(p.placement.is_some());
            if let Some(pl) = p.placement {
                *branch.entry(format!("placement {pl}")).or_default() += 1;
            }
            hits[5] += usize::from(p.proposed_severity.is_some());
            place.push(
                vec![json!(vs), c.finding.clone(), json!(c.line_added)],
                json!(p),
            );
        }

        for v in c.votes.iter().filter(|v| !v.is_null()) {
            factual.push(vec![v.clone()], json!(gates::factual_refutation_fails(v)));
            let dq = v.get("decisionQuestioned");
            let r = gates::pre_decided_covers_file(dq, ff, &c.decisions);
            hits[6] += usize::from(!r);
            covers.push(vec![json!(dq), json!(ff), c.decisions.clone()], json!(r));
        }
    }
    // decisionMatches through an unscoped decision, every citation against
    // every summary: the token split, lower-casing and the 0.4 overlap
    // boundary decide each of these alone. The extra texts sit on those
    // edges: overlaps of exactly 2/5 and 4/10, and words `\W` splits
    // ("fa\u{00E7}ade" is "fa" and "ade").
    const EDGES: [&str; 12] = [
        "ledger flush one two three",
        "timing author p q r",
        "a1 a2 a3 a4 a5 a6 a7 a8 b1 b2",
        "a1 a2 a3 a4 c1 c2 c3 c4 c5 c6 c7",
        "fa\u{00E7}ade",
        "fa ade timing",
        "na\u{00EF}ve \u{00FC}n\u{00EF}code",
        "na ve n code",
        "\u{00FC}ber flush",
        "ber flush ledger",
        "stra\u{1E9E}e timing",
        "stra\u{00DF}e timing one two three",
    ];
    let mut matches = Batch::new("preDecidedCoversFile");
    let texts = || {
        SUMMARIES
            .iter()
            .chain(&TITLES)
            .chain(&DESCRIPTIONS)
            .chain(&EDGES)
    };
    for dq in QUESTIONED.iter().chain(texts()) {
        for summary in texts() {
            let decisions = json!([{"summary": summary, "files": []}]);
            let dq = json!(dq);
            matches.push(
                vec![dq.clone(), Value::Null, decisions.clone()],
                json!(gates::pre_decided_covers_file(Some(&dq), None, &decisions)),
            );
        }
    }

    // A non-string citation is matched as `String(x)`: JavaScript prints this
    // number as 233115890514796.12, so its "12" token decides the overlap.
    for dq in [
        json!(f64::from_bits(0x42ea_8090_bb0f_6d84)),
        json!(f64::from_bits(0x4317_9085_685d_83c9)),
    ] {
        for summary in ["12 a b c d", "2 a b c d", "13 a b c d", "3 a b c d"] {
            let decisions = json!([{"summary": summary, "files": []}]);
            matches.push(
                vec![dq.clone(), Value::Null, decisions.clone()],
                json!(gates::pre_decided_covers_file(Some(&dq), None, &decisions)),
            );
        }
    }

    // Final sigma depends on the letters around it, skipping case-ignorable
    // ones (apostrophe, soft hyphen, combining accent, U+FEFF), and decides
    // `sum.includes(dq)` once both sides are lower-cased.
    const SIGMA: [&str; 9] = [
        "\u{0391}\u{03A3}'\u{0392}",
        "\u{0391}\u{03A3}'",
        "\u{0391}\u{03A3}\u{00AD} x",
        "\u{0391}\u{03A3}\u{0301} b",
        "\u{03A3}",
        " \u{03A3}\u{FEFF}",
        "\u{03B1}\u{03C2}'\u{03B2}",
        "\u{03B1}\u{03C2}",
        "\u{03B1}\u{03C3}",
    ];
    for dq in SIGMA {
        for summary in SIGMA {
            let decisions = json!([{"summary": summary, "files": []}]);
            let dq = json!(dq);
            matches.push(
                vec![dq.clone(), Value::Null, decisions.clone()],
                json!(gates::pre_decided_covers_file(Some(&dq), None, &decisions)),
            );
        }
    }

    // Severity keys resolved through `String(v)`: nested arrays flatten, a
    // two-element array joins with a comma, and a number is not a key.
    for (own, proposed) in [
        (json!("critical"), json!([["nit"]])),
        (json!("critical"), json!(["nit", "x"])),
        (json!([["critical"]]), json!("important")),
        (json!("important"), json!(0)),
        (json!("important"), json!([null, "nit"])),
        (json!({"a": 1}), json!("nit")),
    ] {
        let v = json!({"slotStatus": "OK", "introducedByPr": "yes", "proposedSeverity": proposed});
        let votes = vec![v.clone(), v];
        let f = json!({"severity": own});
        place.push(
            vec![json!(votes), f.clone(), json!(false)],
            json!(gates::place_survivor(&votes, &f, false)),
        );
    }

    let path_or_null = std::iter::once(None).chain(PATHS.iter().map(|p| Some(*p)));
    for a in path_or_null.clone() {
        for b in path_or_null.clone() {
            same.push(vec![json!(a), json!(b)], json!(gates::same_path(a, b)));
        }
    }

    assert!(contested_null >= 25);
    eprintln!("panel branch counts: {branch:?}");
    for key in [
        "discard no-decisions-given",
        "discard citation-does-not-cover-finding",
        "discard no-decision-cited",
        "discard judgement-kill-reason",
        "discard no-contradicting-quote",
        "discard quote-not-at-commit",
        "escalate non-boolean",
        "escalate undefined",
        "placement intended",
        "placement pre-existing",
    ] {
        let n = branch.get(key).copied().unwrap_or(0);
        assert!(n >= 5, "the generator reached {key} only {n} times");
    }
    // Each branch the gates decide on has to be reached, or a green run says
    // nothing about it.
    for (what, n) in [
        "discarded votes",
        "borderline tallies",
        "pre-decided tallies",
        "contested pre-decisions",
        "placements",
        "proposed severities",
        "uncovered citations",
    ]
    .iter()
    .zip(hits)
    {
        assert!(n >= 25, "the generator reached {what} only {n} times");
    }

    let exports = panel_exports();
    for b in [
        matches, tally, escalate, refine, debate, split, under, contested, place, factual, covers,
        same,
    ] {
        b.check("PANEL-GATES", &exports);
    }

    let mut consts = Batch::new("tKill");
    consts.push(vec![], json!(gates::T_KILL));
    consts.check("PANEL-GATES", &exports);
    for (name, value) in [
        ("delta", gates::DELTA),
        ("geminiCap", gates::GEMINI_CONFIDENCE_CAP),
        ("floor", gates::ESCALATE_CONFIDENCE_FLOOR),
    ] {
        let mut b = Batch::new(name);
        b.push(vec![], json!(value));
        b.check("PANEL-GATES", &exports);
    }
}

fn token_count() -> BoxedStrategy<Option<f64>> {
    prop_oneof![
        6 => prop::sample::select(vec![
            0.0, -0.0, 1.0, 10.0, 1000.0, 38_596.0, 40_000.0, 59_999.0, 60_000.0, 60_000.5, 60_001.0,
            300_000.0, 900_000.0, 950_000.0, 1_199_999.0, 1_200_000.0, 1_229_783.0, 1_300_000.0,
            1_500_000.0, 8_000_000.0, 9_000_000.0, -5.0, -1e-7, 1.5e-7, 0.1, 0.30000000000000004,
            123_456.789, 1e21, 1.23e22, 9_007_199_254_740_993.0,
        ])
        .prop_map(Some),
        2 => (0u32..2_000_000).prop_map(|x| Some(f64::from(x))),
        // Any finite double, so the reason strings exercise Number::toString
        // well past the integer token counts a run produces.
        2 => (prop::num::f64::NORMAL | prop::num::f64::SUBNORMAL).prop_map(Some),
        2 => Just(None),
    ]
    .boxed()
}

#[test]
fn cost_breaker_matches_js() {
    let mut cases = sample(
        (
            token_count(),
            token_count(),
            token_count(),
            token_count(),
            token_count(),
        )
            .prop_map(
                |(spent_output, run_output, budget_total, budget_remaining, ceiling)| CostInputs {
                    spent_output,
                    run_output,
                    budget_total,
                    budget_remaining,
                    ceiling,
                },
            ),
    );
    // replay-cost-breaker.mjs. `budgetRemaining: Infinity` has no JSON form;
    // an absent key is equally not finite.
    let seed = |spent: f64,
                run: Option<f64>,
                total: Option<f64>,
                left: Option<f64>,
                ceiling: Option<f64>| CostInputs {
        spent_output: Some(spent),
        run_output: run,
        budget_total: total,
        budget_remaining: left,
        ceiling,
    };
    cases.extend([
        CostInputs::default(),
        seed(1_500_000.0, None, None, None, Some(1_200_000.0)),
        seed(
            0.0,
            None,
            Some(500_000.0),
            Some(60_000.0),
            Some(1_200_000.0),
        ),
        seed(
            0.0,
            None,
            Some(500_000.0),
            Some(60_001.0),
            Some(1_200_000.0),
        ),
        seed(10.0, None, Some(0.0), Some(0.0), Some(1_200_000.0)),
        seed(10.0, None, None, None, Some(0.0)),
        seed(1_229_783.0, Some(38_596.0), None, None, Some(1_200_000.0)),
        seed(
            1_400_000.0,
            Some(1_300_000.0),
            None,
            None,
            Some(1_200_000.0),
        ),
        seed(
            999_000.0,
            Some(10.0),
            Some(1_000_000.0),
            Some(1_000.0),
            Some(1_200_000.0),
        ),
        // 17-digit numbers where Rust's shortest `{:e}` digits are not the
        // closest shortest candidate, which ECMA-262 Number::toString requires:
        // JavaScript prints 1658206780088562.2, Rust's shortest 1658206780088562.3.
        seed(
            f64::from_bits(0x4302_d77f_6703_0d4a),
            Some(f64::from_bits(0x4317_9085_685d_83c9)),
            None,
            None,
            Some(1.0),
        ),
        seed(
            0.0,
            None,
            Some(f64::from_bits(0x431e_0f96_bc7a_cd2d)),
            Some(f64::from_bits(0xc313_7fa1_3193_52ed)),
            None,
        ),
    ]);
    // Arbitrary doubles come back through `String(x)`; see `exact_number`.
    let mut b = Batch::new("costBreakerStateExact").fixing(|st| {
        exact_number(&mut st["spentOutput"]);
        exact_number(&mut st["runOutput"]);
    });
    let mut fired = 0;
    let (mut budget_fired, mut ceiling_fired) = (0, 0);
    for c in &cases {
        let mut arg = Map::new();
        for (k, v) in [
            ("spentOutput", c.spent_output),
            ("runOutput", c.run_output),
            ("budgetTotal", c.budget_total),
            ("budgetRemaining", c.budget_remaining),
            ("ceiling", c.ceiling),
        ] {
            if let Some(v) = v {
                arg.insert(k.into(), json!(v));
            }
        }
        let st = cost_breaker::cost_breaker_state(c);
        fired += usize::from(st.degrade);
        let r = st.reason.as_deref().unwrap_or("");
        budget_fired += usize::from(r.starts_with("declared budget"));
        ceiling_fired += usize::from(r.starts_with("output ceiling"));
        b.push(vec![Value::Object(arg)], json!(st));
    }
    assert!(fired >= 50, "the breaker fired only {fired} times");
    assert!(
        budget_fired >= 25 && ceiling_fired >= 25,
        "budget branch {budget_fired}, ceiling branch {ceiling_fired}"
    );
    b.check(
        "COST-BREAKER",
        &[
            "costBreakerState",
            "costBreakerStateExact:(a)=>{const s=costBreakerState(a);s.spentOutput=String(s.spentOutput);s.runOutput=String(s.runOutput);return s}",
        ],
    );
}

#[test]
fn spent_counter_matches_js() {
    let reading = prop_oneof![
        3 => prop::sample::select(vec![
            0.0, 500.0, 1000.0, 1500.0, 4000.0, 90_000.0, 120_000.0, 1_700_000.0, 1_710_000.0,
            -1_610_496.0, 0.1, 0.2, 0.7, 1e21, 9_007_199_254_740_993.0,
        ]),
        2 => (0u32..5000).prop_map(f64::from),
    ];
    let mut cases = sample(prop::collection::vec(reading, 1..8));
    cases.extend([
        vec![1000.0, 1500.0, 4000.0],
        vec![1_700_000.0, 1_710_000.0, 90_000.0, 120_000.0],
        vec![500.0, 0.0, 0.0],
        vec![10.0, 50.0, 5.0, 30.0, 2.0],
    ]);
    // A function argument has no JSON form, so the replay's `feed` helper runs
    // inside the region; totals come back as strings (see `exact_number`).
    let feed = "feed:(q)=>{const c=spentCounter(()=>q.shift());const out=[];while(q.length)out.push([String(c.now())].concat([c.resets()]));return out}";
    let mut b = Batch::new("feed").fixing(|steps| {
        for step in steps.as_array_mut().into_iter().flatten() {
            exact_number(&mut step[0]);
        }
    });
    for readings in &cases {
        let mut q = readings.iter().copied();
        let mut c = SpentCounter::new(|| q.next().expect("a reading"));
        let steps: Vec<Value> = (1..readings.len())
            .map(|_| {
                let total = c.now();
                json!([total, c.resets()])
            })
            .collect();
        b.push(vec![json!(readings)], json!(steps));
    }
    b.check("SPENT-COUNTER", &["spentCounter", feed]);
}

fn original_finding() -> BoxedStrategy<Value> {
    shuffled_object(vec![
        ("slotId", pick(vec![(3, s("s1")), (1, None)])),
        (
            "title",
            pick(vec![
                (3, s("negative amount creates a negative invoice")),
                (1, None),
            ]),
        ),
        (
            "file",
            pick(vec![
                (3, s("src/controllers/v2/invoice.controller.ts")),
                (1, None),
            ]),
        ),
        ("line", pick(vec![(3, Some(json!(88))), (1, None)])),
        (
            "severity",
            pick(vec![
                (3, s("important")),
                (2, s("critical")),
                (2, s("nit")),
                (1, s("NIT")),
                (1, Some(json!(2))),
                (1, Some(json!(["critical"]))),
                (1, Some(Value::Null)),
                (1, None),
            ]),
        ),
        (
            "description",
            pick(vec![(3, s("No positive check before create.")), (1, None)]),
        ),
        (
            "isBehaviorChanging",
            pick(vec![(1, Some(json!(true))), (1, None)]),
        ),
        (
            "touchesHighRiskSurface",
            pick(vec![(1, Some(json!(true))), (1, None)]),
        ),
        (
            "refineProposedSeverity",
            pick(vec![(1, s("nit")), (4, None)]),
        ),
    ])
}

fn strengthen_answer() -> BoxedStrategy<Option<Value>> {
    let object = shuffled_object(vec![
        (
            "strengthened",
            pick(vec![
                (8, Some(json!(true))),
                (2, Some(json!(false))),
                (1, s("true")),
                (1, Some(json!(1))),
                (1, None),
            ]),
        ),
        (
            "unstrengthenableReason",
            pick(vec![
                (3, None),
                (2, Some(Value::Null)),
                (1, s("")),
                (2, s("the guard is in a middleware I was not given")),
                (1, Some(json!(0))),
                (1, Some(json!({}))),
            ]),
        ),
        (
            "title",
            pick(vec![
                (
                    3,
                    s("negative amount reaches prisma.invoice.create unchecked"),
                ),
                (1, None),
            ]),
        ),
        (
            "file",
            pick(vec![
                (3, s("src/controllers/v2/invoice.controller.ts")),
                (1, None),
            ]),
        ),
        ("line", pick(vec![(1, Some(json!(91))), (1, None)])),
        (
            "severity",
            pick(vec![
                (2, s("critical")),
                (2, s("important")),
                (3, s("nit")),
                (1, s("Critical")),
                (1, Some(json!(["nit"]))),
                (1, Some(json!(1))),
                (1, None),
            ]),
        ),
        (
            "description",
            pick(vec![
                (3, s("POST with amount:-1 persists a negative row.")),
                (1, None),
            ]),
        ),
        ("repro", pick(vec![(1, s("curl -X POST")), (1, None)])),
        ("slotId", pick(vec![(1, s("s9")), (4, None)])),
        (
            "refineProposedSeverity",
            pick(vec![(1, s("critical")), (4, None)]),
        ),
        (
            "newField",
            pick(vec![(1, Some(json!({"a": [1, 2]}))), (2, None)]),
        ),
    ]);
    prop_oneof![
        12 => object.prop_map(Some),
        1 => Just(None),
        1 => prop::sample::select(vec![
            Value::Null,
            json!(false),
            json!(0),
            json!(""),
            json!("x"),
            json!([]),
            json!([true]),
        ])
        .prop_map(Some),
    ]
    .boxed()
}

#[test]
fn strengthen_merge_matches_js() {
    let cases = sample((original_finding(), strengthen_answer()));
    let mut b = Batch::new("applyStrengthen");
    let mut lowered = 0;
    let mut outcomes = std::collections::BTreeMap::new();
    for (original, answer) in &cases {
        let r = strengthen_merge::apply_strengthen(original, answer.as_ref());
        lowered += usize::from(r.finding.get("refineProposedSeverity").is_some());
        *outcomes.entry(r.outcome).or_insert(0usize) += 1;
        let mut args = vec![original.clone()];
        args.extend(answer.clone());
        b.push(args, json!(r));
    }
    assert!(
        lowered >= 30,
        "a lowered severity was reached only {lowered} times"
    );
    // `__proto__` read by JSON.parse is an own data property, and both spreads
    // copy it as one.
    for (original, answer) in [
        (
            json!({"__proto__": {"x": 1}, "severity": "critical", "title": "t"}),
            json!({"strengthened": true, "severity": "nit", "__proto__": 5}),
        ),
        (
            json!({"severity": "nit"}),
            json!({"__proto__": null, "strengthened": true, "severity": "critical"}),
        ),
    ] {
        let r = strengthen_merge::apply_strengthen(&original, Some(&answer));
        b.push(vec![original, answer], json!(r));
    }
    for o in ["dispatch-failed", "unstrengthenable", "strengthened"] {
        let n = outcomes.get(o).copied().unwrap_or(0);
        assert!(n >= 25, "outcome {o} reached only {n} times");
    }
    b.check("STRENGTHEN-MERGE", &["applyStrengthen"]);
}

#[test]
fn debate_degrade_matches_js() {
    let defense = prop::sample::select(vec![
        None,
        Some(Value::Null),
        Some(json!("")),
        Some(json!("   \n  ")),
        Some(json!("\u{FEFF}x\u{FEFF}")),
        Some(json!("\u{0085}")),
        Some(json!("\u{0085}x\u{0085}")),
        Some(json!(" \u{3000}hi\u{00A0}")),
        Some(json!(
            "\u{2028}The guard is absent on the retry path; see line 40.\r\n"
        )),
        Some(json!(0)),
        Some(json!(5)),
        Some(json!(true)),
        Some(json!({"decision": "SURVIVES"})),
        Some(json!(["a"])),
    ]);
    let ruling = prop_oneof![
        10 => object_of(vec![
            (
                "decision",
                pick(vec![
                    (3, s("SURVIVES")),
                    (3, s("REFUTED")),
                    (1, s("survives")),
                    (1, s("MAYBE")),
                    (1, s("")),
                    (1, Some(json!(1))),
                    (1, Some(Value::Null)),
                    (1, None),
                ]),
            ),
            (
                "rationale",
                pick(vec![
                    (3, s("The path is unreachable.")),
                    (1, s("")),
                    (1, Some(Value::Null)),
                    (1, Some(json!(0))),
                    (1, Some(json!(5))),
                    (1, Some(json!({}))),
                    (1, None),
                ]),
            ),
            (
                "changedFromPanel",
                pick(vec![
                    (2, Some(json!(true))),
                    (2, Some(json!(false))),
                    (1, Some(json!(1))),
                    (1, Some(json!(0))),
                    (1, s("")),
                    (1, s("no")),
                    (1, None),
                ]),
            ),
        ])
        .prop_map(Some),
        1 => Just(None),
        1 => prop::sample::select(vec![Value::Null, json!(0), json!("SURVIVES"), json!([])]).prop_map(Some),
    ];
    let cases = sample((defense, ruling, any::<bool>()));
    let kept = gates::tally_votes(&[], &[], None, &json!([]), true);
    let killed = gates::tally_votes(
        &[ok_vote("claude", "REFUTED", 0.9, json!({}))],
        &[],
        None,
        &json!([]),
        true,
    );
    assert!(kept.survived && !killed.survived);

    let mut block = Batch::new("debateDefenseBlock");
    let mut ruled = Batch::new("debateRuling");
    let mut rulings = std::collections::BTreeMap::new();
    for (d, r, survived) in &cases {
        block.push(
            d.iter().cloned().collect(),
            json!(debate_degrade::debate_defense_block(d.as_ref())),
        );
        let t = if *survived { &kept } else { &killed };
        *rulings
            .entry(debate_degrade::debate_ruling(r.as_ref(), t).outcome)
            .or_insert(0usize) += 1;
        // An absent ruling is sent as null: both are falsy to `ruling &&`.
        ruled.push(
            vec![r.clone().unwrap_or(Value::Null), json!(t)],
            json!(debate_degrade::debate_ruling(r.as_ref(), t)),
        );
    }
    for o in ["unadjudicated", "flipped", "held"] {
        let n = rulings.get(o).copied().unwrap_or(0);
        assert!(n >= 25, "ruling {o} reached only {n} times");
    }
    let exports = ["debateDefenseBlock", "debateRuling"];
    block.check("DEBATE-DEGRADE", &exports);
    ruled.check("DEBATE-DEGRADE", &exports);
}
