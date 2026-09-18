use std::sync::Arc;

use super::tests_copilot_unit::DESC_GRAPH;
use super::workflow_build_fixtures_tests::*;
use super::workflow_build_shared_tests::*;
use super::*;
use crate::ports::runs::RunStatus;

/// An unparseable answer returns the card to To-do with no proposal; the attempt
/// settles Failed. Prose or nothing — a graph guessed from prose is exactly the
/// broken graph.
#[tokio::test]
async fn an_unparseable_answer_returns_to_todo_with_no_proposal() {
    let (_home, runtime) = runtime_with(ScriptedModel::replying("I'd just do this by hand.")).await;
    runtime
        .tasks()
        .upsert(runtime.id(), &card("t-4", None))
        .await
        .unwrap();
    let run_id = open_run(&runtime, "t-4").await;

    run_workflow_build_pass(
        Arc::clone(&runtime),
        "t-4".to_string(),
        Some(run_id.clone()),
    )
    .await;

    let after = read(&runtime, "t-4").await;
    assert_eq!(after.column, COLUMN_TODO);
    assert!(after.workflow_proposal.is_none());
    assert_eq!(run_status(&runtime, &run_id).await, RunStatus::Failed);
}

/// A model error returns the card to To-do and settles the attempt Failed —
/// building could not reach the model, so nothing was proposed.
#[tokio::test]
async fn a_model_error_returns_to_todo() {
    let (_home, runtime) = runtime_with(ScriptedModel::failing()).await;
    runtime
        .tasks()
        .upsert(runtime.id(), &card("t-5", None))
        .await
        .unwrap();
    let run_id = open_run(&runtime, "t-5").await;

    run_workflow_build_pass(
        Arc::clone(&runtime),
        "t-5".to_string(),
        Some(run_id.clone()),
    )
    .await;

    let after = read(&runtime, "t-5").await;
    assert_eq!(after.column, COLUMN_TODO);
    assert!(after.workflow_proposal.is_none());
    assert_eq!(run_status(&runtime, &run_id).await, RunStatus::Failed);
}

/// A graph the create path would refuse — an `agent` node naming a teammate not
/// on the roster — never reaches In Review: the courtesy validation catches it,
/// the card returns to To-do naming the error, no proposal, attempt Failed.
#[tokio::test]
async fn a_graph_that_would_be_refused_never_reaches_in_review() {
    let reply = r#"{"automatable":true,"summary":"do it","workflow":{"name":"Bad",
        "nodes":[{"id":"start","kind":"trigger","name":"Start"},
                 {"id":"draft","kind":"agent","name":"Draft","agent":"ghost"}],
        "edges":[{"from":"start","to":"draft"}]}}"#;
    let (_home, runtime) = runtime_with(ScriptedModel::replying(reply)).await;
    runtime
        .tasks()
        .upsert(runtime.id(), &card("t-6", None))
        .await
        .unwrap();
    let run_id = open_run(&runtime, "t-6").await;

    run_workflow_build_pass(
        Arc::clone(&runtime),
        "t-6".to_string(),
        Some(run_id.clone()),
    )
    .await;

    let after = read(&runtime, "t-6").await;
    assert_eq!(after.column, COLUMN_TODO);
    assert!(
        after.workflow_proposal.is_none(),
        "a doomed proposal must not reach In Review"
    );
    assert!(
        after.note.unwrap().contains("ghost"),
        "the roster error is named on the card"
    );
    assert_eq!(run_status(&runtime, &run_id).await, RunStatus::Failed);
}

/// A graph carrying a node kind outside `BUILDER_NODE_KINDS` — one the prompt
/// never taught the model to shape and nothing downstream validates (here an
/// `http_request` with an attacker-influenceable URL) — settles to To-do before
/// the courtesy pass, with no proposal and the attempt Failed. The host owns the
/// kind vocabulary, not the model (issue #580).
#[tokio::test]
async fn an_out_of_vocabulary_kind_settles_to_todo() {
    let reply = r#"{"automatable":true,"summary":"call a url","workflow":{"name":"Reach out",
        "nodes":[{"id":"start","kind":"trigger","name":"Start"},
                 {"id":"call","kind":"http_request","name":"Call",
                  "config":{"url":"http://attacker.example/x","method":"GET"}}],
        "edges":[{"from":"start","to":"call"}]}}"#;
    let (_home, runtime) = runtime_with(ScriptedModel::replying(reply)).await;
    runtime
        .tasks()
        .upsert(runtime.id(), &card("t-9", None))
        .await
        .unwrap();
    let run_id = open_run(&runtime, "t-9").await;

    run_workflow_build_pass(
        Arc::clone(&runtime),
        "t-9".to_string(),
        Some(run_id.clone()),
    )
    .await;

    let after = read(&runtime, "t-9").await;
    assert_eq!(after.column, COLUMN_TODO);
    assert!(
        after.workflow_proposal.is_none(),
        "an unsupported kind must not reach In Review"
    );
    assert!(
        after.note.unwrap().contains("http_request"),
        "the offending kind is named on the card"
    );
    assert_eq!(run_status(&runtime, &run_id).await, RunStatus::Failed);
}

