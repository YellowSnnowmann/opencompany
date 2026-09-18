use super::*;
use crate::ports::tasks::TaskTitle;
use crate::ports::tasks::{COLUMN_IN_REVIEW, COLUMN_PAUSED, TaskRecord};
use crate::store::FsOps;
use std::sync::Arc;

fn card(id: &str, column: &str) -> TaskRecord {
    TaskRecord {
        id: id.to_string(),
        title: TaskTitle::authored("Draft the spec"),
        note: None,
        column: column.to_string(),
        priority: "medium".to_string(),
        assignee: "maya".to_string(),
        updated_at_millis: 1,
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

async fn store() -> (tempfile::TempDir, Arc<dyn TaskStore>) {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(FsOps::new(dir.path()));
    (dir, store)
}

async fn column_of(tasks: &Arc<dyn TaskStore>, company: &CompanyId, id: &str) -> String {
    tasks
        .list(company)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.id == id)
        .expect("card exists")
        .column
}

/// The ordinary move: a card being worked, whose attempt failed, comes back
/// to To-do carrying the reason a person can read.
#[tokio::test]
async fn a_card_in_progress_moves_and_carries_the_reason() {
    let (_dir, tasks) = store().await;
    let company = CompanyId::new("acme");
    tasks
        .upsert(&company, &card("t-1", COLUMN_IN_PROGRESS))
        .await
        .unwrap();

    let moved = advance_settled_card(
        tasks.as_ref(),
        &company,
        "t-1",
        RunStatus::Failed,
        "the host restarted",
    )
    .await
    .unwrap();

    assert_eq!(moved, Some(COLUMN_TODO));
    let after = tasks
        .list(&company)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.id == "t-1")
        .unwrap();
    assert_eq!(after.column, COLUMN_TODO);
    assert_eq!(
        after.note.as_deref(),
        Some("[system] the host restarted"),
        "the reason must be readable on the card"
    );
    // Issue #1865: the board's bounce chip, set on the same landing.
    assert_eq!(
        after.bounced.as_deref(),
        Some("the host restarted"),
        "a card failed back to To-do must carry the bounce reason"
    );
}

/// A landing other than To-do — the far more common case — must never set
/// the bounce chip. `WaitingApproval` lands on Paused, which this test
/// exercises as the representative non-bounce settle.
#[tokio::test]
async fn a_non_todo_landing_never_sets_bounced() {
    let (_dir, tasks) = store().await;
    let company = CompanyId::new("acme");
    tasks
        .upsert(&company, &card("t-2", COLUMN_IN_PROGRESS))
        .await
        .unwrap();

    advance_settled_card(
        tasks.as_ref(),
        &company,
        "t-2",
        RunStatus::WaitingApproval,
        "parked a gate",
    )
    .await
    .unwrap();

    let after = tasks
        .list(&company)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.id == "t-2")
        .unwrap();
    assert_eq!(after.column, COLUMN_PAUSED);
    assert_eq!(
        after.bounced, None,
        "a card parked for approval did not bounce"
    );
}

// ── Issue #1865: `bounced_reason`, the pure rule both card-write sites share ──

#[test]
fn bounced_reason_fires_on_failed_landing_on_todo() {
    assert_eq!(
        bounced_reason(COLUMN_TODO, RunStatus::Failed, "boom"),
        Some("boom".to_string())
    );
}

#[test]
fn bounced_reason_fires_on_cancelled_landing_on_todo() {
    assert_eq!(
        bounced_reason(COLUMN_TODO, RunStatus::Cancelled, "stopped"),
        Some("stopped".to_string())
    );
}

#[test]
fn bounced_reason_is_none_off_todo_even_for_a_failure() {
    // Structurally `column_for_settled_run` never actually pairs `Failed`
    // with anything but `COLUMN_TODO` — this pins the function's OWN
    // contract regardless, so it stays correct if that mapping ever grows
    // a second failure landing.
    assert_eq!(
        bounced_reason(COLUMN_IN_REVIEW, RunStatus::Failed, "boom"),
        None
    );
}

