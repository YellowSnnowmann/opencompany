use std::sync::Arc;

use serde_json::json;

use super::workflow_build_fixtures_tests::*;
use super::workflow_build_shared_tests::*;
use super::*;
use crate::ports::runs::RunStatus;

// ---------------------------------------------------------------------------
// Grounding & gates (issue #813) — unit tier over the pure helpers
// ---------------------------------------------------------------------------

/// A roster teammate for the pure-helper units.
fn roster_entry(id: &str, role: &str, name: Option<&str>) -> RosterEntry {
    RosterEntry {
        id: id.to_string(),
        role: role.to_string(),
        name: name.map(str::to_string),
        description: None,
        global: false,
    }
}

/// A `WorkflowGraphSpec` from a JSON literal.
fn spec_from(value: serde_json::Value) -> WorkflowGraphSpec {
    serde_json::from_value(value).expect("the spec parses")
}

/// A global-baseline roster teammate for the local-vs-global precedence tests.
fn global_roster_entry(id: &str, role: &str, name: Option<&str>) -> RosterEntry {
    RosterEntry {
        global: true,
        ..roster_entry(id, role, name)
    }
}

/// The normalizer collapses `-`, `_` and whitespace runs so a role, an id and a
/// name spelled three ways compare equal — and nothing fuzzier.
#[test]
fn normalize_label_collapses_separators_only() {
    assert_eq!(normalize_label("QA Engineer"), "qa engineer");
    assert_eq!(normalize_label("qa_engineer"), "qa engineer");
    assert_eq!(normalize_label("  Qa--Engineer  "), "qa engineer");
    // Different words never collapse together.
    assert_ne!(normalize_label("writer"), normalize_label("rewriter"));
}

/// Delivery detection stays conservative: a request to deliver to the operator
/// or a concrete target is a signal; the business activity "we email customers"
/// is not (no address, no #channel, no verb aimed at the operator).
#[test]
fn delivery_signals_are_conservative() {
    assert!(!delivery_signals("email me the digest every monday").is_empty());
    assert!(!delivery_signals("post the summary to #ops").is_empty());
    assert!(!delivery_signals("send the report to jo@acme.com").is_empty());
    assert!(delivery_signals("we email customers a weekly newsletter").is_empty());
    assert!(delivery_signals("summarize the week's work").is_empty());
    // A numeric ticket/issue reference is not a #channel (leading-digit guard).
    assert!(delivery_signals("summarize ticket #4521 each friday").is_empty());
    // "send used" is not the whole word "send us" (whole-word verb match).
    assert!(delivery_signals("send used parts to the warehouse").is_empty());
}

/// (a) The resolver rewrites a role-named agent to its roster id and records a
/// note; the rewrite is exact-normalized, never fuzzy.
#[test]
fn the_resolver_rewrites_a_role_named_agent_and_notes_it() {
    let roster = vec![roster_entry("qa_engineer", "QA Engineer", None)];
    let mut spec = spec_from(json!({
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "T" },
            { "id": "a", "kind": "agent", "name": "Test it", "agent": "QA Engineer" }
        ],
        "edges": []
    }));
    let mut notes = Vec::new();
    let mut errors = Vec::new();
    resolve_agent_ids(&mut spec, &roster, &mut notes, &mut errors);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(spec.nodes[1].agent.as_deref(), Some("qa_engineer"));
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("qa_engineer"), "{notes:?}");
}

/// (a) An agent id that matches nothing on the roster is a gate error that NAMES
/// the roster ids — proving the old silent fold (issue #813) is dead. An already
/// valid id is left untouched.
#[test]
fn the_resolver_names_the_roster_on_an_unknown_agent() {
    let roster = vec![
        roster_entry("qa_engineer", "QA Engineer", None),
        roster_entry("ceo", "Chief Executive", None),
    ];
    let mut spec = spec_from(json!({
        "nodes": [
            { "id": "a", "kind": "agent", "name": "X", "agent": "019fcbc3cb55-nope" },
            { "id": "b", "kind": "agent", "name": "Y", "agent": "ceo" }
        ],
        "edges": []
    }));
    let mut notes = Vec::new();
    let mut errors = Vec::new();
    resolve_agent_ids(&mut spec, &roster, &mut notes, &mut errors);
    assert_eq!(errors.len(), 1, "only the unknown agent errors: {errors:?}");
    assert!(
        errors[0].contains("qa_engineer") && errors[0].contains("ceo"),
        "the roster is named so the model can self-correct: {errors:?}"
    );
    // The already-valid `ceo` node is untouched.
    assert_eq!(spec.nodes[1].agent.as_deref(), Some("ceo"));
}

/// (a) A company's own teammate wins a label collision against a global of the
/// same normalized role — the baseline ships a `writer`/`researcher`, and a
/// vertical that names its own "Writer" must still resolve to its own, not the
/// global one, or "the writer" would become unaddressable in every company
/// that has its own.
#[test]
fn the_resolver_prefers_the_local_agent_over_a_same_label_global() {
    let roster = vec![
        global_roster_entry("writer", "Writer", None),
        roster_entry("copy_lead", "Writer", None),
    ];
    let mut spec = spec_from(json!({
        "nodes": [
            { "id": "a", "kind": "agent", "name": "Draft", "agent": "Writer" }
        ],
        "edges": []
    }));
    let mut notes = Vec::new();
    let mut errors = Vec::new();
    resolve_agent_ids(&mut spec, &roster, &mut notes, &mut errors);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        spec.nodes[0].agent.as_deref(),
        Some("copy_lead"),
        "the company's own teammate must win, not the global"
    );
}