/// Issue #1865 (CodeRabbit review, PR #1883): `settle_to_todo` is the builder's
/// only failure exit, and it must carry the same bounce chip every other
/// failed-dispatch-back-to-To-do path does — `advance::advance_settled_card`
/// and `run_task`'s rich settle both compute it. Reuses the out-of-vocabulary
/// scenario above, which already drives a real `settle_to_todo` call, and
/// checks the one field that test does not: without the fix, `settle_to_todo`
/// never touched `bounced`, so a card that had never bounced before (dispatch
/// already cleared it) came back from a genuine builder failure still reading
/// `bounced: None` — indistinguishable from a card that had never failed.
#[tokio::test]
async fn a_builder_failure_settling_to_todo_sets_the_bounce_chip() {
    let reply = r#"{"automatable":true,"summary":"call a url","workflow":{"name":"Reach out",
        "nodes":[{"id":"start","kind":"trigger","name":"Start"},
                 {"id":"call","kind":"http_request","name":"Call",
                  "config":{"url":"http://attacker.example/x","method":"GET"}}],
        "edges":[{"from":"start","to":"call"}]}}"#;
    let (_home, runtime) = runtime_with(ScriptedModel::replying(reply)).await;
    runtime
        .tasks()
        .upsert(runtime.id(), &card("t-bounce", None))
        .await
        .unwrap();
    let run_id = open_run(&runtime, "t-bounce").await;

    run_workflow_build_pass(
        Arc::clone(&runtime),
        "t-bounce".to_string(),
        Some(run_id.clone()),
    )
    .await;

    let after = read(&runtime, "t-bounce").await;
    assert_eq!(after.column, COLUMN_TODO);
    assert_eq!(run_status(&runtime, &run_id).await, RunStatus::Failed);
    let bounced = after
        .bounced
        .expect("a builder pass that failed and landed the card on To-do must set the bounce chip");
    assert!(
        bounced.contains("http_request"),
        "the bounce reason carries the same failure text as the card note: {bounced}"
    );
}

/// Issue #1865 (CodeRabbit review, PR #1883): a builder failure landing on
/// To-do must file the same `dispatch_failed` notification every other
/// bounced-dispatch path does — `CompanyRuntime::abandon_run`, the cycle's
/// terminality backstop, and the boot reaper's card sweep, all via
/// `advance::notify_dispatch_failed`. Unlike those crash-recovery paths, and
/// unlike `brain.rs`'s `refuse_dispatch` (which can relay a reply into the
/// card's origin chat), `settle_to_todo` has no other operator-facing signal
/// off the board — before this fix, a builder-pass failure was the one
/// bounced-dispatch path the notification feed never badged.
///
/// Reuses the same out-of-vocabulary-domain scenario as the sibling bounce-chip
/// test above, which already drives a real `settle_to_todo` call, and checks
/// the notification store instead of the card.
#[tokio::test]
async fn a_builder_failure_settling_to_todo_files_a_dispatch_failed_notification() {
    let reply = r#"{"automatable":true,"summary":"call a url","workflow":{"name":"Reach out",
        "nodes":[{"id":"start","kind":"trigger","name":"Start"},
                 {"id":"call","kind":"http_request","name":"Call",
                  "config":{"url":"http://attacker.example/x","method":"GET"}}],
        "edges":[{"from":"start","to":"call"}]}}"#;
    let (_home, runtime) = runtime_with(ScriptedModel::replying(reply)).await;
    runtime
        .tasks()
        .upsert(runtime.id(), &card("t-notify", None))
        .await
        .unwrap();
    let run_id = open_run(&runtime, "t-notify").await;

    run_workflow_build_pass(
        Arc::clone(&runtime),
        "t-notify".to_string(),
        Some(run_id.clone()),
    )
    .await;

    let after = read(&runtime, "t-notify").await;
    assert_eq!(after.column, COLUMN_TODO);

    let notifications = runtime
        .notifications()
        .list(runtime.id(), "owner")
        .await
        .unwrap();
    assert!(
        notifications
            .iter()
            .any(|n| n.notification.kind == "dispatch_failed"
                && n.notification.subject.id == "t-notify"),
        "a builder pass that failed and bounced its card must file a \
         dispatch_failed notification, got {notifications:?}"
    );
}

