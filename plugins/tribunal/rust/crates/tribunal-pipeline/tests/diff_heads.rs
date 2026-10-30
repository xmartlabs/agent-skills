//! The native external heads against the workflow's own `externalRunBlock`
//! bash: both run against the same fake `codex` and `gemini` on fixtures, and
//! leave the same artifacts (`.prompt`, `.out`, `.meta`) behind. The fake CLIs
//! echo the prompt they received into their answer, so the prompts the two
//! sides hand the CLI are compared as well.
//!
//! The bash's side is read from `tests/goldens/external-heads.json`: the
//! workflow and the `lib/` scripts its bash runs are being retired, so a run
//! starts only the port. The file keeps each case's input, the fixture setup,
//! the bash the span oracle cut out, and a SHA-256 of the workflow code that
//! bash came from, of the span oracle's own files and of each `lib/` script,
//! so editing any of them without a regeneration fails here. A source that is
//! gone from disk is retired and is not checked; the golden is final. The
//! scratch directory and the plugin root are stored as `<ROOT>` and
//! `<PLUGIN>`. Regenerate after changing them:
//! `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-pipeline --test diff_heads`.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{span, Span};
use serde_json::{json, Value};
use tribunal_context::harness_version::sha256_hex;
use tribunal_context::heads::{assemble, run_head, HeadSetup};
use tribunal_pipeline::panel_prompts::{
    external_context_instruction, external_prompt, EXTERNAL_SLICE_MARK,
};

const FILES: &[&str] = &["src/a.ts", "src/b.ts"];

const FAKE: &str = r#"#!/bin/sh
MODE=$(cat "$FAKE_DIR/mode")
N=$(cat "$FAKE_DIR/count" 2>/dev/null || echo 0); N=$((N+1)); echo $N > "$FAKE_DIR/count"
OUT=""; PROMPT=""; PREV=""
for a in "$@"; do
  [ "$PREV" = "-o" ] && [ "$a" != text ] && OUT="$a"
  [ "$PREV" = "-p" ] && PROMPT="$a"
  [ "$PREV" = "-c" ] && EFF="$a"
  [ "$PREV" = "-m" ] && MODEL="$a"
  PREV="$a"; LAST="$a"
done
[ -z "$PROMPT" ] && PROMPT="$LAST"
case "$MODE" in
  ok) ANSWER="VERDICT: REFUTED — the guard holds
CONFIDENCE: 0.8
$EFF $MODEL
---PROMPT---
$PROMPT" ;;
  quota) echo "ERROR: You exceeded your current quota" >&2; exit 1 ;;
  auth) echo "Error: 401 Unauthorized" >&2; exit 1 ;;
  ratelimit) if [ $N = 1 ]; then echo "ERROR: 429 rate limit" >&2; exit 1; fi
    ANSWER="NOT REFUTED
because
---PROMPT---
$PROMPT" ;;
  empty) exit 0 ;;
esac
if [ -n "$OUT" ]; then printf '%s\n' "$ANSWER" > "$OUT"; else printf '%s\n' "$ANSWER"; fi
"#;

const CODEX_CONFIG: &str = "model = \"gpt-top\"\n";
const CODEX_REVIEW_CONFIG: &str = "model = \"gpt-test\"\n";
const SLICE: &str = "diff --git a/src/a.ts b/src/a.ts\n+const x = 1;\n";

fn plugin_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

const RUN_BLOCK: Span<'static> = Span {
    start: "function externalRunBlock(finding, source, round) {",
    end: "  return [...writeBlocks, runBlock];\n}",
    deps: &[
        "VERIFIED-RELAY",
        "EXTERNAL-PROMPT-ASSEMBLY",
        "HEAD-FAILURE",
        "ARTIFACT-KEY",
        "DIFF-SECTIONING",
    ],
    pre: &[
        (
            "const EXTERNAL_HEAD_TIMEOUT_S = ",
            "const RETRY_FAST_THRESHOLD_S = Math.round(EXTERNAL_HEAD_TIMEOUT_S * 0.8);",
        ),
        ("const EXTERNAL_SLICE_MARK = ", ";"),
        (
            "function externalPromptText(finding) {",
            "].join(\"\\n\");\n}",
        ),
        (
            "const voteArtifact = (finding, source, ext, round = 0) =>",
            ";",
        ),
    ],
    fns: &["externalDiffSlice"],
    ret: "externalRunBlock(finding, source, 0).join('\\n')",
};

