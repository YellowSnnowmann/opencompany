use std::sync::Arc;

use serde_json::json;

use super::workflow_build_fixtures_tests::*;
use super::workflow_build_shared_tests::*;
use super::*;

// ---------------------------------------------------------------------------
// Fix a failed run with the copilot (issue #840, PR-3)
// ---------------------------------------------------------------------------

/// The saved graph a run failed on: a scheduled trigger into a `tool_call` on a
/// slug this fixture never wired (`web_search`), which is what the run failed on.
fn failing_spec() -> WorkflowGraphSpec {
    serde_json::from_value(serde_json::json!({
        "id": "weekly-digest",
        "name": "Weekly digest",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "Every Monday", "schedule": "0 9 * * 1" },
            { "id": "search", "kind": "tool_call", "name": "Search", "config": { "slug": "web_search" } }
        ],
        "edges": [{ "from": "t", "to": "search" }]
    }))
    .unwrap()
}

/// A `RunFailureContext` naming the failing node, as the route assembles it from
/// the journal.
fn failure_ctx() -> RunFailureContext {
    RunFailureContext {
        run_id: "dead-run-1".to_string(),
        error: "the tool `web_search` is not wired on this deployment".to_string(),
        failed_node_id: Some("search".to_string()),
        failed_node_name: Some("Search".to_string()),
    }
}

/// The fix path corrects the failing graph AND preserves its identity: the agent
/// drops the unwired tool step and the host pins the corrected spec's id/name to
/// the saved workflow — NOT the deduped `-2` the create path would mint — so the
/// operator's Save is a new VERSION of that workflow, not an orphan.
#[tokio::test]
async fn a_failure_fix_corrects_the_graph_and_preserves_identity() {
    // The corrected graph the agent proposes: the same workflow, the unwired tool
    // step replaced by the roster agent doing the work.
    let corrected = json!({
        "name": "Weekly digest",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "Every Monday", "schedule": "0 9 * * 1" },
            { "id": "draft", "kind": "agent", "name": "Draft", "agent": "maya" }
        ],
        "edges": [{ "from": "t", "to": "draft" }]
    });
    let model = NativeCopilotModel::scripting(vec![
        propose_step("dropped the unwired search step", corrected),
        NativeStep::done("Corrected the workflow."),
    ]);
    let (_home, runtime) = runtime_with_agent(model, None).await;
    // Seed the SAME id/name, so the create path WOULD dedup to `weekly-digest-2`;
    // the fix path must not.
    seed_workflow(&runtime, "weekly-digest", "Weekly digest").await;

    let outcome = fix_workflow_from_failure(&runtime, &failing_spec(), &failure_ctx())
        .await
        .expect("the fixer runs");
    match outcome {
        DescriptionDraftOutcome::Graph { spec, .. } => {
            assert_eq!(
                spec.id, "weekly-digest",
                "the fix preserves the workflow id"
            );
            assert_eq!(spec.name, "Weekly digest", "and its display name");
            assert!(
                spec.nodes.iter().all(|n| n.kind != "tool_call"),
                "the unwired tool step is gone from the correction"
            );
        }
        DescriptionDraftOutcome::NotAutomatable(reason) => panic!("expected a graph: {reason}"),
    }
}

