pub(super) use super::*;
pub(super) use crate::ports::tasks::TaskTitle;

/// Wraps a real [`crate::ports::RunStore`] but fails every `list_runs`
/// call, to prove a run-history read failure surfaces distinctly from "no
/// attempts" instead of being silently swallowed into an empty result.
pub(super) struct FailingRunHistory(pub(super) Arc<dyn crate::ports::RunStore>);

#[async_trait]
impl crate::ports::RunStore for FailingRunHistory {
    async fn create_run(
        &self,
        company: &CompanyId,
        spec: crate::ports::runs::NewRun,
    ) -> crate::Result<crate::ports::runs::RunRecord> {
        self.0.create_run(company, spec).await
    }

    async fn get_run(
        &self,
        company: &CompanyId,
        id: &str,
    ) -> crate::Result<Option<crate::ports::runs::RunRecord>> {
        self.0.get_run(company, id).await
    }

    async fn put_run(
        &self,
        company: &CompanyId,
        run: &crate::ports::runs::RunRecord,
    ) -> crate::Result<()> {
        self.0.put_run(company, run).await
    }

    async fn list_runs(
        &self,
        _company: &CompanyId,
        _filter: &RunFilter,
    ) -> crate::Result<Vec<crate::ports::runs::RunRecord>> {
        Err(OpenCompanyError::Store(
            "simulated run-history read failure".into(),
        ))
    }

    async fn append_run_step(
        &self,
        company: &CompanyId,
        step: &crate::ports::runs::RunStepRecord,
    ) -> crate::Result<()> {
        self.0.append_run_step(company, step).await
    }

    async fn list_run_steps(
        &self,
        company: &CompanyId,
        run_id: &str,
    ) -> crate::Result<Vec<crate::ports::runs::RunStepRecord>> {
        self.0.list_run_steps(company, run_id).await
    }
}

/// Wraps a real [`crate::ports::RunStore`] and counts `list_runs` calls,
/// to prove the handed-task briefing's per-card attempt lookup is bounded
/// rather than growing with however many cards an assignee has open.
pub(super) struct CountingRunHistory {
    pub(super) inner: Arc<dyn crate::ports::RunStore>,
    pub(super) list_runs_calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl crate::ports::RunStore for CountingRunHistory {
    async fn create_run(
        &self,
        company: &CompanyId,
        spec: crate::ports::runs::NewRun,
    ) -> crate::Result<crate::ports::runs::RunRecord> {
        self.inner.create_run(company, spec).await
    }

    async fn get_run(
        &self,
        company: &CompanyId,
        id: &str,
    ) -> crate::Result<Option<crate::ports::runs::RunRecord>> {
        self.inner.get_run(company, id).await
    }

    async fn put_run(
        &self,
        company: &CompanyId,
        run: &crate::ports::runs::RunRecord,
    ) -> crate::Result<()> {
        self.inner.put_run(company, run).await
    }

    async fn list_runs(
        &self,
        company: &CompanyId,
        filter: &RunFilter,
    ) -> crate::Result<Vec<crate::ports::runs::RunRecord>> {
        self.list_runs_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner.list_runs(company, filter).await
    }

    async fn append_run_step(
        &self,
        company: &CompanyId,
        step: &crate::ports::runs::RunStepRecord,
    ) -> crate::Result<()> {
        self.inner.append_run_step(company, step).await
    }

    async fn list_run_steps(
        &self,
        company: &CompanyId,
        run_id: &str,
    ) -> crate::Result<Vec<crate::ports::runs::RunStepRecord>> {
        self.inner.list_run_steps(company, run_id).await
    }
}

pub(super) use std::sync::Arc;
pub(super) use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) use crate::company::CompanyManifest;
pub(super) use crate::company::runtime::CompanyMail;
pub(super) use crate::policy::ManifestApprovalGate;
pub(super) use crate::ports::ChannelAdapter;
pub(super) use crate::ports::brain::Brain;
pub(super) use crate::ports::types::{
    ActorKind, ChunkAddr, ChunkHit, ChunkMeta, CompressedTrace, ContextChunk, CycleResult,
    EffectGroup, EvictionPolicy, ReplyTo, TaskResult, TokenUsage,
};
pub(super) use crate::ports::{ContextStore, MemoryStore};
pub(super) use crate::runtime::RuntimeBuilder;
pub(super) use crate::runtime::channel::OperatorChannel;
pub(super) use crate::server::ops::mailer::RecordingMailSender;
pub(super) use crate::store::paths::Bundle;
pub(super) use crate::store::{FsContextStore, FsMemoryStore};

pub(super) fn tmp_home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("opencompany-cycle-")
        .tempdir()
        .expect("tempdir")
}

