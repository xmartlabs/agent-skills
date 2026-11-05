//! `build-post-footer.mjs` against `post_footer`: both modes, on generated
//! metrics rows and flags, compared on stdout, stderr and exit status.
//!
//! When neither the row nor the flags carry a timestamp, both sides print
//! the current time; that one segment is compared by shape, and the port's
//! is checked against the clock.
//!
//! Nothing here runs node outside a regeneration. The script's side of the
//! generated cases is read from `tests/goldens/build-post-footer.json`, which
//! keeps the text of the script and of every module it imports, a hash of
//! each case's input, and the `plugin.json` it read its version from. The
//! port is given a plugin root holding that `plugin.json`, so a release does
//! not need a regeneration; changing a module or the generators does. A module
//! no longer on disk was retired with the Workflow: its check is skipped and
//! the golden is final. What `toISOString()` printed for the clock check is
//! read from `tests/goldens/build-post-footer-fixed.json`, with the probe's
//! hash. Regenerate after changing them, over the whole binary:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_post_footer`.

mod posting;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use posting::*;
use proptest::prelude::*;
use regex::Regex;
use serde_json::{json, Value};
use tribunal_context::harness_version::sha256_hex;
use tribunal_context::ledger::LedgerEnv;
use tribunal_context::post_footer;

#[derive(Debug, Clone)]
struct Case {
    rows: Vec<String>,
    argv: Vec<String>,
}

fn opt_field(
    key: &'static str,
    values: Vec<&'static str>,
) -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        1 => Just(None),
        4 => proptest::sample::select(values).prop_map(move |v| Some(format!("\"{key}\":{v}"))),
    ]
}

const TOKENS: &[&str] = &[
    "0",
    "999",
    "1000",
    "999949",
    "999950",
    "999975",
    "1234567",
    "-1500",
    "1.5",
    "\"100\"",
    "null",
    "1e21",
    "2500000000",
    "-999950",
    "1049.99",
    "12345",
    "54321",
    "999.5",
    "1250",
    "12350",
    "2550000",
    "1050",
];

fn agent_cost() -> impl Strategy<Value = String> {
    (
        proptest::sample::select(TOKENS.to_vec()),
        proptest::sample::select(TOKENS.to_vec()),
        any::<bool>(),
    )
        .prop_map(|(i, o, both)| {
            if both {
                format!("{{\"inputTokens\":{i},\"outputTokens\":{o}}}")
            } else {
                format!("{{\"inputTokens\":{i}}}")
            }
        })
}

fn row() -> impl Strategy<Value = String> {
    (
        opt_field("pr", vec!["12", "\"12\"", "12", "13"]),
        opt_field("headSha", vec!["\"abc\"", "\"abc\"", "\"def\""]),
        opt_field(
            "harnessVersion",
            vec!["\"tribunal-v105\"", "\"tribunal-v105\"", "\"tribunal-v105\"", "\"\"", "5", "[\"a\",null,\"b\"]", "{\"x\":1}", "true"],
        ),
        opt_field("reviewMode", vec!["\"full\"", "\"incremental\"", "\"full\"", "\"incremental\"", "\"bogus\"", "null", "[\"full\"]"]),
        opt_field(
            "codexModelUsed",
            vec!["null", "\"gpt-5\"", "[]", "[\"a\",\"b\"]", "[\"\"]", "[null,\"x\"]", "false", "0", "[[1,2],3]"],
        ),
        prop_oneof![
            1 => Just(None),
            1 => proptest::sample::select(vec!["null", "5", "{}"]).prop_map(|v| Some(format!("\"agentCost\":{v}"))),
            4 => agent_cost().prop_map(|v| Some(format!("\"agentCost\":{v}"))),
        ],
        opt_field(
            "wallClockSeconds",
            vec!["30", "59.5", "60", "89", "90", "150", "-5", "\"60\"", "1e21", "0"],
        ),
        opt_field("ts", vec!["\"2025-01-02T03:04:05Z\"", "\"\"", "0", "5", "[1,[2,3]]"]),
    )
        .prop_map(|(a, b, c, d, e, f, g, h)| {
            let parts: Vec<String> = [a, b, c, d, e, f, g, h].into_iter().flatten().collect();
            format!("{{{}}}", parts.join(","))
        })
}