/// **The #1191 regression, at the builder.** The model routes the report to a
/// channel this runtime cannot deliver to — the shape the QA pass found, where
/// the builder appended `-desk` to a desk's display name.
///
/// Courtesy validation now runs the channel rule (it could not before: the rule
/// lived on the write routes and the builder does not go through them), so the
/// card settles back to To-do with the reason instead of reaching In Review
/// carrying a proposal that Apply would persist and the editor would then
/// refuse. The reason names the channels that WOULD work, so the next pass — and
/// the operator reading the card — can see the fix without a second lookup.
#[tokio::test]
async fn a_proposal_naming_an_unwired_channel_settles_to_todo() {
    let reply = r#"{"automatable":true,"summary":"post the digest","workflow":{"name":"Weekly digest",
        "nodes":[{"id":"start","kind":"trigger","name":"Start","schedule":"0 17 * * 5"},
                 {"id":"draft","kind":"agent","name":"Draft","agent":"maya"},
                 {"id":"post_summary","kind":"output","name":"Post to engineering desk",
                  "destination":{"kind":"channel","target":"engineering-desk"}}],
        "edges":[{"from":"start","to":"draft"},{"from":"draft","to":"post_summary"}]}}"#;
    let (_home, runtime) = runtime_with_desk(ScriptedModel::replying(reply)).await;
    runtime
        .tasks()
        .upsert(runtime.id(), &card("t-1191", None))
        .await
        .unwrap();
    let run_id = open_run(&runtime, "t-1191").await;

    run_workflow_build_pass(
        Arc::clone(&runtime),
        "t-1191".to_string(),
        Some(run_id.clone()),
    )
    .await;

    let after = read(&runtime, "t-1191").await;
    assert_eq!(after.column, COLUMN_TODO);
    assert!(
        after.workflow_proposal.is_none(),
        "a graph the editor would refuse must not reach In Review"
    );
    let note = after.note.unwrap_or_default();
    assert!(
        note.contains("is not an automation delivery channel"),
        "the destination problem is named on the card: {note}"
    );
    assert!(
        note.contains("engineering"),
        "the wired ids ride along so the fix is legible: {note}"
    );
    assert_eq!(run_status(&runtime, &run_id).await, RunStatus::Failed);
}

/// The other half of #1191: the model is *told* the channel ids, so it has
/// something to copy instead of inventing one from a desk's display name.
///
/// The evidence pack had a roster section and a tools section and no channel
/// section at all; `graph_contract` names the concept without listing ids.
#[tokio::test]
async fn the_card_prompt_grounds_the_wired_channels() {
    let (_home, runtime) = runtime_with_desk(ScriptedModel::replying(VALID_GRAPH)).await;
    let evidence = gather_evidence(&runtime, &card("t-ground", None))
        .await
        .expect("evidence");
    // Since issue #1757 the always-present Operator channel is a durable delivery
    // target too, so it grounds alongside the desk channel.
    assert_eq!(
        evidence.wired_channels,
        vec!["operator".to_string(), "engineering".to_string()]
    );

    let prompt = evidence_prompt(&evidence);
    assert!(prompt.contains("## Channels"), "{prompt}");
    assert!(prompt.contains("`engineering`"), "{prompt}");
    assert!(prompt.contains("`operator`"), "{prompt}");
    assert!(
        prompt.contains("copied exactly"),
        "the section must say the id is copied, not paraphrased: {prompt}"
    );
}

/// The create-time copilot's prompt grounds the same set through the shared
/// `render_grounding_sections`, so a graph drafted from an operator's sentence
/// and one built from a card name channels from one list.
#[tokio::test]
async fn the_description_prompt_grounds_the_wired_channels() {
    let (_home, runtime) = runtime_with_desk(ScriptedModel::replying(DESC_GRAPH)).await;
    let evidence = gather_company_evidence(&runtime).await.expect("evidence");
    let prompt = description_evidence_prompt(&evidence, &[], &[], "post the weekly digest");
    assert!(prompt.contains("## Channels"), "{prompt}");
    assert!(prompt.contains("`engineering`"), "{prompt}");
    assert!(prompt.contains("`operator`"), "{prompt}");
}

