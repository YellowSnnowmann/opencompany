use super::*;
use crate::ports::types::{CompanyEvent, EventSeq};
use crate::store::FsEventLog;
use async_trait::async_trait;

/// A runner whose `run` **panics** — the path-A failure issue #1009 fixes.
/// An unwind here used to jump straight past the finish journal, leaving the
/// run reading `running: true` until the next boot sweep.
struct PanickingRunner;

#[async_trait]
impl WorkflowRunner for PanickingRunner {
    async fn run(
        &self,
        _company: &CompanyId,
        _workflow: &WorkflowFile,
        _input: Value,
        _ctx: &WorkflowRunContext,
    ) -> Result<WorkflowRun> {
        panic!("the run blew up");
    }
}

/// A runner whose `run` returns an `Err` carrying a distinctive, made-up
/// internal detail — a stand-in for the kind of thing a real engine error
/// can plausibly say. CodeRabbit review (PR #1883) flagged that this text
/// used to be interpolated straight into a company-wide notification.
struct EngineFailingRunner;

/// The made-up internal detail `EngineFailingRunner` fails with. Chosen to
/// look like something that must never fan out to every company user.
const ENGINE_FAILURE_SECRET: &str = "token=sk-leaked-1234 at /internal/host/path";

#[async_trait]
impl WorkflowRunner for EngineFailingRunner {
    async fn run(
        &self,
        _company: &CompanyId,
        _workflow: &WorkflowFile,
        _input: Value,
        _ctx: &WorkflowRunContext,
    ) -> Result<WorkflowRun> {
        Err(crate::error::OpenCompanyError::Store(
            ENGINE_FAILURE_SECRET.to_string(),
        ))
    }
}

/// Issue #1865 (CodeRabbit review, PR #1883): a run that fails with an
/// engine `Err` must NOT leak that error's raw text into the company-wide
/// `workflow_run_failed` notification — `notify_run_unhealthy`'s audience
/// is `None` (everyone), while the real error is only readable back
/// through the authorized run-history route. Before the fix, this arm
/// interpolated `err.to_string()` straight into the notification title,
/// so `ENGINE_FAILURE_SECRET` would have shown up in it verbatim.
#[tokio::test]
async fn a_failed_run_does_not_leak_the_raw_engine_error_into_its_notification() {
    let dir = tempfile::Builder::new()
        .prefix("oc-spawn-failed-")
        .tempdir()
        .expect("tempdir");
    let events: Arc<dyn EventLog> = Arc::new(FsEventLog::new(dir.path()));
    let company = CompanyId::new("acme");
    let notifications = Arc::new(crate::store::FsOps::new(dir.path().to_path_buf()));
    let spawn = WorkflowSpawn {
        company: company.clone(),
        events: events.clone(),
        supervisor: RunSupervisor::new(),
        runner: Arc::new(EngineFailingRunner),
        runs: Arc::new(crate::store::FsOps::new(dir.path().to_path_buf())),
        notifications: notifications.clone(),
    };

    let (run_id, handle) = spawn
        .spawn(empty_workflow(), Value::Null, false, false)
        .expect("under the default cap");
    handle.await.expect("join").expect_err("the engine failed");

    use crate::ports::notifications::NotificationStore;
    let notes = notifications
        .list(&company, "anyone")
        .await
        .expect("list notifications");
    let failed = notes
        .iter()
        .find(|n| {
            n.notification.kind == "workflow_run_failed" && n.notification.subject.id == run_id
        })
        .expect("a failed run must file a durable notification");
    assert!(
        !failed.notification.title.contains(ENGINE_FAILURE_SECRET),
        "the raw engine error must never reach a company-wide notification: {:?}",
        failed.notification.title
    );
    assert!(
        failed.notification.title.contains(RUN_FAILED_DETAIL),
        "the notification must still say the run failed, using fixed text: {:?}",
        failed.notification.title
    );
}

fn empty_workflow() -> WorkflowFile {
    WorkflowFile {
        id: "digest".to_string(),
        name: "Digest".to_string(),
        description: None,
        nodes: Vec::new(),
        edges: Vec::new(),
        global: false,
        owner_desk: None,
    }
}

