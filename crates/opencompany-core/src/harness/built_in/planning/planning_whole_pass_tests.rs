use std::sync::Arc;

use super::planning_fixtures_tests::*;
use super::*;
use crate::ports::tasks::TaskTitle;
use crate::ports::types::CompanyId;
use tempfile;

// ---------------------------------------------------------------------------
// The whole pass
// ---------------------------------------------------------------------------

pub(crate) async fn runtime_with(
    model: Arc<ScriptedModel>,
) -> (tempfile::TempDir, Arc<CompanyRuntime>) {
    let home = tempfile::Builder::new()
        .prefix("opencompany-planning-")
        .tempdir()
        .expect("tempdir");
    let mut runtime = crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest())
        .with_id(CompanyId::new("acme"))
        .build()
        .await
        .expect("runtime");
    runtime.set_planner(Arc::new(TaskPlanner::new(model, "chat-v1")));
    (home, Arc::new(runtime))
}

pub(crate) fn card(id: &str, assignee: &str) -> TaskRecord {
    TaskRecord {
        id: id.to_string(),
        title: TaskTitle::authored("Ship the changelog"),
        note: None,
        column: COLUMN_PLANNING.to_string(),
        priority: "medium".to_string(),
        assignee: assignee.to_string(),
        updated_at_millis: 7,
        origin: None,
        parent_task_id: None,
        plan: None,
        planning_attempts: Vec::new(),
        deliverable: crate::ports::tasks::TaskDeliverable::Once,
        workflow_proposal: None,
        // A card entering Planning has never run, so it has produced nothing
        // to link to (issue #339). Load-bearing rather than a default: the
        // re-plan test below starts from a card that HAS an output.
        output: None,
        origin_run_id: None,
        origin_workflow_id: None,
        origin_message_seq: None,
        bounced: None,
    }
}

pub(crate) async fn read(runtime: &Arc<CompanyRuntime>, id: &str) -> TaskRecord {
    runtime
        .tasks()
        .list(runtime.id())
        .await
        .expect("board")
        .into_iter()
        .find(|t| t.id == id)
        .expect("the card exists")
}

/// The happy path, end to end: the brief lands on the card and the card hands
/// itself on to be dispatched — through `upsert_task`, so the real dispatch
/// edge fires rather than a second copy of it.
#[tokio::test]
async fn a_clean_plan_lands_and_hands_the_card_on() {
    let model = ScriptedModel::replying(CLEAN_PLAN);
    let (_home, runtime) = runtime_with(Arc::clone(&model)).await;
    runtime
        .tasks()
        .upsert(runtime.id(), &card("t-1", "maya"))
        .await
        .unwrap();

    run_planning_pass(Arc::clone(&runtime), "t-1".to_string()).await;

    let after = read(&runtime, "t-1").await;
    assert_eq!(after.column, COLUMN_IN_PROGRESS);
    let plan = after.plan.expect("the brief is on the card");
    assert_eq!(plan.steps.len(), 1);
    assert_eq!(plan.steps[0].title, "Draft it");
    assert_eq!(
        plan.verification,
        "the entry is in the file and reads correctly"
    );
    assert!(plan.is_dispatchable());
    assert_eq!(after.assignee, "maya");
    let note = after.note.expect("the outcome is on the note");
    assert!(note.contains("[system] planned in 1 step"), "{note}");
    assert_eq!(model.calls(), 1, "one card, one model call");
}

fn operator() -> crate::ports::types::Actor {
    crate::ports::types::Actor {
        kind: crate::ports::types::ActorKind::Operator,
        id: "operator".to_string(),
    }
}

