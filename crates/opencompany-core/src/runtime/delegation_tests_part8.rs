use super::tests_core::*;
use super::tests_core2::*;
use super::*;

// ── Issue #453 residual: an id that names no card ───────────────────────

#[tokio::test]
async fn assigning_a_card_that_is_not_on_the_board_does_not_report_success() {
    let fx = Fixture::new();
    let turns = ScriptedTurns::new(
        &fx,
        vec![Turn::queueing(
            "assigning it",
            vec![Delegation::AssignTask {
                task_id: "card-that-never-existed".to_string(),
                assignee: "engineer".to_string(),
                note: Some("please pick this up".to_string()),
            }],
        )],
    );

    let outcome = fx
        .runner(&turns)
        .handle_operator_message(
            "chief",
            "put the launch plan on engineering",
            Some("general"),
        )
        .await
        .expect("an unknown card is a reported fact, not a turn failure");

    assert!(
        fx.cards().await.iter().all(|card| card.assignee.is_empty()),
        "nothing may be assigned on the strength of an id that names no card"
    );
    assert!(
        outcome.reply.contains("card-that-never-existed"),
        "the operator's reply must name the card the assignment could not reach rather than \
         warn into the log and let the receipt stand silently: {:?}",
        outcome.reply
    );
}

/// The same residual on the arm the code's own comment calls the more
/// consequential one: `review_task`'s receipt says the card "moves to done
/// as this turn completes". An unknown id moves nothing and records the
/// verdict nowhere, surfaced the same way `assign_task`'s is.
#[tokio::test]
async fn approving_a_card_that_is_not_on_the_board_does_not_report_success() {
    let fx = Fixture::new();
    let turns = ScriptedTurns::new(
        &fx,
        vec![Turn::queueing(
            "approved",
            vec![Delegation::ReviewTask {
                task_id: "card-that-never-existed".to_string(),
                decision: lifecycle::ReviewDecision::Approve,
                note: Some("looks good".to_string()),
            }],
        )],
    );

    let outcome = fx
        .runner(&turns)
        .handle_operator_message("chief", "approve the launch plan card", Some("general"))
        .await
        .expect("an unknown card is a reported fact, not a turn failure");

    assert!(
        fx.cards().await.is_empty(),
        "a verdict on an id that names no card may not mint one"
    );
    assert!(
        outcome.reply.contains("card-that-never-existed"),
        "the operator's reply must name the card whose approval landed nowhere rather than \
         warn into the log while the turn is told it moved: {:?}",
        outcome.reply
    );
}

/// The defect one layer over the false-success receipt: `run_delegation`
/// is driven from `drain_and_execute`'s loop with `?`, so an `Err` on one
/// delegation would abort the whole drain and discard every delegation
/// queued behind it in the SAME turn — including ones the model queued
/// validly. A hallucinated `task_id` is a routine model mistake, not an
/// exotic one, so a batch of two — an `assign_task` naming no card,
/// followed by one naming a real card — must still land the second write
/// AND still report the first's failure. The two tests above alone cannot
/// catch a regression to `Err`: they each queue exactly one delegation, so
/// an abort and a reported fact look identical from their vantage point.
#[tokio::test]
async fn a_valid_delegation_after_an_unknown_card_still_lands() {
    let fx = Fixture::new();
    let card = TaskRecord {
        opened_by: None,
        id: "card-real".to_string(),
        title: TaskTitle::authored("Draft the launch plan"),
        note: None,
        column: COLUMN_TODO.to_string(),
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
    };
    fx.tasks
        .upsert(&fx.record.id, &card)
        .await
        .expect("seed the real card");

    let turns = ScriptedTurns::new(
        &fx,
        vec![Turn::queueing(
            "assigning both",
            vec![
                Delegation::AssignTask {
                    task_id: "card-that-never-existed".to_string(),
                    assignee: "engineer".to_string(),
                    note: None,
                },
                Delegation::AssignTask {
                    task_id: "card-real".to_string(),
                    assignee: "engineer".to_string(),
                    note: None,
                },
            ],
        )],
    );

    let outcome = fx
        .runner(&turns)
        .handle_operator_message(
            "chief",
            "put the launch plan and the real card on engineering",
            Some("general"),
        )
        .await
        .expect("one unknown card must not fail the turn");

    let cards = fx.cards().await;
    let real = cards
        .iter()
        .find(|c| c.id == "card-real")
        .expect("the real card is still on the board");
    assert_eq!(
        real.assignee, "engineer",
        "the valid delegation queued AFTER the unknown-card one must still land — a false \
         success traded for silently discarding queued work is the same defect family, one \
         layer over"
    );
    assert!(
        outcome.reply.contains("card-that-never-existed"),
        "the unknown card's failure must still be reported even though the drain kept \
         going: {:?}",
        outcome.reply
    );
}

