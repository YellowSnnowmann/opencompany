//! Runtime blocker resume: resume anchoring, console approve/deny, board-lock waits, and restart durability.

use crate::company::blocker_sender::BlockerSenderSignals;
use crate::company::runtime::CompanyRuntime;
use crate::company::task_intent::BlockerReplyIntent;
use crate::ports::blockers::{BlockerKind, BlockerPayload, BlockerSource, BlockerStep};
use crate::ports::tasks::{
    COLUMN_IN_PROGRESS, COLUMN_PAUSED, COLUMN_TODO, TaskDeliverable, TaskRecord, TaskTitle,
};
use crate::ports::types::CompanyId;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

async fn build(home: &Path) -> Arc<CompanyRuntime> {
    let manifest: crate::company::CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n\
         [[agent]]\nid = \"ceo\"\nrole = \"Chief\"\n\
         [[agent]]\nid = \"eng\"\nrole = \"Engineer\"\n",
    )
    .expect("manifest");
    Arc::new(
        crate::runtime::RuntimeBuilder::new(home.to_path_buf(), manifest)
            .with_id(CompanyId::new("acme"))
            .build()
            .await
            .expect("runtime"),
    )
}

async fn runtime() -> (Arc<CompanyRuntime>, TempDir) {
    let home = tempfile::Builder::new()
        .prefix("opencompany-blocker-resume-")
        .tempdir()
        .expect("tempdir");
    let runtime = build(home.path()).await;
    (runtime, home)
}

fn blocker(task_id: &str) -> BlockerPayload {
    BlockerPayload {
        kind: BlockerKind::Infrastructure,
        source: BlockerSource::Provider,
        step: Some(BlockerStep::Task {
            task_id: task_id.to_string(),
        }),
        reason: format!("the model id `gpt-nope` was rejected for {task_id}"),
        needed: "a model id this provider serves".to_string(),
        group_key: None,
    }
}

fn assignee(id: &str) -> BlockerSenderSignals {
    BlockerSenderSignals {
        started_by: None,
        owner_desk: None,
        assignee: Some(id.to_string()),
    }
}

fn card(id: &str, column: &str) -> TaskRecord {
    TaskRecord {
        id: id.to_string(),
        title: TaskTitle::authored("Draft the launch note"),
        note: None,
        column: column.to_string(),
        priority: "medium".to_string(),
        assignee: "eng".to_string(),
        updated_at_millis: 1,
        origin: crate::ports::TaskOrigin::new(Some("dm:eng".to_string()), None),
        parent_task_id: None,
        output: None,
        plan: None,
        planning_attempts: Vec::new(),
        deliverable: TaskDeliverable::Once,
        workflow_proposal: None,
        origin_run_id: None,
        origin_workflow_id: None,
        origin_message_seq: None,
        bounced: None,
    }
}

async fn seed(runtime: &Arc<CompanyRuntime>, c: &TaskRecord) {
    runtime
        .ops
        .tasks
        .upsert(&runtime.id, c)
        .await
        .expect("seed card");
}

async fn stored(runtime: &Arc<CompanyRuntime>, id: &str) -> TaskRecord {
    runtime
        .ops
        .tasks
        .list(&runtime.id)
        .await
        .expect("list")
        .into_iter()
        .find(|t| t.id == id)
        .expect("card exists")
}

