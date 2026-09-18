//! Run-history and journal fixtures for the hosted-mode `workflows` ops
//! tests — split out of `workflows_test_support_hosted.rs` to keep each
//! source file under the 750-line cap. Re-exported by its parent, so
//! `hosted_mode::*` still reaches everything.

use super::*;

/// Journals a `WorkflowRunFinished` naming a `run_id`, the shape
/// `journaled_run_failure` scans for — distinct from `journal_run` above,
/// which always journals `run_id: None` for the delivery-history tests.
#[cfg(feature = "openhuman")]
pub(crate) async fn journal_run_with_id(
    state: &AppState,
    id: &CompanyId,
    workflow_id: &str,
    run_id: &str,
    error: &str,
) {
    let runtime = state.registry().get(id).expect("registered");
    runtime
        .events()
        .append(
            id,
            CompanyEvent::WorkflowRunFinished {
                workflow_id: workflow_id.to_string(),
                scheduled: false,
                run_id: Some(run_id.to_string()),
                deliveries: Vec::new(),
                pending_approvals: Vec::new(),
                error: Some(error.to_string()),
                cancelled: false,
                notices: Vec::new(),
                board: Vec::new(),
                // Added by #881/#880 after this fixture was written. A
                // failed run parks nothing and blocks nothing, so both
                // are empty here — see `Settled::from`'s Err arm, which
                // makes the same choice for the same reason.
                blocked_nodes: Vec::new(),
                approvals: Vec::new(),
            },
        )
        .await
        .expect("append");
}

/// A create body whose trigger carries a cron.
pub(crate) fn scheduled_create_body() -> serde_json::Value {
    serde_json::json!({
        "id": "digest",
        "name": "Digest",
        "nodes": [
            { "id": "start", "kind": "trigger", "name": "Start", "schedule": "0 9 * * *" },
            { "id": "done", "kind": "output", "name": "Report" }
        ],
        "edges": [ { "from": "start", "to": "done", "label": "ok" } ]
    })
}

// ── Issue #228: the run-history read ────────────────────────────────

/// Journals a finished-run outcome directly on the company's event log,
/// the way both entry points do via `record_run_finished`.
pub(crate) async fn journal_run(
    state: &AppState,
    id: &CompanyId,
    workflow_id: &str,
    scheduled: bool,
    deliveries: Vec<crate::ports::DeliveryReport>,
    error: Option<&str>,
) {
    let runtime = state.registry().get(id).expect("registered");
    runtime
        .events()
        .append(
            id,
            CompanyEvent::WorkflowRunFinished {
                workflow_id: workflow_id.to_string(),
                scheduled,
                run_id: None,
                deliveries,
                pending_approvals: Vec::new(),
                error: error.map(str::to_string),
                cancelled: false,
                notices: Vec::new(),
                board: Vec::new(),
                blocked_nodes: Vec::new(),
                approvals: Vec::new(),
            },
        )
        .await
        .expect("append");
}

/// A report that reached its destination.
pub(crate) fn sent_row(node: &str) -> crate::ports::DeliveryReport {
    crate::ports::DeliveryReport {
        node: node.to_string(),
        kind: "owner".to_string(),
        target: Some("ada@example.com".to_string()),
        status: crate::ports::DeliveryStatus::Sent,
        detail: "emailed the company's admin".to_string(),
        reason: crate::ports::DeliveryReason::OwnerEmailed,
    }
}

pub(crate) fn undelivered_row(node: &str) -> crate::ports::DeliveryReport {
    crate::ports::DeliveryReport {
        node: node.to_string(),
        kind: "email".to_string(),
        target: Some("ada@example.com".to_string()),
        status: crate::ports::DeliveryStatus::Skipped,
        detail: "this recipient has never written to the company".to_string(),
        reason: crate::ports::DeliveryReason::RecipientNotEstablished,
    }
}

// ------------------------------------------------------------------
// `GET …/workflows/runs/{rid}/artifacts` — the files one run produced,
// joined through `origin_run_id` (issue #1684).
// ------------------------------------------------------------------

/// Builds a board card, optionally stamped with the run that opened it
/// (`origin_run_id`) — the field [`run_artifacts`] joins on (issue
/// #1684). Everything else is the neutral shape the board's own tests
/// use.
pub(crate) fn run_card(
    id: &str,
    title: &str,
    origin_run_id: Option<&str>,
) -> crate::ports::TaskRecord {
    crate::ports::TaskRecord {
        id: id.into(),
        title: crate::ports::tasks::TaskTitle::authored(title),
        note: None,
        column: "in_review".into(),
        priority: "medium".into(),
        assignee: "ceo".into(),
        updated_at_millis: 1,
        origin: None,
        parent_task_id: None,
        output: None,
        plan: None,
        planning_attempts: Vec::new(),
        deliverable: crate::ports::tasks::TaskDeliverable::Once,
        workflow_proposal: None,
        origin_run_id: origin_run_id.map(str::to_string),
        origin_workflow_id: None,
        origin_message_seq: None,
        bounced: None,
    }
}