/// Issue #1009 (path A): a run whose task panics still journals a finish, so
/// it stops reading `running: true`.
///
/// The watchdog catches the unwind, writes the finish while the guard is
/// still held, then re-raises — so both halves hold at once: the JoinHandle
/// still resolves to a `JoinError` (the console's synchronous-mode 500 and
/// the scheduler's `tracing::error` are unchanged) AND the journal now
/// carries a `WorkflowRunFinished` for the run. Before the fix the handle
/// still errored but nothing was journaled — this asserts the JOURNAL, which
/// is the half the bug was about.
#[tokio::test]
async fn a_panicking_run_still_journals_its_finish() {
    let dir = tempfile::Builder::new()
        .prefix("oc-spawn-panic-")
        .tempdir()
        .expect("tempdir");
    let events: Arc<dyn EventLog> = Arc::new(FsEventLog::new(dir.path()));
    let company = CompanyId::new("acme");
    let notifications = Arc::new(crate::store::FsOps::new(dir.path().to_path_buf()));
    let spawn = WorkflowSpawn {
        company: company.clone(),
        events: events.clone(),
        supervisor: RunSupervisor::new(),
        runner: Arc::new(PanickingRunner),
        runs: Arc::new(crate::store::FsOps::new(dir.path().to_path_buf())),
        notifications: notifications.clone(),
    };

    let (run_id, handle) = spawn
        .spawn(empty_workflow(), Value::Null, false, false)
        .expect("under the default cap");

    let joined = handle.await;
    assert!(
        joined.is_err() && joined.unwrap_err().is_panic(),
        "the panic still propagates to the JoinHandle, preserving the sync-mode 500"
    );

    let stored = events
        .read_from(&company, EventSeq::new(0), usize::MAX)
        .await
        .expect("read journal");
    let finished = stored.iter().any(|s| {
        matches!(
            &s.event,
            CompanyEvent::WorkflowRunFinished {
                run_id: Some(id),
                error: Some(err),
                ..
            } if id == &run_id && err == PANICKED_BEFORE_FINISH
        )
    });
    assert!(
        finished,
        "the watchdog journaled a WorkflowRunFinished for the panicked run"
    );

    // Issue #1865: the panic is exactly the shape `notify_run_unhealthy`
    // exists for — a run that came apart entirely, with nobody watching
    // (a detached/scheduled fire is the case this matters most for).
    use crate::ports::notifications::NotificationStore;
    let notes = notifications
        .list(&company, "anyone")
        .await
        .expect("list notifications");
    assert!(
        notes
            .iter()
            .any(|n| n.notification.kind == "workflow_run_failed"
                && n.notification.subject.id == run_id),
        "a panicked run must file a durable notification: {notes:?}"
    );
}

/// A dry (test) run that panics journals **nothing** — the watchdog honours
/// the same `dry_run` skip the clean path does, so a test run leaves no
/// `WorkflowRunFinished` for the history to fold even when it blows up.
#[tokio::test]
async fn a_panicking_dry_run_journals_nothing() {
    let dir = tempfile::Builder::new()
        .prefix("oc-spawn-panic-dry-")
        .tempdir()
        .expect("tempdir");
    let events: Arc<dyn EventLog> = Arc::new(FsEventLog::new(dir.path()));
    let company = CompanyId::new("acme");
    let spawn = WorkflowSpawn {
        company: company.clone(),
        events: events.clone(),
        supervisor: RunSupervisor::new(),
        runner: Arc::new(PanickingRunner),
        runs: Arc::new(crate::store::FsOps::new(dir.path().to_path_buf())),
        notifications: Arc::new(crate::store::FsOps::new(dir.path().to_path_buf())),
    };

    let (_run_id, handle) = spawn
        .spawn(empty_workflow(), Value::Null, false, true)
        .expect("under the default cap");
    assert!(handle.await.is_err(), "the panic still propagates");

    let stored = events
        .read_from(&company, EventSeq::new(0), usize::MAX)
        .await
        .expect("read journal");
    assert!(
        !stored
            .iter()
            .any(|s| matches!(s.event, CompanyEvent::WorkflowRunFinished { .. })),
        "a dry run journals no finish, panic or not"
    );
}