#[test]
fn bounced_reason_is_none_on_todo_for_a_non_failure_status() {
    assert_eq!(
        bounced_reason(COLUMN_TODO, RunStatus::Succeeded, "n/a"),
        None
    );
}

/// The guard, which is the whole reason this function exists: a card an
/// operator (or a later attempt, or an approval) has already moved on is
/// **never** yanked back by a late settle.
#[tokio::test]
async fn a_card_outside_in_progress_is_never_moved() {
    let (_dir, tasks) = store().await;
    let company = CompanyId::new("acme");
    for (id, column) in [
        ("t-paused", COLUMN_PAUSED),
        ("t-review", COLUMN_IN_REVIEW),
        ("t-todo", COLUMN_TODO),
    ] {
        tasks.upsert(&company, &card(id, column)).await.unwrap();
    }

    // Try every settled status against every parked column — none may move.
    for status in [
        RunStatus::Succeeded,
        RunStatus::WaitingApproval,
        RunStatus::Paused,
        RunStatus::Failed,
        RunStatus::Cancelled,
    ] {
        for (id, column) in [
            ("t-paused", COLUMN_PAUSED),
            ("t-review", COLUMN_IN_REVIEW),
            ("t-todo", COLUMN_TODO),
        ] {
            let moved = advance_settled_card(tasks.as_ref(), &company, id, status, "late settle")
                .await
                .unwrap();
            assert_eq!(moved, None, "{status} moved {id} out of {column}");
            assert_eq!(column_of(&tasks, &company, id).await, column);
        }
    }

    // And nothing scribbled on their notes either.
    for id in ["t-paused", "t-review", "t-todo"] {
        let note = tasks
            .list(&company)
            .await
            .unwrap()
            .into_iter()
            .find(|t| t.id == id)
            .unwrap()
            .note;
        assert_eq!(note, None, "{id} was annotated by a refused move");
    }
}

/// An unsettled status is not a landing. A run still claiming to be live
/// must leave its card exactly where it is.
#[tokio::test]
async fn an_unsettled_status_moves_nothing() {
    let (_dir, tasks) = store().await;
    let company = CompanyId::new("acme");
    tasks
        .upsert(&company, &card("t-1", COLUMN_IN_PROGRESS))
        .await
        .unwrap();

    for status in [RunStatus::Pending, RunStatus::Running] {
        assert_eq!(
            advance_settled_card(tasks.as_ref(), &company, "t-1", status, "still going")
                .await
                .unwrap(),
            None
        );
    }
    assert_eq!(column_of(&tasks, &company, "t-1").await, COLUMN_IN_PROGRESS);
}

/// A card deleted between dispatch and settle is a no-op, not an error —
/// the attempt row is already settled and there is nothing left to annotate.
#[tokio::test]
async fn a_vanished_card_is_a_quiet_no_op() {
    let (_dir, tasks) = store().await;
    let company = CompanyId::new("acme");
    assert_eq!(
        advance_settled_card(
            tasks.as_ref(),
            &company,
            "t-gone",
            RunStatus::Failed,
            "orphaned",
        )
        .await
        .unwrap(),
        None
    );
}

/// A success does not skip the review stop, even on the system paths. The
/// board's automatic edge has no route to Done at all.
#[tokio::test]
async fn a_succeeded_settle_stops_in_review() {
    let (_dir, tasks) = store().await;
    let company = CompanyId::new("acme");
    tasks
        .upsert(&company, &card("t-1", COLUMN_IN_PROGRESS))
        .await
        .unwrap();

    let moved = advance_settled_card(
        tasks.as_ref(),
        &company,
        "t-1",
        RunStatus::Succeeded,
        "settled elsewhere",
    )
    .await
    .unwrap();
    assert_eq!(moved, Some(COLUMN_IN_REVIEW));
}

// --- The planning boot sweep (issue #337) -------------------------------

