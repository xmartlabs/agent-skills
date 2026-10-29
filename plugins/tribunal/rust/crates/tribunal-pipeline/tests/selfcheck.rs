//! `selfcheck` end to end with a scripted agent: what it audits, which
//! diagnoses reach the verifiers, which survive them, and when a patch is
//! asked for. `no_battery` keeps `cargo test` out of these runs.

use std::path::PathBuf;

use serde_json::{json, Value};
use tribunal_agent::{FakeAgent, FakeAnswer};
use tribunal_context::audit_run;
use tribunal_context::exec::Tools;
use tribunal_context::operator::OperatorEnv;
use tribunal_pipeline::context::Runtime;
use tribunal_pipeline::selfcheck::{selfcheck, SelfcheckArgs};

struct World {
    root: PathBuf,
    rt: Runtime,
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn world(name: &str, row: Option<&Value>) -> World {
    let root =
        std::env::temp_dir().join(format!("tribunal-selfcheck-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    if let Some(r) = row {
        std::fs::write(data.join("metrics.jsonl"), format!("{{}}\n{r}\n\n")).unwrap();
    }
    let rt = Runtime {
        plugin_root: root.join("plugin"),
        tmpdir: root.to_string_lossy().into_owned(),
        tools: Tools::default(),
        operator: OperatorEnv {
            review_harness_config: None,
            home: root.to_string_lossy().into_owned(),
            cwd: root.clone(),
        },
        data_dir: data,
    };
    World { root, rt }
}

fn args() -> SelfcheckArgs {
    SelfcheckArgs {
        model: "m".into(),
        no_battery: true,
        ..SelfcheckArgs::default()
    }
}

/// A row that fails two invariants, and their ids as the audit names them.
fn failing_row(engine: &str) -> (Value, Vec<String>) {
    let row = json!({
        "pr": 7, "headSha": "abc", "engine": engine, "findings": [],
        "reportVerdictCorrected": {"from": "APPROVE", "reason": "r", "to": "COMMENT"},
        "relays": [{"label": "fetch", "verified": false}],
    });
    let ids = audit_run::run(&[""], &row.to_string())
        .unwrap()
        .violations
        .into_iter()
        .map(|v| v.id)
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 2, "{ids:?}");
    (row, ids)
}

fn diagnosis(id: &str, evidence: Value) -> FakeAnswer {
    FakeAnswer::Structured(json!({
        "violationId": id, "rootCause": "the relay file was never written",
        "evidence": evidence, "confidence": 0.8, "proposedFix": "write it",
    }))
}

fn verdict(refuted: bool, reason: &str) -> FakeAnswer {
    FakeAnswer::Structured(
        json!({"refuted": refuted, "reason": reason, "alternativeCause": if refuted { json!("a stale cache") } else { Value::Null }}),
    )
}

fn labels(agent: &FakeAgent) -> Vec<String> {
    let mut l: Vec<String> = agent.calls().into_iter().map(|c| c.label).collect();
    l.sort();
    l
}

#[tokio::test]
async fn no_row_is_reported_as_not_audited_not_as_clean() {
    let w = world("norow", None);
    let agent = FakeAgent::new();
    let mut logs = Vec::new();
    let out = selfcheck(&agent, &w.rt, &args(), &mut logs).await;
    assert_eq!(out["audited"], json!(false));
    assert_eq!(out["reason"], json!("no-metrics-row"));
    assert!(agent.calls().is_empty());
    assert!(logs[0].contains("broken path"), "{logs:?}");
}

#[tokio::test]
async fn a_clean_row_dispatches_nothing() {
    let w = world("clean", Some(&json!({"pr": 7, "findings": []})));
    let agent = FakeAgent::new();
    let out = selfcheck(&agent, &w.rt, &args(), &mut Vec::new()).await;
    assert_eq!(out["audited"], json!(true));
    assert_eq!(out["violations"], json!([]));
    assert!(agent.calls().is_empty());
}

/// A diagnosis both verifiers fail to refute is confirmed and gets a patch
/// proposal; one that quotes nothing never reaches the verifiers.
#[tokio::test]
async fn only_an_evidenced_unrefuted_diagnosis_is_confirmed_and_patched() {
    let (row, ids) = failing_row("rust");
    let w = world("confirm", Some(&row));
    let (a, b) = (&ids[0], &ids[1]);
    let agent = FakeAgent::new()
        .answer(
            format!("diagnose:{a}"),
            diagnosis(a, json!([{"source": "run.log", "quote": "no relay file"}])),
        )
        .answer(format!("refute:{a}"), verdict(false, "the quote is there"))
        .answer(format!("reproduce:{a}"), verdict(false, "reproduced"))
        .answer(format!("diagnose:{b}"), diagnosis(b, json!([])))
        .answer("propose-patch", FakeAnswer::Text("the patch".into()));
    let out = selfcheck(&agent, &w.rt, &args(), &mut Vec::new()).await;
    assert_eq!(out["confirmed"][0]["id"], json!(a));
    assert_eq!(out["confirmed"].as_array().unwrap().len(), 1);
    assert_eq!(
        out["dropped"],
        json!([{"id": b, "why": "no evidence cited", "alternatives": []}])
    );
    assert_eq!(out["patch"], json!("the patch"));
    assert_eq!(out["battery"]["ran"], json!(false));
    assert_eq!(out["applied"], json!(false));
    let mut want = vec![
        format!("diagnose:{a}"),
        format!("diagnose:{b}"),
        format!("refute:{a}"),
        format!("reproduce:{a}"),
        "propose-patch".to_owned(),
    ];
    want.sort();
    assert_eq!(labels(&agent), want);

    let calls = agent.calls();
    let diag = calls
        .iter()
        .find(|c| c.label == format!("diagnose:{a}"))
        .unwrap();
    assert!(
        diag.prompt.contains("/plugin/rust/crates"),
        "{}",
        diag.prompt
    );
    assert!(diag.schema.is_some());
    let patch = calls.iter().find(|c| c.label == "propose-patch").unwrap();
    assert!(patch.prompt.contains("Do NOT edit any file"));
    assert!(!patch.tools.iter().any(|t| t == "Edit" || t == "Write"));
}

/// One refutation is enough to drop a diagnosis, with the refuter's reason
/// and the cause it named instead; nothing is then asked to patch.
#[tokio::test]
async fn a_refuted_diagnosis_is_dropped_and_nothing_is_patched() {
    let (row, ids) = failing_row("");
    let w = world("refuted", Some(&row));
    let mut agent = FakeAgent::new();
    for id in &ids {
        agent = agent
            .answer(
                format!("diagnose:{id}"),
                diagnosis(id, json!([{"source": "a", "quote": "b"}])),
            )
            .answer(
                format!("refute:{id}"),
                verdict(true, "the quote is not in the file"),
            )
            .answer(format!("reproduce:{id}"), verdict(false, "reproduced"));
    }
    let out = selfcheck(&agent, &w.rt, &args(), &mut Vec::new()).await;
    assert_eq!(out["confirmed"], json!([]));
    assert_eq!(
        out["dropped"][0]["why"],
        json!("the quote is not in the file")
    );
    assert_eq!(out["dropped"][0]["alternatives"], json!(["a stale cache"]));
    assert_eq!(out["patch"], Value::Null);
    assert!(!labels(&agent).contains(&"propose-patch".to_owned()));
    let diag = agent
        .calls()
        .into_iter()
        .find(|c| c.label.starts_with("diagnose:"))
        .unwrap();
    assert!(
        diag.prompt.contains("/plugin/workflows/tribunal.js"),
        "{}",
        diag.prompt
    );
}

/// A verifier that does not answer leaves the diagnosis unverified, which
/// drops it: acting on a wrong diagnosis edits the harness.
#[tokio::test]
async fn a_silent_verifier_leaves_the_diagnosis_unverified() {
    let (row, ids) = failing_row("rust");
    let w = world("silent", Some(&row));
    let mut agent = FakeAgent::new();
    for id in &ids {
        agent = agent
            .answer(
                format!("diagnose:{id}"),
                diagnosis(id, json!([{"source": "a", "quote": "b"}])),
            )
            .answer(format!("refute:{id}"), verdict(false, "holds"));
    }
    let out = selfcheck(&agent, &w.rt, &args(), &mut Vec::new()).await;
    assert_eq!(out["confirmed"], json!([]));
    assert_eq!(out["dropped"].as_array().unwrap().len(), 2);
    assert!(out["dropped"][0]["why"]
        .as_str()
        .unwrap()
        .contains("did not return"));
}

/// `--apply` hands the patch step the editing tools and the battery command.
#[tokio::test]
async fn apply_lets_the_patch_step_edit_and_gates_it_on_cargo_test() {
    let (row, ids) = failing_row("rust");
    let w = world("apply", Some(&row));
    let mut agent = FakeAgent::new().answer("propose-patch", FakeAnswer::Text("applied".into()));
    for id in &ids {
        agent = agent
            .answer(
                format!("diagnose:{id}"),
                diagnosis(id, json!([{"source": "a", "quote": "b"}])),
            )
            .answer(format!("refute:{id}"), verdict(false, "holds"))
            .answer(format!("reproduce:{id}"), verdict(false, "holds"));
    }
    let a = SelfcheckArgs {
        apply: true,
        ..args()
    };
    let out = selfcheck(&agent, &w.rt, &a, &mut Vec::new()).await;
    assert_eq!(out["applied"], json!(true));
    let patch = agent
        .calls()
        .into_iter()
        .find(|c| c.label == "propose-patch")
        .unwrap();
    assert!(patch.tools.iter().any(|t| t == "Edit"));
    assert!(patch.prompt.contains("cargo test --workspace --locked"));
}
