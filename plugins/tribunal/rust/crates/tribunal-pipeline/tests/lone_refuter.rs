//! A finding with no external head starts with one Claude vote when the engine
//! sets `PanelFlags::lone_refuter`, and gets a second only when the first is a
//! `REFUTED`, is under-confident or is missing. With the flag off the panel
//! keeps the Workflow's two votes, which `diff_panel.rs` compares against.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{json, Value};
use tribunal_pipeline::panel::{panel_phase, Ask, HeadJob, PanelEnv, PanelFlags, PanelIo};
use tribunal_pipeline::synthesize::Relay;

struct Io {
    answers: Mutex<HashMap<String, Vec<Option<Value>>>>,
    calls: Mutex<Vec<String>>,
}

impl Io {
    fn new(votes: Vec<Option<Value>>) -> Self {
        let mut m = HashMap::new();
        m.insert("refute:F0 defect in handler:claude".to_owned(), votes);
        Io {
            answers: Mutex::new(m),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn refuters(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.starts_with("refute:"))
            .count()
    }
}

impl PanelIo for Io {
    async fn ask(&self, label: String, _prompt: String, _kind: Ask) -> Option<Value> {
        self.calls.lock().unwrap().push(label.clone());
        let mut answers = self.answers.lock().unwrap();
        let queue = answers.get_mut(&label)?;
        if queue.is_empty() {
            None
        } else {
            queue.remove(0)
        }
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

fn vote(verdict: &str, confidence: f64) -> Option<Value> {
    let mut v = json!({
        "verdict": verdict, "confidence": confidence, "source": "claude",
        "slotStatus": "OK", "reason": "read the code",
        "refutationType": if verdict == "REFUTED" { "TECHNICALLY_WRONG" } else { "NO_REFUTATION" },
    });
    if verdict == "REFUTED" {
        v["killReason"] = json!("not-triggered");
        v["contradiction"] = json!({"file": "src/a.ts", "line": 3, "quote": "const x = 1;"});
    }
    Some(v)
}

/// The panel for one important finding, no external head installed.
fn run(votes: Vec<Option<Value>>, lone_refuter: bool) -> (usize, Value) {
    let io = Io::new(votes);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let ctx = json!({"files": ["src/a.ts"], "headRefOid": "feedface", "inThreadDecisions": []});
    let prof = json!({"riskPaths": [], "crossRepoAnchors": []});
    let retest = |_: &Value| false;
    let spent = || 0.0;
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
        critic_gap: None,
        blast_radius: None,
        file_count: 1.0,
        flags: PanelFlags {
            lone_refuter,
            ..PanelFlags::default()
        },
        codex_installed: false,
        gemini_installed: false,
        external_cwd: None,
        max_panels: 40.0,
        max_agents: 150.0,
        output_ceiling: 1_200_000.0,
        spent: &spent,
        spent_before: 0.0,
    };
    let finding = json!({
        "title": "F0 defect in handler", "file": "src/a.ts", "line": 10, "severity": "important",
        "description": "d", "evidence": "e", "lens": "correctness", "slotId": 0, "confidence": 0.7,
    });
    let mut logs = Vec::new();
    let out = rt
        .block_on(panel_phase(&io, &env, vec![finding], Vec::new(), &mut logs))
        .unwrap();
    let verified = out.verified[0].clone().expect("the finding has a result");
    (io.refuters(), verified)
}

#[test]
fn a_confident_not_refuted_vote_stands_alone() {
    let (calls, result) = run(vec![vote("NOT_REFUTED", 0.8)], true);
    assert_eq!(calls, 1);
    assert_eq!(result["escalated"], false);
}

#[test]
fn a_lone_refuted_vote_gets_a_second_one() {
    let votes = || {
        vec![
            vote("REFUTED", 0.9),
            vote("NOT_REFUTED", 0.8),
            vote("NOT_REFUTED", 0.8),
        ]
    };
    let (calls, result) = run(votes(), true);
    let (calls_pair, pair) = run(votes(), false);
    assert_eq!(result["escalated"], true);
    assert_eq!(
        calls, calls_pair,
        "the escalated panel asks what the pair asks"
    );
    assert_eq!(result["refuteFraction"], pair["refuteFraction"]);
    assert_eq!(result["survived"], pair["survived"]);
}

#[test]
fn an_under_confident_vote_gets_a_second_one() {
    let (calls, result) = run(
        vec![vote("NOT_REFUTED", 0.5), vote("NOT_REFUTED", 0.8)],
        true,
    );
    assert_eq!(calls, 2);
    assert_eq!(result["escalated"], true);
}

#[test]
fn a_missing_vote_gets_a_second_one() {
    let (calls, result) = run(vec![None, vote("NOT_REFUTED", 0.8)], true);
    assert_eq!(calls, 2);
    assert_eq!(result["escalated"], true);
}

#[test]
fn without_the_flag_both_votes_are_asked_up_front() {
    let (calls, result) = run(
        vec![vote("NOT_REFUTED", 0.8), vote("NOT_REFUTED", 0.8)],
        false,
    );
    assert_eq!(calls, 2);
    assert_eq!(result["escalated"], false);
}