/// **Codex review finding on PR #2140 (`3960203729`).** A card can reach
/// `Planning` through a plain board write while the company is
/// emergency-stopped — that write never goes through `run_cycle`, so none of
/// `ensure_not_emergency_stopped`'s other doorways see it — and this pass is
/// its own paid-model call, so without its own check it would bill inference
/// while the company reports itself stopped. Proves the model is never
/// called, and the card bounces back to To-do with the stop named, exactly
/// like every other pass failure that spent nothing.
#[tokio::test]
async fn a_stopped_company_refuses_the_pass_before_the_model_is_called() {
    let model = ScriptedModel::replying(CLEAN_PLAN);
    let (_home, runtime) = runtime_with(Arc::clone(&model)).await;
    runtime
        .tasks()
        .upsert(runtime.id(), &card("t-stopped", "maya"))
        .await
        .unwrap();

    runtime
        .emergency_pause(operator(), None)
        .await
        .expect("pause");

    run_planning_pass(Arc::clone(&runtime), "t-stopped".to_string()).await;

    let after = read(&runtime, "t-stopped").await;
    assert_eq!(
        after.column, COLUMN_TODO,
        "a pass refused by the stop must not leave the card sitting in Planning"
    );
    let note = after.note.expect("the refusal is on the note");
    assert!(
        note.contains("stopped"),
        "the note should name the emergency stop as the reason: {note}"
    );
    assert_eq!(
        model.calls(),
        0,
        "the model must never be called while the company is stopped"
    );
}

/// A blocked plan is still written. It is the most useful thing on the card:
/// the operator's next move is to close the gap, and the brief is what says
/// which gap and why.
#[tokio::test]
async fn a_blocked_plan_returns_the_card_with_the_gap_named() {
    let reply = r#"{"description":"Post the announcement","steps":[{"title":"Post it","detail":"in #general"}],
        "prerequisites":[{"kind":"connection","name":"slack","why":"the announcement goes there"}],
        "risks":[],"verification":"it is visible in the channel","scope":"the post only"}"#;
    let (_home, runtime) = runtime_with(ScriptedModel::replying(reply)).await;
    runtime
        .tasks()
        .upsert(runtime.id(), &card("t-2", "maya"))
        .await
        .unwrap();

    run_planning_pass(Arc::clone(&runtime), "t-2".to_string()).await;

    let after = read(&runtime, "t-2").await;
    // Issue #1861: `paused`, not `todo`. The gap is answerable — somebody
    // connects Slack — so the card waits with a question on it rather than
    // dropping back among the cards nobody has started.
    assert_eq!(
        after.column,
        crate::ports::tasks::COLUMN_PAUSED,
        "it must not dispatch, and it must not read as fresh work either"
    );
    let plan = after.plan.expect("the brief is kept, not discarded");
    assert!(!plan.is_dispatchable());
    assert_eq!(plan.blockers().len(), 1);
    assert_eq!(plan.blockers()[0].status, PrereqStatus::Missing);
    let note = after.note.expect("note");
    assert!(note.contains("it cannot start yet"), "{note}");
    assert!(
        note.contains("slack"),
        "an operator must be able to read the gap off the board: {note}"
    );
}

/// Issue #1861: the gap does not only land on the note, it lands on the
/// operator's queue — durably, so it survives the pass that raised it and
/// expires through the approval TTL rather than waiting forever.
///
/// A missing `connection` is **infrastructure**: nobody on the roster can see
/// whether the operator's Slack is connected, so the class has to route past
/// #1866's ask-around rung rather than into it.
#[tokio::test]
async fn a_missing_prerequisite_parks_a_question_for_the_operator() {
    use crate::ports::blockers::{BlockerKind, BlockerPayload, BlockerSource, BlockerStep};

    let reply = r#"{"description":"Post the announcement","steps":[{"title":"Post it","detail":"in #general"}],
        "prerequisites":[{"kind":"connection","name":"slack","why":"the announcement goes there"}],
        "risks":[],"verification":"it is visible in the channel","scope":"the post only"}"#;
    let (_home, runtime) = runtime_with(ScriptedModel::replying(reply)).await;
    runtime
        .tasks()
        .upsert(runtime.id(), &card("t-prereq", "maya"))
        .await
        .unwrap();

    run_planning_pass(Arc::clone(&runtime), "t-prereq".to_string()).await;

    let pending = runtime.pending_approvals();
    assert_eq!(pending.len(), 1, "the gap reaches the operator's queue");

    let parked = runtime
        .journal
        .pending()
        .into_iter()
        .find(|p| p.effect.kind.starts_with("blocker."))
        .expect("a blocker is parked");
    assert_eq!(parked.effect.kind, "blocker.infrastructure");
    assert!(
        parked.effect.agent.is_none(),
        "a planning blocker is nobody's blocked tool call"
    );

    let payload: BlockerPayload =
        serde_json::from_value(parked.effect.payload.clone()).expect("payload round-trips");
    assert_eq!(payload.kind, BlockerKind::Infrastructure);
    assert_eq!(payload.source, BlockerSource::Prereq);
    assert_eq!(
        payload.step,
        Some(BlockerStep::Task {
            task_id: "t-prereq".to_string()
        }),
        "the resume tiers need to know which card stopped"
    );
    assert!(payload.reason.contains("slack"), "{}", payload.reason);
    assert!(!payload.needed.trim().is_empty());
}

