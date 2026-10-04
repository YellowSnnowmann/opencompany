use super::tests_core2::*;

/// `Chatter` is NOT gated. It is the ambiguous bucket, and taking board
/// tools away on a maybe would turn a triage miss into work the company
/// silently refuses to do.
#[tokio::test]
async fn an_ambiguous_message_keeps_its_board_tools() {
    let neutral = "the deck looks good to me";
    assert_eq!(
        crate::company::task_intent::triage_message(neutral),
        crate::company::task_intent::MessageTriage::Chatter,
        "fixture must be the ambiguous bucket, or this proves nothing"
    );
    let fx = Fixture::new();
    let turns = ScriptedTurns::new(
        &fx,
        vec![
            Turn::tooling(
                "noted — I'll track the follow-up",
                vec![Delegation::SpawnTask {
                    title: "Follow up on the deck".to_string(),
                    note: None,
                    assignee: None,
                }],
            ),
            Turn::reply("relayed"),
        ],
    );
    let turn = fx
        .runner(&turns)
        .handle_operator_message("chief", neutral, None)
        .await
        .expect("operator message handled");

    assert!(
        turns.committed_at_turn(0),
        "chatter must still run under a claim"
    );
    assert_eq!(turns.staged(), vec![orchestrator::Staged::Queued]);
    let cards = fx.cards().await;
    assert_eq!(cards.len(), 1, "the delegation still ran: {cards:?}");
    assert_eq!(cards[0].title, "Follow up on the deck");
    assert_eq!(turn.spawned_task.as_deref(), Some(cards[0].id.as_str()));
}

/// A bare greeting is `Chatter` by a RULE FIRING (`is_matched_chatter`),
/// not by abstention like `an_ambiguous_message_keeps_its_board_tools`'s
/// fixture above — so it never reaches the escalation block, which only
/// ever runs on an abstained triage. The greeting fast path (issue #1725)
/// must still fire for it.
///
/// Goes through the real classification path
/// (`handle_operator_message`) rather than forcing
/// `with_chat_only_hint(true, ..)` directly, per review: a test that
/// forces the scope itself cannot catch the classifier failing to derive
/// the hint in the first place.
#[tokio::test]
async fn a_bare_greeting_enters_the_chat_only_fast_path() {
    let greeting = "hi";
    let triaged = crate::company::task_intent::triage_message_detailed(greeting);
    assert_eq!(
        triaged.triage,
        crate::company::task_intent::MessageTriage::Chatter,
        "fixture must be chatter, or this proves nothing"
    );
    assert!(
        !triaged.abstained(),
        "fixture must be a MATCHED chatter (a bare-greeting rule firing) — \
         the exact case that never reaches the escalation block, and the \
         one the classifier used to miss"
    );

    let fx = Fixture::new();
    let turns = ScriptedTurns::new(&fx, vec![Turn::reply("Hi! How can I help you today?")]);
    fx.runner(&turns)
        .handle_operator_message("chief", greeting, Some("general"))
        .await
        .expect("operator message handled");

    assert!(
        turns.chat_only_at_turn(0),
        "a bare greeting must enter CHAT_ONLY_TURN"
    );
}

/// Codex review round 2: `is_pure_small_talk`'s `SMALLTALK_OPENERS` (a
/// first-WORD opener list, `runtime::delegation`) is independently
/// maintained from `task_intent::GREETINGS` (a whole-MESSAGE match list) —
/// so a message the lexical triage matches as `Chatter` can still fail
/// `is_pure_small_talk` and fall back to the full agentic turn. "sup" is in
/// `GREETINGS` but has no corresponding entry in `SMALLTALK_OPENERS`,
/// making it a fixture the vocabularies disagree on.
#[tokio::test]
async fn a_matched_greeting_absent_from_smalltalk_openers_still_fast_paths() {
    let greeting = "sup";
    let triaged = crate::company::task_intent::triage_message_detailed(greeting);
    assert_eq!(
        triaged.triage,
        crate::company::task_intent::MessageTriage::Chatter,
        "fixture must be chatter, or this proves nothing"
    );
    assert!(
        !triaged.abstained(),
        "fixture must be a MATCHED chatter (a GREETINGS whole-message hit)"
    );

    let fx = Fixture::new();
    let turns = ScriptedTurns::new(&fx, vec![Turn::reply("Not much, what's up?")]);
    fx.runner(&turns)
        .handle_operator_message("chief", greeting, Some("general"))
        .await
        .expect("operator message handled");

    assert!(
        turns.chat_only_at_turn(0),
        "a lexically matched greeting must enter CHAT_ONLY_TURN even when \
         `is_pure_small_talk`'s independently maintained opener list has no \
         matching entry for it"
    );
}

// ── Issue #884 D1: a desk lead can hand a slice to a peer ───────────────

/// The orchestrator's own chat turn is not tracked here. It is the front
/// door — every message arrives there — and what it does with *work* is hand
/// it off, which opens a card on the other path.
#[tokio::test]
async fn the_orchestrators_own_chat_turn_opens_no_card() {
    let fx = Fixture::new();
    let turns = ScriptedTurns::new(&fx, vec![Turn::reply("noted")]);
    fx.runner(&turns)
        .handle_operator_message("chief", "draft the investor update", None)
        .await
        .expect("operator message handled");
    assert!(fx.cards().await.is_empty());
}