fn flag(
    name: &'static str,
    values: Vec<&'static str>,
    absent: u32,
) -> impl Strategy<Value = Vec<String>> {
    prop_oneof![
        absent => Just(Vec::new()),
        1 => Just(vec![format!("--{name}")]),
        120 => proptest::sample::select(values).prop_map(move |v| vec![format!("--{name}"), v.to_owned()]),
    ]
}

const COUNTS: &[&str] = &[
    "0", "1", "1", "3", "36", "36", "36", "-1", "1.5", "x", "1e3", "-0", " 7 ",
];

fn workflow_argv() -> impl Strategy<Value = Vec<String>> {
    (
        flag("pr", vec!["12", "12", "12", "12", "12", "13", "13", ""], 3),
        flag("head-sha", vec!["abc", "abc", "abc", "def"], 1),
        flag("agent-count", COUNTS.to_vec(), 1),
    )
        .prop_map(|(a, b, c)| [vec!["--mode".into(), "workflow".into()], a, b, c].concat())
}

fn in_context_argv() -> impl Strategy<Value = Vec<String>> {
    (
        (
            flag(
                "harness-version",
                vec![
                    "pr-harness-1",
                    "pr-harness-1",
                    "pr-harness-1",
                    "pr-harness-1",
                    "pr-harness-1",
                    "pr-harness-1",
                    "pr-harness-1",
                    "",
                ],
                3,
            ),
            flag(
                "review-mode",
                vec!["full", "incremental", "full", "incremental", "bogus"],
                1,
            ),
            flag("agent-count", COUNTS.to_vec(), 1),
            flag("lenses-planned", vec!["2", "3", "x"], 1),
            flag("lenses-dispatched", vec!["2", "3", "3", "3", "1"], 1),
        ),
        (
            flag("critic-planned", vec!["true", "false"], 1),
            flag("critic-ran", vec!["true", "true", "true", "false"], 1),
            flag("plan-deviation", vec!["  ", "time budget"], 30),
            flag(
                "codex-model-json",
                vec![
                    "\"gpt-5\"",
                    "[\"a\",\"b\"]",
                    "{bad",
                    "null",
                    "[]",
                    "7",
                    "{\"m\":1}",
                ],
                30,
            ),
            flag("timestamp", vec!["2024-05-06T07:08:09Z", ""], 20),
        ),
    )
        .prop_map(|((a, b, c, d, e), (f, g, h, i, j))| {
            [
                vec!["--mode".into(), "in-context".into()],
                a,
                b,
                c,
                d,
                e,
                f,
                g,
                h,
                i,
                j,
            ]
            .concat()
        })
}

/// A row written by the run the flags name: valid fields, varied numbers.
fn own_row() -> impl Strategy<Value = String> {
    (
        agent_cost(),
        proptest::sample::select(vec!["30", "59.5", "60", "89", "90", "150", "-5", "\"60\"", "null"]),
        proptest::sample::select(vec!["\"2025-01-02T03:04:05Z\"", "\"\"", "5"]),
        proptest::sample::select(vec!["null", "\"gpt-5\"", "[\"a\",\"b\"]", "[]"]),
        any::<bool>(),
        proptest::sample::select(vec!["", "", ",\"engine\":\"rust\"", ",\"engine\":\"rust\"", ",\"engine\":\"skill\"", ",\"engine\":5"]),
        proptest::sample::select(vec!["", ",\"engineVersion\":\"rust-0123456789ab\"", ",\"engineVersion\":\"\"", ",\"engineVersion\":7"]),
        proptest::sample::select(vec!["", "", ",\"pluginVersion\":\"106.2.0\"", ",\"pluginVersion\":\"\"", ",\"pluginVersion\":106", ",\"pluginVersion\":null"]),
    )
        .prop_map(|(cost, wall, ts, codex, full, engine, build, version)| {
            let mode = if full { "full" } else { "incremental" };
            format!(
                "\"harnessVersion\":\"tribunal-v105\",\"reviewMode\":\"{mode}\",\"codexModelUsed\":{codex},\"agentCost\":{cost},\"wallClockSeconds\":{wall},\"ts\":{ts}{engine}{build}{version}"
            )
        })
}

