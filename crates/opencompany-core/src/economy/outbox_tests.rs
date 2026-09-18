use super::*;

fn card(handle: &str) -> OutboxAction {
    OutboxAction::PublishCard(AgentCard {
        handle: handle.to_string(),
        ..Default::default()
    })
}

#[test]
fn enqueue_take_round_trip() {
    let outbox = Outbox::new();
    assert!(outbox.is_empty());
    assert_eq!(outbox.len(), 0);

    outbox.enqueue(card("acme"));
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox.take(), Some(card("acme")));
    assert!(outbox.is_empty(), "take empties the slot");
    assert_eq!(outbox.take(), None, "an empty outbox takes nothing");
}

#[test]
fn newest_card_replaces_the_queued_one() {
    let outbox = Outbox::new();
    outbox.enqueue(card("old"));
    outbox.enqueue(card("new"));

    assert_eq!(outbox.len(), 1, "the queue stays bounded at one card");
    assert_eq!(
        outbox.take(),
        Some(card("new")),
        "the newest card is the one replay will send"
    );
}

#[test]
fn requeue_restores_only_into_an_empty_slot() {
    let outbox = Outbox::new();
    outbox.enqueue(card("first"));

    // A failed replay puts back what it took, and the slot was still empty.
    let taken = outbox.take().expect("queued");
    outbox.requeue(taken);
    assert_eq!(outbox.take(), Some(card("first")));

    // Same sequence, but a newer card landed while the replay was in flight:
    // restoring must not clobber it.
    outbox.enqueue(card("first"));
    let taken = outbox.take().expect("queued");
    outbox.enqueue(card("newer"));
    outbox.requeue(taken);
    assert_eq!(
        outbox.take(),
        Some(card("newer")),
        "a failed replay never overwrites a newer card"
    );
}