/// A runner whose one node gated a call that failed to park — nothing is
/// left waiting on anyone, but `blocked_nodes` is non-empty too (issue
/// #1865 Codex review): `HarnessAgentRunner` pushes a
/// `WorkflowBlockedNode` the moment a turn gated anything at all, parked
/// or not, so a fully-unparkable node reads exactly like a node with a
/// live card on that field alone.
struct FullyStrandedRunner;

#[async_trait]
impl WorkflowRunner for FullyStrandedRunner {
    async fn run(
        &self,
        _company: &CompanyId,
        _workflow: &WorkflowFile,
        _input: Value,
        _ctx: &WorkflowRunContext,
    ) -> Result<WorkflowRun> {
        Ok(WorkflowRun {
            output: Value::Null,
            pending_approvals: vec!["node1".to_string()],
            deliveries: Vec::new(),
            cancelled: false,
            nodes: Vec::new(),
            notices: Vec::new(),
            board: Vec::new(),
            blocked_nodes: vec![crate::ports::WorkflowBlockedNode {
                node_id: "node1".to_string(),
                tools: vec!["some_tool".to_string()],
                // Empty: every park attempt on this node failed. This is
                // what makes `blocked_nodes.is_empty()` alone the wrong
                // test — it is non-empty here exactly as it would be for
                // a node with a live, decidable card.
                approval_ids: Vec::new(),
                unparkable: 1,
                stranded: 0,
                blockers: 0,
            }],
            approvals: vec![crate::ports::WorkflowRunApprovalRow {
                node_id: Some("node1".to_string()),
                tool: Some("some_tool".to_string()),
                outcome: crate::ports::WorkflowApprovalOutcome::ParkFailed,
                approval_id: None,
            }],
        })
    }
}

/// Issue #1865 (Codex review): a run whose only pending node has zero live
/// parked calls must file a `workflow_run_stranded` notification, not
/// `workflow_run_blocked` — nobody is actually waiting on a person to
/// decide anything, so the "blocked" wording would send an operator
/// looking for an Approvals card that does not exist.
///
/// Before the fix, the match in `WorkflowSpawn::spawn` tested
/// `!run.blocked_nodes.is_empty()` before the stranded arm, and that field
/// is non-empty for this exact case (see `FullyStrandedRunner`), so the
/// blocked arm always won and the stranded arm below it was unreachable
/// for a fully-stranded run.
#[tokio::test]
async fn a_fully_stranded_run_notifies_stranded_not_blocked() {
    let dir = tempfile::Builder::new()
        .prefix("oc-spawn-stranded-")
        .tempdir()
        .expect("tempdir");
    let events: Arc<dyn EventLog> = Arc::new(FsEventLog::new(dir.path()));
    let company = CompanyId::new("acme");
    let notifications = Arc::new(crate::store::FsOps::new(dir.path().to_path_buf()));
    let spawn = WorkflowSpawn {
        company: company.clone(),
        events: events.clone(),
        supervisor: RunSupervisor::new(),
        runner: Arc::new(FullyStrandedRunner),
        runs: Arc::new(crate::store::FsOps::new(dir.path().to_path_buf())),
        notifications: notifications.clone(),
    };

    let (run_id, handle) = spawn
        .spawn(empty_workflow(), Value::Null, false, false)
        .expect("under the default cap");
    handle.await.expect("join").expect("run settles Ok");

    use crate::ports::notifications::NotificationStore;
    let notes = notifications
        .list(&company, "anyone")
        .await
        .expect("list notifications");
    assert!(
        notes
            .iter()
            .any(|n| n.notification.kind == "workflow_run_stranded"
                && n.notification.subject.id == run_id),
        "a fully-stranded run must file a stranded notification: {notes:?}"
    );
    assert!(
        !notes
            .iter()
            .any(|n| n.notification.kind == "workflow_run_blocked"),
        "a fully-stranded run must NOT file the misleading 'blocked' \
         notification — nobody is waiting on a person to decide anything: \
         {notes:?}"
    );
}

