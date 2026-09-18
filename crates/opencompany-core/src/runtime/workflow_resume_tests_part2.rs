use super::tests_core::*;

/// A cancel abandons the work, so neither the writer nor the reader will
/// carry one into a run.
#[test]
fn a_cancelled_answer_is_refused_at_both_ends() {
    use crate::ports::blockers::BlockerVerdict;
    assert!(
        blocker_continuation_input(
            serde_json::json!({ "request": "x" }),
            "draft",
            &resolution(BlockerVerdict::Cancel, ""),
        )
        .is_err(),
        "a cancel builds no continuation input"
    );
    let smuggled = serde_json::json!({
        CONTINUATION_BLOCKER_KEY: [{ "node": "draft", "verdict": "cancel" }]
    });
    assert!(
        blocker_answer_for(&smuggled, "draft").is_err(),
        "a node reached carrying a cancel must stop, not carry on"
    );
}

/// The key describes what was already decided, not what is being decided,
/// so it must not split one gate into two cards on a continuation.
#[test]
fn an_answered_blocker_is_not_part_of_a_gates_identity() {
    use crate::ports::blockers::BlockerVerdict;
    let input = serde_json::json!({ "request": "x" });
    let paused = gate_effect("digest", "gate", &input, "run-1", &[], &[], None);
    let resumed_input =
        blocker_continuation_input(input, "draft", &resolution(BlockerVerdict::Retry, ""))
            .expect("continues");
    let resumed = gate_effect("digest", "gate", &resumed_input, "run-2", &[], &[], None);
    assert!(
        is_same_gate(&paused, &resumed),
        "one gate, one decision, however many blockers the lineage answered"
    );
}

/// The ledger must not make a continuation's gate look like a *different*
/// decision — that would stack a second card for one gate on every resume,
/// which is the dedupe failure #395 closed.
#[test]
fn the_ledger_does_not_split_one_decision_into_two_cards() {
    let paused = gate_effect(
        "digest",
        "gate",
        &serde_json::json!({ "request": "x" }),
        "run-1",
        &[delivery("summary", "owner", DeliveryStatus::Sent)],
        &[],
        None,
    );
    // The same gate, re-reached by the continuation the card started: same
    // input plus the approval… minus the approval, which the gate node
    // consumed. What differs is the ledger key alone.
    let mut continuation = single_continuation_input(&paused).expect("continues");
    continuation
        .as_object_mut()
        .expect("object")
        .remove("approvals");
    let re_reached = gate_effect("digest", "gate", &continuation, "run-2", &[], &[], None);

    assert!(
        is_same_gate(&paused, &re_reached),
        "the ledger is not part of the decision:\n{:?}\n{:?}",
        paused.payload,
        re_reached.payload
    );
}

/// The parked gate carries the verbatim output of the nodes feeding it — the
/// content awaiting sign-off — keyed by upstream node id.
#[test]
fn a_parked_gate_carries_its_upstream_content() {
    let output = serde_json::json!({
        "nodes": {
            "writer": { "items": [{ "text": "the draft tweet" }] },
            "unrelated": { "items": ["not upstream of the gate"] },
        }
    });
    let edges = [edge("start", "writer"), edge("writer", "publish")];

    let mut effect = gate_effect("wf", "publish", &Value::Null, "run-1", &[], &[], None);
    attach_upstream_content(&mut effect, &output, &edges, "publish");

    let content = &effect.payload[PAYLOAD_CONTENT];
    assert_eq!(
        content["writer"]["items"][0]["text"], "the draft tweet",
        "the gate's upstream node output must ride the card: {content}"
    );
    assert!(
        content.get("unrelated").is_none(),
        "a node that does not feed the gate must not be previewed: {content}"
    );
}

/// A gate whose upstream produced nothing (or a graph with no such edge) gets
/// an empty preview rather than a missing key — the console renders "no
/// content".
#[test]
fn a_gate_with_no_upstream_output_gets_an_empty_preview() {
    let output = serde_json::json!({ "nodes": {} });
    let edges = [edge("writer", "publish")];
    let mut effect = gate_effect("wf", "publish", &Value::Null, "run-1", &[], &[], None);
    attach_upstream_content(&mut effect, &output, &edges, "publish");
    assert_eq!(effect.payload[PAYLOAD_CONTENT], serde_json::json!({}));
}