/// The ownership cases stay #1106's: a card with no usable owner still returns
/// to To-do carrying its candidates, and parks nothing. Asking a second time,
/// on a second surface, for one decision would be worse than the silence.
#[tokio::test]
async fn an_unowned_card_still_returns_to_todo_and_parks_nothing() {
    let reply = r#"{"description":"Post the announcement","steps":[{"title":"Post it","detail":"in #general"}],
        "prerequisites":[],"risks":[],"verification":"visible","scope":"the post only",
        "proposedAssignee":""}"#;
    let (_home, runtime) = runtime_with(ScriptedModel::replying(reply)).await;
    let mut unowned = card("t-unowned", "maya");
    unowned.assignee = String::new();
    runtime
        .tasks()
        .upsert(runtime.id(), &unowned)
        .await
        .unwrap();

    run_planning_pass(Arc::clone(&runtime), "t-unowned".to_string()).await;

    let after = read(&runtime, "t-unowned").await;
    assert_eq!(after.column, COLUMN_TODO);
    assert!(
        runtime.pending_approvals().is_empty(),
        "who owns a card is one decision, asked in one place"
    );
}

/// A failed pass writes **no** plan. A brief half-produced by a model that
/// errored reads exactly like a finished one, and an operator would act on it.
#[tokio::test]
async fn a_failed_pass_returns_the_card_with_no_plan() {
    let (_home, runtime) = runtime_with(ScriptedModel::failing()).await;
    runtime
        .tasks()
        .upsert(runtime.id(), &card("t-3", "maya"))
        .await
        .unwrap();

    run_planning_pass(Arc::clone(&runtime), "t-3".to_string()).await;

    let after = read(&runtime, "t-3").await;
    assert_eq!(after.column, COLUMN_TODO);
    assert!(
        after.plan.is_none(),
        "nothing is better than something wrong"
    );
    let note = after.note.expect("note");
    assert!(note.contains("could not reach the model"), "{note}");
}

/// Unparseable output is a failure, not a shrug. The card comes back saying so
/// and pointing at the unplanned route, rather than resting in a column nothing
/// will re-drive.
#[tokio::test]
async fn an_unparseable_answer_returns_the_card() {
    let (_home, runtime) = runtime_with(ScriptedModel::replying("I'd start by writing it.")).await;
    runtime
        .tasks()
        .upsert(runtime.id(), &card("t-4", "maya"))
        .await
        .unwrap();

    run_planning_pass(Arc::clone(&runtime), "t-4".to_string()).await;

    let after = read(&runtime, "t-4").await;
    assert_eq!(after.column, COLUMN_TODO);
    assert!(after.plan.is_none());
    assert!(
        after
            .note
            .unwrap()
            .contains("could not read the model's answer"),
        "the note must say what went wrong, not just that something did"
    );
}