/// The crash case. A card left in Planning by a dead process comes back to
/// To-do saying what happened — because nothing else can recover it: a pass
/// mints no run row, so the orphan reaper never sees it, and the trigger is
/// the transition into the column, which already happened.
#[tokio::test]
async fn a_card_stranded_in_planning_comes_back_to_todo() {
    let (_dir, tasks) = store().await;
    let company = CompanyId::new("acme");
    tasks
        .upsert(&company, &card("t-1", COLUMN_PLANNING))
        .await
        .unwrap();

    let returned = sweep_stranded_planning(tasks.as_ref(), &company)
        .await
        .unwrap();
    assert_eq!(returned, vec!["t-1".to_string()]);

    let after = tasks
        .list(&company)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.id == "t-1")
        .unwrap();
    assert_eq!(after.column, COLUMN_TODO);
    let note = after.note.expect("the reason is on the card");
    assert!(note.starts_with("[system] "), "{note}");
    assert!(
        note.contains("host restarted during planning"),
        "an operator must be able to tell this from a plan that failed: {note}"
    );
    // Idempotent: a second boot finds nothing left to move.
    assert!(
        sweep_stranded_planning(tasks.as_ref(), &company)
            .await
            .unwrap()
            .is_empty()
    );
}

/// The sweep is scoped to Planning and nothing else. A board full of cards
/// in every other column is untouched — this must never become a
/// general-purpose "reset the board" pass.
#[tokio::test]
async fn the_planning_sweep_touches_no_other_column() {
    let (_dir, tasks) = store().await;
    let company = CompanyId::new("acme");
    let others = [
        ("t-todo", COLUMN_TODO),
        ("t-progress", COLUMN_IN_PROGRESS),
        ("t-paused", COLUMN_PAUSED),
        ("t-review", COLUMN_IN_REVIEW),
        ("t-done", crate::ports::tasks::COLUMN_DONE),
    ];
    for (id, column) in others {
        tasks.upsert(&company, &card(id, column)).await.unwrap();
    }
    tasks
        .upsert(&company, &card("t-planning", COLUMN_PLANNING))
        .await
        .unwrap();

    let returned = sweep_stranded_planning(tasks.as_ref(), &company)
        .await
        .unwrap();
    assert_eq!(returned, vec!["t-planning".to_string()]);

    for (id, column) in others {
        assert_eq!(column_of(&tasks, &company, id).await, column, "{id} moved");
        let note = tasks
            .list(&company)
            .await
            .unwrap()
            .into_iter()
            .find(|t| t.id == id)
            .unwrap()
            .note;
        assert_eq!(note, None, "{id} was annotated by a sweep that skipped it");
    }
}

/// Per-company, like every other sweep. One tenant's interrupted pass must
/// not move another tenant's card.
#[tokio::test]
async fn the_planning_sweep_is_scoped_to_one_company() {
    let (_dir, tasks) = store().await;
    let alpha = CompanyId::new("alpha");
    let beta = CompanyId::new("beta");
    tasks
        .upsert(&alpha, &card("a-1", COLUMN_PLANNING))
        .await
        .unwrap();
    tasks
        .upsert(&beta, &card("b-1", COLUMN_PLANNING))
        .await
        .unwrap();

    assert_eq!(
        sweep_stranded_planning(tasks.as_ref(), &alpha)
            .await
            .unwrap(),
        vec!["a-1".to_string()]
    );
    assert_eq!(column_of(&tasks, &beta, "b-1").await, COLUMN_PLANNING);
}

/// CodeRabbit review (PR #1883): `Notification.title` is documented as
/// one line, but `reason` is interpolated unnormalized. A failure reason
/// carrying `\r`/`\n` — plausible, since it is an error's `Display` in
/// practice — used to persist a multiline title.
#[tokio::test]
async fn notify_dispatch_failed_keeps_the_title_single_line() {
    let dir = tempfile::tempdir().unwrap();
    let notifications: Arc<dyn NotificationStore> = Arc::new(FsOps::new(dir.path()));
    let company = CompanyId::new("acme");

    notify_dispatch_failed(
        notifications.as_ref(),
        &company,
        "t-1",
        "boom\nsecond line\r\nthird line",
    )
    .await;

    let notes = notifications.list(&company, "anyone").await.unwrap();
    let filed = notes
        .iter()
        .find(|n| n.notification.subject.id == "t-1")
        .expect("the notification was filed");
    assert!(
        !filed.notification.title.contains('\n') && !filed.notification.title.contains('\r'),
        "the title must stay one line: {:?}",
        filed.notification.title
    );
    assert!(
        filed
            .notification
            .title
            .contains("boom second line  third line"),
        "the reason's content must survive, just flattened: {:?}",
        filed.notification.title
    );
}