fn case() -> impl Strategy<Value = Case> {
    (
        proptest::collection::vec(row(), 0..4),
        prop_oneof![
            6 => workflow_argv(),
            6 => in_context_argv(),
            1 => proptest::sample::select(vec![vec![], vec!["--mode".to_owned()], vec!["--mode".to_owned(), "other".to_owned()]]),
        ],
        prop_oneof![1 => Just(None), 2 => own_row().prop_map(Some)],
    )
        .prop_map(|(mut rows, argv, own)| {
            let value = |name: &str| {
                let i = argv.iter().position(|a| a == name)?;
                argv.get(i + 1).filter(|v| !v.starts_with("--")).cloned()
            };
            if let (Some(fields), Some(pr), Some(sha)) = (own, value("--pr"), value("--head-sha")) {
                rows.push(format!("{{\"pr\":\"{pr}\",\"headSha\":\"{sha}\",{fields}}}"));
            }
            Case { rows, argv }
        })
}

/// A whole semver version after `tribunal v`, as the port prints one whose
/// minor or patch is not zero.
fn semver_label() -> Regex {
    Regex::new(r"tribunal v(\d+)\.\d+\.\d+ ").unwrap()
}

#[test]
fn engine_label_prints_a_semver_version_whole() {
    use tribunal_context::post_footer::engine_label;
    let row = |v: &str| json!({"pluginVersion": v, "engine": "rust", "engineVersion": "rust-abc"});
    assert_eq!(
        engine_label(&row("115.1.0"), None),
        "v115.1.0 (rust rust-abc)"
    );
    assert_eq!(
        engine_label(&row("115.0.2"), None),
        "v115.0.2 (rust rust-abc)"
    );
    assert_eq!(engine_label(&row("115.0.0"), None), "v115 (rust rust-abc)");
    assert_eq!(engine_label(&row("106"), None), "v106 (rust rust-abc)");
    assert_eq!(
        semver_label().replace("tribunal v106.2.0 (skill)", "tribunal v$1 "),
        "tribunal v106 (skill)"
    );
}

fn lens_samples_segment(k: usize) -> String {
    if k == 1 {
        "1 lens sample".to_owned()
    } else {
        format!("{k} lens samples")
    }
}

/// A generated row (always a JSON object) with `lensSamples` added first.
fn with_lens_samples(row: &str, k: usize) -> String {
    match row.strip_prefix('{') {
        Some("}") => format!("{{\"lensSamples\":{k}}}"),
        Some(rest) => format!("{{\"lensSamples\":{k},{rest}"),
        None => panic!("a generated row is not an object: {row}"),
    }
}