/// The optimistic settle guard. An operator who moves the card while it is
/// being planned wins — the whole pass is discarded rather than yanking the
/// card back out from under them.
#[tokio::test]
async fn an_operator_move_mid_pass_discards_the_pass() {
    let (_home, runtime) = runtime_with(ScriptedModel::replying(CLEAN_PLAN)).await;
    let mut original = card("t-5", "maya");
    runtime
        .tasks()
        .upsert(runtime.id(), &original)
        .await
        .unwrap();

    // Simulate the operator's drag landing after the pass captured its token:
    // the pass will read `token = 7`, and by settle time the card is elsewhere
    // with a newer stamp.
    let stale_token = original.updated_at_millis;
    original.column = COLUMN_TODO.to_string();
    original.updated_at_millis = stale_token + 1;
    runtime
        .tasks()
        .upsert(runtime.id(), &original)
        .await
        .unwrap();

    settle_dispatch(
        &runtime,
        "t-5",
        stale_token,
        TaskPlan {
            description: "d".to_string(),
            steps: Vec::new(),
            prerequisites: Vec::new(),
            risks: Vec::new(),
            verification: "v".to_string(),
            scope: "s".to_string(),
            proposed_assignee: None,
            assignee_candidates: Vec::new(),
            planned_at_millis: 0,
        },
        "maya".to_string(),
    )
    .await;

    let after = read(&runtime, "t-5").await;
    assert_eq!(after.column, COLUMN_TODO, "the operator's move wins");
    assert!(after.plan.is_none(), "a discarded pass writes nothing");
    assert_eq!(after.note, None);
}

/// A card that has already left Planning by the time the spawned pass runs
/// costs nothing at all — the check happens before the model is called, not
/// after.
#[tokio::test]
async fn a_card_that_left_planning_first_is_never_billed() {
    let model = ScriptedModel::replying(CLEAN_PLAN);
    let (_home, runtime) = runtime_with(Arc::clone(&model)).await;
    let mut moved = card("t-6", "maya");
    moved.column = COLUMN_TODO.to_string();
    runtime.tasks().upsert(runtime.id(), &moved).await.unwrap();

    run_planning_pass(Arc::clone(&runtime), "t-6".to_string()).await;

    assert_eq!(model.calls(), 0, "no model call for a card that moved on");
    assert_eq!(read(&runtime, "t-6").await.column, COLUMN_TODO);
}

/// The in-flight set. A second pass for the same card, while the first is
/// running, is refused — so a drag out and back in mid-pass cannot double-spend.
#[tokio::test]
async fn a_second_pass_for_one_card_is_refused_while_the_first_runs() {
    let planner = Arc::new(TaskPlanner::new(
        ScriptedModel::replying(CLEAN_PLAN),
        "chat-v1",
    ));
    let first = planner
        .claim("t-7")
        .expect("the first pass claims the card");
    assert!(
        planner.claim("t-7").is_none(),
        "a second pass for the same card must be refused"
    );
    // A different card is unaffected — the set is per card, not a global lock.
    assert!(planner.claim("t-8").is_some());
    drop(first);
    assert!(
        planner.claim("t-7").is_some(),
        "the claim is released when the pass ends, including on an early return"
    );
}

/// The whole point of the cost decision, checked where it can actually be seen:
/// the meter. Planning spend lands under the company bucket and there are
/// **zero** samples under the assignee, so a teammate's daily cap and their
/// token chart are untouched by having work planned for them.
#[tokio::test]
async fn planning_spend_lands_on_the_company_and_never_on_the_assignee() {
    use crate::ports::usage::SampleKind;

    let (_home, runtime) = runtime_with(ScriptedModel::replying(CLEAN_PLAN)).await;
    // The scripted model reports no usage, so drive the meter directly through
    // the same recorder the pass uses — this test is about attribution, and a
    // provider that reports nothing would make it vacuous.
    crate::metering::record_planning_usage(
        &TokenUsage {
            input: 1_000,
            output: 200,
            cached_input: 0,
            cost_usd: 0.03,
        },
        "managed",
        None,
        runtime.id(),
        runtime.store().as_ref(),
        runtime.usage().as_ref(),
    )
    .await;

    let samples = runtime.usage().query(runtime.id(), 0).await.expect("query");
    let planning: Vec<_> = samples
        .iter()
        .filter(|s| s.kind == SampleKind::PlanningCall)
        .collect();
    assert_eq!(planning.len(), 1);
    assert_eq!(planning[0].agent, crate::metering::UNATTRIBUTED_AGENT);
    assert!(planning[0].run_id.is_none());
    assert_eq!(
        samples.iter().filter(|s| s.agent == "maya").count(),
        0,
        "the assignee must carry no planning spend at all"
    );
}

