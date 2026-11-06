//! The engine's completeness critic drops a gap filed near a raised finding
//! only when the titles also overlap, and logs the drop. A different defect
//! at the same place reaches the panel. With `critic_gap: None` the panel keeps
//! the Workflow's place-only rule, which `diff_panel.rs` compares against.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{json, Value};
use tribunal_pipeline::panel::{
    panel_phase, Ask, CriticGap, HeadJob, PanelEnv, PanelFlags, PanelIo, PanelOutcome,
};
use tribunal_pipeline::synthesize::Relay;

struct Io {
    answers: Mutex<HashMap<String, Option<Value>>>,
}

impl PanelIo for Io {
    async fn ask(&self, label: String, _prompt: String, _kind: Ask) -> Option<Value> {
        self.answers.lock().unwrap().get(&label).cloned().flatten()
    }
    async fn run_heads(&self, _jobs: Vec<HeadJob>) {}
    async fn classify(&self, _round: u64) -> Option<Value> {
        None
    }
    async fn resolve_anchors(&self, _payload: String, _head: String) -> Relay {
        Relay {
            ok: false,
            data: None,
            reason: Some("not under test".to_owned()),
        }
    }
    async fn sweep(&self, _prompt: String) -> Option<String> {
        None
    }
}

fn gap(title: &str, line: u64) -> Value {
    json!({
        "title": title, "file": "src/widgets/retry.ts", "line": line, "severity": "important",
        "description": "d", "evidence": "e", "verifiedBy": [],
    })
}

fn run(critic_findings: Vec<Value>, engine_critic: bool) -> (PanelOutcome, Vec<String>) {
    let mut answers = HashMap::new();
    answers.insert(
        "completeness-critic".to_owned(),
        Some(json!({"summary": "s", "findings": critic_findings})),
    );
    let io = Io {
        answers: Mutex::new(answers),
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let ctx = json!({
        "files": ["src/widgets/retry.ts", "src/widgets/types.ts", "src/widgets/worker.ts"],
        "headRefOid": "feedface", "inThreadDecisions": [],
    });
    let prof = json!({"riskPaths": [], "crossRepoAnchors": []});
    let retest = |_: &Value| false;
    let spent = || 0.0;
    let none: [String; 0] = [];
    let env = PanelEnv {
        pr_number: &json!(77),
        repo: "acme/widgets",
        pr_context: &ctx,
        profile: &prof,
        combined_decisions: "",
        retest: &retest,
        diff_block: "DIFF BLOCK",
        section_index: None,
        diff_path: Some("/tmp/x.patch"),
        critic_gap: engine_critic.then_some(CriticGap {
            not_reached: &none,
            uncovered: &none,
            siblings: &none,
        }),
        blast_radius: None,
        file_count: 3.0,
        critic: true,
        flags: PanelFlags::default(),
        codex_installed: false,
        gemini_installed: false,
        external_cwd: None,
        max_panels: 40.0,
        max_agents: 150.0,
        output_ceiling: 1_200_000.0,
        spent: &spent,
        spent_before: 0.0,
    };
    let raised = json!({
        "title": "Retry job is never enqueued", "file": "src/widgets/retry.ts", "line": 127,
        "severity": "important", "description": "d", "evidence": "e", "lens": "correctness",
        "slotId": 0, "confidence": 0.7,
    });
    let mut logs = Vec::new();
    let out = rt
        .block_on(panel_phase(&io, &env, vec![raised], Vec::new(), &mut logs))
        .unwrap();
    (out, logs)
}

fn repeat_logs(logs: &[String]) -> Vec<&String> {
    logs.iter()
        .filter(|l| l.contains("dropped as a repeat"))
        .collect()
}

#[test]
fn a_different_defect_at_the_same_place_reaches_the_panel() {
    let (out, logs) = run(
        vec![gap(
            "Exhausted rows cannot be replayed from the failure list",
            127,
        )],
        true,
    );
    assert_eq!(out.critic_gaps, 1);
    assert_eq!(out.panel_input.len(), 2);
    assert!(repeat_logs(&logs).is_empty());
}

#[test]
fn a_reworded_repeat_at_the_same_place_is_dropped_and_logged() {
    let (out, logs) = run(vec![gap("The retry job never gets enqueued", 129)], true);
    assert_eq!(out.critic_gaps, 0);
    assert_eq!(out.panel_input.len(), 1);
    assert!(out.dropped_pre_panel.is_empty());
    let dropped = repeat_logs(&logs);
    assert_eq!(dropped.len(), 1);
    assert!(dropped[0].contains("\"Retry job is never enqueued\""));
    assert!(dropped[0].contains("src/widgets/retry.ts:129"));
}

#[test]
fn the_same_title_far_away_is_a_new_gap() {
    let (out, _) = run(vec![gap("The retry job never gets enqueued", 300)], true);
    assert_eq!(out.critic_gaps, 1);
}

#[test]
fn without_the_engine_critic_the_place_alone_drops_it_silently() {
    let (out, logs) = run(
        vec![gap(
            "Exhausted rows cannot be replayed from the failure list",
            127,
        )],
        false,
    );
    assert_eq!(out.critic_gaps, 0);
    assert!(repeat_logs(&logs).is_empty());
}