/// **Regression, issue #1882 review (PR #1882 bot finding, comment 3879878907).**
/// A workflow whose owning desk has since been deleted is still correctable: the
/// stale desk rides through the correction as an UNCHANGED value, so the
/// courtesy pre-flight grandfathers it exactly as the edit route would, instead
/// of refusing the whole correction over a field the operator never touched.
///
/// RED-FIRST: pre-fix `courtesy_validate_draft` always validated the corrected
/// graph as a fresh create, so the echoed `ghost-desk` was a hard refusal and
/// the fixer folded to not-automatable — an unrelated stale owner blocked the
/// repair of the actual run failure.
#[tokio::test]
async fn a_failure_fix_survives_an_owning_desk_that_no_longer_exists() {
    // The model does what the prompt asks and echoes back the fields it did not
    // change — including the `ownerDesk` it saw on the failing graph.
    let corrected = json!({
        "name": "Weekly digest",
        "ownerDesk": "ghost-desk",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "Every Monday", "schedule": "0 9 * * 1" },
            { "id": "draft", "kind": "agent", "name": "Draft", "agent": "maya" }
        ],
        "edges": [{ "from": "t", "to": "draft" }]
    });
    let model = NativeCopilotModel::scripting(vec![
        propose_step("dropped the unwired search step", corrected),
        NativeStep::done("Corrected the workflow."),
    ]);
    let (_home, runtime) = runtime_with_agent(model, None).await;
    seed_workflow(&runtime, "weekly-digest", "Weekly digest").await;

    let mut failing = failing_spec();
    // No such desk on this company — the owning desk was deleted after the
    // workflow was saved.
    failing.owner_desk = Some("ghost-desk".to_string());

    let outcome = fix_workflow_from_failure(&runtime, &failing, &failure_ctx())
        .await
        .expect("the fixer runs");
    match outcome {
        DescriptionDraftOutcome::Graph { spec, .. } => {
            assert_eq!(
                spec.owner_desk.as_deref(),
                Some("ghost-desk"),
                "the stale owner is carried through the correction, not silently rewritten"
            );
            assert!(
                spec.nodes.iter().all(|n| n.kind != "tool_call"),
                "and the actual run failure is still corrected"
            );
        }
        DescriptionDraftOutcome::NotAutomatable(reason) => {
            panic!("a stale owning desk must not block the correction: {reason}")
        }
    }
}

/// The other half of host authority over `ownerDesk` on the fix path (issue
/// #1882 review): neither copilot tool advertises the field, so a model that
/// simply omits it from its correction — the common case — must not silently
/// UNASSIGN the workflow. The host pins the saved desk back on, the same way it
/// pins the saved id and name.
///
/// RED-FIRST: pre-fix `FixTarget` carried no desk and nothing re-applied it, so
/// the corrected spec came back with `owner_desk: None`.
#[tokio::test]
async fn a_failure_fix_keeps_the_saved_owner_desk_when_the_model_drops_it() {
    let corrected = json!({
        "name": "Weekly digest",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "Every Monday", "schedule": "0 9 * * 1" },
            { "id": "draft", "kind": "agent", "name": "Draft", "agent": "maya" }
        ],
        "edges": [{ "from": "t", "to": "draft" }]
    });
    let model = NativeCopilotModel::scripting(vec![
        propose_step("dropped the unwired search step", corrected),
        NativeStep::done("Corrected the workflow."),
    ]);
    let (_home, runtime) = runtime_with_agent(model, None).await;
    seed_workflow(&runtime, "weekly-digest", "Weekly digest").await;

    let mut failing = failing_spec();
    failing.owner_desk = Some("ghost-desk".to_string());

    let outcome = fix_workflow_from_failure(&runtime, &failing, &failure_ctx())
        .await
        .expect("the fixer runs");
    match outcome {
        DescriptionDraftOutcome::Graph { spec, .. } => assert_eq!(
            spec.owner_desk.as_deref(),
            Some("ghost-desk"),
            "an omitted ownerDesk restores the saved one rather than unassigning it"
        ),
        DescriptionDraftOutcome::NotAutomatable(reason) => panic!("expected a graph: {reason}"),
    }
}

/// A correction that keeps failing a host gate — an `agent` node naming a teammate
/// not on the roster — is never accepted, so the fixer folds to not-automatable
/// naming the gate. The SAME gate pipeline the create path runs, not a bypass.
#[tokio::test]
async fn a_fix_that_cannot_be_rewired_folds_to_not_automatable() {
    let ghosted = json!({
        "name": "Weekly digest",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "Start" },
            { "id": "a", "kind": "agent", "name": "Do", "agent": "ghost" }
        ],
        "edges": [{ "from": "t", "to": "a" }]
    });
    let model = NativeCopilotModel::scripting(vec![
        propose_step("fix it", ghosted),
        NativeStep::done("I could not fix this with the teammates available."),
    ]);
    let (_home, runtime) = runtime_with_agent(model, None).await;

    let outcome = fix_workflow_from_failure(&runtime, &failing_spec(), &failure_ctx())
        .await
        .expect("the fixer runs");
    match outcome {
        DescriptionDraftOutcome::NotAutomatable(reason) => {
            assert!(
                reason.contains("maya") || reason.contains("could not be drafted"),
                "the gate reason is carried: {reason}"
            );
        }
        DescriptionDraftOutcome::Graph { .. } => panic!("a gate-failing fix must not be accepted"),
    }
}

