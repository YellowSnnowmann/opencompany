use super::*;
fn handoff(name: &str) -> Delegation {
    Delegation::DelegateToTeammate {
        teammate: name.into(),
        instruction: "Synthetic review".into(),
    }
}

#[test]
fn second_task_handoff_refusal_text_is_specific_and_actionable() {
    for (tool, effect) in [
        (
            DELEGATE_TO_DESK_TOOL,
            "nothing was handed to the design desk",
        ),
        (
            DELEGATE_TO_TEAMMATE_TOOL,
            "nothing was handed to the writer",
        ),
    ] {
        let text = no_drain(tool, effect, NoDrainReason::TaskHandoffAlreadyQueued);
        assert!(text.contains("ownership transfer queued"), "{text}");
        assert!(text.contains("Only the first colleague will run"), "{text}");
        assert!(text.contains("does not return their answer"), "{text}");
        assert!(text.contains(effect), "{text}");
        assert!(
            text.contains("Do not claim this second colleague was assigned"),
            "{text}"
        );
    }
}

#[tokio::test]
async fn dispatched_card_refuses_second_handoff_but_chat_collects_both() {
    let queue = DelegationQueue::default();
    let task_claim = queue.claim_task("card");
    task_claim
        .scoped(async {
            assert_eq!(
                queue.push_within_cap(handoff("maker"), 3, 3),
                Staged::Queued
            );
            assert_eq!(
                queue.push_within_cap(handoff("reviewer"), 3, 3),
                Staged::NoDrain(NoDrainReason::TaskHandoffAlreadyQueued)
            );
            assert_eq!(queue.drain_task_handoff_refusals(3), vec!["reviewer"]);
            let drained = queue.drain(3);
            assert_eq!(drained.len(), 1);
            assert_eq!(drained[0], handoff("maker"));
        })
        .await;
    drop(task_claim);
    let chat_claim = queue.claim();
    assert_eq!(
        queue.push_within_cap(handoff("maker"), 3, 3),
        Staged::Queued
    );
    assert_eq!(
        queue.push_within_cap(handoff("reviewer"), 3, 3),
        Staged::Queued
    );
    assert_eq!(queue.drain(3).len(), 2);
    drop(chat_claim);
}
#[tokio::test]
async fn task_handoff_keeps_unrelated_board_writes_and_redirect_reset() {
    let queue = DelegationQueue::default();
    let claim = queue.claim_task("card");
    claim
        .scoped(async {
            assert_eq!(
                queue.push_within_cap(handoff("maker"), 3, 3),
                Staged::Queued
            );
            assert_eq!(
                queue.push_within_cap(
                    Delegation::SpawnTask {
                        title: "Later work".into(),
                        note: None,
                        assignee: None
                    },
                    3,
                    3
                ),
                Staged::Queued
            );
            claim.clear();
            assert_eq!(
                queue.push_within_cap(handoff("reviewer"), 3, 3),
                Staged::Queued
            );
        })
        .await;
}
