pub(super) use serde_json::Value;

pub(super) use super::*;
pub(super) use crate::ports::types::EventSeq;
pub(super) use crate::ports::workflow_runner::DeliveryStatus;
pub(super) use crate::store::FsEventLog;

/// The real filesystem journal over a temp home, not a test double: the
/// claim being made is that a run outcome survives to disk and reads back,
/// so the JSONL round trip is the thing under test.
pub(super) fn log() -> (tempfile::TempDir, Arc<dyn EventLog>) {
    let dir = tempfile::Builder::new()
        .prefix("oc-run-outcome-")
        .tempdir()
        .expect("tempdir");
    let events: Arc<dyn EventLog> = Arc::new(FsEventLog::new(dir.path()));
    (dir, events)
}

pub(super) fn run_with(deliveries: Vec<DeliveryReport>, pending: Vec<String>) -> WorkflowRun {
    WorkflowRun {
        output: Value::Null,
        pending_approvals: pending,
        deliveries,
        cancelled: false,
        nodes: Vec::new(),
        notices: Vec::new(),
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    }
}

pub(super) fn report(node: &str, status: DeliveryStatus) -> DeliveryReport {
    DeliveryReport {
        node: node.to_string(),
        kind: "owner".to_string(),
        target: Some("ada@example.com".to_string()),
        status,
        detail: "this recipient has never written to the company".to_string(),
        reason: crate::ports::DeliveryReason::RecipientNotEstablished,
    }
}

pub(super) async fn journaled(
    events: &Arc<dyn EventLog>,
    company: &CompanyId,
) -> Vec<CompanyEvent> {
    events
        .read_from(company, EventSeq::new(0), usize::MAX)
        .await
        .expect("read")
        .into_iter()
        .map(|s| s.event)
        .collect()
}

/// Appends a `WorkflowRunStarted` for `run_id`.
pub(super) async fn start(
    events: &Arc<dyn EventLog>,
    company: &CompanyId,
    run_id: &str,
    scheduled: bool,
) {
    events
        .append(
            company,
            CompanyEvent::WorkflowRunStarted {
                workflow_id: "digest".to_string(),
                run_id: run_id.to_string(),
                scheduled,
                started_by: None,
                resume_semantic: None,
            },
        )
        .await
        .expect("append");
}

// --- issue #529: the durable delivery fold --------------------------------

/// Appends a `WorkflowReportDelivered` — the write-behind record a dispatch
/// leaves at run time.
pub(super) async fn deliver(
    events: &Arc<dyn EventLog>,
    company: &CompanyId,
    workflow_id: &str,
    run_id: &str,
    node: &str,
    kind: &str,
) {
    events
        .append(
            company,
            CompanyEvent::WorkflowReportDelivered {
                workflow_id: workflow_id.to_string(),
                run_id: run_id.to_string(),
                node: node.to_string(),
                kind: kind.to_string(),
                target: Some("ada@example.com".to_string()),
            },
        )
        .await
        .expect("append");
}

/// The nodes the fold returns for `workflow_id`, in order.
pub(super) async fn stranded(
    events: &Arc<dyn EventLog>,
    company: &CompanyId,
    workflow_id: &str,
) -> Vec<String> {
    delivered_by_unsettled_runs(events, company, workflow_id)
        .await
        .into_iter()
        .map(|d| d.node)
        .collect()
}

/// An [`EventLog`] whose `append` always fails — the store went away
/// mid-run. Issue #1009 uses it to prove the swallowed-append path reports
/// its loss instead of only warning.
pub(super) struct FailingAppendLog;

#[async_trait::async_trait]
impl EventLog for FailingAppendLog {
    async fn append(&self, _id: &CompanyId, _event: CompanyEvent) -> crate::Result<EventSeq> {
        Err(crate::error::OpenCompanyError::Store(
            "append failed (test double)".to_string(),
        ))
    }

    async fn read_from(
        &self,
        _id: &CompanyId,
        _seq: EventSeq,
        _limit: usize,
    ) -> crate::Result<Vec<crate::ports::types::StoredEvent>> {
        Ok(Vec::new())
    }

    fn subscribe(
        &self,
        _id: &CompanyId,
    ) -> futures::stream::BoxStream<'static, crate::ports::events::EventStreamItem> {
        Box::pin(futures::stream::empty())
    }
}