/// The footer of a workflow run, read off a row with the given extra fields.
fn workflow_footer(name: &str, extra: &str) -> String {
    let dir = scratch(name);
    std::fs::write(
        dir.join("metrics.jsonl"),
        format!("{{\"pr\":12,\"headSha\":\"abc\",\"harnessVersion\":\"tribunal-v115\",\"reviewMode\":\"full\",\"ts\":\"2026-11-03T10:00:00Z\"{extra}}}\n"),
    )
    .unwrap();
    let env = LedgerEnv {
        home: dir.to_string_lossy().into_owned(),
        cwd: dir.to_string_lossy().into_owned(),
        data_dir: Some(dir.to_string_lossy().into_owned()),
        project_dir: None,
        lock_wait: tribunal_context::ledger::WAIT,
    };
    let argv: Vec<String> = [
        "--mode",
        "workflow",
        "--pr",
        "12",
        "--head-sha",
        "abc",
        "--agent-count",
        "7",
    ]
    .map(str::to_owned)
    .to_vec();
    let out = post_footer::run(&argv, &env, SystemTime::now(), &dir.to_string_lossy());
    assert_eq!(out.code, 0, "{}", out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    out.stdout
}

#[test]
fn the_footer_reads_lens_samples_off_the_row() {
    let line = |name: &str, extra: &str| workflow_footer(name, extra);
    assert_eq!(
        line("samples-1", ",\"lensSamples\":1"),
        "<sub>tribunal (skill) · tribunal-v115 · full review · 1 lens sample · 7 agents · 2026-11-03T10:00:00Z</sub>\n"
    );
    assert_eq!(
        line("samples-3", ",\"lensSamples\":3"),
        "<sub>tribunal (skill) · tribunal-v115 · full review · 3 lens samples · 7 agents · 2026-11-03T10:00:00Z</sub>\n"
    );
    assert_eq!(
        line("samples-none", ""),
        "<sub>tribunal (skill) · tribunal-v115 · full review · 7 agents · 2026-11-03T10:00:00Z</sub>\n"
    );
    for (i, odd) in ["null", "0", "\"3\"", "2.5"].into_iter().enumerate() {
        assert!(
            !line(
                &format!("samples-odd-{i}"),
                &format!(",\"lensSamples\":{odd}")
            )
            .contains("lens sample"),
            "{odd}"
        );
    }
}

fn now_segment() -> Regex {
    Regex::new(r"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ)</sub>\n$").unwrap()
}

/// Replaces a timestamp both sides took from the clock.
fn without_clock(s: &str, clocked: bool) -> String {
    if clocked {
        now_segment().replace(s, "<now></sub>\n").into_owned()
    } else {
        s.to_owned()
    }
}

fn kinds(js: &Run, c: &Case) -> Vec<&'static str> {
    let mut k = Vec::new();
    let mode = c.argv.get(1).map(String::as_str);
    match js.code {
        0 => {
            k.push(if mode == Some("workflow") {
                "workflow-ok"
            } else {
                "in-context-ok"
            });
            let o = &js.stdout;
            for (needle, kind) in [
                (" (rust rust-", "rust build"),
                (" (rust) ", "rust no build"),
                (" (skill) ", "skill"),
                ("tribunal v106 ", "row version"),
                ("tribunal (", "no version"),
                (" in / ≥", "tokens"),
                ("M in", "millions"),
                ("K in", "thousands"),
                (".3K in", "tie-rounded"),
                (" wall", "wall"),
                ("m wall", "minutes"),
                (" · codex ", "codex"),
                (" · 1 agent ·", "one-agent"),
            ] {
                if o.contains(needle) {
                    k.push(kind);
                }
            }
        }
        2 => k.push("usage"),
        _ => {
            let e = &js.stderr;
            k.push(if e.contains("refusing:") {
                "row-refused"
            } else if e.contains("refusing the footer") {
                "plan-shortfall"
            } else if e.contains("buildFooterLine") {
                "invalid-field"
            } else {
                "other-failure"
            });
        }
    }
    if js.stderr.contains("did not parse as JSON") {
        k.push("codex-json-bad");
    }
    k
}

const GOLDEN: &str = "build-post-footer.json";
const SCRIPT: &str = "skills/review-pr/scripts/build-post-footer.mjs";
const REGENERATE: &str =
    "regenerate: TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test diff_post_footer";