/// (a) Two LOCAL teammates sharing a label stays a genuine, reported ambiguity
/// — the local-over-global tie-break only resolves a local-vs-global
/// collision, never a local-vs-local one, since there is no meaningful
/// precedence between two teammates the company itself declared.
#[test]
fn two_local_agents_sharing_a_label_stay_ambiguous() {
    let roster = vec![
        roster_entry("writer_a", "Writer", None),
        roster_entry("writer_b", "Writer", None),
    ];
    let mut spec = spec_from(json!({
        "nodes": [
            { "id": "a", "kind": "agent", "name": "Draft", "agent": "Writer" }
        ],
        "edges": []
    }));
    let mut notes = Vec::new();
    let mut errors = Vec::new();
    resolve_agent_ids(&mut spec, &roster, &mut notes, &mut errors);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("matches more than one teammate"),
        "{errors:?}"
    );
    assert_eq!(spec.nodes[0].agent.as_deref(), Some("Writer"), "unresolved");
}

/// (a) A label that only a global teammate answers to still resolves — the
/// local-preference tie-break only narrows a multi-hit collision, it never
/// drops a global-only match down to zero hits.
#[test]
fn a_global_only_label_still_resolves() {
    let roster = vec![
        global_roster_entry("researcher", "Researcher", None),
        roster_entry("ceo", "Chief Executive", None),
    ];
    let mut spec = spec_from(json!({
        "nodes": [
            { "id": "a", "kind": "agent", "name": "Dig in", "agent": "Researcher" }
        ],
        "edges": []
    }));
    let mut notes = Vec::new();
    let mut errors = Vec::new();
    resolve_agent_ids(&mut spec, &roster, &mut notes, &mut errors);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(spec.nodes[0].agent.as_deref(), Some("researcher"));
}

/// (b) The delivery gate fires when a delivery is asked for and no `output` node
/// carries a destination; it is silent with one, and silent absent a signal.
#[test]
fn the_delivery_gate_fires_only_when_delivery_is_asked_and_missing() {
    let no_output = spec_from(json!({
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "T" },
            { "id": "a", "kind": "agent", "name": "Draft", "agent": "maya" }
        ],
        "edges": []
    }));
    let mut fired = Vec::new();
    delivery_gate(&no_output, "email me the digest", &mut fired);
    assert_eq!(fired.len(), 1, "{fired:?}");
    assert!(fired[0].contains("output"), "{fired:?}");

    let with_output = spec_from(json!({
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "T" },
            { "id": "o", "kind": "output", "name": "Send", "destination": { "kind": "owner" } }
        ],
        "edges": []
    }));
    let mut satisfied = Vec::new();
    delivery_gate(&with_output, "email me the digest", &mut satisfied);
    assert!(satisfied.is_empty(), "{satisfied:?}");

    let mut no_signal = Vec::new();
    delivery_gate(&no_output, "summarize the week", &mut no_signal);
    assert!(no_signal.is_empty(), "{no_signal:?}");
}

/// (c) The wrong-but-real arm flags a draft that uses a real teammate the request
/// did not name while ignoring the one it did; it is silent when the draft uses
/// the named teammate.
#[test]
fn the_wrong_but_real_arm_flags_the_unnamed_teammate() {
    let roster = vec![
        roster_entry("qa_engineer", "QA Engineer", None),
        roster_entry("ceo", "Chief Executive", None),
    ];
    let uses_ceo = spec_from(json!({
        "nodes": [{ "id": "a", "kind": "agent", "name": "Do it", "agent": "ceo" }],
        "edges": []
    }));
    let mut errors = Vec::new();
    wrong_but_real_agent_gate(
        &uses_ceo,
        "have the qa engineer run the tests",
        &roster,
        &mut errors,
    );
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("qa_engineer"), "{errors:?}");

    let uses_qa = spec_from(json!({
        "nodes": [{ "id": "a", "kind": "agent", "name": "Do it", "agent": "qa_engineer" }],
        "edges": []
    }));
    let mut ok = Vec::new();
    wrong_but_real_agent_gate(
        &uses_qa,
        "have the qa engineer run the tests",
        &roster,
        &mut ok,
    );
    assert!(ok.is_empty(), "{ok:?}");
}

/// The card entering the pass is not the assignee's dispatch: no artifact, no
/// delegation — only a proposal or a return. A `once` deliverable is never routed
/// here, which the runtime's dispatch branch enforces; this pins that the builder
/// itself refuses a card that is not a `workflow` card even if called directly
/// (a rebuild race, or a card flipped back to once mid-flight).
#[tokio::test]
async fn a_once_card_is_not_built() {
    let model = ScriptedModel::replying(VALID_GRAPH);
    let (_home, runtime) = runtime_with(Arc::clone(&model)).await;
    let mut once = card("t-8", None);
    once.deliverable = TaskDeliverable::Once;
    runtime.tasks().upsert(runtime.id(), &once).await.unwrap();
    let run_id = open_run(&runtime, "t-8").await;

    run_workflow_build_pass(
        Arc::clone(&runtime),
        "t-8".to_string(),
        Some(run_id.clone()),
    )
    .await;

    let after = read(&runtime, "t-8").await;
    assert!(after.workflow_proposal.is_none());
    assert_eq!(
        after.column, COLUMN_IN_PROGRESS,
        "the card is left untouched"
    );
    assert_eq!(model.calls(), 0, "no model call for a non-workflow card");
    assert_eq!(run_status(&runtime, &run_id).await, RunStatus::Cancelled);
}