/// The fix turn is metered like any priced turn: a FRESH run id (never the dead
/// run's) under the `workflow:copilot` sentinel, carrying the backend-charged cost.
#[tokio::test]
async fn a_fix_turn_meters_a_fresh_run_id_under_the_copilot_sentinel() {
    let corrected = json!({
        "name": "Weekly digest",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "Every Monday", "schedule": "0 9 * * 1" },
            { "id": "draft", "kind": "agent", "name": "Draft", "agent": "maya" }
        ],
        "edges": [{ "from": "t", "to": "draft" }]
    });
    let model = NativeCopilotModel::scripting(vec![
        propose_step("corrected", corrected),
        NativeStep::done("done"),
    ])
    .with_charge(120, 40, 0.0031);
    let meter = Arc::new(RecordingUsageMeter::default());
    let (_home, runtime) = runtime_with_agent(model, Some(meter.clone())).await;

    let failure = failure_ctx();
    let outcome = fix_workflow_from_failure(&runtime, &failing_spec(), &failure)
        .await
        .expect("the fixer runs");
    assert!(matches!(outcome, DescriptionDraftOutcome::Graph { .. }));

    let samples = meter.samples();
    assert_eq!(samples.len(), 1, "one metered sample: {samples:?}");
    assert_eq!(
        samples[0].agent, COPILOT_AGENT,
        "the fix spend is metered under the copilot sentinel"
    );
    assert!(
        samples[0].cost_usd > 0.0,
        "a charged fix pins a non-zero cost"
    );
    assert_ne!(
        samples[0].run_id.as_deref(),
        Some(failure.run_id.as_str()),
        "metering mints a FRESH run id, never reusing the dead run's"
    );
}

/// The fix prompt states the failing graph, the run's error and failing node, and
/// still renders the same roster grounding the create-time draft does — so a
/// corrected `agent`/`tool_call` node is grounded on the same real ids.
#[tokio::test]
async fn the_fix_prompt_states_the_failing_graph_and_failure() {
    let (_home, runtime) = runtime_with(ScriptedModel::replying(VALID_GRAPH)).await;
    let company = gather_company_evidence(&runtime).await.unwrap();
    let prompt = fix_evidence_prompt(
        &company,
        &["web_fetch".to_string()],
        &[],
        &failing_spec(),
        &failure_ctx(),
    );
    assert!(prompt.contains("Correct this saved workflow"), "{prompt}");
    assert!(prompt.contains("web_search"), "the failing graph is shown");
    assert!(
        prompt.contains("not wired on this deployment"),
        "the error is stated: {prompt}"
    );
    assert!(prompt.contains("Search"), "the failing node is named");
    assert!(
        prompt.contains("dead-run-1"),
        "the dead run id is provenance"
    );
    assert!(prompt.contains("`maya`"), "roster grounding is present");
}

/// Static readiness flags a graph's remaining authoring smells — an envelope-null
/// binding that resolves to null at run time — but NEVER blocks: a clean graph is
/// `ok`, a smelly one is not-`ok` with named advisories, and both still return.
#[test]
fn workflow_readiness_flags_gate_smells_but_never_blocks() {
    let clean: WorkflowGraphSpec = serde_json::from_value(json!({
        "id": "w", "name": "W",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "Start" },
            { "id": "a", "kind": "agent", "name": "Draft", "agent": "maya" }
        ],
        "edges": [{ "from": "t", "to": "a" }]
    }))
    .unwrap();
    let (ok, advisories) = workflow_readiness(&clean);
    assert!(
        ok && advisories.is_empty(),
        "a clean graph is ok: {advisories:?}"
    );

    // `b` reads `=nodes.a.item.summary` from agent `a`, whose output is enveloped —
    // the `.json` is missing, so it resolves to null. `gates::failures` catches it.
    let smelly: WorkflowGraphSpec = serde_json::from_value(json!({
        "id": "w", "name": "W",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "Start" },
            { "id": "a", "kind": "agent", "name": "Draft", "agent": "maya" },
            { "id": "b", "kind": "agent", "name": "Polish", "agent": "maya",
              "config": { "input": "=nodes.a.item.summary" } }
        ],
        "edges": [{ "from": "t", "to": "a" }, { "from": "a", "to": "b" }]
    }))
    .unwrap();
    let (ok, advisories) = workflow_readiness(&smelly);
    assert!(!ok, "the envelope-null binding is flagged as not-ready");
    assert!(!advisories.is_empty(), "and named for the operator");
}
