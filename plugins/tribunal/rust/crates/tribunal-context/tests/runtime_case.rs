//! Runtime confirmation (`docs/runtime-confirmation-v1.md`). The pure parts
//! run on tables and generated cases; the child process and `confirm` run a
//! fake `vitest` the test writes, a shell script, so no node runs here. The
//! reporter files under `fixtures/runtime-case/` are real vitest 4.1 output of
//! the template, with the paths renamed. `real_vitest_confirms_and_passes` runs
//! the real vitest when `TRIBUNAL_VITEST_FROM` names a directory whose
//! `node_modules` holds it, and is `#[ignore]` otherwise.

use std::path::{Path, PathBuf};

use proptest::prelude::*;
use serde_json::{json, Value};
use tribunal_context::runtime_case::{
    classify, is_identifier, parse_claim, render_case, runnable_file, top_level_call,
    vitest_script, Claim, Expect, Outcome,
};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/runtime-case")
            .join(name),
    )
    .unwrap()
}

#[test]
fn a_claim_parses_or_says_why_not() {
    assert_eq!(parse_claim(None), None);
    assert_eq!(parse_claim(Some(&Value::Null)), None);
    assert_eq!(
        parse_claim(Some(
            &json!({"function": "ratio", "args": [1, 0], "expect": {"returns": 0}})
        )),
        Some(Ok(Claim {
            function: "ratio".into(),
            args: vec![json!(1), json!(0)],
            expect: Expect::Returns(json!(0)),
        }))
    );
    assert_eq!(
        parse_claim(Some(
            &json!({"function": "parse", "args": [], "expect": {"throws": true}})
        ))
        .unwrap()
        .unwrap()
        .expect,
        Expect::Throws
    );
    for bad in [
        json!("ratio"),
        json!({"function": "a.b", "args": [], "expect": {"returns": 1}}),
        json!({"function": "x", "args": {}, "expect": {"returns": 1}}),
        json!({"function": "x", "args": [], "expect": {}}),
        json!({"function": "x", "args": [], "expect": {"throws": false}}),
        json!({"function": "x", "args": [], "expect": {"returns": 1, "throws": true}}),
        json!({"args": [], "expect": {"returns": 1}}),
    ] {
        assert_eq!(
            parse_claim(Some(&bad)),
            Some(Err("malformed-claim")),
            "{bad}"
        );
    }
    assert_eq!(
        parse_claim(Some(
            &json!({"function": "x", "args": [], "expect": {"returns": null}})
        ))
        .unwrap()
        .unwrap()
        .expect,
        Expect::Returns(Value::Null)
    );
}

#[test]
fn only_source_files_of_js_and_ts_run() {
    for ok in [
        "src/a.ts",
        "a.tsx",
        "lib/b.mjs",
        "c.cjs",
        "d.js",
        "e.mts",
        "f.cts",
    ] {
        assert!(runnable_file(ok), "{ok}");
    }
    for no in [
        "src/a.d.ts",
        "a.py",
        "README.md",
        "Makefile",
        "src/.ts",
        "a.json",
    ] {
        assert!(!runnable_file(no), "{no}");
    }
}

#[test]
fn a_top_level_call_is_found_and_a_guarded_one_is_not() {
    let yes = [
        "main();",
        "export function f() {}\napp.listen(3000);",
        "(async () => { await run(); })();",
        "void start();",
        "await connect();",
        "new Server();",
        "module.exports = build();",
        "export default make();",
        "init?.();",
    ];
    for src in yes {
        assert_eq!(top_level_call("a.ts", src), Some(true), "{src}");
    }
    let no = [
        "export function ratio(a: number, b: number) { return a / b; }",
        "if (require.main === module) main();",
        "const schema = z.object({ a: z.string() });\nexport { schema };",
        "module.exports = { ratio };",
        "exports.ratio = ratio;",
        "export default function f() { g(); }",
        "export class A { m() { this.n(); } }",
        "'use strict';\nfunction f() {}",
    ];
    for src in no {
        assert_eq!(top_level_call("a.ts", src), Some(false), "{src}");
    }
    assert_eq!(top_level_call("a.ts", "export function ("), None);
    assert_eq!(
        top_level_call("a.tsx", "export const A = () => <div />;"),
        Some(false)
    );
}