/// The bound on both refusals above: an id that DOES name a card must not
/// be caught by them. Without this the two tests are satisfied by a drain
/// that refuses every lifecycle write.
#[tokio::test]
async fn a_known_card_id_still_assigns_and_reports_no_failure() {
    let fx = Fixture::new();
    let card = TaskRecord {
        opened_by: None,
        id: "card-real".to_string(),
        title: TaskTitle::authored("Draft the launch plan"),
        note: None,
        column: COLUMN_TODO.to_string(),
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
    };
    fx.tasks
        .upsert(&fx.record.id, &card)
        .await
        .expect("seed the card");

    let turns = ScriptedTurns::new(
        &fx,
        vec![Turn::queueing(
            "assigning it",
            vec![Delegation::AssignTask {
                task_id: "card-real".to_string(),
                assignee: "engineer".to_string(),
                note: None,
            }],
        )],
    );

    fx.runner(&turns)
        .handle_operator_message(
            "chief",
            "put the launch plan on engineering",
            Some("general"),
        )
        .await
        .expect("a real card assigns without complaint");

    let cards = fx.cards().await;
    assert_eq!(cards.len(), 1, "{cards:?}");
    assert_eq!(cards[0].assignee, "engineer");
}

/// `assign_task`'s write is deliberately narrow — "the column is untouched
/// on purpose" per the arm's own comment — but nothing drove that through
/// a card that was NOT freshly opened in `todo`. A card already finished
/// is the state where a column write sneaking in in the future would be
/// most visible and most wrong: reassigning a `done` card must not reopen
/// it.
#[tokio::test]
async fn assigning_a_done_card_moves_only_the_assignee_not_the_column() {
    let fx = Fixture::new();
    fx.tasks
        .upsert(&fx.record.id, &card_in("card-done", COLUMN_DONE))
        .await
        .expect("seed a finished card");

    let turns = ScriptedTurns::new(
        &fx,
        vec![Turn::queueing(
            "assigning it",
            vec![Delegation::AssignTask {
                task_id: "card-done".to_string(),
                assignee: "engineer".to_string(),
                note: None,
            }],
        )],
    );
    fx.runner(&turns)
        .handle_operator_message(
            "chief",
            "hand the finished plan to engineering",
            Some("general"),
        )
        .await
        .expect("assigning a finished card is not refused");

    let cards = fx.cards().await;
    assert_eq!(cards.len(), 1);
    assert_eq!(
        cards[0].assignee, "engineer",
        "the assignee write still lands"
    );
    assert_eq!(
        cards[0].column, COLUMN_DONE,
        "assigning a card must never move it — a finished card stays finished"
    );
}

#[tokio::test]
async fn approving_a_card_never_dispatched_is_refused_without_changing_it() {
    let fx = Fixture::new();
    fx.tasks
        .upsert(&fx.record.id, &card_in("card-untouched", COLUMN_TODO))
        .await
        .expect("seed a card that was never dispatched");

    let turns = ScriptedTurns::new(
        &fx,
        vec![Turn::queueing(
            "approved",
            vec![Delegation::ReviewTask {
                task_id: "card-untouched".to_string(),
                decision: lifecycle::ReviewDecision::Approve,
                note: None,
            }],
        )],
    );
    let outcome = fx
        .runner(&turns)
        .handle_operator_message("chief", "approve the launch plan card", Some("general"))
        .await
        .expect("a refused review must not abort the delegation drain");

    let cards = fx.cards().await;
    assert_eq!(cards.len(), 1);
    assert_eq!(
        cards[0].column, COLUMN_TODO,
        "review_task must refuse a card that was never under review"
    );
    assert!(
        cards[0].note.is_none(),
        "a refused review must not record a verdict"
    );
    assert!(
        outcome.reply.contains("card-untouched") && outcome.reply.contains("not in_review"),
        "the operator must see why the review was refused: {}",
        outcome.reply
    );
}