/// A plan may fill a blank assignee but never overrule one a person chose.
#[tokio::test]
async fn a_plan_fills_a_blank_assignee_but_never_reassigns_one() {
    // Blank on the card, and the plan proposes `maya` → filled in.
    let (_home, runtime) = runtime_with(ScriptedModel::replying(CLEAN_PLAN)).await;
    runtime
        .tasks()
        .upsert(runtime.id(), &card("t-9", ""))
        .await
        .unwrap();
    run_planning_pass(Arc::clone(&runtime), "t-9".to_string()).await;
    let after = read(&runtime, "t-9").await;
    assert_eq!(after.assignee, "maya");
    assert_eq!(after.column, COLUMN_IN_PROGRESS);

    // Already assigned to `sam`, and the plan proposes `maya` → sam keeps it.
    let (_home2, runtime) = runtime_with(ScriptedModel::replying(CLEAN_PLAN)).await;
    runtime
        .tasks()
        .upsert(runtime.id(), &card("t-10", "sam"))
        .await
        .unwrap();
    run_planning_pass(Arc::clone(&runtime), "t-10".to_string()).await;
    let after = read(&runtime, "t-10").await;
    assert_eq!(
        after.assignee, "sam",
        "the operator's routing decision is not the planner's to overrule"
    );
    assert_eq!(
        after.plan.expect("plan").proposed_assignee.as_deref(),
        Some("maya"),
        "the proposal is still recorded on the brief, it is just not applied"
    );
}

/// Re-planning a card that has already produced something must **not** erase
/// the link to it (#337 meeting #339).
///
/// The two features write the same record from opposite ends: #339 stamps
/// `output` when an attempt succeeds, #337 writes `plan` when a card is
/// planned. A settle that rebuilt the card from its own fields rather than
/// read-modify-writing the live one would silently drop the other's stamp —
/// and the operator would lose the link to finished work by asking for it to be
/// re-planned, which is the worst possible moment to lose it.
///
/// Pinned as its own test because nothing about the code makes the coupling
/// visible: the settle never mentions `output` at all, and it is precisely that
/// silence that has to keep being true.
#[tokio::test]
async fn a_re_plan_does_not_erase_what_an_earlier_attempt_produced() {
    use crate::ports::tasks::{TaskOutput, TaskOutputSource};

    let (_home, runtime) = runtime_with(ScriptedModel::replying(CLEAN_PLAN)).await;
    let produced = TaskOutput {
        source: TaskOutputSource::Run {
            run_id: "run-7".to_string(),
            attempt: Some(2),
        },
        at_millis: 1_000,
        artifacts: Vec::new(),
        workflows: Vec::new(),
    };
    let mut already_delivered = card("t-13", "maya");
    already_delivered.output = Some(produced.clone());
    runtime
        .tasks()
        .upsert(runtime.id(), &already_delivered)
        .await
        .unwrap();

    run_planning_pass(Arc::clone(&runtime), "t-13".to_string()).await;

    let after = read(&runtime, "t-13").await;
    assert_eq!(after.column, COLUMN_IN_PROGRESS);
    assert!(after.plan.is_some(), "the new plan lands");
    assert_eq!(
        after.output,
        Some(produced),
        "the link to what the card already produced must survive a re-plan"
    );
}

/// A card with nobody on it and a plan that names nobody real cannot dispatch —
/// there would be no teammate to hand the work to.
#[tokio::test]
async fn a_card_with_no_valid_assignee_cannot_dispatch() {
    let reply = r#"{"description":"do it","steps":[],"prerequisites":[],"risks":[],
        "verification":"v","scope":"s",
        "assigneeCandidates":[{"id":"someone-who-left","reason":"used to own this"}]}"#;
    let (_home, runtime) = runtime_with(ScriptedModel::replying(reply)).await;
    runtime
        .tasks()
        .upsert(runtime.id(), &card("t-11", ""))
        .await
        .unwrap();

    run_planning_pass(Arc::clone(&runtime), "t-11".to_string()).await;

    let after = read(&runtime, "t-11").await;
    assert_eq!(after.column, COLUMN_TODO);
    assert!(after.plan.is_some(), "the brief is still useful");
    assert!(
        after.plan.unwrap().proposed_assignee.is_none(),
        "a proposal the roster does not recognise is dropped rather than shown"
    );
    assert!(after.note.unwrap().contains("nobody on the roster"));
}
