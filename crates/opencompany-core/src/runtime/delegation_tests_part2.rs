use super::tests_core2::*;

// ── path one: a desk asked directly ─────────────────────────────────────

/// The desk's **own** `spawn_task` is how a direct ask gets tracked: the
/// card it opens is this turn's card, assigned as the agent said, raised in
/// the conversation the ask came from, and reported on the operator bubble.
#[tokio::test]
async fn a_desk_asked_directly_tracks_the_ask_with_its_own_spawn_task() {
    let fx = Fixture::new();
    let turns = ScriptedTurns::new(
        &fx,
        vec![Turn::queueing(
            "on it — tracked",
            vec![Delegation::SpawnTask {
                title: "Write modules.md".to_string(),
                note: Some("read the pricing repo first".to_string()),
                assignee: Some("engineer".to_string()),
            }],
        )],
    );
    let turn = fx
        .runner(&turns)
        .in_thread(Some(EventSeq::new(41)))
        .handle_operator_message(
            "engineer",
            "read the pricing repo and write modules.md",
            Some("eng_desk"),
        )
        .await
        .expect("operator message handled");

    let cards = fx.cards().await;
    assert_eq!(cards.len(), 1, "{cards:?}");
    assert_eq!(cards[0].assignee, "engineer");
    assert_eq!(cards[0].title, "Write modules.md");
    assert_eq!(cards[0].origin_chat_id(), Some("eng_desk"));
    assert_eq!(
        cards[0].origin_parent(),
        Some(EventSeq::new(41)),
        "raised in the thread the ask came from"
    );
    assert_eq!(turn.spawned_task.as_deref(), Some(cards[0].id.as_str()));
}

/// The same, for the card a `spawn_task` queues rather than the one a
/// hand-off opens. Two card-raising sites, one rule — and they are far
/// enough apart in this file that only a test keeps them agreeing.
#[tokio::test]
async fn a_spawned_card_records_the_thread_that_queued_it() {
    let fx = Fixture::new();
    let turns = ScriptedTurns::new(
        &fx,
        vec![Turn::queueing(
            "opening a card",
            vec![Delegation::SpawnTask {
                title: "write the migration plan".to_string(),
                note: None,
                assignee: Some("engineer".to_string()),
            }],
        )],
    );
    fx.runner(&turns)
        .in_thread(Some(EventSeq::new(41)))
        .handle_operator_message("chief", "open a card for the migration", Some("general"))
        .await
        .expect("operator message handled");

    let cards = fx.cards().await;
    assert_eq!(cards.len(), 1, "{cards:?}");
    assert_eq!(cards[0].origin_chat_id(), Some("general"));
    assert_eq!(cards[0].origin_parent(), Some(EventSeq::new(41)));
}

/// One message, one card — including the road #463 could not see (issue #1035).
///
/// The REST chat handler opens a card on **two** signals: the triage naming
/// a title, and the operator's composer asking for a workflow, which it
/// takes as an override and supplies a title for when the triage declined
/// to. The runtime re-derived "did the handler card this?" from the triage
/// alone, which is true for the first road and false for the second — so a
/// workflow request whose wording no lexical rule recognises arrived here
/// looking uncarded and got a second card beside the one it already had.
///
/// The fixture is the same residue `a_non_chatter_verdict_still_opens_the_direct_card`
/// uses, and that is the point: with no deliverable it cards, so a run that
/// opens nothing here is the flag doing the work rather than the message
/// being unremarkable.
#[tokio::test]
async fn a_workflow_the_handler_already_carded_opens_no_second_card() {
    let residue = "the pricing page copy, before Friday if you can";
    assert!(
        crate::company::task_intent::triage_message_detailed(residue)
            .triage
            .title()
            .is_none(),
        "fixture must be a message the triage does NOT name — that is the \
         road the handler took its override on"
    );

    let fx = Fixture::new();
    let turns = ScriptedTurns::new(&fx, vec![Turn::reply("on it")]);
    let turn = fx
        .runner(&turns)
        .requested(Some(crate::ports::types::MessageIntent::Workflow))
        .handle_operator_message("engineer", residue, Some("eng_desk"))
        .await
        .expect("operator message handled");

    assert!(
        fx.cards().await.is_empty(),
        "the handler carded this message on the operator's request; the \
         runtime must not open a second one"
    );
    assert_eq!(turn.spawned_task, None, "and nothing is linked to one");
}

/// One message, one card. When the REST chat handler has opened a card for
/// this message (it does so for the composer's explicit workflow request),
/// this path adopts it rather than opening another — and, since a direct
/// ask opens nothing here anyway, the turn simply links to the handler's.
#[tokio::test]
async fn a_message_the_chat_handler_already_carded_opens_no_second_card() {
    let fx = Fixture::new();
    let mut handler = handler_card_in("Draft the launch plan".to_string(), COLUMN_TODO);
    handler.id = "handler-card".to_string();
    TaskStore::upsert(&*fx.tasks, &fx.record.id, &handler)
        .await
        .expect("seed the handler card");
    let turns = ScriptedTurns::new(&fx, vec![Turn::reply("planned")]);
    let turn = fx
        .runner(&turns)
        .answering(Some(handler_seq()))
        .requested(Some(crate::ports::types::MessageIntent::Workflow))
        .handle_operator_message(
            "engineer",
            "draft the launch plan for next quarter",
            Some("eng_desk"),
        )
        .await
        .expect("operator message handled");
    assert_eq!(
        fx.cards().await.len(),
        1,
        "the chat handler's card is the card; this path opens none"
    );
    assert_eq!(turn.spawned_task.as_deref(), Some("handler-card"));
}
