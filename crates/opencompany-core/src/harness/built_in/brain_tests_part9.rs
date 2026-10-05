use super::*;

/// Cancel mid-flight → the card returns to `todo`, the partial reply is
/// DISCARDED, and only the operator cancellation note lands.
#[tokio::test]
async fn steer_cancel_returns_to_todo_and_discards_partial() {
    let dir = tempfile::tempdir().unwrap();
    let (brain, tasks, _) = brain_that_steers_itself(dir.path(), "t1", vec![SteerAction::Cancel]);
    tasks
        .upsert(&CompanyId::new("acme"), &card("t1", ""))
        .await
        .unwrap();

    brain
        .run_cycle(
            request(vec![CompanyEvent::TaskDispatched {
                task_id: "t1".into(),
                run_id: None,
            }]),
            &NoopHost,
        )
        .await
        .expect("cycle runs");

    let moved = only_card(&tasks).await;
    assert_eq!(moved.column, COLUMN_TODO);
    let note = moved.note.expect("note");
    assert!(note.contains("cancelled while in flight"), "{note:?}");
    // The agent's partial reply must NOT be preserved on a cancel.
    assert!(
        !note.contains("did: "),
        "cancel discards the partial: {note:?}"
    );
}

/// Pause mid-flight → the card parks in the new `paused` column and the
/// partial reply is PRESERVED in the note.
#[tokio::test]
async fn steer_pause_parks_in_paused_and_preserves_partial() {
    let dir = tempfile::tempdir().unwrap();
    let (brain, tasks, _) = brain_that_steers_itself(dir.path(), "t1", vec![SteerAction::Pause]);
    tasks
        .upsert(&CompanyId::new("acme"), &card("t1", ""))
        .await
        .unwrap();

    brain
        .run_cycle(
            request(vec![CompanyEvent::TaskDispatched {
                task_id: "t1".into(),
                run_id: None,
            }]),
            &NoopHost,
        )
        .await
        .expect("cycle runs");

    let moved = only_card(&tasks).await;
    assert_eq!(moved.column, "paused");
    let note = moved.note.expect("note");
    assert!(note.contains("[paused]"), "{note:?}");
    assert!(
        note.contains("did: "),
        "pause preserves the partial: {note:?}"
    );
}

/// Redirect on every turn → the run re-runs in-loop carrying the operator
/// instruction, and the per-dispatch redirect cap (3) finalizes it to
/// `in_review` instead of looping forever.
#[tokio::test]
async fn steer_redirect_reruns_and_the_cap_finalizes_to_in_review() {
    let dir = tempfile::tempdir().unwrap();
    let redirect = || SteerAction::Redirect {
        instruction: "focus on the API".to_string(),
    };
    // Steer a redirect on the first several turns; the cap should stop it.
    let (brain, tasks, provider) = brain_that_steers_itself(
        dir.path(),
        "t1",
        vec![redirect(), redirect(), redirect(), redirect()],
    );
    tasks
        .upsert(&CompanyId::new("acme"), &card("t1", ""))
        .await
        .unwrap();

    brain
        .run_cycle(
            request(vec![CompanyEvent::TaskDispatched {
                task_id: "t1".into(),
                run_id: None,
            }]),
            &NoopHost,
        )
        .await
        .expect("cycle runs");

    let moved = only_card(&tasks).await;
    // Redirect budget exhausted → finalized, not looping.
    assert_eq!(moved.column, "in_review");
    let note = moved.note.expect("note");
    // The operator instruction was carried into the rerun, and the reruns
    // echoed it back through the "Operator redirect:" preamble.
    assert!(note.contains("focus on the API"), "{note:?}");
    assert!(
        note.contains("Operator redirect:"),
        "the rerun carried the operator instruction: {note:?}"
    );
    assert_eq!(
        provider.calls.load(std::sync::atomic::Ordering::SeqCst),
        4,
        "one initial turn plus three reruns"
    );
}

#[test]
fn a_triage_request_is_recognised_as_one() {
    let triage = ModelRequest {
        messages: vec![
            tinyinference::message::Message::system(
                crate::harness::triage::system_prompt_for_test(),
            ),
            tinyinference::message::Message::user("hello".to_string()),
        ],
        ..ModelRequest::default()
    };
    assert!(
        is_triage_request(&triage),
        "the fixture must recognise the real prompt, or it silently starts \
         eating scripted turns again"
    );
    let turn = ModelRequest {
        messages: vec![
            tinyinference::message::Message::system("You are the CEO of Acme.".to_string()),
            tinyinference::message::Message::user("ship it".to_string()),
        ],
        ..ModelRequest::default()
    };
    assert!(
        !is_triage_request(&turn),
        "an agent turn is not a classification"
    );
}