/// A runner with two pending nodes: `node1` failed to park (nothing
/// waiting on it), `node2` has a live `Parked` card. `stranded_approvals`
/// over the whole run is `1`, which is `> 0` but not equal to the `2`
/// pending nodes — the run is only **partly** stranded, and `node2`'s
/// card is still there for an operator to decide.
struct PartlyStrandedRunner;

#[async_trait]
impl WorkflowRunner for PartlyStrandedRunner {
    async fn run(
        &self,
        _company: &CompanyId,
        _workflow: &WorkflowFile,
        _input: Value,
        _ctx: &WorkflowRunContext,
    ) -> Result<WorkflowRun> {
        Ok(WorkflowRun {
            output: Value::Null,
            pending_approvals: vec!["node1".to_string(), "node2".to_string()],
            deliveries: Vec::new(),
            cancelled: false,
            nodes: Vec::new(),
            notices: Vec::new(),
            board: Vec::new(),
            blocked_nodes: vec![
                crate::ports::WorkflowBlockedNode {
                    node_id: "node1".to_string(),
                    tools: vec!["some_tool".to_string()],
                    approval_ids: Vec::new(),
                    unparkable: 1,
                    stranded: 0,
                    blockers: 0,
                },
                crate::ports::WorkflowBlockedNode {
                    node_id: "node2".to_string(),
                    tools: vec!["other_tool".to_string()],
                    approval_ids: vec!["appr-2".to_string()],
                    unparkable: 0,
                    stranded: 0,
                    blockers: 0,
                },
            ],
            approvals: vec![
                crate::ports::WorkflowRunApprovalRow {
                    node_id: Some("node1".to_string()),
                    tool: Some("some_tool".to_string()),
                    outcome: crate::ports::WorkflowApprovalOutcome::ParkFailed,
                    approval_id: None,
                },
                crate::ports::WorkflowRunApprovalRow {
                    node_id: Some("node2".to_string()),
                    tool: Some("other_tool".to_string()),
                    outcome: crate::ports::WorkflowApprovalOutcome::Parked,
                    approval_id: Some("appr-2".to_string()),
                },
            ],
        })
    }
}

/// Codex review on PR #1883 (comment 3874654376): the "stranded" arm must
/// require the stranded count to equal the *total* pending count, matching
/// the invariant [`crate::ports::workflow_verdict::RunVerdictFacts::fully_stranded`]
/// documents for the sync run response ("a run only **partly** stranded
/// keeps its old verdict: something there really is still decidable").
/// Before the fix, this arm tested `stranded_approvals(...) > 0`, which is
/// true here too (`node1` is stranded) even though `node2` still has a
/// live, actionable card — misclassifying a decidable run as one with
/// "nobody... asked".
#[tokio::test]
async fn a_partly_stranded_run_notifies_blocked_not_stranded() {
    let dir = tempfile::Builder::new()
        .prefix("oc-spawn-partly-stranded-")
        .tempdir()
        .expect("tempdir");
    let events: Arc<dyn EventLog> = Arc::new(FsEventLog::new(dir.path()));
    let company = CompanyId::new("acme");
    let notifications = Arc::new(crate::store::FsOps::new(dir.path().to_path_buf()));
    let spawn = WorkflowSpawn {
        company: company.clone(),
        events: events.clone(),
        supervisor: RunSupervisor::new(),
        runner: Arc::new(PartlyStrandedRunner),
        runs: Arc::new(crate::store::FsOps::new(dir.path().to_path_buf())),
        notifications: notifications.clone(),
    };

    let (run_id, handle) = spawn
        .spawn(empty_workflow(), Value::Null, false, false)
        .expect("under the default cap");
    handle.await.expect("join").expect("run settles Ok");

    use crate::ports::notifications::NotificationStore;
    let notes = notifications
        .list(&company, "anyone")
        .await
        .expect("list notifications");
    assert!(
        notes
            .iter()
            .any(|n| n.notification.kind == "workflow_run_blocked"
                && n.notification.subject.id == run_id),
        "a partly-stranded run still has a decidable card and must file \
         'blocked', not 'stranded': {notes:?}"
    );
    assert!(
        !notes
            .iter()
            .any(|n| n.notification.kind == "workflow_run_stranded"),
        "a partly-stranded run must NOT be announced as fully stranded — \
         node2's card is still live and actionable: {notes:?}"
    );
}

