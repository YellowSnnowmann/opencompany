pub(super) use super::tests_core::*;
pub(super) use crate::ports::tasks::TaskTitle;

pub(super) use std::sync::Mutex;

pub(super) use crate::ports::TaskStore;

impl ScriptedTriage {
    pub(super) fn new(verdict: crate::harness::triage::TriageVerdict) -> Self {
        Self {
            verdict,
            asked: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn asked(&self) -> Vec<String> {
        self.asked.lock().expect("asked").clone()
    }
}

#[async_trait]
impl crate::harness::triage::TriageEscalation for ScriptedTriage {
    async fn classify(&self, message: &str) -> crate::harness::triage::TriageVerdict {
        self.asked.lock().expect("asked").push(message.to_string());
        self.verdict
    }
}

// ── Issue #678: a workflow authored in-turn settles its card ────────────

/// Stages what `CreateWorkflowTool` would stage, so these tests exercise the
/// drain rather than the tool.
/// A card standing in for the one the REST chat handler opened (#463), in
/// the column it landed in — To-do for a machine's card, Planning for a
/// person's (issue #576).
/// The journal position of the operator message a seeded handler card was
/// opened for.
///
/// Adoption keys on this alone, so a fixture that seeds a card without it is
/// a card no turn can claim — which is the point: the runner must be told
/// which message it is answering (`.answering(Some(HANDLER_SEQ))`) exactly
/// as the chat drain tells it in production.
pub(super) fn handler_seq() -> EventSeq {
    EventSeq::new(41)
}

pub(super) fn handler_card_in(title: String, column: &str) -> TaskRecord {
    TaskRecord {
        id: "t-handler".to_string(),
        title: TaskTitle::authored(&title),
        note: None,
        column: column.to_string(),
        priority: "medium".to_string(),
        assignee: String::new(),
        updated_at_millis: now_millis(),
        origin: None,
        parent_task_id: None,
        output: None,
        plan: None,
        planning_attempts: Vec::new(),
        deliverable: crate::ports::tasks::TaskDeliverable::Once,
        workflow_proposal: None,
        origin_run_id: None,
        origin_workflow_id: None,
        origin_message_seq: Some(handler_seq()),
        bounced: None,
    }
}

/// A titling pass that answers with one canned name, and records what it
/// was asked to name.
pub(super) struct ScriptedTitler {
    title: &'static str,
    asked: Mutex<Vec<String>>,
}

impl ScriptedTitler {
    pub(super) fn new(title: &'static str) -> Self {
        Self {
            title,
            asked: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn asked(&self) -> Vec<String> {
        self.asked.lock().expect("asked").clone()
    }
}

#[async_trait]
impl crate::ports::tasks::TitleSummariser for ScriptedTitler {
    async fn title(&self, request: &str) -> Option<TaskTitle> {
        self.asked.lock().expect("asked").push(request.to_string());
        TaskTitle::summarised(self.title)
    }
}

pub(super) fn authored(workflow_id: &str) -> TaskOutputWorkflow {
    TaskOutputWorkflow {
        workflow_id: workflow_id.to_string(),
        run_id: None,
        action: TaskOutputAction::Created,
    }
}

// ── HT-077 / HT-078: state, concurrency, and store-failure residuals ────

pub(super) fn card_in(id: &str, column: &str) -> TaskRecord {
    TaskRecord {
        id: id.to_string(),
        title: TaskTitle::authored("Draft the launch plan"),
        note: None,
        column: column.to_string(),
        priority: "medium".to_string(),
        assignee: String::new(),
        updated_at_millis: now_millis(),
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

/// A [`TaskStore`] whose `upsert` always fails, passing `list`/`delete`
/// straight through to a real backing store — so a lookup succeeds and a
/// write does not, driving a delegation through the store-fault arm
/// rather than the "no such card" one.
pub(super) struct FailingUpsertStore {
    pub(super) inner: Arc<dyn TaskStore>,
}

#[async_trait]
impl TaskStore for FailingUpsertStore {
    async fn list(&self, company: &CompanyId) -> Result<Vec<TaskRecord>> {
        self.inner.list(company).await
    }
    async fn upsert(&self, _company: &CompanyId, _task: &TaskRecord) -> Result<()> {
        Err(crate::error::OpenCompanyError::Harness(
            "FailingUpsertStore: forced failure on the write".to_string(),
        ))
    }
    async fn update_if_column(
        &self,
        _company: &CompanyId,
        _task: &TaskRecord,
        _observed: &TaskRecord,
        _expected_column: &str,
    ) -> Result<bool> {
        Err(crate::error::OpenCompanyError::Harness(
            "FailingUpsertStore: forced failure on the write".to_string(),
        ))
    }
    async fn delete(&self, company: &CompanyId, id: &str) -> Result<bool> {
        self.inner.delete(company, id).await
    }
}

/// Delays `list()` until every concurrent caller has also read, so two
/// `run_delegation` calls racing the same card are guaranteed to both
/// load the SAME pre-race snapshot before either writes — the real
/// interleaving a read-then-write cycle with no per-card lock allows,
/// made deterministic instead of left to chance.
pub(super) struct BothReadBeforeEitherWritesStore {
    pub(super) inner: Arc<dyn TaskStore>,
    pub(super) barrier: Arc<tokio::sync::Barrier>,
}

#[async_trait]
impl TaskStore for BothReadBeforeEitherWritesStore {
    async fn list(&self, company: &CompanyId) -> Result<Vec<TaskRecord>> {
        let result = self.inner.list(company).await;
        self.barrier.wait().await;
        result
    }
    async fn upsert(&self, company: &CompanyId, task: &TaskRecord) -> Result<()> {
        self.inner.upsert(company, task).await
    }
    async fn update_if_column(
        &self,
        company: &CompanyId,
        task: &TaskRecord,
        observed: &TaskRecord,
        expected_column: &str,
    ) -> Result<bool> {
        self.inner
            .update_if_column(company, task, observed, expected_column)
            .await
    }
    async fn delete(&self, company: &CompanyId, id: &str) -> Result<bool> {
        self.inner.delete(company, id).await
    }
}

pub(super) struct AssignmentBeforeReviewStore {
    pub(super) inner: Arc<dyn TaskStore>,
    pub(super) both_read: Arc<tokio::sync::Barrier>,
    pub(super) assignment_written: Arc<tokio::sync::Barrier>,
}

#[async_trait]
impl TaskStore for AssignmentBeforeReviewStore {
    async fn list(&self, company: &CompanyId) -> Result<Vec<TaskRecord>> {
        let result = self.inner.list(company).await;
        self.both_read.wait().await;
        result
    }

    async fn upsert(&self, company: &CompanyId, task: &TaskRecord) -> Result<()> {
        self.inner.upsert(company, task).await
    }

    async fn update_if_column(
        &self,
        company: &CompanyId,
        task: &TaskRecord,
        observed: &TaskRecord,
        expected_column: &str,
    ) -> Result<bool> {
        if task.column == expected_column {
            let updated = self
                .inner
                .update_if_column(company, task, observed, expected_column)
                .await;
            self.assignment_written.wait().await;
            updated
        } else {
            self.assignment_written.wait().await;
            self.inner
                .update_if_column(company, task, observed, expected_column)
                .await
        }
    }

    async fn delete(&self, company: &CompanyId, id: &str) -> Result<bool> {
        self.inner.delete(company, id).await
    }
}