fn workflow_blocks(
    finding: &Value,
    source: &str,
    external_cwd: &str,
    vote_dir: &str,
    slice: &Value,
) -> String {
    let out = span(
        &RUN_BLOCK,
        &[json!({
            "finding": finding, "source": source, "externalCwd": external_cwd, "VOTE_DIR": vote_dir,
            "prContext": {"files": FILES, "headRefOid": null}, "externalContextInstruction": external_context_instruction(""),
            "externalDiffSlice": format!("() => ({})", slice),
        })],
    )
    .remove(0);
    out.as_str().unwrap().to_owned()
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|_| "<missing>".into())
}

// ---------------------------------------------------------------------------
// Golden: the bash's side recorded once, so a run starts only the port.

const GOLDEN: &str = "external-heads.json";
const REGENERATE: &str =
    "regenerate with `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-pipeline --test diff_heads`";
const WORKFLOW: &str = "workflow-src/tribunal.js";
/// The span oracle's own files and the scripts the bash runs through
/// `tribunal-root`, by path in the plugin.
const ORACLE_FILES: &[&str] = &[
    "rust/oracle/span-oracle.mjs",
    "skills/review-pr/eval/extract-pure-region.mjs",
    "lib/classify-head-failure.sh",
    "lib/classify-verdict.sh",
    "lib/codex-model.sh",
    "lib/gemini-exec.sh",
];

fn regenerating() -> bool {
    std::env::var("TRIBUNAL_REGEN_GOLDENS").is_ok_and(|v| v == "1")
}

/// The workflow code the span oracle runs for `s`, cut out the way it cuts it;
/// `None` when an anchor is no longer in `src`.
fn span_source(src: &str, s: &Span) -> Option<String> {
    let cut = |start: &str, end: &str| -> Option<String> {
        let a = src.find(start)?;
        let e = a + src[a..].find(end)?;
        Some(src[a..e + end.len()].to_owned())
    };
    let mut parts = Vec::new();
    for d in s.deps {
        let (begin, end) = (
            format!("// ===== BEGIN {d} ====="),
            format!("// ===== END {d} ====="),
        );
        let mut from = 0;
        while let Some(b) = src[from..].find(&begin).map(|b| b + from) {
            let e = b + src[b..].find(&end)?;
            parts.push(src[b + begin.len()..e].to_owned());
            from = e + end.len();
        }
    }
    for (a, b) in s.pre {
        parts.push(cut(a, b)?);
    }
    parts.push(cut(s.start, s.end)?);
    Some(parts.join("\n"))
}

/// The SHA-256 of what the recorded bash came from, by path in the plugin
/// under `root`: the span's code for the workflow, the whole file otherwise.
/// A file that is gone maps to `None`: that oracle is retired.
fn sources(root: &Path) -> BTreeMap<String, Option<String>> {
    let mut out = BTreeMap::new();
    let workflow = std::fs::read_to_string(root.join(WORKFLOW)).ok();
    out.insert(
        WORKFLOW.to_owned(),
        workflow.map(|w| sha256_hex(span_source(&w, &RUN_BLOCK).unwrap_or_default().as_bytes())),
    );
    for f in ORACLE_FILES {
        let sha = std::fs::read(root.join(f)).ok().map(|b| sha256_hex(&b));
        out.insert((*f).to_owned(), sha);
    }
    out
}

/// The sources still on disk that differ from `recorded`; a retired one is
/// skipped, since nothing can regenerate from it.
fn stale(recorded: &Value, now: &BTreeMap<String, Option<String>>) -> Vec<String> {
    let mut keys: Vec<&String> = recorded
        .as_object()
        .map(|o| o.keys().collect())
        .unwrap_or_default();
    keys.sort();
    if keys != now.keys().collect::<Vec<_>>() {
        return vec!["the list of sources".to_owned()];
    }
    now.iter()
        .filter(|(k, sha)| {
            sha.as_ref()
                .is_some_and(|s| recorded[k.as_str()] != json!(s))
        })
        .map(|(k, _)| k.clone())
        .collect()
}