/// Same approvals shape as `FullyStrandedRunner` — `node1` is the run's
/// only pending node and it lost its card completely — but the run also
/// carries a `deliveries` row parked for approval on the same run.
struct StrandedApprovalWithPendingDeliveryRunner;

#[async_trait]
impl WorkflowRunner for StrandedApprovalWithPendingDeliveryRunner {
    async fn run(
        &self,
        _company: &CompanyId,
        _workflow: &WorkflowFile,
        _input: Value,
        _ctx: &WorkflowRunContext,
    ) -> Result<WorkflowRun> {
        Ok(WorkflowRun {
            output: Value::Null,
            pending_approvals: vec!["node1".to_string()],
            deliveries: vec![crate::ports::DeliveryReport {
                node: "output1".to_string(),
                kind: "email".to_string(),
                target: None,
                status: crate::ports::DeliveryStatus::Pending,
                detail: "parked for operator approval".to_string(),
                reason: crate::ports::DeliveryReason::default(),
            }],
            cancelled: false,
            nodes: Vec::new(),
            notices: Vec::new(),
            board: Vec::new(),
            blocked_nodes: vec![crate::ports::WorkflowBlockedNode {
                node_id: "node1".to_string(),
                tools: vec!["some_tool".to_string()],
                approval_ids: Vec::new(),
                unparkable: 1,
                stranded: 0,
                blockers: 0,
            }],
            approvals: vec![crate::ports::WorkflowRunApprovalRow {
                node_id: Some("node1".to_string()),
                tool: Some("some_tool".to_string()),
                outcome: crate::ports::WorkflowApprovalOutcome::ParkFailed,
                approval_id: None,
            }],
        })
    }
}

/// Codex review on PR #1883 (comment 3875617184): `node1` is fully
/// stranded on its own — `stranded_approvals(...) == pending_approvals.len()`
/// holds exactly as it does for `FullyStrandedRunner` — but the run also
/// has a report parked on the deliveries queue
/// (`DeliveryStatus::Pending`). `RunVerdictFacts::fully_stranded`
/// excludes a run with a pending delivery because that report is a
/// *second* thing still waiting on a person; the notification guard must
/// apply the same exclusion so it does not claim "nobody was asked" while
/// exactly that is true of the parked report.
///
/// Before the fix, this arm ignored `run.deliveries` entirely, so it fired
/// `workflow_run_stranded` here too. `blocked_nodes` is non-empty (same
/// shape as `FullyStrandedRunner`), so once the stranded arm correctly
/// declines, the run falls through to the `blocked` arm below it — same
/// fallback the partly-stranded case uses.
#[tokio::test]
async fn a_stranded_run_with_a_pending_delivery_does_not_notify_stranded() {
    let dir = tempfile::Builder::new()
        .prefix("oc-spawn-stranded-pending-delivery-")
        .tempdir()
        .expect("tempdir");
    let events: Arc<dyn EventLog> = Arc::new(FsEventLog::new(dir.path()));
    let company = CompanyId::new("acme");
    let notifications = Arc::new(crate::store::FsOps::new(dir.path().to_path_buf()));
    let spawn = WorkflowSpawn {
        company: company.clone(),
        events: events.clone(),
        supervisor: RunSupervisor::new(),
        runner: Arc::new(StrandedApprovalWithPendingDeliveryRunner),
        runs: Arc::new(crate::store::FsOps::new(dir.path().to_path_buf())),
        notifications: notifications.clone(),
    };

    let (run_id, handle) = spawn
        .spawn(empty_workflow(), Value::Null, false, false)
        .expect("under the default cap");
    handle.await.expect("join").expect("run settles Ok");

    use crate::ports::notifications::NotificationStore;
    let notes = notifications
        .list(&company, "anyone")
        .await
        .expect("list notifications");
    assert!(
        !notes
            .iter()
            .any(|n| n.notification.kind == "workflow_run_stranded"
                && n.notification.subject.id == run_id),
        "a run with a pending delivery must NOT be announced as fully \
         stranded — the parked report is still actionable, exactly like \
         `RunVerdictFacts::fully_stranded` excludes it: {notes:?}"
    );
}