pub(super) fn manifest(policy_mode: &str) -> CompanyManifest {
    let toml_src = format!(
        r#"
        [company]
        name = "Acme"

        [[agent]]
        id = "ceo"
        role = "Chief"

        [policy]
        mode = "{policy_mode}"
        "#
    );
    toml::from_str(&toml_src).expect("parse manifest")
}

pub(super) fn operator() -> Actor {
    Actor {
        kind: ActorKind::Operator,
        id: "owner".into(),
    }
}

/// A brain that emits one caller-supplied effect on each `OperatorMessage`.
pub(super) struct EffectBrain {
    pub(super) effect: Effect,
}

#[async_trait]
impl Brain for EffectBrain {
    async fn run_cycle(&self, req: CycleRequest, host: &dyn CycleHost) -> Result<CycleResult> {
        let mut responses = Vec::new();
        for event in &req.events {
            if let CompanyEvent::OperatorMessage { text, .. } = event {
                host.emit_effect(self.effect.clone()).await?;
                responses.push(OutboundMessage {
                    message_id: None,
                    task_id: None,
                    outputs: Vec::new(),
                    channel: "operator".into(),
                    agent: None,
                    text: format!("handled: {text}"),
                    steps: Vec::new(),
                    reply_to: None,
                    mentions: Vec::new(),
                });
            }
        }
        Ok(CycleResult {
            channel_responses: responses,
            new_traces: vec![CompressedTrace::now(&req.cycle_id, "effect cycle")],
            ledger_deltas: Vec::new(),
            token_usage: TokenUsage::default(),
        })
    }
}