/// Journals an operator line in the teammate's DM and hands back its
/// sequence, the root a reply in that DM threads off.
async fn asked_in_dm(runtime: &Arc<CompanyRuntime>) -> crate::ports::types::EventSeq {
    runtime
        .events
        .append(
            &runtime.id,
            crate::ports::types::CompanyEvent::OperatorMessage {
                text: "which brief is current?".to_string(),
                chat: Some("dm:eng".to_string()),
                parent: None,
                by: None,
                deliverable: None,
                mentions: Vec::new(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("journal the question")
}

async fn dm_replies(
    runtime: &Arc<CompanyRuntime>,
) -> Vec<(String, Option<crate::ports::types::EventSeq>)> {
    runtime
        .events
        .read_from(
            runtime.id(),
            crate::ports::types::EventSeq::new(0),
            usize::MAX,
        )
        .await
        .expect("read events")
        .into_iter()
        .filter_map(|stored| match stored.event {
            crate::ports::types::CompanyEvent::AgentReply {
                chat_id,
                text,
                parent,
                ..
            } if chat_id == "dm:eng" => Some((text, parent)),
            _ => None,
        })
        .collect()
}

/// Every resume acknowledgement lands in the thread the question was
/// asked in, not at the channel root.
///
/// The anchor is the one the approval recorded when it parked. Driven
/// directly because `park_blocker` records no
/// parent of its own: only an `escalate_to_human` park carries one, and
/// what is under test is that each resume passes on the anchor it is
/// handed rather than dropping it.
#[tokio::test]
async fn a_resume_note_threads_off_the_question_it_answers() {
    use crate::ports::blockers::{BlockerResolution, BlockerVerdict};

    for (verdict, expected) in [
        (BlockerVerdict::Retry, "Got it — picking that back up now."),
        (
            BlockerVerdict::Cancel,
            "Okay — I've cancelled that. It's back in To-do if you want to pick it up \
             later.",
        ),
        (
            BlockerVerdict::Skip,
            "Okay — I've waived that blocker. The card is in review; nothing ran again.",
        ),
    ] {
        let (runtime, _home) = runtime().await;
        seed(&runtime, &card("t-1", COLUMN_PAUSED)).await;
        let root = asked_in_dm(&runtime).await;
        let resolution = BlockerResolution {
            verdict,
            answer: String::new(),
            step: None,
        };

        match verdict {
            BlockerVerdict::Cancel => runtime
                .cancel_task_card("t-1", Some("dm:eng"), Some(root))
                .await
                .expect("cancels"),
            BlockerVerdict::Skip => runtime
                .skip_task_card("t-1", Some("dm:eng"), Some(root))
                .await
                .expect("skips"),
            BlockerVerdict::Retry => runtime
                .resume_task_card("t-1", &resolution, Some("dm:eng"), Some(root))
                .await
                .expect("resumes"),
            BlockerVerdict::Amend => unreachable!(),
        }

        let threaded: Vec<Option<crate::ports::types::EventSeq>> = dm_replies(&runtime)
            .await
            .into_iter()
            .filter(|(text, _)| text == expected)
            .map(|(_, parent)| parent)
            .collect();
        assert_eq!(
            threaded,
            vec![Some(root)],
            "the {verdict:?} acknowledgement must hang off the question it answers"
        );
    }
}

/// A resume whose recorded anchor no longer exists still answers, in
/// the channel. A root that is gone threads nothing, and the
/// acknowledgement is owed either way.
#[tokio::test]
async fn a_resume_note_whose_anchor_is_gone_still_answers() {
    use crate::ports::blockers::{BlockerResolution, BlockerVerdict};

    let (runtime, _home) = runtime().await;
    seed(&runtime, &card("t-1", COLUMN_PAUSED)).await;
    let resolution = BlockerResolution {
        verdict: BlockerVerdict::Retry,
        answer: String::new(),
        step: None,
    };

    runtime
        .resume_task_card(
            "t-1",
            &resolution,
            Some("dm:eng"),
            Some(crate::ports::types::EventSeq::new(9_999)),
        )
        .await
        .expect("resumes");

    let posted = dm_replies(&runtime).await;
    assert_eq!(
        posted,
        vec![("Got it — picking that back up now.".to_string(), None)],
        "an anchor that is gone falls back to the channel rather than swallowing the \
         acknowledgement"
    );
}

/// The guard the whole tier turns on: a blocker's effect is inert, so a
/// resuming verdict (mapped to Approve) must **never** execute it. The
/// execute path records an `EffectExecuted` key; the resume path records
/// none.
#[tokio::test]
async fn a_resolved_blocker_never_executes_its_effect() {
    let (runtime, _home) = runtime().await;
    seed(&runtime, &card("t-1", COLUMN_PAUSED)).await;
    runtime
        .park_blocker(&blocker("t-1"), "t-1", assignee("eng"))
        .await
        .expect("parks");
    let ids: Vec<_> = runtime
        .pending_approvals()
        .into_iter()
        .map(|a| a.id)
        .collect();
    let approval_id = ids[0].clone();

    runtime
        .apply_blocker_reply(&ids, BlockerReplyIntent::Retry, "retry", None)
        .await
        .expect("resumes");

    assert!(
        !runtime
            .journal
            .is_executed(&format!("approval:{approval_id}")),
        "a resolving blocker verdict must route to resume, never perform_effect"
    );
}

/// A card an operator has since dragged out of `paused` is theirs — a
/// resume must not yank it back, exactly as the expiry mover leaves it.
#[tokio::test]
async fn a_skip_leaves_a_card_moved_out_of_paused_alone() {
    let (runtime, _home) = runtime().await;
    seed(&runtime, &card("t-1", COLUMN_TODO)).await;
    runtime
        .park_blocker(&blocker("t-1"), "t-1", assignee("eng"))
        .await
        .expect("parks");
    let ids: Vec<_> = runtime
        .pending_approvals()
        .into_iter()
        .map(|a| a.id)
        .collect();

    runtime
        .apply_blocker_reply(&ids, BlockerReplyIntent::Skip, "skip", None)
        .await
        .expect("resumes");

    assert_eq!(
        stored(&runtime, "t-1").await.column,
        COLUMN_TODO,
        "a card the operator already moved on is left where they put it"
    );
}

/// Restart durability: a blocker parked before a restart is resolved
/// after it — the runtime rebuilt from the journal still resumes.
#[tokio::test]
async fn a_blocker_parked_before_a_restart_still_resumes_after_it() {
    let home = tempfile::Builder::new()
        .prefix("opencompany-blocker-durable-")
        .tempdir()
        .expect("tempdir");
    let first = build(home.path()).await;
    seed(&first, &card("t-1", COLUMN_PAUSED)).await;
    first
        .park_blocker(&blocker("t-1"), "t-1", assignee("eng"))
        .await
        .expect("parks");
    drop(first);

    // A fresh runtime over the same journal and board — a restart.
    let second = build(home.path()).await;
    let ids: Vec<_> = second
        .pending_approvals()
        .into_iter()
        .map(|a| a.id)
        .collect();
    assert_eq!(ids.len(), 1, "the parked blocker survived the restart");

    second
        .apply_blocker_reply(&ids, BlockerReplyIntent::Retry, "retry", None)
        .await
        .expect("resumes");

    assert_eq!(
        stored(&second, "t-1").await.column,
        COLUMN_IN_PROGRESS,
        "a blocker parked before the restart re-enters the card after it"
    );
}

fn operator() -> crate::ports::types::Actor {
    crate::ports::types::Actor {
        kind: crate::ports::types::ActorKind::Operator,
        id: "operator".to_string(),
    }
}

/// Issue #2008: an operator answering a blocker from the console
/// Approvals page — not the DM — must arm the same resolution a DM reply
/// does, so the paused card re-enters through the dispatch edge. Without
/// the console-side arming the verdict settles but the resume fork finds
/// nothing and the card stays `paused`.
#[tokio::test]
async fn console_approve_resumes_the_paused_card() {
    let (runtime, _home) = runtime().await;
    seed(&runtime, &card("t-1", COLUMN_PAUSED)).await;
    runtime
        .park_blocker(&blocker("t-1"), "t-1", assignee("eng"))
        .await
        .expect("parks");
    let id = runtime
        .pending_approvals()
        .into_iter()
        .next()
        .expect("parked")
        .id;

    runtime
        .resolve_approval(&id, crate::ports::types::Verdict::Approve, operator())
        .await
        .expect("resolves");

    assert!(
        runtime.pending_approvals().is_empty(),
        "the approved blocker is retired"
    );
    assert_eq!(
        stored(&runtime, "t-1").await.column,
        COLUMN_IN_PROGRESS,
        "a console Approve arms the blocker resolution and re-enters the paused card"
    );
}

/// Issue #2008: the console Approve must not execute the inert blocker
/// effect — the #1861 never-execute guard still holds on this path, the
/// same way it does for a DM answer.
#[tokio::test]
async fn console_approve_never_executes_the_blocker_effect() {
    let (runtime, _home) = runtime().await;
    seed(&runtime, &card("t-1", COLUMN_PAUSED)).await;
    runtime
        .park_blocker(&blocker("t-1"), "t-1", assignee("eng"))
        .await
        .expect("parks");
    let id = runtime
        .pending_approvals()
        .into_iter()
        .next()
        .expect("parked")
        .id;

    runtime
        .resolve_approval(&id, crate::ports::types::Verdict::Approve, operator())
        .await
        .expect("resolves");

    assert!(
        !runtime.journal.is_executed(&format!("approval:{id}")),
        "a console Approve of a blocker must resume, never perform_effect"
    );
}

/// **Codex review finding on PR #2140 (`3955615146`).** A durable
/// blocker answer banked but not yet settled — the exact window between
/// `arm_console_blocker_resolution` and `settle_claimed_blocker` a crash
/// or a stop can land in — is neither an explicit continuation nor a
/// blocked-node stash, so releasing the stop must redrive it itself
/// rather than leaving it for the next restart.
#[tokio::test]
async fn releasing_the_stop_redrives_a_blocker_answer_the_stop_itself_refused() {
    let (runtime, _home) = runtime().await;
    seed(&runtime, &card("t-1", COLUMN_PAUSED)).await;
    runtime
        .park_blocker(&blocker("t-1"), "t-1", assignee("eng"))
        .await
        .expect("parks");
    let id = runtime
        .pending_approvals()
        .into_iter()
        .next()
        .expect("parked")
        .id;

    // Bank the operator's answer durably without settling it — the same
    // intermediate state a crash or a stop between the claim and the
    // resume leaves behind.
    runtime
        .arm_console_blocker_resolution(&id, crate::ports::types::Verdict::Approve)
        .await
        .expect("arms the resolution")
        .then_some(())
        .expect("the parked blocker must actually arm");

    runtime
        .emergency_pause(operator(), None)
        .await
        .expect("pause");

    assert!(
        runtime
            .journal
            .replayed_blocker_resolutions()
            .iter()
            .any(|(rid, _)| rid == &id),
        "the banked answer is durable and still owed a settle"
    );

    runtime
        .emergency_resume(operator(), None)
        .await
        .expect("resume");

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while runtime
            .journal
            .replayed_blocker_resolutions()
            .iter()
            .any(|(rid, _)| rid == &id)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("releasing the stop must redrive the banked answer, not strand it"));

    assert_eq!(
        stored(&runtime, "t-1").await.column,
        COLUMN_IN_PROGRESS,
        "the redriven answer re-enters the paused card through the dispatch edge"
    );
}

/// Issue #2008: the resumed run's **output** must land back in the thread
/// the blocker was answered in. The card here was raised in `general`,
/// but its blocker parked into `dm:eng`; on resume the card's origin is
/// re-pointed at that DM so the dispatch relay delivers the output there,
/// and a `blocker_resumed` notification badges the same thread.
#[tokio::test]
async fn console_approve_routes_output_to_the_blocker_thread() {
    let (runtime, _home) = runtime().await;
    let mut seeded = card("t-1", COLUMN_PAUSED);
    seeded.origin = crate::ports::TaskOrigin::new(Some("general".to_string()), None);
    seed(&runtime, &seeded).await;
    runtime
        .park_blocker(&blocker("t-1"), "t-1", assignee("eng"))
        .await
        .expect("parks");
    let id = runtime
        .pending_approvals()
        .into_iter()
        .next()
        .expect("parked")
        .id;

    runtime
        .resolve_approval(&id, crate::ports::types::Verdict::Approve, operator())
        .await
        .expect("resolves");

    assert_eq!(
        stored(&runtime, "t-1").await.origin_chat_id(),
        Some("dm:eng"),
        "the resumed run reports back into the thread the blocker was answered in"
    );
    let resumed = runtime
        .notifications()
        .list(runtime.id(), "eng")
        .await
        .expect("notifications")
        .into_iter()
        .find(|n| n.notification.kind == "blocker_resumed")
        .expect("a blocker-resumed notification is filed");
    assert_eq!(
        resumed.notification.context.as_deref(),
        Some("dm:eng"),
        "the badge lands on the DM the blocker was answered in"
    );
}

/// Issue #2008: a console Deny abandons the work — it maps to a cancel,
/// which settles the card back to To-do and re-dispatches nothing.
#[tokio::test]
async fn console_deny_cancels_the_card() {
    let (runtime, _home) = runtime().await;
    seed(&runtime, &card("t-1", COLUMN_PAUSED)).await;
    runtime
        .park_blocker(&blocker("t-1"), "t-1", assignee("eng"))
        .await
        .expect("parks");
    let id = runtime
        .pending_approvals()
        .into_iter()
        .next()
        .expect("parked")
        .id;

    runtime
        .resolve_approval(&id, crate::ports::types::Verdict::Deny, operator())
        .await
        .expect("resolves");

    assert_eq!(
        stored(&runtime, "t-1").await.column,
        COLUMN_TODO,
        "a console Deny abandons the work rather than re-dispatching it"
    );
}

/// **Issue #2028 (finding 2).** The resume's board edit is a
/// read-modify-write — list the card, check it is still paused, write it
/// back — so it must serialize against every other board writer. Same
/// shape as `review_card_serializes_against_the_task_writes_lock`: hold
/// `task_writes` from the test and the resume must not move the card;
/// release it and the resume must complete.
#[tokio::test]
async fn a_resume_waits_for_the_board_write_lock() {
    let (runtime, _home) = runtime().await;
    seed(&runtime, &card("t-1", COLUMN_PAUSED)).await;
    runtime
        .park_blocker(&blocker("t-1"), "t-1", assignee("eng"))
        .await
        .expect("parks");
    let ids: Vec<_> = runtime
        .pending_approvals()
        .into_iter()
        .map(|a| a.id)
        .collect();

    let guard = runtime.task_writes.lock().await;

    let rt = Arc::clone(&runtime);
    let mut task = tokio::spawn(async move {
        rt.apply_blocker_reply(&ids, BlockerReplyIntent::Retry, "go ahead", None)
            .await
    });

    let raced_ahead = tokio::time::timeout(std::time::Duration::from_millis(200), &mut task)
        .await
        .is_ok();
    assert!(
        !raced_ahead,
        "a resume moved the card while task_writes was held elsewhere — its \
         read-modify-write is not serializing against concurrent board writers"
    );
    assert_eq!(
        stored(&runtime, "t-1").await.column,
        COLUMN_PAUSED,
        "the card must not move while another writer holds the lock"
    );

    drop(guard);
    tokio::time::timeout(std::time::Duration::from_secs(10), task)
        .await
        .expect("the resume never continued after task_writes was released")
        .expect("the resume task panicked")
        .expect("the resume completes once the lock is free");
    assert_eq!(
        stored(&runtime, "t-1").await.column,
        COLUMN_IN_PROGRESS,
        "and it re-dispatches once it has the lock"
    );
}

/// The operator-visible half of the same finding: a resume parked on
/// `task_writes` must re-read the board when it resumes, not act on the
/// snapshot it took before it blocked. An operator who drags the card
/// out of `paused` in that window has decided where it goes, and a retry
/// that yanks it back to In Progress overrides a person's own edit.
#[tokio::test]
async fn a_resume_leaves_a_card_an_operator_moved_while_it_waited() {
    let (runtime, _home) = runtime().await;
    seed(&runtime, &card("t-1", COLUMN_PAUSED)).await;
    runtime
        .park_blocker(&blocker("t-1"), "t-1", assignee("eng"))
        .await
        .expect("parks");
    let ids: Vec<_> = runtime
        .pending_approvals()
        .into_iter()
        .map(|a| a.id)
        .collect();

    let guard = runtime.task_writes.lock().await;

    let rt = Arc::clone(&runtime);
    let mut task = tokio::spawn(async move {
        rt.apply_blocker_reply(&ids, BlockerReplyIntent::Retry, "go ahead", None)
            .await
    });

    // The premise this test rests on: the resume really is still parked
    // on the lock when the operator's edit lands. Without that it would
    // pass trivially — the resume would have finished before the move,
    // and the final column would be the operator's either way.
    let raced_ahead = tokio::time::timeout(std::time::Duration::from_millis(200), &mut task)
        .await
        .is_ok();
    assert!(
        !raced_ahead,
        "the resume finished before the operator's edit, so this test would prove \
         nothing about what it does with a card that moved under it"
    );

    // The operator moves the card themselves while the resume is parked
    // on the lock — the write the resume must notice.
    let mut moved = stored(&runtime, "t-1").await;
    moved.column = COLUMN_TODO.to_string();
    seed(&runtime, &moved).await;

    drop(guard);
    tokio::time::timeout(std::time::Duration::from_secs(10), task)
        .await
        .expect("the resume never continued after task_writes was released")
        .expect("the resume task panicked")
        .expect("the resume completes");

    assert_eq!(
        stored(&runtime, "t-1").await.column,
        COLUMN_TODO,
        "a resume must re-read the board after waiting: the card is where the \
         operator put it, and yanking it back to In Progress overrides their edit"
    );
}
