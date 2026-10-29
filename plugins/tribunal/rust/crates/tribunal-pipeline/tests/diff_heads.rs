//! The native external heads against the workflow's own `externalRunBlock`
//! bash: both run against the same fake `codex` and `gemini` on fixtures, and
//! leave the same artifacts (`.prompt`, `.out`, `.meta`) behind. The fake CLIs
//! echo the prompt they received into their answer, so the prompts the two
//! sides hand the CLI are compared as well.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{span, Span};
use serde_json::{json, Value};
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

fn plugin_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

fn workflow_blocks(
    finding: &Value,
    source: &str,
    external_cwd: &str,
    vote_dir: &str,
    slice: &Value,
) -> String {
    let out = span(
        &Span {
            start: "function externalRunBlock(finding, source, round) {",
            end: "  return [...writeBlocks, runBlock];\n}",
            deps: &["VERIFIED-RELAY", "EXTERNAL-PROMPT-ASSEMBLY", "HEAD-FAILURE", "ARTIFACT-KEY", "DIFF-SECTIONING"],
            pre: &[
                ("const EXTERNAL_HEAD_TIMEOUT_S = ", "const RETRY_FAST_THRESHOLD_S = Math.round(EXTERNAL_HEAD_TIMEOUT_S * 0.8);"),
                ("const EXTERNAL_SLICE_MARK = ", ";"),
                ("function externalPromptText(finding) {", "].join(\"\\n\");\n}"),
                ("const voteArtifact = (finding, source, ext, round = 0) =>", ";"),
            ],
            fns: &["externalDiffSlice"],
            ret: "externalRunBlock(finding, source, 0).join('\\n')",
        },
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

#[tokio::test]
async fn native_heads_leave_the_workflows_artifacts() {
    let root = std::env::temp_dir().join(format!("tribunal-heads-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let bin = root.join("bin");
    let home = root.join("home");
    let wt = root.join("wt");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(home.join(".codex/config.toml"), "model = \"gpt-top\"\n").unwrap();
    std::fs::write(
        home.join(".codex/review.config.toml"),
        "model = \"gpt-test\"\n",
    )
    .unwrap();
    let slice_file = root.join("own.slice");
    std::fs::write(
        &slice_file,
        "diff --git a/src/a.ts b/src/a.ts\n+const x = 1;\n",
    )
    .unwrap();
    for name in ["codex", "gemini"] {
        let p = bin.join(name);
        std::fs::write(&p, FAKE).unwrap();
        std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    }
    let tr = bin.join("tribunal-root");
    std::fs::write(
        &tr,
        format!("#!/bin/sh\necho '{}'\n", plugin_root().display()),
    )
    .unwrap();
    std::fs::set_permissions(&tr, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());

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
    for (n, (source, mode, wt_exists, slice)) in cases.iter().enumerate() {
        let finding = json!({"title": format!("T{n} the claim"), "file": "src/a.ts", "line": 3, "description": "d", "slotId": n, "touchesHighRiskSurface": n % 2 == 0});
        let cwd = if *wt_exists {
            wt.clone()
        } else {
            root.join("gone")
        };
        let fake = root.join(format!("fake-{n}"));
        for side in ["js", "rs"] {
            let dir = fake.join(side);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("mode"), mode).unwrap();
        }
        let js_votes = root.join(format!("votes-js-{n}"));
        let rs_votes = root.join(format!("votes-rs-{n}"));
        let bash = workflow_blocks(
            &finding,
            source,
            &cwd.to_string_lossy(),
            &js_votes.to_string_lossy(),
            slice,
        );
        let st = Command::new("bash")
            .arg("-c")
            .arg(&bash)
            .current_dir(&root)
            .env("PATH", &path)
            .env("HOME", &home)
            .env("FAKE_DIR", fake.join("js"))
            .env("GEMINI_API_KEY", "k")
            .output()
            .unwrap();
        assert!(
            st.status.success(),
            "{}",
            String::from_utf8_lossy(&st.stderr)
        );

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
            external_cwd: Some(cwd.clone()),
            fallback_cwd: root.clone(),
            home: home.clone(),
            codex_bin: bin.join("codex").to_string_lossy().into_owned(),
            gemini_bin: bin.join("gemini").to_string_lossy().into_owned(),
        };
        std::env::set_var("FAKE_DIR", fake.join("rs"));
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
            let a = read(&js_votes.join(format!("{n}-{source}.{ext}")));
            let b = read(&rs_votes.join(format!("{n}-{source}.{ext}")));
            assert_eq!(b, a, "case {n} ({source} {mode}) .{ext}");
        }
        assert!(
            !run_head(&setup, &format!("{n}-{source}"), source, &prompt, false).await,
            "the latch holds"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}