#[derive(Default)]
pub(super) struct ExplicitExpiryBrain {
    pub(super) decisions: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl Brain for ExplicitExpiryBrain {
    async fn run_cycle(&self, req: CycleRequest, host: &dyn CycleHost) -> Result<CycleResult> {
        for event in &req.events {
            match event {
                CompanyEvent::OperatorMessage { .. } => {
                    host.park_effect(harness_effect(
                        "finance",
                        crate::ports::types::REQUEST_APPROVAL_EFFECT_KIND,
                        serde_json::json!({
                            "title": "Submit the filing",
                            "question": "May I submit it?"
                        }),
                    ))
                    .await?;
                }
                CompanyEvent::ApprovalResolved {
                    verdict: Verdict::Deny,
                    ..
                } => {
                    self.decisions
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                _ => {}
            }
        }
        Ok(CycleResult {
            channel_responses: Vec::new(),
            new_traces: vec![CompressedTrace::now(&req.cycle_id, "explicit expiry")],
            ledger_deltas: Vec::new(),
            token_usage: TokenUsage::default(),
        })
    }
}

/// A brain that counts how many times it was asked to think, and bills for
/// it — the instrument for issue #1725, where the cost of a turn is the
/// thing under test.
#[derive(Default)]
pub(super) struct CountingBrain {
    calls: std::sync::atomic::AtomicUsize,
}

impl CountingBrain {
    pub(super) fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl Brain for CountingBrain {
    async fn run_cycle(&self, req: CycleRequest, _host: &dyn CycleHost) -> Result<CycleResult> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(CycleResult {
            channel_responses: vec![OutboundMessage {
                message_id: None,
                task_id: None,
                outputs: Vec::new(),
                channel: "operator".into(),
                agent: Some("ceo".into()),
                text: "a full turn ran".into(),
                steps: Vec::new(),
                reply_to: None,
                mentions: Vec::new(),
            }],
            new_traces: vec![CompressedTrace::now(&req.cycle_id, "a turn's memory")],
            ledger_deltas: Vec::new(),
            token_usage: TokenUsage {
                input: 4_000,
                output: 500,
                cached_input: 0,
                cost_usd: 0.12,
            },
        })
    }
}

/// A brain that parks one caller-supplied effect per `OperatorMessage`
/// through [`CycleHost::park_effect`] — the shape the harness brain produces
/// when its openhuman policy blocked a tool call inside the turn (#172).
pub(super) struct ParkingBrain {
    pub(super) effect: Effect,
}

#[async_trait]
impl Brain for ParkingBrain {
    async fn run_cycle(&self, req: CycleRequest, host: &dyn CycleHost) -> Result<CycleResult> {
        let mut responses = Vec::new();
        for event in &req.events {
            if let CompanyEvent::OperatorMessage { text, .. } = event {
                host.park_effect(self.effect.clone()).await?;
                responses.push(OutboundMessage {
                    message_id: None,
                    task_id: None,
                    outputs: Vec::new(),
                    channel: "operator".into(),
                    agent: None,
                    text: format!("that needs your approval: {text}"),
                    steps: Vec::new(),
                    reply_to: None,
                    mentions: Vec::new(),
                });
            }
        }
        Ok(CycleResult {
            channel_responses: responses,
            new_traces: vec![CompressedTrace::now(&req.cycle_id, "parking cycle")],
            ledger_deltas: Vec::new(),
            token_usage: TokenUsage::default(),
        })
    }
}

/// A brain that answers on the operator channel and *also* emits a
/// delegated reply addressed by agent id — the shape `run_delegation` and a
/// dispatched card's post-back both produce.
pub(super) struct DelegatingBrain;

#[async_trait]
impl Brain for DelegatingBrain {
    async fn run_cycle(&self, req: CycleRequest, _host: &dyn CycleHost) -> Result<CycleResult> {
        Ok(CycleResult {
            channel_responses: vec![
                OutboundMessage {
                    message_id: None,
                    task_id: None,
                    outputs: Vec::new(),
                    channel: "operator".into(),
                    agent: None,
                    text: "orchestrator".into(),
                    steps: Vec::new(),
                    reply_to: None,
                    mentions: Vec::new(),
                },
                OutboundMessage {
                    message_id: None,
                    task_id: None,
                    outputs: Vec::new(),
                    // Addressed by *agent id*: no adapter answers to this.
                    channel: "maya".into(),
                    agent: None,
                    text: "delegated reply".into(),
                    steps: Vec::new(),
                    reply_to: Some(ReplyTo {
                        chat_id: "strategy".into(),
                    }),
                    mentions: Vec::new(),
                },
            ],
            new_traces: vec![CompressedTrace::now(&req.cycle_id, "delegating cycle")],
            ledger_deltas: Vec::new(),
            token_usage: TokenUsage::default(),
        })
    }
}

/// A brain that fails every cycle — the shape the terminality backstop has
/// to cover, because a `?` on `run_cycle` would otherwise skip every settle
/// and strand the attempt row `Running` until the next boot.
pub(super) struct FailingBrain;

#[async_trait]
impl Brain for FailingBrain {
    async fn run_cycle(&self, _req: CycleRequest, _host: &dyn CycleHost) -> Result<CycleResult> {
        Err(OpenCompanyError::Store("the brain fell over".into()))
    }
}

/// A brain that settles the dispatched run itself, the way `run_task` does
/// on the harness path — so the backstop can be shown to leave a rich settle
/// alone rather than racing it.
pub(super) struct SettlingBrain {
    pub(super) runs: Arc<dyn crate::ports::RunStore>,
    pub(super) status: RunStatus,
}

#[async_trait]
impl Brain for SettlingBrain {
    async fn run_cycle(&self, req: CycleRequest, _host: &dyn CycleHost) -> Result<CycleResult> {
        for event in &req.events {
            if let CompanyEvent::TaskDispatched {
                run_id: Some(run_id),
                ..
            } = event
            {
                let mut outcome = RunOutcome::new(self.status);
                if self.status == RunStatus::Failed {
                    outcome = outcome.with_error("the brain said so");
                }
                self.runs
                    .finish_run(&req.company_id, run_id, outcome)
                    .await?;
            }
        }
        Ok(CycleResult {
            channel_responses: vec![OutboundMessage {
                message_id: None,
                task_id: None,
                outputs: Vec::new(),
                channel: "operator".into(),
                agent: None,
                text: "settled".into(),
                steps: Vec::new(),
                reply_to: None,
                mentions: Vec::new(),
            }],
            new_traces: vec![CompressedTrace::now(&req.cycle_id, "settling cycle")],
            ledger_deltas: Vec::new(),
            token_usage: TokenUsage::default(),
        })
    }
}

/// Mints a `Pending` run for `task`, so a test can drive a dispatch cycle
/// the way `CompanyRuntime::dispatch_task` does.
pub(super) async fn pending_run(
    rt: &crate::company::runtime::CompanyRuntime,
    task: &str,
) -> String {
    rt.runs()
        .create_run(
            rt.id(),
            crate::ports::runs::NewRun::for_task(crate::ports::generate_id(), task, "ceo"),
        )
        .await
        .expect("mint a run")
        .id
}

/// A [`MemoryStore`] that counts the calls a cycle makes, delegating the
/// work to a real fs store so the runtime behaves normally around it.
pub(super) struct CountingMemory {
    inner: FsMemoryStore,
    pub(super) reads: AtomicUsize,
    pub(super) writes: AtomicUsize,
}

impl CountingMemory {
    pub(super) fn new(inner: FsMemoryStore) -> Self {
        Self {
            inner,
            reads: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl MemoryStore for CountingMemory {
    async fn save_trace(&self, id: &CompanyId, trace: CompressedTrace) -> Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.save_trace(id, trace).await
    }

    async fn recent_traces(&self, id: &CompanyId, limit: usize) -> Result<Vec<CompressedTrace>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.recent_traces(id, limit).await
    }

    async fn save_task_result(&self, id: &CompanyId, result: TaskResult) -> Result<()> {
        self.inner.save_task_result(id, result).await
    }

    async fn evict(&self, id: &CompanyId, policy: EvictionPolicy) -> Result<u64> {
        self.inner.evict(id, policy).await
    }
}

/// The [`ContextStore`] half of the same instrument.
pub(super) struct CountingContext {
    inner: FsContextStore,
    pub(super) lists: AtomicUsize,
}

impl CountingContext {
    pub(super) fn new(inner: FsContextStore) -> Self {
        Self {
            inner,
            lists: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl ContextStore for CountingContext {
    async fn put(&self, id: &CompanyId, chunk: ContextChunk) -> Result<ChunkAddr> {
        self.inner.put(id, chunk).await
    }

    async fn list(&self, id: &CompanyId, prefix: &str) -> Result<Vec<ChunkMeta>> {
        self.lists.fetch_add(1, Ordering::SeqCst);
        self.inner.list(id, prefix).await
    }

    async fn peek(
        &self,
        id: &CompanyId,
        addr: &ChunkAddr,
        range: Option<std::ops::Range<usize>>,
    ) -> Result<String> {
        self.inner.peek(id, addr, range).await
    }

    async fn search(&self, id: &CompanyId, query: &str, limit: usize) -> Result<Vec<ChunkHit>> {
        self.inner.search(id, query, limit).await
    }

    async fn delete(&self, id: &CompanyId, addr: &ChunkAddr) -> Result<bool> {
        self.inner.delete(id, addr).await
    }

    async fn delete_label(&self, id: &CompanyId, addr: &ChunkAddr, label: &str) -> Result<bool> {
        self.inner.delete_label(id, addr, label).await
    }
}

// --- Single-use grants on approve (issue #243) ---------------------------

/// A harness-projected effect, i.e. one carrying `agent`. Its payload is a
/// tool's argument object, not something the runtime can perform.
pub(super) fn harness_effect(agent: &str, tool: &str, args: serde_json::Value) -> Effect {
    Effect {
        kind: tool.into(),
        group: EffectGroup::Sign,
        // A real spend amount, deliberately: it is what proves the effect
        // was NOT executed. `perform_effect` ledgers any `amount_usd`, so an
        // empty ledger is positive evidence that the native path was skipped
        // rather than merely evidence that nothing observable happened.
        amount_usd: Some(42.0),
        established_thread: false,
        first_time_counterparty: false,
        payload: args,
        agent: Some(agent.to_string()),
        run_id: None,
    }
}

/// Parks `effect` through a real cycle and returns the runtime + approval id.
/// Returns the runtime behind an `Arc`, as the server's registry holds it:
/// resolving an approval spawns its follow-up cycle onto a clone of that
/// handle, so the cycle outlives the request that asked for it (issue #383).
pub(super) async fn park_one(
    home: std::path::PathBuf,
    effect: Effect,
) -> (Arc<CompanyRuntime>, ApprovalId) {
    let rt = Arc::new(
        RuntimeBuilder::new(home, manifest("supervised"))
            .with_brain(Arc::new(ParkingBrain { effect }))
            .build()
            .await
            .unwrap(),
    );
    let report = rt
        .run_cycle(vec![CompanyEvent::OperatorMessage {
            mentions: Vec::new(),
            parent: None,
            text: "do it".into(),
            by: None,
            chat: None,
            deliverable: None,
            attachments: Vec::new(),
        }])
        .await
        .unwrap();
    assert_eq!(report.parked.len(), 1);
    let id = report.parked[0].clone();
    (rt, id)
}

/// Parks one tool call, then fails every follow-up turn.
pub(super) struct FailingContinuationBrain {
    pub(super) effect: Effect,
}

#[async_trait]
impl Brain for FailingContinuationBrain {
    async fn run_cycle(&self, req: CycleRequest, host: &dyn CycleHost) -> Result<CycleResult> {
        for event in &req.events {
            match event {
                CompanyEvent::OperatorMessage { .. } => {
                    host.park_effect(self.effect.clone()).await?;
                }
                CompanyEvent::ApprovalResolved { .. } => {
                    return Err(OpenCompanyError::Unimplemented("the follow-up turn failed"));
                }
                _ => {}
            }
        }
        Ok(CycleResult {
            channel_responses: Vec::new(),
            new_traces: vec![CompressedTrace::now(&req.cycle_id, "failing continuation")],
            ledger_deltas: Vec::new(),
            token_usage: TokenUsage::default(),
        })
    }
}