/// The content preview is NOT part of the gate's decision identity: two parks
/// that differ only in the upstream content their nodes produced are still one
/// decision on one gate, and must dedupe to a single card. Without this a
/// workflow whose upstream text changes each run would stack a fresh card
/// every time — the rubber-stamp failure #395 closed, re-opened by #596.
#[test]
fn two_parks_differing_only_in_content_still_dedupe() {
    let edges = [edge("writer", "publish")];
    let input = serde_json::json!({ "request": "x" });

    let mut a = gate_effect("wf", "publish", &input, "run-1", &[], &[], None);
    attach_upstream_content(
        &mut a,
        &serde_json::json!({ "nodes": { "writer": { "items": ["draft one"] } } }),
        &edges,
        "publish",
    );

    let mut b = gate_effect("wf", "publish", &input, "run-2", &[], &[], None);
    attach_upstream_content(
        &mut b,
        &serde_json::json!({ "nodes": { "writer": { "items": ["a totally different draft"] } } }),
        &edges,
        "publish",
    );

    assert_ne!(
        a.payload[PAYLOAD_CONTENT], b.payload[PAYLOAD_CONTENT],
        "the two cards really do carry different content"
    );
    assert!(
        is_same_gate(&a, &b),
        "…but they are one decision on one gate and must dedupe to a single card"
    );
}

/// The card says, in plain words, what approving actually does. On a
/// build with checkpoint-backed resume wired, that is conditional —
/// normally no re-run, a re-run only if the graph changed underneath the
/// pending approval.
#[cfg(feature = "openhuman")]
#[test]
fn the_card_states_what_approving_costs() {
    let e = effect("digest", "gate", Value::Null);
    let note = e.payload[PAYLOAD_NOTE].as_str().expect("a note");
    assert!(note.contains("resumes"), "{note}");
    assert!(note.contains("graph changed"), "{note}");
    assert!(note.contains("tokens"), "{note}");
    assert!(note.contains("not be sent"), "{note}");
}

/// The build with no checkpoint machinery wired at all: every approval
/// really is the full re-run, so the operator is told exactly that, with
/// no conditional hedging the runtime cannot back up.
#[cfg(not(feature = "openhuman"))]
#[test]
fn the_card_states_what_approving_costs() {
    let e = effect("digest", "gate", Value::Null);
    let note = e.payload[PAYLOAD_NOTE].as_str().expect("a note");
    assert!(note.contains("re-runs"), "{note}");
    assert!(note.contains("tokens"), "{note}");
    assert!(note.contains("not be sent"), "{note}");
}

/// A garbled ledger degrades to "nothing known to be delivered" rather than
/// refusing the resume. Failing here would turn one malformed row into a
/// continuation that delivers nothing at all — the worse error.
#[test]
fn a_malformed_ledger_is_ignored_rather_than_fatal() {
    for garbage in [
        serde_json::json!("not an array"),
        serde_json::json!([{ "node": 7 }]),
        serde_json::json!([null]),
    ] {
        let input = serde_json::json!({ CONTINUATION_DELIVERED_KEY: garbage });
        assert!(delivered_in_input(&input).is_empty());
    }
}

#[test]
fn a_malformed_gate_record_names_the_key_it_is_missing() {
    let mut e = effect("digest", "gate", Value::Null);
    e.payload = serde_json::json!({ PAYLOAD_NODE_ID: "gate" });
    let err = required_str(&e, PAYLOAD_WORKFLOW_ID).expect_err("must refuse");
    assert!(err.to_string().contains(PAYLOAD_WORKFLOW_ID), "{err}");

    // A blank id is as unusable as a missing one and must not reach the
    // loader as an empty filename.
    e.payload = serde_json::json!({ PAYLOAD_WORKFLOW_ID: "   " });
    assert!(required_str(&e, PAYLOAD_WORKFLOW_ID).is_err());
}

