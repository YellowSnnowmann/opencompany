use super::*;
use serde_json::json;

fn spec(require: &str) -> Value {
    json!({ "require": require })
}

fn spec_with_field(require: &str, field: &str) -> Value {
    json!({ "require": require, "field": field })
}

#[test]
fn non_empty_passes_on_real_text() {
    let output = json!({ "text": "the report is done", "agent_ref": "a" });
    assert_eq!(evaluate_postcondition(&spec("non_empty"), &output), Ok(()));
}

#[test]
fn non_empty_fails_on_blank_text() {
    let output = json!({ "text": "   ", "agent_ref": "a" });
    assert!(evaluate_postcondition(&spec("non_empty"), &output).is_err());
}

#[test]
fn non_empty_fails_on_missing_text() {
    let output = json!({ "agent_ref": "a" });
    assert!(evaluate_postcondition(&spec("non_empty"), &output).is_err());
}

#[test]
fn field_present_passes_when_the_field_resolves() {
    let output = json!({ "items": [1, 2] });
    assert_eq!(
        evaluate_postcondition(&spec_with_field("field_present", "items"), &output),
        Ok(())
    );
}

#[test]
fn field_present_fails_when_the_field_is_absent() {
    let output = json!({ "text": "prose only" });
    assert!(evaluate_postcondition(&spec_with_field("field_present", "items"), &output).is_err());
}

#[test]
fn field_present_fails_when_the_field_is_explicitly_null() {
    let output = json!({ "items": null });
    assert!(evaluate_postcondition(&spec_with_field("field_present", "items"), &output).is_err());
}

#[test]
fn field_present_resolves_a_dotted_path() {
    let output = json!({ "json": { "result": { "count": 3 } } });
    assert_eq!(
        evaluate_postcondition(
            &spec_with_field("field_present", "json.result.count"),
            &output
        ),
        Ok(())
    );
}

#[test]
fn field_present_dotted_path_fails_partway_through() {
    let output = json!({ "json": { "result": {} } });
    assert!(
        evaluate_postcondition(
            &spec_with_field("field_present", "json.result.count"),
            &output
        )
        .is_err()
    );
}

/// Codex #3894162757 on #1937 — `field_present` on the bare `json` root
/// resolves for ANY present, non-null value there, including a bare
/// scalar the best-effort JSON parse produces just as readily as an
/// object or array. Unlike an object/array, a scalar can never reach a
/// downstream `=item.json` binding (tinyflows' own envelope construction
/// normalizes anything but `Object`/`Array` to `Value::Null`), so
/// certifying it here would pass a gate whose value the workflow can
/// never actually read — refused instead. See
/// `workflows::runner::tests_node_output::a_scalar_reply_cannot_satisfy_field_present_on_the_bare_json_root`
/// for the full-engine proof of the delivery gap this closes.
#[test]
fn field_present_on_the_bare_json_root_rejects_a_scalar() {
    for scalar in [json!(42), json!(true), json!("ok")] {
        let output = json!({ "text": "irrelevant", "agent_ref": "a", "json": scalar });
        assert!(
            evaluate_postcondition(&spec_with_field("field_present", "json"), &output).is_err(),
            "a bare scalar under `json` must not satisfy field_present on the bare \
             `json` root: {output}"
        );
    }
}

/// Companion GREEN: the rejection above is scoped to the exact `json`
/// root, not to scalars in general. A dotted path UNDER `json`
/// (`json.count`) reaching a scalar is fine — getting there at all means
/// the reply was already an object, which merges into the emitted value
/// intact (`HarnessAgentRunner::run_turn`'s `Value::Object` arm), so
/// `item.json.count` really does resolve downstream.
#[test]
fn field_present_on_a_dotted_path_under_json_still_accepts_a_scalar() {
    let output = json!({ "json": { "count": 42 } });
    assert_eq!(
        evaluate_postcondition(&spec_with_field("field_present", "json.count"), &output),
        Ok(())
    );
}

/// Companion GREEN: the bare `text`/`agent_ref` roots are unaffected —
/// they are always strings in the envelope tinyflows exposes directly as
/// `item.text` (never nulled), so a scalar there is the ordinary,
/// deliverable case, not the `json`-root delivery gap.
#[test]
fn field_present_on_bare_text_or_agent_ref_still_accepts_their_string_value() {
    let output = json!({ "text": "hello", "agent_ref": "researcher", "json": null });
    assert_eq!(
        evaluate_postcondition(&spec_with_field("field_present", "text"), &output),
        Ok(())
    );
    assert_eq!(
        evaluate_postcondition(&spec_with_field("field_present", "agent_ref"), &output),
        Ok(())
    );
}

