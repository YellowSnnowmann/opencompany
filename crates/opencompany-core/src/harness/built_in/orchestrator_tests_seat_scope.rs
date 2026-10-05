use super::*;

fn spawn(title: &str) -> Delegation {
    Delegation::SpawnTask {
        title: title.to_string(),
        note: None,
        assignee: None,
    }
}

#[tokio::test]
async fn a_seats_bucket_is_invisible_to_the_pooled_drain_and_to_another_seat() {
    let queue = DelegationQueue::default();
    let pooled = queue.claim();
    let writer = queue.claim_seat("ep:writer", false);
    let analyst = queue.claim_seat("ep:analyst", false);
    writer
        .scoped(async {
            let _ = queue.push_within_cap(spawn("Mine"), MAX_DELEGATIONS_PER_TURN);
        })
        .await;
    assert!(queue.drain(MAX_DELEGATIONS_PER_TURN).is_empty());
    assert!(analyst.drain(MAX_DELEGATIONS_PER_TURN).is_empty());
    assert_eq!(writer.drain(MAX_DELEGATIONS_PER_TURN), vec![spawn("Mine")]);
    drop(pooled);
}

#[tokio::test]
async fn a_seat_answering_a_question_refuses_cards_as_a_pooled_question_turn_does() {
    let queue = DelegationQueue::default();
    let claim = queue.claim_seat("ep:writer", true);
    let staged = claim
        .scoped(async { queue.push_within_cap(spawn("Draft"), MAX_DELEGATIONS_PER_TURN) })
        .await;
    assert_eq!(staged, Staged::NoDrain(NoDrainReason::Triage));
    assert!(claim.drain(MAX_DELEGATIONS_PER_TURN).is_empty());
}

#[test]
fn the_seat_refusal_points_at_asking_a_teammate() {
    let text = no_drain(
        SPAWN_TASK_TOOL,
        "the card was NOT opened",
        NoDrainReason::Seat,
    );
    assert!(text.contains("hivemind_ask"), "{text}");
    assert!(text.starts_with("Refused"), "{text}");
}

struct OneEach(StdMutex<Vec<String>>);

impl crate::harness::built_in::card_budget::CardBudget for OneEach {
    fn reserve(
        &self,
        title: &str,
    ) -> Result<(), crate::harness::built_in::card_budget::CardRefusal> {
        let mut held = self.0.lock().unwrap();
        let key = crate::harness::built_in::card_budget::normalize_title(title);
        if held.contains(&key) {
            return Err(crate::harness::built_in::card_budget::CardRefusal::Duplicate);
        }
        if held.len() >= 2 {
            return Err(crate::harness::built_in::card_budget::CardRefusal::Full { cap: 2 });
        }
        held.push(key);
        Ok(())
    }

    fn release(&self, title: &str) {
        let key = crate::harness::built_in::card_budget::normalize_title(title);
        self.0.lock().unwrap().retain(|held| *held != key);
    }
}

fn tool(queue: &DelegationQueue) -> SpawnTaskTool {
    SpawnTaskTool::new(
        queue.clone(),
        CompanyId::new("acme"),
        Arc::new(MemStore::default()),
    )
}

#[tokio::test]
async fn a_seated_spawn_is_queued_honestly_and_refused_in_turn_past_its_budget() {
    let queue = DelegationQueue::default();
    let claim = queue.claim_seat("ep:writer", false);
    let budget: Arc<dyn crate::harness::built_in::card_budget::CardBudget> =
        Arc::new(OneEach(StdMutex::new(Vec::new())));
    let spawn_task = tool(&queue);
    let (first, again, second, third) = claim
        .scoped(crate::harness::built_in::card_budget::scoped(
            budget,
            async {
                (
                    spawn_task
                        .execute(json!({ "title": "Draft the post" }))
                        .await,
                    spawn_task
                        .execute(json!({ "title": "draft the POST." }))
                        .await,
                    spawn_task
                        .execute(json!({ "title": "Book the venue" }))
                        .await,
                    spawn_task.execute(json!({ "title": "Order lunch" })).await,
                )
            },
        ))
        .await;
    let first = first.unwrap();
    assert!(!first.is_error, "{}", first.text());
    assert!(
        first.text().contains("Do not describe it as open yet"),
        "{}",
        first.text()
    );
    assert!(
        first
            .text()
            .contains("you will be told here if it cannot be"),
        "{}",
        first.text()
    );
    let again = again.unwrap();
    assert!(
        again.is_error && again.text().contains("already open or queued"),
        "{}",
        again.text()
    );
    assert!(!second.unwrap().is_error);
    let third = third.unwrap();
    assert!(
        third.is_error && third.text().contains("already opened 2 cards"),
        "{}",
        third.text()
    );
    assert_eq!(claim.drain(MAX_DELEGATIONS_PER_TURN).len(), 2);
}