#[tokio::test]
async fn review_refuses_every_non_review_column_and_preserves_later_valid_work() {
    for column in [
        COLUMN_TODO,
        COLUMN_IN_PROGRESS,
        COLUMN_DONE,
        COLUMN_PAUSED,
        COLUMN_PLANNING,
        "custom",
    ] {
        for decision in [
            lifecycle::ReviewDecision::Approve,
            lifecycle::ReviewDecision::Revise,
        ] {
            let fx = Fixture::new();
            let mut original = card_in("card-refused", column);
            original.note = Some("original note".to_string());
            original.updated_at_millis = 123;
            fx.tasks.upsert(&fx.record.id, &original).await.unwrap();
            fx.tasks
                .upsert(&fx.record.id, &card_in("card-reviewable", COLUMN_IN_REVIEW))
                .await
                .unwrap();
            let turns = ScriptedTurns::new(
                &fx,
                vec![Turn::queueing(
                    "reviewed",
                    vec![
                        Delegation::ReviewTask {
                            task_id: original.id.clone(),
                            decision,
                            note: Some("must not land".to_string()),
                        },
                        Delegation::ReviewTask {
                            task_id: "card-reviewable".to_string(),
                            decision,
                            note: Some("valid review".to_string()),
                        },
                    ],
                )],
            );
            let outcome = fx
                .runner(&turns)
                .handle_operator_message("chief", "review the launch plan cards", Some("general"))
                .await
                .expect("a refused review does not discard a valid sibling");
            let cards = fx.cards().await;
            let refused = cards.iter().find(|card| card.id == original.id).unwrap();
            assert_eq!(
                refused.column, original.column,
                "refused review must preserve its column"
            );
            assert_eq!(
                refused.note, original.note,
                "refused review must preserve its note"
            );
            assert_eq!(
                refused.updated_at_millis, original.updated_at_millis,
                "refused review must preserve its revision"
            );
            let reviewed = cards
                .iter()
                .find(|card| card.id == "card-reviewable")
                .unwrap();
            assert_eq!(reviewed.column, lifecycle::review_landing_column(decision));
            assert!(reviewed.note.as_deref().unwrap().contains("valid review"));
            assert!(
                outcome.reply.contains("card-refused") && outcome.reply.contains("not in_review"),
                "refusal must reach the operator: {}",
                outcome.reply
            );
        }
    }
}

#[tokio::test]
async fn a_task_store_write_failure_on_assign_task_surfaces_as_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backing: Arc<dyn TaskStore> = Arc::new(FsOps::new(dir.path()));
    let record = record();
    backing
        .upsert(&record.id, &card_in("card-real", COLUMN_TODO))
        .await
        .expect("seed the real card");
    let tasks: Arc<dyn TaskStore> = Arc::new(FailingUpsertStore {
        inner: backing.clone(),
    });
    let queue = DelegationQueue::default();
    let idle_turns_fx = Fixture::new();
    let idle_turns = ScriptedTurns::new(&idle_turns_fx, vec![]);

    let runner = DelegationRunner::new(
        &idle_turns,
        &record,
        Some(&tasks),
        &record.id,
        &queue,
        orchestrator::MAX_DELEGATIONS_PER_TURN,
    );
    let outcome = runner
        .run_delegation(
            Delegation::AssignTask {
                task_id: "card-real".to_string(),
                assignee: "engineer".to_string(),
                note: None,
            },
            None,
        )
        .await;
    assert!(
        outcome.is_err(),
        "a real write failure must surface as an error, not a reported fact: {:?}",
        outcome.err().map(|e| e.to_string())
    );

    let cards = backing.list(&record.id).await.unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(
        cards[0].assignee, "",
        "the card must be untouched by the failed write"
    );
}

/// The same store-fault distinction for `review_task`.
#[tokio::test]
async fn a_task_store_write_failure_on_review_task_surfaces_as_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backing: Arc<dyn TaskStore> = Arc::new(FsOps::new(dir.path()));
    let record = record();
    backing
        .upsert(&record.id, &card_in("card-real", COLUMN_IN_REVIEW))
        .await
        .expect("seed the real card");
    let tasks: Arc<dyn TaskStore> = Arc::new(FailingUpsertStore {
        inner: backing.clone(),
    });
    let queue = DelegationQueue::default();
    let idle_turns_fx = Fixture::new();
    let idle_turns = ScriptedTurns::new(&idle_turns_fx, vec![]);

    let runner = DelegationRunner::new(
        &idle_turns,
        &record,
        Some(&tasks),
        &record.id,
        &queue,
        orchestrator::MAX_DELEGATIONS_PER_TURN,
    );
    let outcome = runner
        .run_delegation(
            Delegation::ReviewTask {
                task_id: "card-real".to_string(),
                decision: lifecycle::ReviewDecision::Approve,
                note: None,
            },
            None,
        )
        .await;
    assert!(
        outcome.is_err(),
        "a real write failure must surface as an error, not a reported fact: {:?}",
        outcome.err().map(|e| e.to_string())
    );

    let cards = backing.list(&record.id).await.unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(
        cards[0].column, COLUMN_IN_REVIEW,
        "the card must be untouched by the failed write"
    );
}