/// A published artifact on `task_id`: one agent version, a `source`
/// path, stamped `updated_at_millis` so the newest-first sort is
/// testable.
pub(crate) fn published(
    id: &str,
    task_id: &str,
    title: &str,
    source: &str,
    at_millis: u64,
) -> crate::ports::ArtifactRecord {
    let mut rec = crate::ports::ArtifactRecord::new(
        id,
        task_id,
        title,
        crate::ports::ArtifactKind::Markdown,
        "the agent's draft",
        "ceo",
        at_millis,
    )
    .with_source(source);
    rec.updated_at_millis = at_millis;
    rec
}

// ── Issue #371: the per-node progress fold ─────────────────────────

/// Journals a `WorkflowRunStarted`, the way the runner does before the
/// engine call.
pub(crate) async fn journal_start(
    state: &AppState,
    id: &CompanyId,
    workflow_id: &str,
    run_id: &str,
    scheduled: bool,
) {
    let runtime = state.registry().get(id).expect("registered");
    runtime
        .events()
        .append(
            id,
            CompanyEvent::WorkflowRunStarted {
                workflow_id: workflow_id.to_string(),
                run_id: run_id.to_string(),
                scheduled,
                started_by: None,
                resume_semantic: None,
            },
        )
        .await
        .expect("append");
}

/// Journals one `WorkflowNodeFinished`, the way the run observer does.
pub(crate) async fn journal_node(
    state: &AppState,
    id: &CompanyId,
    workflow_id: &str,
    run_id: &str,
    node_id: &str,
    status: WorkflowNodeStatus,
) {
    let runtime = state.registry().get(id).expect("registered");
    runtime
        .events()
        .append(
            id,
            CompanyEvent::WorkflowNodeFinished {
                workflow_id: workflow_id.to_string(),
                run_id: run_id.to_string(),
                node_id: node_id.to_string(),
                status,
                elapsed_ms: 42,
                diagnostics: Vec::new(),
                agent_run_id: None,
            },
        )
        .await
        .expect("append");
}

/// Journals one `WorkflowNodeStarted`, the way the run observer does
/// immediately before a node's first attempt (issue #382).
pub(crate) async fn journal_node_started(
    state: &AppState,
    id: &CompanyId,
    workflow_id: &str,
    run_id: &str,
    node_id: &str,
) {
    let runtime = state.registry().get(id).expect("registered");
    runtime
        .events()
        .append(
            id,
            CompanyEvent::WorkflowNodeStarted {
                workflow_id: workflow_id.to_string(),
                run_id: run_id.to_string(),
                node_id: node_id.to_string(),
            },
        )
        .await
        .expect("append");
}

/// Journals a finished outcome carrying a run id, the way every entry
/// point does post-#371.
pub(crate) async fn journal_finish(
    state: &AppState,
    id: &CompanyId,
    workflow_id: &str,
    run_id: &str,
    scheduled: bool,
    error: Option<&str>,
) {
    let runtime = state.registry().get(id).expect("registered");
    runtime
        .events()
        .append(
            id,
            CompanyEvent::WorkflowRunFinished {
                workflow_id: workflow_id.to_string(),
                scheduled,
                run_id: Some(run_id.to_string()),
                deliveries: Vec::new(),
                pending_approvals: Vec::new(),
                error: error.map(str::to_string),
                cancelled: false,
                notices: Vec::new(),
                board: Vec::new(),
                blocked_nodes: Vec::new(),
                approvals: Vec::new(),
            },
        )
        .await
        .expect("append");
}

/// An event log that lets exactly one run settle **inside** `list_runs`'
/// window.
///
/// `read_from` delegates, and on its FIRST call appends `finish` to the
/// inner log *after* taking the snapshot it returns. That is precisely
/// the interleaving the race needs and the only way to get it
/// deterministically: the run's real finish is missing from the snapshot
/// the fold sees, and its supervisor entry is already gone by the time
/// `live()` is consulted — so on those two facts alone it is
/// indistinguishable from a run that died.
///
/// Every later `read_from` — including the settle's own re-read of the
/// tail — sees the finish, which is exactly what lets the read tell the
/// two apart.
pub(crate) struct FinishesDuringTheRead {
    pub(crate) inner: std::sync::Arc<dyn crate::ports::EventLog>,
    pub(crate) finish: std::sync::Mutex<Option<(CompanyId, CompanyEvent)>>,
}