#[test]
fn the_vitest_script_and_its_config_are_read_from_package_json() {
    let pkg = |scripts: Value| json!({"scripts": scripts}).to_string();
    assert_eq!(
        vitest_script(&pkg(
            json!({"test:unit": "vitest run --config src/tests/unit.config.mts", "test": "jest"})
        )),
        Some(Some("src/tests/unit.config.mts".into()))
    );
    assert_eq!(
        vitest_script(&pkg(json!({"test": "vitest run"}))),
        Some(None)
    );
    assert_eq!(
        vitest_script(&pkg(
            json!({"test": "prisma generate && vitest run -c vt.config.ts"})
        )),
        Some(Some("vt.config.ts".into()))
    );
    assert_eq!(
        vitest_script(&pkg(json!({"test": "npx vitest --config=v.mts"}))),
        Some(Some("v.mts".into()))
    );
    assert_eq!(
        vitest_script(&pkg(json!({"test": "jest", "test:e2e": "vitest"}))),
        None
    );
    assert_eq!(vitest_script(&pkg(json!({}))), None);
    assert_eq!(vitest_script("{not json"), None);
}

#[test]
fn the_reporter_output_is_classified() {
    let cases = [
        ("returns-mismatch.json", Outcome::Confirmed, "AssertionError: expected Infinity to deeply equal +0"),
        ("assertion-failed.json", Outcome::Confirmed, "AssertionError: TRIBUNAL_EXPECTED_THROW: expected false to be true // Object.is equality"),
        ("passed.json", Outcome::Passed, ""),
        ("threw.json", Outcome::Inconclusive, "Error: TRIBUNAL_THREW TypeError: empty"),
        ("not-a-function.json", Outcome::Inconclusive, "Error: TRIBUNAL_NOT_A_FUNCTION"),
        ("import-error.json", Outcome::Inconclusive, "Cannot find module './missing' imported from /work/project/src/calc.t5.test.ts"),
        ("import-threw-json.json", Outcome::Inconclusive, "[ { \"path\": [ \"DB_URL\" ], \"message\": \"Invalid input: expected string, received undefined\" } ]"),
    ];
    for (file, outcome, detail) in cases {
        assert_eq!(
            classify(Some(&fixture(file))),
            (outcome, detail.to_owned()),
            "{file}"
        );
    }
    assert_eq!(classify(None).0, Outcome::Inconclusive);
    assert_eq!(classify(Some("not json")).0, Outcome::Inconclusive);
    assert_eq!(
        classify(Some("{}")),
        (Outcome::Inconclusive, "no test result".into())
    );
}

#[test]
fn the_case_is_the_template() {
    let claim = Claim {
        function: "ratio".into(),
        args: vec![json!(1), json!(0)],
        expect: Expect::Returns(json!(0)),
    };
    assert_eq!(
        render_case("./calc.ts", &claim),
        "import { test, expect } from 'vitest';\nimport * as m from \"./calc.ts\";\ntest('tribunal runtime case', async () => {\n  const f = (m as Record<string, unknown>)[\"ratio\"];\n  if (typeof f !== 'function') throw new Error('TRIBUNAL_NOT_A_FUNCTION');\n  let threw = false;\n  let value: unknown;\n  let error: unknown;\n  try { value = await (f as (...a: unknown[]) => unknown)(...[1,0]); }\n  catch (e) { threw = true; error = e; }\n  if (threw) throw new Error('TRIBUNAL_THREW ' + String(error));\n  expect(value).toEqual(0);\n});\n"
    );
    let throws = Claim {
        expect: Expect::Throws,
        ..claim
    };
    let text = render_case("./calc.ts", &throws);
    assert!(text.contains("  expect(threw, 'TRIBUNAL_EXPECTED_THROW').toBe(true);\n});\n"));
    assert!(!text.contains("TRIBUNAL_THREW"));
}

fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i32>().prop_map(|n| json!(n)),
        ".*".prop_map(Value::String),
    ];
    leaf.prop_recursive(3, 16, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::btree_map("[a-z]{1,4}", inner, 0..3)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

proptest! {
    #[test]
    fn a_name_is_accepted_exactly_when_it_is_an_identifier(name in ".{0,12}") {
        let parsed = parse_claim(Some(&json!({"function": name, "args": [], "expect": {"throws": true}})));
        prop_assert_eq!(parsed.unwrap().is_ok(), is_identifier(&name));
        let simple = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
        prop_assert_eq!(is_identifier(&name), simple);
    }

    #[test]
    fn values_reach_the_case_only_as_json_literals(
        name in "[a-zA-Z_$][a-zA-Z0-9_$]{0,8}",
        args in prop::collection::vec(json_value(), 0..4),
        ret in json_value(),
    ) {
        let claim = Claim { function: name.clone(), args: args.clone(), expect: Expect::Returns(ret.clone()) };
        let text = render_case("./m.ts", &claim);
        let args_lit = serde_json::to_string(&Value::Array(args)).unwrap();
        let ret_lit = serde_json::to_string(&ret).unwrap();
        prop_assert!(text.contains(&format!("(...{args_lit}); }}\n")), "{text}");
        prop_assert!(text.contains(&format!("expect(value).toEqual({ret_lit});\n")), "{text}");
        prop_assert!(text.contains(&format!("[{}]", serde_json::to_string(&name).unwrap())), "{text}");
        prop_assert_eq!(text.lines().count(), 13);
    }
}

#[cfg(unix)]
mod process {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};
    use tribunal_context::runtime_case::{
        capped, child_env, confirm, run_child, Setup, OUTPUT_CAP,
    };

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tribunal-rtc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A fake vitest in `<checkout>/node_modules/.bin`. It records its
    /// arguments, environment, working directory and the case it was given in
    /// that directory, copies `reports/<case file name>` to the
    /// `--outputFile`, sleeps for the seconds in `sleep` and prints the bytes
    /// in `flood` to stdout and again to stderr, when those files exist.
    fn fake_vitest(checkout: &Path) -> PathBuf {
        let bin = checkout.join("node_modules/.bin");
        let script = r#"#!/bin/sh
d=$(dirname "$0")
printf '%s\n' "$@" > "$d/args"
env > "$d/env"
pwd > "$d/cwd"
cat "$2" > "$d/case-$(basename "$2")" 2>/dev/null
out=""
for a in "$@"; do case "$a" in --outputFile=*) out="${a#--outputFile=}";; esac; done
[ -f "$d/sleep" ] && sleep "$(cat "$d/sleep")"
[ -f "$d/flood" ] && head -c "$(cat "$d/flood")" /dev/zero | tr '\0' 'x'
[ -f "$d/flood" ] && head -c "$(cat "$d/flood")" /dev/zero | tr '\0' 'y' >&2
[ -f "$d/reports/$(basename "$2")" ] && cp "$d/reports/$(basename "$2")" "$out"
exit 1
"#;
        write(&bin.join("vitest"), script);
        std::fs::set_permissions(bin.join("vitest"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        bin
    }

    const CALC: &str = "export function ratio(a: number, b: number): number { return a / b; }\nexport function parse(s: string): number { if (!s) throw new TypeError('empty'); return Number(s); }\n";

    fn project(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let tree = root.join("tree");
        let checkout = root.join("checkout");
        write(
            &tree.join("package.json"),
            &json!({"scripts": {"test:unit": "vitest run --config vt.config.mts"}}).to_string(),
        );
        write(&tree.join("src/calc.ts"), CALC);
        write(
            &tree.join("src/widgets-autostart.ts"),
            "export function ratio() { return 1; }\nstart();\n",
        );
        let bin = fake_vitest(&checkout);
        (tree, checkout, bin)
    }

    fn cand(file: &str, claim: Value) -> Value {
        json!({"title": "T", "file": file, "description": "D", "runtimeClaim": claim})
    }

    fn claim(function: &str, args: Value, expect: Value) -> Value {
        json!({"function": function, "args": args, "expect": expect})
    }

    #[tokio::test]
    async fn the_child_gets_only_the_listed_environment() {
        let root = scratch("env");
        let (tree, _, bin) = project(&root);
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        assert!(
            std::env::var_os("CARGO").is_some(),
            "cargo sets CARGO for tests"
        );
        let env = child_env(std::env::var_os("PATH").as_deref(), &home, &root);
        let ran = run_child(
            &bin.join("vitest"),
            &["run".into(), "x".into()],
            &tree,
            &env,
            &root.join("none.json"),
            Duration::from_secs(20),
        )
        .await;
        assert!(!ran.timed_out);
        let seen = std::fs::read_to_string(bin.join("env")).unwrap();
        let keys: Vec<&str> = seen
            .lines()
            .filter_map(|l| l.split_once('=').map(|(k, _)| k))
            .collect();
        for k in &keys {
            assert!(
                ["HOME", "TMPDIR", "CI", "NODE_ENV", "PATH", "PWD", "SHLVL", "_", "OLDPWD"]
                    .contains(k),
                "unexpected variable {k} in {seen}"
            );
        }
        assert!(
            seen.contains(&format!("HOME={}\n", home.display())),
            "{seen}"
        );
        assert!(!seen.contains("CARGO"), "{seen}");
        assert_eq!(
            std::fs::read_to_string(bin.join("cwd")).unwrap().trim(),
            tree.canonicalize().unwrap().to_string_lossy()
        );
    }

    #[tokio::test]
    async fn a_stream_is_kept_up_to_the_cap_and_read_to_its_end() {
        use tokio::io::AsyncReadExt;
        let big = tokio::io::repeat(b'x').take(100_000);
        assert_eq!(capped(big).await.len(), OUTPUT_CAP);
        let small = tokio::io::repeat(b'y').take(10);
        assert_eq!(capped(small).await, b"yyyyyyyyyy".to_vec());
    }

    #[tokio::test]
    async fn a_child_that_hangs_is_killed_at_the_timeout() {
        let root = scratch("timeout");
        let (tree, _, bin) = project(&root);
        write(&bin.join("sleep"), "30");
        let started = Instant::now();
        let ran = run_child(
            &bin.join("vitest"),
            &["run".into(), "x".into()],
            &tree,
            &[("PATH".into(), std::env::var_os("PATH").unwrap())],
            &root.join("none.json"),
            Duration::from_secs(1),
        )
        .await;
        assert!(ran.timed_out);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert!(ran.report.is_none());
    }

    #[tokio::test]
    async fn the_output_kept_is_capped() {
        let root = scratch("flood");
        let (tree, _, bin) = project(&root);
        write(&bin.join("flood"), "200000");
        let ran = run_child(
            &bin.join("vitest"),
            &["run".into(), "x".into()],
            &tree,
            &[("PATH".into(), std::env::var_os("PATH").unwrap())],
            &root.join("none.json"),
            Duration::from_secs(20),
        )
        .await;
        assert!(!ran.timed_out);
        assert_eq!(ran.output.len(), OUTPUT_CAP);
    }

    #[tokio::test]
    async fn confirm_runs_each_claim_and_marks_only_the_confirmed_one() {
        let root = scratch("confirm");
        let (tree, checkout, bin) = project(&root);
        write(
            &bin.join("reports/calc.tribunal-case-2.test.ts"),
            &fixture("returns-mismatch.json"),
        );
        write(
            &bin.join("reports/calc.tribunal-case-3.test.ts"),
            &fixture("passed.json"),
        );
        write(
            &bin.join("reports/calc.tribunal-case-4.test.ts"),
            &fixture("threw.json"),
        );
        let mut candidates = vec![
            json!({"title": "no claim", "file": "src/calc.ts", "description": "D"}),
            cand(
                "src/calc.ts",
                claim("ratio", json!([1, 0]), json!({"returns": 0})),
            ),
            cand(
                "src/calc.ts",
                claim("ratio", json!([4, 2]), json!({"returns": 2})),
            ),
            cand(
                "src/calc.ts",
                claim("parse", json!([""]), json!({"returns": 0})),
            ),
            cand("src/calc.ts", json!({"function": "ratio"})),
            cand(
                "src/other.ts",
                claim("ratio", json!([1, 0]), json!({"returns": 0})),
            ),
            cand(
                "src/widgets-autostart.ts",
                claim("ratio", json!([]), json!({"returns": 1})),
            ),
            cand(
                "README.md",
                claim("ratio", json!([]), json!({"returns": 1})),
            ),
        ];
        let data = root.join("data");
        let pr_files: Vec<String> = ["src/calc.ts", "src/widgets-autostart.ts", "README.md"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let record = confirm(
            &mut candidates,
            &Setup {
                pr_files: &pr_files,
                tree: Some(&tree),
                checkout: &checkout,
                data_dir: &data,
                timeout: Duration::from_secs(20),
            },
        )
        .await;
        let outcomes: Vec<(&str, &str)> = record["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    c["outcome"].as_str().unwrap(),
                    c["reason"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            outcomes,
            vec![
                (
                    "confirmed",
                    "AssertionError: expected Infinity to deeply equal +0"
                ),
                ("passed", ""),
                ("inconclusive", "Error: TRIBUNAL_THREW TypeError: empty"),
                ("skipped", "malformed-claim"),
                ("skipped", "file-not-in-pr"),
                ("skipped", "top-level-call"),
                ("skipped", "not-a-js-or-ts-file"),
            ]
        );
        assert_eq!(
            (
                &record["withoutClaim"],
                &record["confirmed"],
                &record["passed"],
                &record["inconclusive"],
                &record["skipped"]
            ),
            (&json!(1), &json!(1), &json!(1), &json!(1), &json!(4))
        );
        assert_eq!(record["config"], json!("vt.config.mts"));
        let marked: Vec<usize> = candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| c.get("runtimeCase").is_some())
            .map(|(i, _)| i)
            .collect();
        assert_eq!(marked, vec![1]);
        assert_eq!(
            candidates[1]["runtimeCase"],
            json!({"function": "ratio", "args": [1, 0], "expect": {"returns": 0}, "failure": "AssertionError: expected Infinity to deeply equal +0"})
        );
        let args = std::fs::read_to_string(bin.join("args")).unwrap();
        assert!(args.starts_with("run\nsrc/calc.tribunal-case-4.test.ts\n--config\nvt.config.mts\n--reporter=json\n--outputFile="), "{args}");
        let leftover: Vec<_> = std::fs::read_dir(tree.join("src"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("tribunal-case"))
            .collect();
        assert!(leftover.is_empty(), "{leftover:?}");
        let ran_case =
            std::fs::read_to_string(bin.join("case-calc.tribunal-case-2.test.ts")).unwrap();
        assert!(
            ran_case.contains("import * as m from \"./calc.ts\";"),
            "{ran_case}"
        );
        assert_eq!(
            std::fs::read_to_string(data.join("case-2/calc.tribunal-case-2.test.ts")).unwrap(),
            ran_case
        );
        assert!(tree.join("node_modules").exists());
    }

    #[tokio::test]
    async fn no_more_than_eight_cases_run() {
        let root = scratch("limit");
        let (tree, checkout, _) = project(&root);
        let mut candidates: Vec<Value> = (0..10)
            .map(|i| {
                cand(
                    "src/calc.ts",
                    claim("ratio", json!([i, 1]), json!({"returns": i})),
                )
            })
            .collect();
        let pr_files = vec!["src/calc.ts".to_owned()];
        let record = confirm(
            &mut candidates,
            &Setup {
                pr_files: &pr_files,
                tree: Some(&tree),
                checkout: &checkout,
                data_dir: &root.join("data"),
                timeout: Duration::from_secs(20),
            },
        )
        .await;
        assert_eq!(record["inconclusive"], json!(8));
        assert_eq!(record["skipped"], json!(2));
        assert_eq!(record["cases"][9]["reason"], json!("case-limit"));
    }

    #[tokio::test]
    async fn without_a_runner_every_claim_is_skipped_with_the_reason() {
        let root = scratch("norunner");
        let (tree, checkout, _) = project(&root);
        let pr_files = vec!["src/calc.ts".to_owned()];
        let one = || {
            vec![cand(
                "src/calc.ts",
                claim("ratio", json!([1, 0]), json!({"returns": 0})),
            )]
        };
        let reason = |record: &Value| record["cases"][0]["reason"].clone();

        let mut c = one();
        let no_tree = confirm(
            &mut c,
            &Setup {
                pr_files: &pr_files,
                tree: None,
                checkout: &checkout,
                data_dir: &root.join("d1"),
                timeout: Duration::from_secs(5),
            },
        )
        .await;
        assert_eq!(reason(&no_tree), json!("no-pinned-tree"));

        write(
            &tree.join("package.json"),
            &json!({"scripts": {"test": "jest"}}).to_string(),
        );
        let mut c = one();
        let no_script = confirm(
            &mut c,
            &Setup {
                pr_files: &pr_files,
                tree: Some(&tree),
                checkout: &checkout,
                data_dir: &root.join("d2"),
                timeout: Duration::from_secs(5),
            },
        )
        .await;
        assert_eq!(reason(&no_script), json!("no-vitest-script"));

        write(
            &tree.join("package.json"),
            &json!({"scripts": {"test": "vitest run"}}).to_string(),
        );
        let mut c = one();
        let missing = confirm(
            &mut c,
            &Setup {
                pr_files: &pr_files,
                tree: Some(&tree),
                checkout: &root.join("elsewhere"),
                data_dir: &root.join("d3"),
                timeout: Duration::from_secs(5),
            },
        )
        .await;
        assert_eq!(reason(&missing), json!("vitest-not-installed"));
        assert!(c[0].get("runtimeCase").is_none());
    }

    #[tokio::test]
    #[ignore = "needs TRIBUNAL_VITEST_FROM: a directory whose node_modules holds vitest"]
    async fn real_vitest_confirms_and_passes() {
        let Some(from) = std::env::var_os("TRIBUNAL_VITEST_FROM") else {
            panic!("set TRIBUNAL_VITEST_FROM");
        };
        let root = scratch("real");
        let tree = root.join("tree");
        write(
            &tree.join("package.json"),
            &json!({"scripts": {"test": "vitest run"}}).to_string(),
        );
        write(&tree.join("src/calc.ts"), CALC);
        let mut candidates = vec![
            cand(
                "src/calc.ts",
                claim("ratio", json!([1, 0]), json!({"returns": 0})),
            ),
            cand(
                "src/calc.ts",
                claim("ratio", json!([4, 2]), json!({"returns": 2})),
            ),
            cand(
                "src/calc.ts",
                claim("parse", json!([""]), json!({"throws": true})),
            ),
            cand(
                "src/calc.ts",
                claim("ratio", json!([1, 1]), json!({"throws": true})),
            ),
            cand(
                "src/calc.ts",
                claim("parse", json!([""]), json!({"returns": 0})),
            ),
            cand(
                "src/calc.ts",
                claim("missing", json!([]), json!({"returns": 0})),
            ),
        ];
        let pr_files = vec!["src/calc.ts".to_owned()];
        let record = confirm(
            &mut candidates,
            &Setup {
                pr_files: &pr_files,
                tree: Some(&tree),
                checkout: Path::new(&from),
                data_dir: &root.join("data"),
                timeout: Duration::from_secs(60),
            },
        )
        .await;
        let outcomes: Vec<&str> = record["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["outcome"].as_str().unwrap())
            .collect();
        assert_eq!(
            outcomes,
            [
                "confirmed",
                "passed",
                "passed",
                "confirmed",
                "inconclusive",
                "inconclusive"
            ],
            "{record:#}"
        );
    }
}