/// The bash's side of each case: run by `record` and written to the golden
/// when regenerating, else read back after checking the sources, the setup
/// and the inputs. `swaps` pairs this run's paths with their placeholders.
fn recorded(
    swaps: &[(&str, &str)],
    setup: &Value,
    inputs: &[Value],
    record: impl FnOnce() -> Vec<Value>,
) -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens")
        .join(GOLDEN);
    if regenerating() {
        let with: BTreeMap<String, String> = sources(&plugin_root())
            .into_iter()
            .map(|(k, sha)| {
                let sha = sha.unwrap_or_else(|| panic!("{k} is gone: nothing to regenerate from"));
                (k, sha)
            })
            .collect();
        let js = record();
        let cases: Vec<Value> = inputs
            .iter()
            .zip(&js)
            .map(|(input, js)| json!({"input": input, "js": js}))
            .collect();
        let golden = json!({"recordedWith": with, "setup": setup, "cases": cases});
        let text = swaps.iter().fold(
            serde_json::to_string_pretty(&golden).unwrap(),
            |t, (real, placeholder)| t.replace(real, placeholder),
        );
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text + "\n").unwrap();
        return js;
    }
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; {REGENERATE}", path.display()));
    let text = swaps
        .iter()
        .fold(text, |t, (real, placeholder)| t.replace(placeholder, real));
    let golden: Value = serde_json::from_str(&text).unwrap();
    let stale = stale(&golden["recordedWith"], &sources(&plugin_root()));
    assert!(
        stale.is_empty(),
        "{} changed since {} was recorded; {REGENERATE}",
        stale.join(", "),
        path.display()
    );
    assert_eq!(
        &golden["setup"], setup,
        "the fixtures changed; {REGENERATE}"
    );
    let cases = golden["cases"].as_array().unwrap();
    assert_eq!(cases.len(), inputs.len(), "the cases changed; {REGENERATE}");
    cases
        .iter()
        .zip(inputs)
        .map(|(c, input)| {
            assert_eq!(&c["input"], input, "the cases changed; {REGENERATE}");
            c["js"].clone()
        })
        .collect()
}