/// CodeRabbit review (PR #1883): the boot-reaper conformance test
/// (`runtime::builder::tests::boot_reaper_notifies_a_bounced_card_same_as_the_live_paths`)
/// checks only `kind` and `subject.id`. This unit-tests the two things
/// that shared assertion never exercised: the title actually names the
/// task and carries the reason, and the row is company-wide
/// (`audience: None`) — the whole reason this notification exists (issue
/// #1865's doc comment) rather than a targeted one only the assignee
/// would see.
#[tokio::test]
async fn notify_dispatch_failed_files_a_company_wide_row_naming_task_and_reason() {
    let dir = tempfile::tempdir().unwrap();
    let notifications: Arc<dyn NotificationStore> = Arc::new(FsOps::new(dir.path()));
    let company = CompanyId::new("acme");

    notify_dispatch_failed(
        notifications.as_ref(),
        &company,
        "t-42",
        "the host vanished",
    )
    .await;

    let notes = notifications.list(&company, "anyone-at-all").await.unwrap();
    let filed = notes
        .iter()
        .find(|n| n.notification.subject.id == "t-42")
        .expect("the notification was filed");
    assert_eq!(filed.notification.kind, "dispatch_failed");
    assert_eq!(filed.notification.subject.kind, SubjectKind::Task);
    assert!(
        filed.notification.title.contains("the host vanished"),
        "the title must carry the reason: {:?}",
        filed.notification.title
    );
    assert_eq!(
        filed.notification.audience, None,
        "a bounced card has no single decider the way a mention does — this must be \
         company-wide, not targeted at the assignee alone"
    );
}

/// A [`NotificationStore`] whose `append` always fails — the "the durable
/// row itself could not be recorded" case `notify_dispatch_failed`'s own
/// doc comment says is best-effort and logged, never propagated.
struct FailingNotifications;

#[async_trait::async_trait]
impl NotificationStore for FailingNotifications {
    async fn append(&self, _company: &CompanyId, _notification: &Notification) -> Result<()> {
        Err(crate::error::OpenCompanyError::Store(
            "notification append always fails in this test".to_string(),
        ))
    }

    async fn list(
        &self,
        _company: &CompanyId,
        _user: &str,
    ) -> Result<Vec<crate::ports::notifications::NotificationView>> {
        Ok(Vec::new())
    }

    async fn mark_read(
        &self,
        _company: &CompanyId,
        _user: &str,
        _ids: Option<&[String]>,
    ) -> Result<u64> {
        Ok(0)
    }
}

/// CodeRabbit review (PR #1883): the boot-reaper test never exercises a
/// failing store, so nothing proved the append failure stays best-effort.
/// This is the whole point of the doc comment on `notify_dispatch_failed`
/// — a card that bounced must not un-bounce because the notification
/// bookkeeping write happened to fail.
#[tokio::test]
async fn notify_dispatch_failed_does_not_panic_or_propagate_when_append_fails() {
    let company = CompanyId::new("acme");
    // The whole assertion: this returns `()`, not a `Result`, and the call
    // completes without panicking even though `append` always errors.
    notify_dispatch_failed(&FailingNotifications, &company, "t-1", "boom").await;
}

/// The note is append-only: a second block never eats the first.
#[test]
fn append_result_keeps_what_the_note_already_said() {
    assert_eq!(append_result(None, "system", "gone"), "[system] gone");
    assert_eq!(
        append_result(Some("[maya] draft"), "system", "gone"),
        "[maya] draft\n\n[system] gone"
    );
    // An empty prior note must not leave a leading blank block.
    assert_eq!(append_result(Some(""), "system", "gone"), "[system] gone");
}