/// A company with no desk and no provider channel still has the always-present
/// Operator channel (issue #1757), so the Channels section grounds on it rather
/// than the empty-set fallback — every company can deliver *somewhere* now.
#[tokio::test]
async fn a_company_with_no_desks_still_grounds_on_the_operator_channel() {
    let (_home, runtime) = runtime_with(ScriptedModel::replying(VALID_GRAPH)).await;
    let evidence = gather_evidence(&runtime, &card("t-empty", None))
        .await
        .expect("evidence");
    assert_eq!(evidence.wired_channels, vec!["operator".to_string()]);

    let prompt = evidence_prompt(&evidence);
    assert!(prompt.contains("## Channels"), "{prompt}");
    assert!(prompt.contains("`operator`"), "{prompt}");
}

/// The empty-set fallback message still renders for a truly channel-less set —
/// unreachable from a live runtime now (every company has `operator`), but the
/// pure section renderer must still speak honestly when handed nothing.
#[test]
fn an_empty_channel_slice_renders_the_honest_fallback() {
    let mut out = String::new();
    super::render_channel_section(&mut out, &[]);
    assert!(out.contains("## Channels"), "{out}");
    assert!(out.contains("no channels are wired"), "{out}");
}

/// The model does not get a vote on approval gating: whatever `requires_approval`
/// it emits — `true` on one node, `false` on another — the host strips before the
/// proposal is stored, so a builder-authored node inherits the platform default
/// (#460) rather than the model's choice. Both are dropped in the stored `ops`.
#[tokio::test]
async fn the_host_strips_model_chosen_approval_gating() {
    let reply = r#"{"automatable":true,"summary":"gated","workflow":{"name":"Gated",
        "nodes":[{"id":"start","kind":"trigger","name":"Start","requires_approval":true},
                 {"id":"draft","kind":"agent","name":"Draft","agent":"maya","requires_approval":false}],
        "edges":[{"from":"start","to":"draft"}]}}"#;
    let (_home, runtime) = runtime_with(ScriptedModel::replying(reply)).await;
    runtime
        .tasks()
        .upsert(runtime.id(), &card("t-approval", None))
        .await
        .unwrap();
    let run_id = open_run(&runtime, "t-approval").await;

    run_workflow_build_pass(
        Arc::clone(&runtime),
        "t-approval".to_string(),
        Some(run_id.clone()),
    )
    .await;

    let after = read(&runtime, "t-approval").await;
    assert_eq!(after.column, COLUMN_IN_REVIEW);
    let spec: WorkflowGraphSpec =
        serde_json::from_value(after.workflow_proposal.expect("proposal").ops).unwrap();
    assert!(
        spec.nodes.iter().all(|n| n.requires_approval.is_none()),
        "the host drops the model's requires_approval on every node (true and false alike)"
    );
    assert_eq!(run_status(&runtime, &run_id).await, RunStatus::Succeeded);
}

/// An operator moving the card out from under the pass wins: the pass discards
/// its result (no proposal, the card stays where the operator put it) and the
/// attempt settles Cancelled — the tokens stay metered because they were spent.
#[tokio::test]
async fn an_operator_move_mid_build_is_discarded() {
    let model = ScriptedModel::replying(VALID_GRAPH);
    let (_home, runtime) = runtime_with(Arc::clone(&model)).await;
    runtime
        .tasks()
        .upsert(runtime.id(), &card("t-7", None))
        .await
        .unwrap();
    let run_id = open_run(&runtime, "t-7").await;
    // The model moves the card to To-do while it "thinks".
    *model.move_card.lock().unwrap() = Some((Arc::downgrade(&runtime), "t-7".to_string()));

    run_workflow_build_pass(
        Arc::clone(&runtime),
        "t-7".to_string(),
        Some(run_id.clone()),
    )
    .await;

    let after = read(&runtime, "t-7").await;
    assert_eq!(after.column, COLUMN_TODO, "the operator's move wins");
    assert!(
        after.workflow_proposal.is_none(),
        "the pass's result is discarded"
    );
    assert_eq!(run_status(&runtime, &run_id).await, RunStatus::Cancelled);
}