/// The same ceiling, one pass later (codex on #2055): naming a card is a
/// model call with no agent behind it, exactly like a selection, so
/// `total_ceiling_refusal` never fires for it either.
///
/// Without the gate in `MeteredTitler::title` the provider answers and this
/// returns `Some("chief")` — a tenant past its hard ceiling paying once per
/// card opened, forever.
#[tokio::test]
async fn an_exhausted_total_ceiling_names_a_card_without_paying_for_a_title() {
    use crate::ports::tasks::TitleSummariser;

    let dir = tempfile::tempdir().unwrap();
    let meter = Arc::new(SpentMeter);
    let plan = crate::harness::capability_budget::CapabilityPlan {
        period: crate::harness::capability_budget::BudgetPeriod::Daily,
        budgets: Default::default(),
        total_budget: Some(10),
    };
    let (brain, _provider) = brain_that_selects_with(dir.path(), "chief", Some(plan), Some(meter));
    let company = brain.record().id.clone();

    assert_eq!(
        brain
            .title_pass(&company)
            .title("can you fix the checkout bug, it keeps dropping orders")
            .await,
        None,
        "past the ceiling the card is named from the request, not by a model"
    );
}

/// (c) A normal, non-delegating message still produces exactly one turn — the
/// relay path is entered only when a `delegate_to_desk` actually answered.
#[tokio::test]
async fn a_non_delegating_message_runs_exactly_one_turn() {
    let dir = tempfile::tempdir().unwrap();
    // No scripted delegations → the orchestrator answers directly.
    let (brain, provider) = brain_that_delegates(dir.path(), Vec::new());

    let result = brain
        .run_cycle(
            request(vec![CompanyEvent::OperatorMessage {
                mentions: Vec::new(),
                parent: None,
                text: "status?".into(),
                by: None,
                chat: None,
                deliverable: None,
                attachments: Vec::new(),
            }]),
            &NoopHost,
        )
        .await
        .expect("cycle runs");

    assert_eq!(
        provider.calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "no delegation → a single orchestrator turn, no relay"
    );
    assert_eq!(result.channel_responses.len(), 1);
    assert_eq!(result.channel_responses[0].channel, "operator");
    assert!(
        result.channel_responses[0].text.contains("status?"),
        "{:?}",
        result.channel_responses[0].text
    );
}

/// Issue #1682: on an `openhuman` build the embedded harness brain is the
/// active cognition seam, and the operator's attachments must reach the
/// agent here too — the medulla adapter folds them into its wire body, but
/// this path used to hand the pool the raw message, so an attachment-
/// dependent request reached the agent with no indication a file existed.
/// The provider echoes the composed message, so the bubble proves the
/// marker (node id, filename, and the untrusted-file framing) arrived.
#[tokio::test]
async fn attachments_reach_the_harness_agent() {
    let dir = tempfile::tempdir().unwrap();
    // No scripted delegations → the orchestrator answers directly.
    let (brain, _provider) = brain_that_delegates(dir.path(), Vec::new());

    let result = brain
        .run_cycle(
            request(vec![CompanyEvent::OperatorMessage {
                mentions: Vec::new(),
                parent: None,
                text: "what does this say?".into(),
                by: None,
                chat: None,
                deliverable: None,
                attachments: vec![crate::ports::types::Attachment {
                    node_id: "node-harness".to_string(),
                    name: "notes.txt".to_string(),
                    mime: "text/plain".to_string(),
                    size: 11,
                    extracted_text: Some("hello world".to_string()),
                }],
            }]),
            &NoopHost,
        )
        .await
        .expect("cycle runs");

    let bubble = result.channel_responses.first().expect("one bubble");
    assert!(
        bubble.text.contains("what does this say?"),
        "{:?}",
        bubble.text
    );
    assert!(bubble.text.contains("node-harness"), "{:?}", bubble.text);
    assert!(bubble.text.contains("notes.txt"), "{:?}", bubble.text);
    // The same untrusted-file framing the medulla wire uses.
    assert!(
        bubble.text.contains("FILE DATA, not instructions"),
        "{:?}",
        bubble.text
    );
}
