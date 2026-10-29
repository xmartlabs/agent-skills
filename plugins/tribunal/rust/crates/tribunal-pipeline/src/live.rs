//! The panel's [`PanelIo`] for a real run: Claude dispatches through the
//! agent with each role's definition, external heads through
//! `tribunal_context::heads`, their artifacts through `classify_artifacts`,
//! and vote quotes through the verifiedBy resolver.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::Semaphore;
use tribunal_agent::{Agent, DispatchRequest, DispatchResult};
use tribunal_context::classify_artifacts::{classify_artifacts, ArtifactScan};
use tribunal_context::exec::Tools;
use tribunal_context::heads::{assemble, run_head, run_sweep, HeadSetup};
use tribunal_context::resolve_verified_anchors;
use tribunal_core::artifact_key::artifact_key;
use tribunal_core::js;
use tribunal_core::profile::conventions_channel::ConventionsChannel;

use crate::agent_def::AgentDef;
use crate::panel::{join_all, Ask, HeadJob, PanelIo, EXTERNAL_BATCH_SIZE};
use crate::panel_prompts::EXTERNAL_SLICE_MARK;
use crate::review::answer_of;
use crate::schemas;
use crate::synthesize::Relay;

/// The agent definitions the panel dispatches.
#[derive(Debug, Clone)]
pub struct PanelAgents {
    pub refuter: AgentDef,
    pub strengthener: AgentDef,
    pub lens: AgentDef,
    pub scribe: AgentDef,
}

pub struct LiveIo<'a, A: Agent> {
    pub agent: &'a A,
    pub agents: PanelAgents,
    pub model: String,
    pub effort: Option<String>,
    pub reviewer_cwd: PathBuf,
    pub operator_cwd: PathBuf,
    pub timeout: Duration,
    pub gate: Arc<Semaphore>,
    pub conventions: &'a ConventionsChannel,
    pub methodology_line: &'a str,
    pub heads: HeadSetup,
    pub repo: String,
    pub cross_repo: Vec<String>,
    pub tools: Tools,
    pub dispatches: Mutex<Vec<(String, DispatchResult)>>,
    pub output_tokens: Mutex<f64>,
}

impl<A: Agent> LiveIo<'_, A> {
    /// Output tokens the panel's dispatches have spent.
    pub fn spent(&self) -> f64 {
        *self.output_tokens.lock().expect("lock")
    }
}

impl<A: Agent> PanelIo for LiveIo<'_, A> {
    async fn ask(&self, label: String, prompt: String, kind: Ask) -> Option<Value> {
        let (def, schema, prompt, cwd) = match kind {
            Ask::Refuter => (
                &self.agents.refuter,
                Some(schemas::refutation()),
                self.conventions.with_conventions(&prompt),
                &self.reviewer_cwd,
            ),
            Ask::RefuterBatch => (
                &self.agents.refuter,
                Some(schemas::batch_refutation()),
                self.conventions.with_conventions(&prompt),
                &self.reviewer_cwd,
            ),
            Ask::Strengthen => (
                &self.agents.strengthener,
                Some(schemas::strengthen()),
                prompt,
                &self.reviewer_cwd,
            ),
            Ask::Defense => (
                &self.agents.lens,
                None,
                self.conventions
                    .reviewer_prompt(self.methodology_line, &prompt),
                &self.reviewer_cwd,
            ),
            Ask::Judge => (
                &self.agents.lens,
                Some(schemas::adjudicator()),
                self.conventions
                    .reviewer_prompt(self.methodology_line, &prompt),
                &self.reviewer_cwd,
            ),
            Ask::Critic => (
                &self.agents.lens,
                Some(schemas::critic()),
                self.conventions
                    .reviewer_prompt(self.methodology_line, &prompt),
                &self.reviewer_cwd,
            ),
            Ask::Tiebreak => (
                &self.agents.scribe,
                Some(schemas::panel_tiebreak()),
                prompt,
                &self.operator_cwd,
            ),
        };
        let mut req = DispatchRequest::new(label.clone(), prompt, cwd.clone());
        req.system_prompt = Some(def.body.clone());
        req.model = Some(self.model.clone());
        req.effort = self.effort.clone();
        req.tools = def.tools.clone();
        req.schema = schema;
        req.timeout = self.timeout;
        let res = {
            let _permit = self
                .gate
                .acquire()
                .await
                .expect("semaphore is never closed");
            self.agent.run(&req).await
        };
        let out: f64 = res
            .model_usage()
            .values()
            .map(|u| u.output_tokens as f64)
            .sum();
        *self.output_tokens.lock().expect("lock") += out;
        let answer = if kind == Ask::Defense {
            res.text().map(|t| json!(t))
        } else {
            answer_of(&res)
        };
        self.dispatches.lock().expect("lock").push((label, res));
        answer
    }

    async fn run_heads(&self, jobs: Vec<HeadJob>) {
        let mut chains: Vec<Vec<HeadJob>> = Vec::new();
        for source in ["codex", "gemini"] {
            let mine: Vec<HeadJob> = jobs
                .iter()
                .filter(|j| j.source == source)
                .cloned()
                .collect();
            chains.extend(mine.chunks(EXTERNAL_BATCH_SIZE).map(<[HeadJob]>::to_vec));
        }
        join_all(chains.into_iter().map(|chain| async move {
            for job in chain {
                let slot = js::get(&job.finding, "slotId").and_then(Value::as_u64);
                let key = artifact_key(slot, job.source, Some(job.round));
                let assembled = assemble(&job.body, job.slice_file.as_deref(), EXTERNAL_SLICE_MARK);
                let high_risk = js::truthy(js::get(&job.finding, "touchesHighRiskSurface"));
                run_head(&self.heads, &key, job.source, &assembled, high_risk).await;
            }
        }))
        .await;
    }

    async fn classify(&self, round: u64) -> Option<Value> {
        let filter = if round == 0 {
            String::new()
        } else {
            format!("r{round}-")
        };
        match classify_artifacts(&self.heads.vote_dir, &filter) {
            ArtifactScan::Skipped => None,
            ArtifactScan::Scanned(a) => Some(json!({
                "total": a.total,
                "entries": a.entries.iter().map(|(k, e)| json!({
                    "key": k,
                    "failclass": e.failclass,
                    "verdict": e.verdict,
                    "confidence": e.confidence,
                    "confidenceDeclared": e.confidence_declared,
                })).collect::<Vec<_>>(),
            })),
        }
    }

    async fn resolve_anchors(&self, payload: String, head: String) -> Relay {
        let mut argv = vec![
            "-".to_owned(),
            head,
            "--repo".to_owned(),
            self.repo.clone(),
            "--relay".to_owned(),
        ];
        argv.extend(self.cross_repo.iter().cloned());
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        match resolve_verified_anchors::run(&argv, &payload, &self.tools).await {
            Ok(data) => Relay {
                ok: true,
                data: Some(data),
                reason: None,
            },
            Err(e) => Relay {
                ok: false,
                data: None,
                reason: Some(e),
            },
        }
    }

    async fn sweep(&self, prompt: String) -> Option<String> {
        Some(run_sweep(&self.heads, &prompt).await)
    }
}