#[tokio::test]
async fn a_spawn_the_queue_refuses_gives_its_title_back_to_the_budget() {
    let queue = DelegationQueue::default();
    let claim = queue.claim_seat("ep:writer", true);
    let budget = Arc::new(OneEach(StdMutex::new(Vec::new())));
    let spawn_task = tool(&queue);
    let refused = claim
        .scoped(crate::harness::built_in::card_budget::scoped(
            budget.clone() as Arc<dyn crate::harness::built_in::card_budget::CardBudget>,
            spawn_task.execute(json!({ "title": "Draft the post" })),
        ))
        .await
        .unwrap();
    assert!(refused.is_error, "{}", refused.text());
    assert!(budget.0.lock().unwrap().is_empty(), "the hold is released");
}

#[tokio::test]
async fn a_pooled_spawn_keeps_its_receipt_and_the_description_names_no_other_tool() {
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let spawn_task = tool(&queue);
    let receipt = spawn_task
        .execute(json!({ "title": "Ship it" }))
        .await
        .unwrap();
    assert!(
        receipt
            .text()
            .contains("It will be opened on the board this turn.")
    );
    let description = spawn_task.description();
    for other in ["delegate_to", "assign_task", "review_task", "desk_"] {
        assert!(!description.contains(other), "{description}");
    }
}

#[tokio::test]
async fn a_seat_refuses_a_card_whose_assignee_the_roster_read_could_not_check() {
    let queue = DelegationQueue::default();
    let claim = queue.claim_seat("ep:writer", false);
    let budget = Arc::new(OneEach(StdMutex::new(Vec::new())));
    let spawn_task =
        SpawnTaskTool::new(queue.clone(), CompanyId::new("acme"), Arc::new(BrokenStore));
    let refused = claim
        .scoped(crate::harness::built_in::card_budget::scoped(
            budget.clone() as Arc<dyn crate::harness::built_in::card_budget::CardBudget>,
            spawn_task.execute(json!({ "title": "Draft the post", "assignee": "eng" })),
        ))
        .await
        .unwrap();
    assert!(refused.is_error, "{}", refused.text());
    assert!(
        refused.text().contains("could not be read")
            && refused.text().contains("Nothing was queued"),
        "{}",
        refused.text()
    );
    assert!(budget.0.lock().unwrap().is_empty(), "nothing is reserved");
    assert!(claim.drain(MAX_DELEGATIONS_PER_TURN).is_empty());
}

#[tokio::test]
async fn a_seat_with_no_assignee_still_opens_its_card_when_the_roster_is_unreadable() {
    let queue = DelegationQueue::default();
    let claim = queue.claim_seat("ep:writer", false);
    let spawn_task =
        SpawnTaskTool::new(queue.clone(), CompanyId::new("acme"), Arc::new(BrokenStore));
    let queued = claim
        .scoped(spawn_task.execute(json!({ "title": "Draft the post" })))
        .await
        .unwrap();
    assert!(!queued.is_error, "{}", queued.text());
    assert_eq!(
        claim.drain(MAX_DELEGATIONS_PER_TURN),
        vec![spawn("Draft the post")]
    );
}

#[tokio::test]
async fn an_absent_company_record_queues_the_assignee_as_typed_with_the_plain_receipt() {
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let receipt = tool(&queue)
        .execute(json!({ "title": "Ship it", "assignee": "eng" }))
        .await
        .unwrap();
    assert!(!receipt.is_error, "{}", receipt.text());
    assert!(
        !receipt.text().contains("could not be checked"),
        "{}",
        receipt.text()
    );

    let seat_queue = DelegationQueue::default();
    let seat = seat_queue.claim_seat("ep:writer", false);
    let seated = seat
        .scoped(tool(&seat_queue).execute(json!({ "title": "Ship it", "assignee": "eng" })))
        .await
        .unwrap();
    assert!(!seated.is_error, "{}", seated.text());
    assert_eq!(
        seat.drain(MAX_DELEGATIONS_PER_TURN),
        vec![Delegation::SpawnTask {
            title: "Ship it".to_string(),
            note: None,
            assignee: Some("eng".to_string()),
        }]
    );
}