#[test]
fn build_post_footer_matches_js_on_generated_rows_and_flags() {
    let root = scratch("footer");
    let cases = sample(CASES, case());
    let scratch_text = root.to_string_lossy().into_owned();
    let plugin_text = plugin_root().to_string_lossy().into_owned();
    let to_golden = [
        (scratch_text.as_str(), "<SCRATCH>"),
        (plugin_text.as_str(), "<PLUGIN>"),
    ];
    let from_golden = [
        ("<SCRATCH>", scratch_text.as_str()),
        ("<PLUGIN>", plugin_text.as_str()),
    ];
    let plugin_json =
        std::fs::read_to_string(plugin_root().join(".claude-plugin/plugin.json")).unwrap();
    let golden = recorded(GOLDEN, REGENERATE, |w| stale_modules(w, SCRIPT));
    let replay: Option<&Vec<Value>> = golden.as_ref().map(|g| {
        let recorded = g["cases"].as_array().unwrap();
        assert_eq!(
            recorded.len(),
            cases.len(),
            "the generator changed; {REGENERATE}"
        );
        recorded
    });
    // The port reads the version from `<root>/.claude-plugin/plugin.json`;
    // replaying, it gets the one the script read when the golden was made.
    let port_root = match &golden {
        Some(g) => {
            let fake = root.join("recorded-plugin");
            std::fs::create_dir_all(fake.join(".claude-plugin")).unwrap();
            std::fs::write(
                fake.join(".claude-plugin/plugin.json"),
                g["pluginJson"]
                    .as_str()
                    .expect("the golden keeps pluginJson"),
            )
            .unwrap();
            fake.to_string_lossy().into_owned()
        }
        None => plugin_text.clone(),
    };
    let seen = Mutex::new(BTreeMap::<&'static str, usize>::new());
    let runs = parallel(&cases, |i, c| {
        let dir = root.join(i.to_string());
        let data = dir.join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(dir.join("home")).unwrap();
        if !c.rows.is_empty() {
            std::fs::write(data.join("metrics.jsonl"), c.rows.join("\n") + "\n").unwrap();
        }
        let env = LedgerEnv {
            home: dir.join("home").to_string_lossy().into_owned(),
            cwd: dir.to_string_lossy().into_owned(),
            data_dir: Some(data.to_string_lossy().into_owned()),
            project_dir: None,
            lock_wait: tribunal_context::ledger::WAIT,
        };
        let vars = vec![
            ("HOME", env.home.clone()),
            ("TRIBUNAL_DATA_DIR", env.data_dir.clone().unwrap()),
        ];
        // A hash: the generated rows would make most of the file.
        let input = json!(sha256_hex(format!("{c:?}").as_bytes()));
        let (js, clocked) = match replay {
            Some(r) => {
                assert_eq!(r[i]["input"], input, "the generator changed; {REGENERATE}");
                let v = swapped(&r[i]["js"], &from_golden);
                (run_from(&v), v["clocked"].as_bool().unwrap())
            }
            None => {
                let js = node(
                    &script("build-post-footer.mjs"),
                    &c.argv,
                    &dir,
                    &vars,
                    &safe_path(None),
                );
                let clocked = js.code == 0
                    && !js.stdout.contains("2025-01-02T03:04:05Z")
                    && !js.stdout.contains("2024-05-06T07:08:09Z")
                    && now_segment().is_match(&js.stdout);
                // The golden keeps the script's clock reading masked.
                let js = Run {
                    stdout: without_clock(&js.stdout, clocked),
                    ..js
                };
                (js, clocked)
            }
        };
        let mut kept_js = run_json(&js);
        kept_js["clocked"] = json!(clocked);
        let kept = json!({"input": input, "js": swapped(&kept_js, &to_golden)});
        let now = SystemTime::now();
        let mut rs = post_footer::run(&c.argv, &env, now, &port_root);
        if clocked {
            let stamp = now_segment().captures(&rs.stdout).map(|m| m[1].to_owned());
            let expected = post_footer::iso_timestamp(now);
            if stamp.as_deref() != Some(expected.as_str()) {
                return (
                    Some(format!("{c:?}\nport clock {stamp:?} vs {expected}")),
                    kept,
                );
            }
            *seen.lock().unwrap().entry("clocked").or_default() += 1;
        }
        rs.stdout = without_clock(&rs.stdout, clocked);
        // Declared difference: the port prints a version with a minor or a
        // patch whole (`tribunal v106.2.0`), where the script printed its
        // major only. Witness: `engine_label_prints_a_semver_version_whole`.
        rs.stdout = semver_label()
            .replace(&rs.stdout, "tribunal v$1 ")
            .into_owned();
        for k in kinds(&js, c) {
            *seen.lock().unwrap().entry(k).or_default() += 1;
        }
        if let Some(d) = diff_cli(&js, &rs) {
            return (Some(format!("{c:?}\n{d}")), kept);
        }
        // Declared difference: a row that carries `lensSamples` adds a lens
        // samples segment after the review mode, a field the script never
        // read. No generated row carries it, so the case runs again with it
        // added to every row: a workflow footer must print the segment, and
        // without it must still be the script's line. Witness:
        // `the_footer_reads_lens_samples_off_the_row`.
        let samples = i % 4 + 1;
        let with = dir.join("with-lens-samples");
        std::fs::create_dir_all(&with).unwrap();
        if !c.rows.is_empty() {
            let rows: Vec<String> = c
                .rows
                .iter()
                .map(|r| with_lens_samples(r, samples))
                .collect();
            std::fs::write(with.join("metrics.jsonl"), rows.join("\n") + "\n").unwrap();
        }
        let env_with = LedgerEnv {
            data_dir: Some(with.to_string_lossy().into_owned()),
            ..env
        };
        let mut rw = post_footer::run(&c.argv, &env_with, now, &port_root);
        let marked = format!(" review · {} · ", lens_samples_segment(samples));
        let printed = rw.stdout.contains(&marked);
        let expected = js.code == 0 && c.argv.get(1).map(String::as_str) == Some("workflow");
        if printed != expected {
            return (
                Some(format!(
                    "{c:?}\nwith lensSamples {samples}, segment printed: {printed}, expected: {expected}\n{}",
                    rw.stdout
                )),
                kept,
            );
        }
        if printed {
            let k = if samples == 1 {
                "one-lens-sample"
            } else {
                "lens-samples"
            };
            *seen.lock().unwrap().entry(k).or_default() += 1;
        }
        rw.stdout = without_clock(&rw.stdout.replacen(&marked, " review · ", 1), clocked);
        rw.stdout = semver_label()
            .replace(&rw.stdout, "tribunal v$1 ")
            .into_owned();
        rw.stderr = rw
            .stderr
            .replace(&*with.to_string_lossy(), &data.to_string_lossy());
        (
            diff_cli(&js, &rw).map(|d| format!("{c:?}\nwith lensSamples {samples}\n{d}")),
            kept,
        )
    });
    let (diffs, kept): (Vec<Option<String>>, Vec<Value>) = runs.into_iter().unzip();
    if replay.is_none() {
        write_golden_with(
            GOLDEN,
            json!(module_texts(SCRIPT)),
            kept,
            json!({"pluginJson": plugin_json}),
        );
    }
    eprintln!("{:?}", seen.lock().unwrap());
    assert_same("build-post-footer", &diffs);
    let seen = seen.into_inner().unwrap();
    for k in [
        "workflow-ok",
        "in-context-ok",
        "usage",
        "row-refused",
        "plan-shortfall",
        "invalid-field",
        "codex-json-bad",
        "tokens",
        "millions",
        "thousands",
        "tie-rounded",
        "wall",
        "minutes",
        "codex",
        "one-agent",
        "clocked",
        "rust build",
        "rust no build",
        "skill",
        "row version",
        "no version",
        "one-lens-sample",
        "lens-samples",
    ] {
        assert!(
            seen.get(k).copied().unwrap_or(0) >= 3,
            "{k} reached too rarely: {seen:?}"
        );
    }
}

/// The probe `iso_timestamp_matches_to_iso_string` runs under node.
const ISO_PROBE: &str = "for (const ms of process.argv.slice(2)) console.log(new Date(Number(ms)).toISOString().replace(/\\.\\d{3}Z$/, \"Z\"));";

/// The clock reading itself: the port's `iso_timestamp` agrees with
/// `toISOString()` without milliseconds across eras.
#[test]
fn iso_timestamp_matches_to_iso_string() {
    let millis: Vec<u64> = vec![
        0,
        951_782_400_000,
        1_792_663_200_999,
        4_102_444_799_000,
        253_402_300_799_000,
    ];
    let recorded = fixed(
        "build-post-footer-fixed.json",
        "iso_timestamp_matches_to_iso_string",
        REGENERATE,
        &[json!({"probe": sha(ISO_PROBE), "millis": millis})],
        || json!({}),
        |_| Vec::new(),
        || {
            let dir = scratch("footer-iso");
            let probe = dir.join("iso.cjs");
            std::fs::write(&probe, ISO_PROBE).unwrap();
            let args: Vec<String> = millis.iter().map(u64::to_string).collect();
            let js = node(&probe, &args, Path::new(&dir), &[], &safe_path(None));
            assert_eq!(js.code, 0, "{}", js.stderr);
            vec![json!(js.stdout)]
        },
    );
    let rs: String = millis
        .iter()
        .map(|ms| post_footer::iso_timestamp(UNIX_EPOCH + Duration::from_millis(*ms)) + "\n")
        .collect();
    assert_eq!(recorded[0], json!(rs));
}