#[test]
fn a_retired_source_is_not_checked_but_a_changed_one_is() {
    let root = std::env::temp_dir().join(format!("tribunal-heads-retired-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let gone = sources(&root);
    assert!(gone.values().all(Option::is_none));
    let recorded = json!(gone
        .keys()
        .map(|k| (k, "recorded"))
        .collect::<BTreeMap<_, _>>());
    assert!(stale(&recorded, &gone).is_empty());
    for (i, f) in [WORKFLOW, "lib/classify-verdict.sh"]
        .into_iter()
        .enumerate()
    {
        let at = root.join(f);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(&at, "edited").unwrap();
        assert_eq!(stale(&recorded, &sources(&root)).len(), i + 1, "{f}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn native_heads_leave_the_workflows_artifacts() {
    let root = std::env::temp_dir().join(format!("tribunal-heads-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let bin = root.join("bin");
    let home = root.join("home");
    let wt = root.join("wt");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(home.join(".codex/config.toml"), CODEX_CONFIG).unwrap();
    std::fs::write(home.join(".codex/review.config.toml"), CODEX_REVIEW_CONFIG).unwrap();
    let slice_file = root.join("own.slice");
    std::fs::write(&slice_file, SLICE).unwrap();
    for name in ["codex", "gemini"] {
        let p = bin.join(name);
        std::fs::write(&p, FAKE).unwrap();
        std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    }

    let cases = [
        (
            "codex",
            "ok",
            true,
            json!({"file": slice_file.to_string_lossy()}),
        ),
        (
            "codex",
            "quota",
            true,
            json!({"text": "No diff excerpt is provided."}),
        ),
        ("codex", "auth", true, json!({"text": "x"})),
        (
            "codex",
            "ratelimit",
            true,
            json!({"file": slice_file.to_string_lossy()}),
        ),
        ("codex", "empty", true, json!({"text": "x"})),
        ("codex", "ok", false, json!({"text": "x"})),
        ("gemini", "ok", true, json!({"text": "y"})),
        ("gemini", "empty", true, json!({"text": "y"})),
    ];
    let finding = |n: usize| json!({"title": format!("T{n} the claim"), "file": "src/a.ts", "line": 3, "description": "d", "slotId": n, "touchesHighRiskSurface": n.is_multiple_of(2)});
    let cwd = |wt_exists: bool| {
        if wt_exists {
            wt.clone()
        } else {
            root.join("gone")
        }
    };
    let inputs: Vec<Value> = cases
        .iter()
        .enumerate()
        .map(|(n, (source, mode, wt_exists, slice))| {
            json!({"source": source, "mode": mode, "wtExists": wt_exists, "slice": slice, "finding": finding(n)})
        })
        .collect();
    let setup = json!({
        "fake": sha256_hex(FAKE.as_bytes()), "files": FILES, "codexConfig": CODEX_CONFIG,
        "codexReviewConfig": CODEX_REVIEW_CONFIG, "slice": SLICE,
        "externalContextInstruction": external_context_instruction(""),
    });
    let plugin = plugin_root();
    let (root_text, plugin_text) = (root.to_string_lossy(), plugin.to_string_lossy());
    let js = recorded(
        &[(&root_text, "<ROOT>"), (&plugin_text, "<PLUGIN>")],
        &setup,
        &inputs,
        || {
            let tr = bin.join("tribunal-root");
            std::fs::write(&tr, format!("#!/bin/sh\necho '{plugin_text}'\n")).unwrap();
            std::fs::set_permissions(&tr, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .unwrap();
            let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
            cases
                .iter()
                .enumerate()
                .map(|(n, (source, mode, wt_exists, slice))| {
                    let fake = root.join(format!("fake-{n}/js"));
                    std::fs::create_dir_all(&fake).unwrap();
                    std::fs::write(fake.join("mode"), mode).unwrap();
                    let js_votes = root.join(format!("votes-js-{n}"));
                    let bash = workflow_blocks(
                        &finding(n),
                        source,
                        &cwd(*wt_exists).to_string_lossy(),
                        &js_votes.to_string_lossy(),
                        slice,
                    );
                    let st = Command::new("bash")
                        .arg("-c")
                        .arg(&bash)
                        .current_dir(&root)
                        .env("PATH", &path)
                        .env("HOME", &home)
                        .env("FAKE_DIR", &fake)
                        .env("GEMINI_API_KEY", "k")
                        .output()
                        .unwrap();
                    assert!(
                        st.status.success(),
                        "{}",
                        String::from_utf8_lossy(&st.stderr)
                    );
                    let mut artifacts = json!({"bash": bash});
                    for ext in ["out", "meta", "prompt"] {
                        artifacts[ext] = json!(read(&js_votes.join(format!("{n}-{source}.{ext}"))));
                    }
                    artifacts
                })
                .collect()
        },
    );

    for (n, ((source, mode, wt_exists, slice), js)) in cases.iter().zip(&js).enumerate() {
        let finding = finding(n);
        let fake = root.join(format!("fake-{n}/rs"));
        std::fs::create_dir_all(&fake).unwrap();
        std::fs::write(fake.join("mode"), mode).unwrap();
        let rs_votes = root.join(format!("votes-rs-{n}"));
        let text = slice.get("text").and_then(Value::as_str);
        let body = external_prompt(
            &finding,
            &FILES.iter().map(|f| (*f).to_owned()).collect::<Vec<_>>(),
            &external_context_instruction(""),
            text,
        );
        let prompt = assemble(
            &body,
            slice.get("file").and_then(Value::as_str),
            EXTERNAL_SLICE_MARK,
        );
        let setup = HeadSetup {
            vote_dir: rs_votes.clone(),
            external_cwd: Some(cwd(*wt_exists)),
            fallback_cwd: root.clone(),
            home: home.clone(),
            codex_bin: bin.join("codex").to_string_lossy().into_owned(),
            gemini_bin: bin.join("gemini").to_string_lossy().into_owned(),
        };
        std::env::set_var("FAKE_DIR", &fake);
        std::env::set_var("GEMINI_API_KEY", "k");
        assert!(
            run_head(
                &setup,
                &format!("{n}-{source}"),
                source,
                &prompt,
                n % 2 == 0
            )
            .await
        );
        if *mode == "ok" && *wt_exists {
            let out = read(&rs_votes.join(format!("{n}-{source}.out")));
            assert!(
                out.starts_with("VERDICT: REFUTED") && out.contains("---PROMPT---\nREFUTE"),
                "{out}"
            );
            if *source == "codex" {
                assert!(
                    out.contains("model_reasoning_effort=high gpt-test"),
                    "{out}"
                );
            } else {
                assert!(out.contains(" gemini-2.5-flash\n"), "{out}");
            }
        }
        for ext in ["out", "meta", "prompt"] {
            let b = read(&rs_votes.join(format!("{n}-{source}.{ext}")));
            assert_eq!(json!(b), js[ext], "case {n} ({source} {mode}) .{ext}");
        }
        assert!(
            !run_head(&setup, &format!("{n}-{source}"), source, &prompt, false).await,
            "the latch holds"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}