/// Codex #3894038816 on #1937 — the silent-disable finding. `postcondition`
/// rides inside the engine-resolved node config (see
/// `workflows::caps::tests_field_present::a_field_resolved_away_by_an_authored_expression_fails_closed_at_run_turn`
/// for the full authored-`"=item.missing"` → config-resolution trace), so
/// a `field` that resolved to anything other than a present string reaches
/// this function looking IDENTICAL to a `field_present` declared with no
/// `field` at all — a shape `workflow_file::validate` refuses to ever save
/// (`postcondition_field_present_without_a_field_is_rejected`). Before
/// this fix, that shape was fail-OPEN here: a `tracing::warn!` and
/// `Ok(())`, silently letting every reply through a gate the workflow file
/// plainly declares. `field_present`'s entire job is checking one named
/// field exists — evaluating it with no field to check is not an
/// ambiguous "maybe intended" gap the way an unrecognized `require` is
/// (the module doc's fail-open case), so this fails CLOSED instead.
#[test]
fn field_present_declared_with_a_field_that_resolved_away_fails_closed() {
    let spec = json!({ "require": "field_present", "field": null });
    let output = json!({ "text": "a reply that would satisfy nothing in particular" });
    assert!(
        evaluate_postcondition(&spec, &output).is_err(),
        "a `field_present` postcondition whose own `field` did not resolve to a \
         string must halt the node, not silently pass it — RED on the code as it \
         stood before this fix: this returned Ok(())"
    );
}

#[test]
fn non_empty_list_passes_on_a_populated_array() {
    let output = json!(["a"]);
    assert_eq!(
        evaluate_postcondition(&spec("non_empty_list"), &output),
        Ok(())
    );
}

#[test]
fn non_empty_list_fails_on_an_empty_array() {
    let output = json!([]);
    assert!(evaluate_postcondition(&spec("non_empty_list"), &output).is_err());
}

#[test]
fn non_empty_list_fails_on_a_non_array() {
    let output = json!({ "text": "not a list" });
    assert!(evaluate_postcondition(&spec("non_empty_list"), &output).is_err());
}

/// Codex #3894277296 on #1937 — the specific unsatisfiable shape
/// `company::workflow_file::validate` now refuses at author time
/// (`postcondition_non_empty_list_on_text_or_agent_ref_is_rejected`):
/// even if one reached this evaluator anyway, `text`/`agent_ref` are
/// unconditionally strings in the envelope, so `non_empty_list` fails
/// them the same honest way it fails any other non-array target — this
/// pins that the evaluator-level behavior was ALREADY correct, and the
/// gap was purely that authoring one was ever allowed to save.
#[test]
fn non_empty_list_on_text_or_agent_ref_fails_honestly() {
    let output = json!({ "text": "some prose", "agent_ref": "researcher", "json": null });
    for field in ["text", "agent_ref"] {
        assert!(
            evaluate_postcondition(&spec_with_field("non_empty_list", field), &output).is_err(),
            "field `{field}` is always a string, never an array: {output}"
        );
    }
}

/// Codex review on #1937 (issue #1866): the no-`field` form must look at
/// the standard envelope's structured `json` payload, not the envelope
/// object itself — an agent-node envelope always carries `text`/
/// `agent_ref` alongside `json`, so checking the envelope directly could
/// never see a `Value::Array` even when the agent's parsed reply
/// genuinely is a non-empty list.
#[test]
fn non_empty_list_with_no_field_checks_the_envelopes_json_payload() {
    let output = json!({ "text": "[\"a\",\"b\"]", "agent_ref": "a", "json": ["a", "b"] });
    assert_eq!(
        evaluate_postcondition(&spec("non_empty_list"), &output),
        Ok(())
    );
}

/// Companion RED-shape: when the envelope's `json` payload didn't parse
/// (a plain-prose reply), the no-`field` form still fails honestly —
/// it must not silently pass just because a `json` key exists.
#[test]
fn non_empty_list_with_no_field_fails_when_the_envelopes_json_is_null() {
    let output = json!({ "text": "just prose, no list here", "agent_ref": "a", "json": null });
    assert!(evaluate_postcondition(&spec("non_empty_list"), &output).is_err());
}

#[test]
fn non_empty_list_checks_the_named_field_when_given() {
    let output = json!({ "items": ["a", "b"] });
    assert_eq!(
        evaluate_postcondition(&spec_with_field("non_empty_list", "items"), &output),
        Ok(())
    );

    let empty = json!({ "items": [] });
    assert!(evaluate_postcondition(&spec_with_field("non_empty_list", "items"), &empty).is_err());
}

/// An unrecognized `require` is a declared gate this binary cannot
/// evaluate. Advancing on it would let the node through with its authored
/// quality check having done nothing at all, which is the one outcome a
/// postcondition exists to prevent — so it fails the node instead.
#[test]
fn unknown_require_fails_closed() {
    let output = json!({});
    let gap = evaluate_postcondition(&spec("some_future_predicate"), &output)
        .expect_err("an unevaluable gate must not advance the node");
    assert!(
        gap.contains("some_future_predicate"),
        "the gap names the predicate this binary could not evaluate: {gap:?}"
    );
}

/// ...and a `require` this binary DOES understand still advances on output
/// that clears it. Failing closed on the unknown case must not turn every
/// postcondition into a halt.
#[test]
fn a_recognized_require_still_passes_on_good_output() {
    let output = json!({ "text": "the report is done", "json": { "items": [1] } });
    assert_eq!(evaluate_postcondition(&spec("non_empty"), &output), Ok(()));
    assert_eq!(
        evaluate_postcondition(&spec_with_field("field_present", "json.items"), &output),
        Ok(())
    );
    assert_eq!(
        evaluate_postcondition(&spec_with_field("non_empty_list", "json.items"), &output),
        Ok(())
    );

    let listed = json!({ "text": "two of them", "json": [1, 2] });
    assert_eq!(
        evaluate_postcondition(&spec("non_empty_list"), &listed),
        Ok(())
    );
}
