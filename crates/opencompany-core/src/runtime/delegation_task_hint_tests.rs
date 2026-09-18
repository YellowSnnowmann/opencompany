use super::*;

/// The hint is readable inside its scope and gone outside it — the same
/// contract `CHAT_ONLY_TURN` holds, and for the same reason: a hint that
/// leaked past its turn would describe the previous request to the next
/// turn's extractor, which is worse than having none.
#[tokio::test]
async fn the_task_hint_is_scoped_to_its_turn() {
    assert!(
        current_task_hint().is_none(),
        "no hint before any turn is in scope"
    );
    with_task_hint("find the open issues".to_string(), async {
        assert_eq!(
            current_task_hint().as_deref(),
            Some("find the open issues"),
            "inside the scope the turn's task is readable"
        );
    })
    .await;
    assert!(
        current_task_hint().is_none(),
        "the hint does not leak past its scope"
    );
}
