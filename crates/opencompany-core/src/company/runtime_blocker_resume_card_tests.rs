//! Runtime blocker resume: retry/amend/skip/cancel of a paused card, and agent-question re-entry.

use crate::company::blocker_sender::BlockerSenderSignals;
use crate::company::runtime::CompanyRuntime;
use crate::company::task_intent::BlockerReplyIntent;
use crate::ports::blockers::{BlockerKind, BlockerPayload, BlockerSource, BlockerStep};
use crate::ports::tasks::{
    COLUMN_IN_PROGRESS, COLUMN_IN_REVIEW, COLUMN_PAUSED, COLUMN_TODO, TaskDeliverable, TaskRecord,
    TaskTitle,
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

async fn runtime_with_harness() -> (Arc<CompanyRuntime>, TempDir) {
    let (mut runtime, home) = runtime().await;
    Arc::get_mut(&mut runtime)
        .expect("runtime is not shared yet")
        .set_harness(Arc::new(crate::harness::HarnessPool::new()));
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

/// The headline of the tier: an operator's "retry" moves the paused card
/// back into In Progress so its dispatch edge fires, and the blocker is
/// cleared.
#[tokio::test]
async fn retry_redispatches_the_paused_card() {
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

    runtime
        .apply_blocker_reply(&ids, BlockerReplyIntent::Retry, "go ahead and retry", None)
        .await
        .expect("resumes");

    assert!(
        runtime.pending_approvals().is_empty(),
        "the answered blocker is retired"
    );
    assert_eq!(
        stored(&runtime, "t-1").await.column,
        COLUMN_IN_PROGRESS,
        "a retry re-enters the stopped card through the dispatch edge"
    );
}

/// An amend carries the operator's answer onto the card so the re-run
/// reads the correction, and re-dispatches it.
#[tokio::test]
async fn amend_carries_the_answer_onto_the_card() {
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

    runtime
        .apply_blocker_reply(
            &ids,
            BlockerReplyIntent::Amend,
            "use gpt-4o-mini instead",
            None,
        )
        .await
        .expect("resumes");

    let after = stored(&runtime, "t-1").await;
    assert_eq!(after.column, COLUMN_IN_PROGRESS);
    let note = after.note.expect("the answer is on the card");
    assert!(
        note.contains("use gpt-4o-mini instead"),
        "the re-run must read the operator's correction: {note}"
    );
}

#[tokio::test]
async fn skip_settles_the_paused_card_without_another_run() {
    use crate::ports::runs::RunFilter;

    let (runtime, _home) = runtime_with_harness().await;
    let mut paused = card("t-1", COLUMN_PAUSED);
    paused.origin = crate::ports::TaskOrigin::new(Some("general".to_string()), None);
    paused.bounced = Some("an older attempt failed".to_string());
    seed(&runtime, &paused).await;
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
        .apply_blocker_reply(&ids, BlockerReplyIntent::Skip, "skip it", None)
        .await
        .expect("resumes");

    let runs = runtime
        .runs()
        .list_runs(runtime.id(), &RunFilter::for_task("t-1"))
        .await
        .expect("list runs");
    assert_eq!(runs.len(), 0, "a skip must not open another attempt");

    let after = stored(&runtime, "t-1").await;
    assert_eq!(after.column, COLUMN_IN_REVIEW);
    assert!(after.output.is_none());
    assert!(after.bounced.is_none());
    assert_eq!(after.origin_chat_id(), Some("dm:eng"));
    assert!(
        after
            .note
            .as_deref()
            .is_some_and(|note| note.contains("blocker question waived by the operator"))
    );

    let replies = dm_notes(&runtime).await;
    assert!(replies.iter().any(|reply| {
        reply == "Okay — I've waived that blocker. The card is in review; nothing ran again."
    }));

    let notification = runtime
        .notifications()
        .list(runtime.id(), "eng")
        .await
        .expect("notifications")
        .into_iter()
        .find(|notification| notification.notification.kind == "blocker_resumed")
        .expect("settle notification");
    assert!(notification.notification.title.contains("was waived"));
    assert!(
        !notification
            .notification
            .title
            .contains("picking it back up")
    );
}

/// A cancel settles the card and starts nothing: it lands back in To-do
/// carrying the reason, and — the sharpest risk — the paused card is not
/// re-dispatched.
#[tokio::test]
async fn cancel_settles_the_card_to_todo() {
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

    runtime
        .apply_blocker_reply(&ids, BlockerReplyIntent::Cancel, "cancel it", None)
        .await
        .expect("settles");

    let after = stored(&runtime, "t-1").await;
    assert_eq!(
        after.column, COLUMN_TODO,
        "a cancel abandons the work rather than re-dispatching it"
    );
    assert!(after.bounced.is_some(), "the card is marked not-fresh");
}

/// What an agent's own `escalate_to_human` parks: a question with no
/// step, because the tool holds neither a card nor a node.
async fn dm_notes(runtime: &Arc<CompanyRuntime>) -> Vec<String> {
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
            crate::ports::types::CompanyEvent::AgentReply { chat_id, text, .. }
                if chat_id == "dm:eng" =>
            {
                Some(text)
            }
            _ => None,
        })
        .collect()
}

fn agent_question() -> BlockerPayload {
    BlockerPayload {
        kind: BlockerKind::Information,
        source: BlockerSource::AgentQuestion,
        step: None,
        reason: "which of the two briefs is the current one?".to_string(),
        needed: "an answer from you".to_string(),
        group_key: None,
    }
}