#[async_trait::async_trait]
impl crate::ports::EventLog for FinishesDuringTheRead {
    async fn append(
        &self,
        id: &CompanyId,
        event: CompanyEvent,
    ) -> crate::Result<crate::ports::types::EventSeq> {
        self.inner.append(id, event).await
    }

    async fn read_from(
        &self,
        id: &CompanyId,
        seq: crate::ports::types::EventSeq,
        limit: usize,
    ) -> crate::Result<Vec<crate::ports::types::StoredEvent>> {
        let snapshot = self.inner.read_from(id, seq, limit).await?;
        // Taken out under the lock, so the append happens once however
        // many readers race here.
        let pending = self.finish.lock().expect("poisoned").take();
        if let Some((company, event)) = pending {
            self.inner.append(&company, event).await?;
        }
        Ok(snapshot)
    }

    fn subscribe(
        &self,
        id: &CompanyId,
    ) -> futures::stream::BoxStream<'static, crate::ports::events::EventStreamItem> {
        self.inner.subscribe(id)
    }
}

// -------------------------------------------------------------------
// Page cut / cursor partition (issue #1012 follow-up)
//
// `select_run_page` is exercised directly because the anomaly these
// tests are about — a run journaled with an `at_millis` OLDER than the
// row before it, after the clock stepped backwards — cannot be staged
// through the router at all: `FileStore::append` stamps
// `at_millis: now_millis()` itself, and `journal_start`/`journal_finish`
// hand it only a `CompanyEvent`. There is no seam to fake a clock
// regression end-to-end, so the cut is tested where it lives and the
// route is tested for the one thing the pure function cannot carry —
// the serialized field.
// -------------------------------------------------------------------

/// A settled run at `(seq, at_millis)`, with every other field at its
/// nothing-happened value. Only the two keys `select_run_page` reads
/// matter here.
pub(crate) fn page_run(seq: u64, at_millis: u64) -> WorkflowRunOutcome {
    WorkflowRunOutcome {
        seq,
        at_millis,
        workflow_id: "wf".to_string(),
        scheduled: false,
        run_id: Some(format!("run-{seq}")),
        resume_semantic: None,
        deliveries: Vec::new(),
        pending_approvals: Vec::new(),
        error: None,
        nodes: Vec::new(),
        started_nodes: Vec::new(),
        started_at_millis: Some(at_millis),
        running: false,
        cancelled: false,
        notices: Vec::new(),
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
        degraded: false,
        stranded_approvals: 0,
        verdict: WorkflowRunVerdict::Ok,
    }
}

/// One request's worth of the read, as the route performs it: everything
/// strictly older than the cursor is a candidate (that is exactly what
/// `EventLog::read_before` bounds by), and the cut runs over it.
///
/// Handing the whole candidate set in is a faithful superset of what the
/// backward walk accumulates — it stops once it has settled `limit + 1`
/// runs, and because it walks by descending `seq` those are the highest
/// `seq`s among the candidates, which is precisely the set the cut keeps.
pub(crate) fn page(
    journal: &[(u64, u64)],
    before_seq: Option<u64>,
    limit: usize,
) -> (Vec<WorkflowRunOutcome>, bool, Option<u64>) {
    let candidates: Vec<WorkflowRunOutcome> = journal
        .iter()
        .filter(|(seq, _)| before_seq.is_none_or(|bound| *seq < bound))
        .map(|(seq, at_millis)| page_run(*seq, *at_millis))
        .collect();
    select_run_page(candidates, limit)
}

/// A journal whose clock stepped backwards: `seq` 40 was appended after
/// 30 but carries a wall-clock time older than both 30 and 20. Every
/// other row is well-behaved.
pub(crate) const REGRESSED: [(u64, u64); 5] = [
    (10, 1_000),
    (20, 2_000),
    (30, 3_000),
    // NTP correction / VM resume / an operator setting the date: the
    // append order is unchanged, the timestamp goes backwards.
    (40, 1_500),
    (50, 5_000),
];

// -------------------------------------------------------------------
// Cron preview (issue #262)
// -------------------------------------------------------------------

/// 2026-08-02 12:00 UTC, as epoch millis — the `after` pin every
/// preview test searches forward from, so the answers are fixed rather
/// than relative to whenever CI runs.
pub(crate) const AFTER: u64 = 1_785_672_000_000;
