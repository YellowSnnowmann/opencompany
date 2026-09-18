pub(super) use super::tests_core::*;
pub(super) use crate::ports::tasks::TaskTitle;
pub(super) use std::sync::Arc;
pub(super) use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) use crate::company::CompanyManifest;
pub(super) use crate::policy::ManifestApprovalGate;
pub(super) use crate::ports::brain::Brain;
pub(super) use crate::ports::types::SecretValue;
pub(super) use crate::ports::types::{
    CompressedTrace, CycleResult, EffectGroup, EventSeq, TokenUsage,
};
pub(super) use crate::runtime::RuntimeBuilder;
pub(super) use crate::server::ops::smtp::{SmtpCredentials, SmtpSecurity};

/// Parks one harness tool call behind a **zero-TTL** gate, so it is past its
/// deadline the instant it lands — the state an operator meets when they get
/// to the queue late (issue #1449).
pub(super) async fn park_one_past_its_deadline(
    home: std::path::PathBuf,
) -> (Arc<CompanyRuntime>, ApprovalId) {
    let gate = Arc::new(
        ManifestApprovalGate::new(manifest("supervised").policy.clone()).with_ttl_millis(0),
    );
    let rt = Arc::new(
        RuntimeBuilder::new(home, manifest("supervised"))
            .with_approvals(gate)
            .with_brain(Arc::new(EffectBrain {
                effect: harness_effect(
                    "finance",
                    "composio_execute",
                    serde_json::json!({ "to": "a@b.test" }),
                ),
            }))
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

// ── Issue #174: the generic cycle seam meters inference usage ────────────

/// A brain that reports a fixed [`TokenUsage`] for every cycle — the shape
/// hosted Medulla cognition produces once its `orch:usage` frames land.
pub(super) struct MeteredBrain {
    pub(super) usage: TokenUsage,
    pub(super) metering: UsageMetering,
}

impl MeteredBrain {
    pub(super) fn per_cycle(usage: TokenUsage) -> Self {
        Self {
            usage,
            metering: UsageMetering::PerCycle,
        }
    }
}

#[async_trait]
impl Brain for MeteredBrain {
    async fn run_cycle(&self, req: CycleRequest, _host: &dyn CycleHost) -> Result<CycleResult> {
        Ok(CycleResult {
            channel_responses: vec![OutboundMessage {
                message_id: None,
                task_id: None,
                outputs: Vec::new(),
                channel: "operator".into(),
                agent: None,
                text: "thought about it".into(),
                steps: Vec::new(),
                reply_to: None,
                mentions: Vec::new(),
            }],
            new_traces: vec![CompressedTrace::now(&req.cycle_id, "metered cycle")],
            ledger_deltas: Vec::new(),
            token_usage: self.usage,
        })
    }

    fn cognition(&self) -> crate::ports::Cognition {
        crate::ports::Cognition {
            path: "test",
            provider: "medulla",
            model: None,
            metering: self.metering,
        }
    }
}

pub(super) fn reported_usage(cost_usd: f64) -> TokenUsage {
    TokenUsage {
        input: 1_200,
        output: 340,
        cached_input: 200,
        cost_usd,
    }
}

/// A brain that tracks the peak number of concurrently-active cycles.
pub(super) struct ConcurrencyBrain {
    pub(super) active: Arc<AtomicUsize>,
    pub(super) peak: Arc<AtomicUsize>,
}

#[async_trait]
impl Brain for ConcurrencyBrain {
    async fn run_cycle(&self, req: CycleRequest, _host: &dyn CycleHost) -> Result<CycleResult> {
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(CycleResult {
            channel_responses: Vec::new(),
            new_traces: vec![CompressedTrace::now(&req.cycle_id, "concurrency")],
            ledger_deltas: Vec::new(),
            token_usage: TokenUsage::default(),
        })
    }
}

pub(super) fn test_smtp(from_email: &str) -> SmtpCredentials {
    SmtpCredentials {
        host: "smtp.example.com".into(),
        port: 587,
        security: SmtpSecurity::Starttls,
        username: "user".into(),
        password: SecretValue("hunter2".into()),
        from_name: "Acme".into(),
        from_email: from_email.into(),
    }
}

/// One workspace write (issue #327), for the neutrality assertions in both
/// `cycle_*_id` tests. Shared because it is the same claim made twice: an
/// event neutral for the card but not for the thread would mis-stamp every
/// cycle that answered a message and touched the tree.
pub(super) fn workspace_changed() -> CompanyEvent {
    CompanyEvent::WorkspaceChanged {
        node_id: "n-1".into(),
        change: "updated".into(),
    }
}

/// One per-node start bracket (issue #382), for the neutrality assertions in
/// both `cycle_*_id` tests. Like `workspace_changed`, it is a record of a
/// workflow walking its graph — it names no card and no thread, so alone it
/// stamps neither, and beside a trigger it must not disqualify the batch.
pub(super) fn workflow_node_started() -> CompanyEvent {
    CompanyEvent::WorkflowNodeStarted {
        workflow_id: "digest".into(),
        run_id: "run-1".into(),
        node_id: "n-1".into(),
    }
}

// -----------------------------------------------------------------------
// Issue #176: delegation host arms + handed-task awareness.
// -----------------------------------------------------------------------

/// A manifest with an Engineering desk (`eng`, lead `eng1`) — for the desk
/// resolution paths of `delegate_to_desk` and the awareness matcher.
pub(super) fn desk_manifest() -> CompanyManifest {
    let toml_src = r#"
        [company]
        name = "Acme"

        [[agent]]
        id = "chief"
        role = "Chief"
        tier = "orchestrator"

        [[agent]]
        id = "eng1"
        role = "Engineer"

        [[group_chat]]
        id = "eng"
        name = "Engineering"
        members = ["eng1"]

        [policy]
        mode = "full"
        "#;
    toml::from_str(toml_src).expect("parse desk manifest")
}

/// A brain that records the text of every operator message it is handed, so
/// a test can assert what awareness the kernel folded in before the brain.
pub(super) struct CapturingBrain {
    pub(super) seen: Arc<StdMutex<Vec<String>>>,
}

#[async_trait]
impl Brain for CapturingBrain {
    async fn run_cycle(&self, req: CycleRequest, _host: &dyn CycleHost) -> Result<CycleResult> {
        for event in &req.events {
            if let CompanyEvent::OperatorMessage { text, .. } = event {
                self.seen.lock().expect("seen").push(text.clone());
            }
        }
        Ok(CycleResult {
            channel_responses: Vec::new(),
            new_traces: vec![CompressedTrace::now(&req.cycle_id, "capture")],
            ledger_deltas: Vec::new(),
            token_usage: TokenUsage::default(),
        })
    }
}

// -----------------------------------------------------------------------
// Standing grants (issue #374)
// -----------------------------------------------------------------------

/// A harness tool call the operator IS allowed to grant broadly.
///
/// `harness_effect` deliberately uses `Sign` and a real amount, because it
/// exists to prove the effect was not executed. Both would refuse a broad
/// scope, so the grantable case needs its own fixture.
///
/// **The tool passed in is now load-bearing** (issue #444). These tests
/// used to grant a standing scope on `workspace_write` — which was
/// grantable only because its name contains no consequence word, while the
/// parking side of the same gate refused to exempt it precisely because it
/// overwrites guidance the operator wrote. That contradiction is what #444
/// is about, and it is resolved in the direction the parking side already
/// argued: `workspace_write` stays a per-call decision. `file_write` is the
/// honest fixture — it mutates, so it still parks, but what it mutates is
/// the agent's own sandboxed workspace, which is exactly the low-consequence
/// shape a standing grant is for.
pub(super) fn grantable_effect(agent: &str, tool: &str, args: serde_json::Value) -> Effect {
    Effect {
        kind: tool.into(),
        group: EffectGroup::Other,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: args,
        agent: Some(agent.to_string()),
        run_id: None,
    }
}

pub(super) fn in_an_hour() -> u64 {
    now_millis() + 60 * 60 * 1000
}

pub(super) fn tool_scope() -> GrantScope {
    GrantScope::Tool {
        expires_at_millis: in_an_hour(),
    }
}

/// Parks `effect` the way a blocked harness tool call actually parks —
/// through `park_effect`, which bypasses the manifest gate's `evaluate`.
///
/// `park_one` cannot serve here: it routes through `emit_effect`, and the
/// manifest gate auto-allows `EffectGroup::Other` under supervised. That is
/// correct for a native effect and irrelevant to a harness one, whose park
/// decision was already made inside the agent's turn by `ApprovalPolicy`.
pub(super) async fn park_one_blocked_tool_call(
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

/// Like [`park_one_blocked_tool_call`], but parks the same effect **twice**
/// — two cycles, two identical cards on one runtime.
///
/// The ordering matters and is why this exists: the deny/grant reconcile
/// tests need both cards parked before either is resolved, because once a
/// standing deny is live the identical call is denied inline and never
/// parks again.
/// [`park_two_blocked_tool_calls`], with a distinct effect per cycle.
///
/// The original parks one effect twice, which is right for the cases that
/// only need two approval ids. A case about two *agents* needs the two
/// parks to differ, or it races one subject against itself.
pub(super) async fn park_two_blocked_tool_calls_for(
    home: std::path::PathBuf,
    effects: [Effect; 2],
) -> (Arc<CompanyRuntime>, Vec<ApprovalId>) {
    struct PerCycleParkingBrain {
        queued: std::sync::Mutex<Vec<Effect>>,
    }

    #[async_trait]
    impl Brain for PerCycleParkingBrain {
        async fn run_cycle(&self, req: CycleRequest, host: &dyn CycleHost) -> Result<CycleResult> {
            for event in &req.events {
                if let CompanyEvent::OperatorMessage { .. } = event {
                    let effect = {
                        let mut queued = self.queued.lock().expect("parking queue");
                        if queued.len() > 1 {
                            queued.remove(0)
                        } else {
                            queued[0].clone()
                        }
                    };
                    host.park_effect(effect).await?;
                }
            }
            Ok(CycleResult {
                channel_responses: Vec::new(),
                new_traces: vec![CompressedTrace::now(&req.cycle_id, "parking cycle")],
                ledger_deltas: Vec::new(),
                token_usage: TokenUsage::default(),
            })
        }
    }

    let rt = Arc::new(
        RuntimeBuilder::new(home, manifest("supervised"))
            .with_brain(Arc::new(PerCycleParkingBrain {
                queued: std::sync::Mutex::new(effects.into_iter().collect()),
            }))
            .build()
            .await
            .unwrap(),
    );
    let mut ids = Vec::new();
    for text in ["do it", "again"] {
        let report = rt
            .run_cycle(vec![CompanyEvent::OperatorMessage {
                mentions: Vec::new(),
                parent: None,
                text: text.into(),
                by: None,
                chat: None,
                deliverable: None,
                attachments: Vec::new(),
            }])
            .await
            .unwrap();
        assert_eq!(report.parked.len(), 1);
        ids.push(report.parked[0].clone());
    }
    (rt, ids)
}

pub(super) async fn park_two_blocked_tool_calls(
    home: std::path::PathBuf,
    effect: Effect,
) -> (Arc<CompanyRuntime>, Vec<ApprovalId>) {
    let rt = Arc::new(
        RuntimeBuilder::new(home, manifest("supervised"))
            .with_brain(Arc::new(ParkingBrain { effect }))
            .build()
            .await
            .unwrap(),
    );
    let mut ids = Vec::new();
    for text in ["do it", "again"] {
        let report = rt
            .run_cycle(vec![CompanyEvent::OperatorMessage {
                mentions: Vec::new(),
                parent: None,
                text: text.into(),
                by: None,
                chat: None,
                deliverable: None,
                attachments: Vec::new(),
            }])
            .await
            .unwrap();
        assert_eq!(report.parked.len(), 1);
        ids.push(report.parked[0].clone());
    }
    (rt, ids)
}

/// A [`JournalStore`](crate::ports::journal::JournalStore) that fails the
/// Nth `StandingGrantMinted` append it sees and passes every other line
/// straight through to an in-memory backend.
pub(super) struct FailNthStandingMintStore {
    inner: crate::ports::journal::MemoryJournalStore,
    seen: std::sync::atomic::AtomicUsize,
    fail_at: usize,
}

impl FailNthStandingMintStore {
    pub(super) fn new(fail_at: usize) -> Self {
        Self {
            inner: crate::ports::journal::MemoryJournalStore::default(),
            seen: std::sync::atomic::AtomicUsize::new(0),
            fail_at,
        }
    }
}

#[async_trait::async_trait]
impl crate::ports::journal::JournalStore for FailNthStandingMintStore {
    async fn append_journal(
        &self,
        id: &CompanyId,
        line: &str,
        durability: crate::ports::journal::Durability,
    ) -> Result<()> {
        if line.contains("StandingGrantMinted") {
            let n = self.seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if n == self.fail_at {
                return Err(crate::error::OpenCompanyError::Store(
                    "FailNthStandingMintStore: forced failure on the mint".to_string(),
                ));
            }
        }
        self.inner.append_journal(id, line, durability).await
    }

    async fn read_journal(&self, id: &CompanyId) -> Result<Vec<String>> {
        self.inner.read_journal(id).await
    }

    async fn journal_imported(&self, id: &CompanyId) -> Result<bool> {
        self.inner.journal_imported(id).await
    }

    async fn complete_import(&self, id: &CompanyId, lines: Vec<String>) -> Result<()> {
        self.inner.complete_import(id, lines).await
    }
}

/// A [`JournalStore`](crate::ports::journal::JournalStore) that fails
/// every `StandingGrantRevoked` append, passing every other line straight
/// through to an in-memory backend.
pub(super) struct FailStandingRevokeStore {
    inner: crate::ports::journal::MemoryJournalStore,
}

impl FailStandingRevokeStore {
    pub(super) fn new() -> Self {
        Self {
            inner: crate::ports::journal::MemoryJournalStore::default(),
        }
    }
}

#[async_trait::async_trait]
impl crate::ports::journal::JournalStore for FailStandingRevokeStore {
    async fn append_journal(
        &self,
        id: &CompanyId,
        line: &str,
        durability: crate::ports::journal::Durability,
    ) -> Result<()> {
        if line.contains("StandingGrantRevoked") {
            return Err(crate::error::OpenCompanyError::Store(
                "FailStandingRevokeStore: forced failure on the revoke".to_string(),
            ));
        }
        self.inner.append_journal(id, line, durability).await
    }

    async fn read_journal(&self, id: &CompanyId) -> Result<Vec<String>> {
        self.inner.read_journal(id).await
    }

    async fn journal_imported(&self, id: &CompanyId) -> Result<bool> {
        self.inner.journal_imported(id).await
    }

    async fn complete_import(&self, id: &CompanyId, lines: Vec<String>) -> Result<()> {
        self.inner.complete_import(id, lines).await
    }
}

/// A [`JournalStore`](crate::ports::journal::JournalStore) that fails
/// every `ApprovalGranted` append, passing every other line straight
/// through to an in-memory backend.
pub(super) struct FailGrantedMintStore {
    inner: crate::ports::journal::MemoryJournalStore,
}

impl FailGrantedMintStore {
    pub(super) fn new() -> Self {
        Self {
            inner: crate::ports::journal::MemoryJournalStore::default(),
        }
    }
}

#[async_trait::async_trait]
impl crate::ports::journal::JournalStore for FailGrantedMintStore {
    async fn append_journal(
        &self,
        id: &CompanyId,
        line: &str,
        durability: crate::ports::journal::Durability,
    ) -> Result<()> {
        if line.contains("ApprovalGranted") {
            return Err(crate::error::OpenCompanyError::Store(
                "FailGrantedMintStore: forced failure on the single-use grant mint".to_string(),
            ));
        }
        self.inner.append_journal(id, line, durability).await
    }

    async fn read_journal(&self, id: &CompanyId) -> Result<Vec<String>> {
        self.inner.read_journal(id).await
    }

    async fn journal_imported(&self, id: &CompanyId) -> Result<bool> {
        self.inner.journal_imported(id).await
    }

    async fn complete_import(&self, id: &CompanyId, lines: Vec<String>) -> Result<()> {
        self.inner.complete_import(id, lines).await
    }
}

/* ---- issue #1890 C: settle markers reach the model ---- */

/// The settled predicate, column by column.
///
/// Named cases rather than a loop, because the interesting arm is `todo` —
/// it is both the failure landing and the fresh state, and the whole point
/// of the sub-issue is that those two must not read the same.
#[test]
fn a_card_has_settled_only_once_its_run_stopped() {
    let card = |column: &str, bounced: Option<&str>| TaskRecord {
        id: "t-1".to_string(),
        title: TaskTitle::authored("Ship the thing"),
        note: None,
        column: column.to_string(),
        priority: "medium".to_string(),
        assignee: "engineer".to_string(),
        updated_at_millis: 0,
        origin: None,
        parent_task_id: None,
        output: None,
        plan: None,
        planning_attempts: Vec::new(),
        deliverable: crate::ports::tasks::TaskDeliverable::Once,
        workflow_proposal: None,
        origin_run_id: None,
        origin_workflow_id: None,
        origin_message_seq: None,
        bounced: bounced.map(str::to_string),
    };
    // Stopped, whether or not it succeeded — the misleading case this
    // briefing exists for is the run that stopped without finishing.
    assert!(has_settled(&card(
        crate::ports::tasks::COLUMN_IN_REVIEW,
        None
    )));
    assert!(has_settled(&card(crate::ports::tasks::COLUMN_DONE, None)));
    assert!(has_settled(&card(crate::ports::tasks::COLUMN_PAUSED, None)));
    // Still running. Calling either of these finished is exactly the
    // "concluded the work had finished when it had in fact parked"
    // misreading #377 set out to remove.
    assert!(!has_settled(&card(
        crate::ports::tasks::COLUMN_IN_PROGRESS,
        None
    )));
    assert!(!has_settled(&card(
        crate::ports::tasks::COLUMN_PLANNING,
        None
    )));
    // The hard arm. A bounced card has run and stopped; a fresh one has
    // not, and they share a column — which is the gap #1865's `bounced`
    // exists to close, asked here rather than re-decided.
    assert!(has_settled(&card(
        COLUMN_TODO,
        Some("the dispatch failed: provider timeout")
    )));
    assert!(
        !has_settled(&card(COLUMN_TODO, None)),
        "a card nobody has touched must not read as finished work"
    );
}

/// A card that has settled, ready for a test to point at a conversation.
pub(super) fn settled_card(id: &str, title: &str) -> TaskRecord {
    TaskRecord {
        id: id.to_string(),
        title: TaskTitle::authored(title),
        note: None,
        column: crate::ports::tasks::COLUMN_IN_REVIEW.to_string(),
        priority: "medium".to_string(),
        // Empty on purpose: the settled briefing matches on the
        // conversation that raised the card, never on who ran it, so an
        // unassigned card must still brief — and this also keeps these
        // fixtures out of the OPEN_WORK briefing, whose filter requires a
        // non-empty assignee.
        assignee: String::new(),
        updated_at_millis: 0,
        origin: None,
        parent_task_id: None,
        output: None,
        plan: None,
        planning_attempts: Vec::new(),
        deliverable: crate::ports::tasks::TaskDeliverable::Once,
        workflow_proposal: None,
        origin_run_id: None,
        origin_workflow_id: None,
        origin_message_seq: None,
        bounced: None,
    }
}

pub(super) fn operator_in_thread(chat: &str, parent: Option<u64>, text: &str) -> CompanyEvent {
    CompanyEvent::OperatorMessage {
        text: text.to_string(),
        by: None,
        chat: Some(chat.to_string()),
        parent: parent.map(EventSeq::new),
        deliverable: None,
        mentions: Vec::new(),
        attachments: Vec::new(),
    }
}

pub(super) fn message_text(event: &CompanyEvent) -> &str {
    match event {
        CompanyEvent::OperatorMessage { text, .. } => text.as_str(),
        other => panic!("expected an operator message, got {other:?}"),
    }
}