/// Parks a blocker the journal records as belonging to **no** card, the
/// way a workflow node's does. `park_blocker` always links the card it
/// is given, so the unlinked case has to be built here.
async fn park_unlinked(
    runtime: &Arc<CompanyRuntime>,
    payload: &BlockerPayload,
) -> crate::ports::types::ApprovalId {
    use crate::ports::now_millis;
    use crate::ports::types::{Effect, EffectGroup};
    use crate::runtime::journal::{ApprovalConversation, TaskLink};

    let effect = Effect {
        kind: payload.effect_kind(),
        group: EffectGroup::Other,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::to_value(payload).expect("payload"),
        agent: None,
        run_id: None,
    };
    let id = runtime
        .approvals
        .park(&runtime.id, effect.clone())
        .await
        .expect("parks");
    runtime
        .journal
        .record_parked(
            &id,
            &effect,
            now_millis(),
            TaskLink::Unlinked,
            ApprovalConversation {
                thread: Some("dm:eng".to_string()),
                parent: None,
            },
            None,
        )
        .await
        .expect("records");
    id
}

/// The defect this tier was missing: a question parked with no step of
/// its own still re-enters the card its approval is linked to, and the
/// operator's answer rides onto it.
#[tokio::test]
async fn an_agent_question_re_enters_the_card_its_approval_links() {
    let (runtime, _home) = runtime().await;
    seed(&runtime, &card("t-1", COLUMN_PAUSED)).await;
    runtime
        .park_blocker(&agent_question(), "t-1", assignee("eng"))
        .await
        .expect("parks");
    let ids: Vec<_> = runtime
        .pending_approvals()
        .into_iter()
        .map(|a| a.id)
        .collect();

    runtime
        .apply_blocker_reply(
            &ids,
            BlockerReplyIntent::Amend,
            "the second brief is current",
            None,
        )
        .await
        .expect("resumes");

    let after = stored(&runtime, "t-1").await;
    assert_eq!(
        after.column, COLUMN_IN_PROGRESS,
        "a stepless question resumes through its approval's task link"
    );
    assert!(
        after
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("the second brief is current"),
        "the answer reaches the re-run: {:?}",
        after.note
    );
}

/// The same fallback settles rather than re-dispatches when the answer
/// is a cancel — the arm that moves a card for the first time.
#[tokio::test]
async fn an_agent_question_cancelled_settles_the_linked_card() {
    let (runtime, _home) = runtime().await;
    seed(&runtime, &card("t-1", COLUMN_PAUSED)).await;
    runtime
        .park_blocker(&agent_question(), "t-1", assignee("eng"))
        .await
        .expect("parks");
    let ids: Vec<_> = runtime
        .pending_approvals()
        .into_iter()
        .map(|a| a.id)
        .collect();

    runtime
        .apply_blocker_reply(&ids, BlockerReplyIntent::Cancel, "drop it", None)
        .await
        .expect("settles");

    let after = stored(&runtime, "t-1").await;
    assert_eq!(after.column, COLUMN_TODO);
    assert!(after.bounced.is_some(), "the card is marked not-fresh");
}

/// The negative that keeps the fallback honest: a blocker the journal
/// records against no card touches no card, however it is answered. A
/// fallback that reached for "whichever card was paused" would resume
/// work nobody asked about.
#[tokio::test]
async fn an_unlinked_question_moves_no_card() {
    let (runtime, _home) = runtime().await;
    seed(&runtime, &card("t-1", COLUMN_PAUSED)).await;
    let id = park_unlinked(&runtime, &agent_question()).await;

    runtime
        .apply_blocker_reply(&[id], BlockerReplyIntent::Retry, "go on", None)
        .await
        .expect("resumes");

    assert_eq!(
        stored(&runtime, "t-1").await.column,
        COLUMN_PAUSED,
        "an unlinked question leaves every card where it was"
    );
}

/// A card an operator moved on from is not the step, so the answer
/// goes back into the conversation.
///
/// Both card resumes leave a card that is no longer paused exactly
/// where it is and return without a word, so following the link to one
/// would deliver the answer nowhere at all while the blocker is still
/// recorded as resumed.
#[tokio::test]
async fn an_agent_question_whose_card_moved_on_answers_the_conversation() {
    let (runtime, _home) = runtime().await;
    seed(&runtime, &card("t-1", COLUMN_PAUSED)).await;
    runtime
        .park_blocker(&agent_question(), "t-1", assignee("eng"))
        .await
        .expect("parks");
    let ids: Vec<_> = runtime
        .pending_approvals()
        .into_iter()
        .map(|a| a.id)
        .collect();
    seed(&runtime, &card("t-1", COLUMN_IN_PROGRESS)).await;

    runtime
        .apply_blocker_reply(
            &ids,
            BlockerReplyIntent::Amend,
            "the second brief is current",
            None,
        )
        .await
        .expect("resumes");

    let after = stored(&runtime, "t-1").await;
    assert_eq!(
        after.column, COLUMN_IN_PROGRESS,
        "a card an operator moved on is left where they put it"
    );
    let notes = dm_notes(&runtime).await;
    assert!(
        notes
            .iter()
            .any(|note| note == "Thanks — using that and carrying on from where it stopped."),
        "the answer must reach the conversation it was asked in; posted: {notes:?}"
    );
}