/// A runner that returns a settled, `cancelled: true` run carrying the
/// exact shape the clean node-boundary cancel arm in
/// `run_workflow_inner` (`src/workflows/runner.rs`, `if
/// outcome.cancelled`) leaves behind: `pending_approvals` is zeroed, per
/// that arm's own doc comment ("a stop still routes nothing and parks no
/// gate"), but `blocked_nodes` is still `blocks.take()` — whatever the
/// run had already gated before the operator's stop landed.
struct CancelledWithBlockedNodesRunner;

#[async_trait]
impl WorkflowRunner for CancelledWithBlockedNodesRunner {
    async fn run(
        &self,
        _company: &CompanyId,
        _workflow: &WorkflowFile,
        _input: Value,
        _ctx: &WorkflowRunContext,
    ) -> Result<WorkflowRun> {
        Ok(WorkflowRun {
            output: Value::Null,
            pending_approvals: Vec::new(),
            deliveries: Vec::new(),
            cancelled: true,
            nodes: Vec::new(),
            notices: Vec::new(),
            board: Vec::new(),
            blocked_nodes: vec![crate::ports::WorkflowBlockedNode {
                node_id: "node1".to_string(),
                tools: vec!["some_tool".to_string()],
                approval_ids: vec!["appr-1".to_string()],
                unparkable: 0,
                stranded: 0,
                blockers: 0,
            }],
            approvals: Vec::new(),
        })
    }
}

/// Issue #1865 (PR #1883 review comment 3878430677): a cancelled run
/// must NOT file the `workflow_run_blocked` notification even when it
/// carries a non-empty `blocked_nodes` — the clean node-boundary cancel
/// arm in `run_workflow_inner` preserves whatever the run had already
/// gated via `blocked_nodes: blocks.take()`, so this shape is real, not
/// synthetic. `WorkflowRunVerdict::of` checks `cancelled` before
/// `blocked_nodes` for the identical reason ("a stop somebody asked for
/// is not a fault"); before the fix, this notification guard had no such
/// check, so an operator's own stop reported "this run stopped because a
/// step is waiting on a person to decide something" — sending them
/// looking for an Approvals card on a run that will never continue
/// either way.
#[tokio::test]
async fn a_cancelled_run_does_not_notify_blocked() {
    let dir = tempfile::Builder::new()
        .prefix("oc-spawn-cancelled-blocked-")
        .tempdir()
        .expect("tempdir");
    let events: Arc<dyn EventLog> = Arc::new(FsEventLog::new(dir.path()));
    let company = CompanyId::new("acme");
    let notifications = Arc::new(crate::store::FsOps::new(dir.path().to_path_buf()));
    let spawn = WorkflowSpawn {
        company: company.clone(),
        events: events.clone(),
        supervisor: RunSupervisor::new(),
        runner: Arc::new(CancelledWithBlockedNodesRunner),
        runs: Arc::new(crate::store::FsOps::new(dir.path().to_path_buf())),
        notifications: notifications.clone(),
    };

    let (run_id, handle) = spawn
        .spawn(empty_workflow(), Value::Null, false, false)
        .expect("under the default cap");
    handle.await.expect("join").expect("run settles Ok");

    use crate::ports::notifications::NotificationStore;
    let notes = notifications
        .list(&company, "anyone")
        .await
        .expect("list notifications");
    assert!(
        notes.iter().all(|n| n.notification.subject.id != run_id),
        "a cancelled run must file NO unhealthy notification at all — a \
         deliberate stop is not one of the failed/blocked/stranded \
         readings this mechanism exists for: {notes:?}"
    );
}