/// A contested id that no longer holds the graph a run parked against is
/// refused, whichever side won it.
///
/// A company graph and a global can hold one id. Deleting the company's
/// copy surfaces the global; authoring one buries it. Both directions end
/// with a graph the operator never answered for.
#[test]
fn a_contested_id_is_refused_in_both_directions() {
    let contested = &crate::globals::workflows()[0].id;
    let parked = crate::company::parse_workflow(FINGERPRINT_V1).expect("parses");
    let other = crate::company::parse_workflow(FINGERPRINT_V2).expect("parses");

    let mut surfaced_global = other.clone();
    surfaced_global.id = contested.clone();
    surfaced_global.global = true;
    assert!(
        a_contested_id_no_longer_holds_the_parked_graph(
            Some(&parked.content_fingerprint()),
            &surfaced_global,
            &[],
        ),
        "a global surfacing under a deleted company graph's id must not run in its place"
    );

    let mut authored_company = other.clone();
    authored_company.id = contested.clone();
    authored_company.global = false;
    assert!(
        a_contested_id_no_longer_holds_the_parked_graph(
            Some(&parked.content_fingerprint()),
            &authored_company,
            &[],
        ),
        "a company graph authored over a parked global must not run in its place either"
    );

    let mut unchanged = surfaced_global.clone();
    unchanged.global = true;
    assert!(
        !a_contested_id_no_longer_holds_the_parked_graph(
            Some(&unchanged.content_fingerprint()),
            &unchanged,
            &[],
        ),
        "the graph a run genuinely parked against still resumes"
    );
    assert!(
        !a_contested_id_no_longer_holds_the_parked_graph(None, &surfaced_global, &[]),
        "a run parked before fingerprints were stashed keeps replaying as it did"
    );
    assert!(
        !a_contested_id_no_longer_holds_the_parked_graph(
            Some(&parked.content_fingerprint()),
            &other,
            &[],
        ),
        "an id no global answers to is the operator's own edit — unchanged behaviour"
    );
    assert!(
        !a_contested_id_no_longer_holds_the_parked_graph(
            Some(&parked.content_fingerprint()),
            &surfaced_global,
            &[format!("workflow:{contested}")],
        ),
        "a global the company disabled cannot contest the id at all"
    );
}

/// A card parked before this check existed carries no
/// `PAYLOAD_WORKFLOW_FINGERPRINT` at all — must not be treated as a
/// mismatch, or every pre-existing parked card would spuriously fall back
/// to a trigger re-run the moment this check shipped.
#[test]
fn a_card_with_no_stashed_fingerprint_is_treated_as_unchanged() {
    let workflow = crate::company::parse_workflow(FINGERPRINT_V1).expect("parses");
    let e = effect("editable", "gate", Value::Null);
    assert!(
        !e.payload
            .as_object()
            .unwrap()
            .contains_key(PAYLOAD_WORKFLOW_FINGERPRINT)
    );
    assert!(graph_unchanged_since_park(&e, &workflow));
}

/// The headline: a graph edited while its run sat parked no longer matches
/// the fingerprint that run's card stashed at park time (PR #1991 review,
/// `3903797615`).
#[test]
fn an_edited_graph_no_longer_matches_its_parked_fingerprint() {
    let parked_against = crate::company::parse_workflow(FINGERPRINT_V1).expect("parses");
    let edited = crate::company::parse_workflow(FINGERPRINT_V2).expect("parses");
    assert_ne!(
        parked_against.content_fingerprint(),
        edited.content_fingerprint(),
        "the two graphs differ, so their fingerprints must too, or this whole check is inert"
    );

    let mut e = effect("editable", "gate", Value::Null);
    if let Value::Object(ref mut payload) = e.payload {
        payload.insert(
            PAYLOAD_WORKFLOW_FINGERPRINT.to_string(),
            json!(parked_against.content_fingerprint()),
        );
    }

    assert!(
        !graph_unchanged_since_park(&e, &edited),
        "an edit made while the approval was pending must be detected"
    );
    assert!(
        graph_unchanged_since_park(&e, &parked_against),
        "the unedited graph must still read as unchanged against its own stashed fingerprint"
    );
}
