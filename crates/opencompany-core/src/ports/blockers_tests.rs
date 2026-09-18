use super::*;

#[test]
fn kind_wire_round_trips() {
    for kind in [
        BlockerKind::Information,
        BlockerKind::Infrastructure,
        BlockerKind::Transient,
    ] {
        assert_eq!(BlockerKind::from_wire(kind.as_str()), Some(kind));
    }
    assert_eq!(BlockerKind::from_wire("nonsense"), None);
}

#[test]
fn source_wire_round_trips() {
    for source in [
        BlockerSource::Provider,
        BlockerSource::Tool,
        BlockerSource::Prereq,
        BlockerSource::AgentQuestion,
    ] {
        assert_eq!(BlockerSource::from_wire(source.as_str()), Some(source));
    }
    assert_eq!(BlockerSource::from_wire("nonsense"), None);
}

/// The serde token and the hand-written token are the same string. They are
/// written twice — `rename_all` and `as_str` — and a divergence would store
/// one spelling in the journal and another in a serialized payload.
#[test]
fn serde_and_as_str_agree() {
    for kind in [
        BlockerKind::Information,
        BlockerKind::Infrastructure,
        BlockerKind::Transient,
    ] {
        let json = serde_json::to_string(&kind).expect("kind serializes");
        assert_eq!(json, format!("\"{}\"", kind.as_str()));
    }
    for source in [
        BlockerSource::Provider,
        BlockerSource::Tool,
        BlockerSource::Prereq,
        BlockerSource::AgentQuestion,
    ] {
        let json = serde_json::to_string(&source).expect("source serializes");
        assert_eq!(json, format!("\"{}\"", source.as_str()));
    }
}

/// The rule the whole epic turns on: only the two answerable classes cost a
/// person their attention.
#[test]
fn only_answerable_kinds_park() {
    assert!(BlockerKind::Information.parks());
    assert!(BlockerKind::Infrastructure.parks());
    assert!(
        !BlockerKind::Transient.parks(),
        "a transient failure is not answerable by a person and must retry, not ask"
    );
}

/// Only the operator can reconnect an integration, so those prerequisites
/// must not be routed through the roster first.
#[test]
fn integration_prereqs_are_infrastructure_and_the_rest_are_information() {
    use crate::ports::tasks::PrereqKind;
    for kind in [
        PrereqKind::Connection,
        PrereqKind::Composio,
        PrereqKind::Mcp,
        PrereqKind::Credential,
        PrereqKind::Permission,
    ] {
        assert_eq!(BlockerKind::for_prereq(kind), BlockerKind::Infrastructure);
    }
    for kind in [PrereqKind::File, PrereqKind::Assignee, PrereqKind::Other] {
        assert_eq!(BlockerKind::for_prereq(kind), BlockerKind::Information);
    }
}

/// A prerequisite never resolves itself, so no prereq class may park as
/// transient — that would settle the card and ask nobody.
#[test]
fn no_prereq_is_transient() {
    use crate::ports::tasks::PrereqKind;
    for kind in [
        PrereqKind::Connection,
        PrereqKind::Composio,
        PrereqKind::Mcp,
        PrereqKind::Credential,
        PrereqKind::File,
        PrereqKind::Permission,
        PrereqKind::Assignee,
        PrereqKind::Other,
    ] {
        assert!(BlockerKind::for_prereq(kind).parks());
    }
}

#[test]
fn a_mixed_set_of_prereqs_takes_the_operator_route() {
    use crate::ports::tasks::PrereqKind;
    assert_eq!(
        BlockerKind::for_prereqs([PrereqKind::File, PrereqKind::Credential]),
        BlockerKind::Infrastructure,
        "a plan blocked on a credential cannot start however well the brief is answered"
    );
    assert_eq!(
        BlockerKind::for_prereqs([PrereqKind::File, PrereqKind::Assignee]),
        BlockerKind::Information
    );
    assert_eq!(
        BlockerKind::for_prereqs([]),
        BlockerKind::Information,
        "vacuous, and there is nothing to ask about"
    );
}

#[test]
fn effect_kind_carries_the_gap_class() {
    assert_eq!(
        BlockerKind::Information.effect_kind(),
        "blocker.information"
    );
    assert_eq!(
        BlockerKind::Infrastructure.effect_kind(),
        "blocker.infrastructure"
    );
}

/// The payload's kind is the one that reaches the wire — a payload cannot
/// announce one class on the event and carry another inside.
#[test]
fn payload_effect_kind_follows_its_kind() {
    let payload = BlockerPayload {
        kind: BlockerKind::Infrastructure,
        source: BlockerSource::Provider,
        step: Some(BlockerStep::Task {
            task_id: "task-1".to_string(),
        }),
        reason: "the model id `gpt-nonexistent` was rejected".to_string(),
        needed: "a model id this provider serves".to_string(),
        group_key: None,
    };
    assert_eq!(payload.effect_kind(), "blocker.infrastructure");
}

/// `group_key` is additive: a payload serialized before it existed — with no
/// such key — reads back as ungrouped rather than failing to parse, and a
/// present key round-trips.
#[test]
fn group_key_is_additive_and_round_trips() {
    let legacy = serde_json::json!({
        "kind": "infrastructure",
        "source": "tool",
        "reason": "could not connect to mcp server `slack`",
        "needed": "the integration reconnected from Apps"
    });
    let parsed: BlockerPayload =
        serde_json::from_value(legacy).expect("a pre-field payload still parses");
    assert_eq!(parsed.group_key, None);

    let grouped = BlockerPayload {
        group_key: Some("connection:slack".to_string()),
        ..parsed
    };
    let json = serde_json::to_value(&grouped).expect("serializes");
    let back: BlockerPayload = serde_json::from_value(json).expect("parses");
    assert_eq!(back.group_key.as_deref(), Some("connection:slack"));
}

#[test]
fn verdict_wire_round_trips() {
    for verdict in [
        BlockerVerdict::Retry,
        BlockerVerdict::Amend,
        BlockerVerdict::Skip,
        BlockerVerdict::Cancel,
    ] {
        assert_eq!(BlockerVerdict::from_wire(verdict.as_str()), Some(verdict));
        let json = serde_json::to_string(&verdict).expect("verdict serializes");
        assert_eq!(json, format!("\"{}\"", verdict.as_str()));
    }
    assert_eq!(BlockerVerdict::from_wire("nonsense"), None);
}

/// The mapping the sharpest risk turns on: only [`BlockerVerdict::Cancel`]
/// denies, and only it declines to re-enter. Every answering verdict is an
/// approve that resumes — and an approve that must never execute the inert
/// blocker effect (that guard lives in the resolve path).
#[test]
fn only_cancel_denies_and_declines_to_resume() {
    use crate::ports::types::Verdict;
    for verdict in [
        BlockerVerdict::Retry,
        BlockerVerdict::Amend,
        BlockerVerdict::Skip,
    ] {
        assert_eq!(verdict.event_verdict(), Verdict::Approve, "{verdict:?}");
        assert!(verdict.resumes(), "{verdict:?} must re-enter the step");
    }
    assert_eq!(BlockerVerdict::Cancel.event_verdict(), Verdict::Deny);
    assert!(
        !BlockerVerdict::Cancel.resumes(),
        "cancel abandons the work and starts nothing"
    );
}

/// The shared operator-facing fragment names all four verdicts, and names
/// them by the tokens the wire uses. A renamed variant that left the words
/// behind would have a surface offering a choice the host cannot take.
#[test]
fn the_shared_choice_fragment_names_every_verdict() {
    for verdict in [
        BlockerVerdict::Retry,
        BlockerVerdict::Amend,
        BlockerVerdict::Skip,
        BlockerVerdict::Cancel,
    ] {
        // Amend is the one arm an operator does not type by name: they type
        // the answer, and the words say so.
        let word = if verdict == BlockerVerdict::Amend {
            "an answer"
        } else {
            verdict.as_str()
        };
        assert!(
            BLOCKER_VERDICT_CHOICES.contains(word),
            "{verdict:?} is offered by the host but not named to the operator: \
             {BLOCKER_VERDICT_CHOICES}"
        );
    }
}

/// The answer is additive: a resolution written with no words reads back
/// with none, and one carrying an amendment round-trips.
#[test]
fn resolution_answer_is_additive_and_round_trips() {
    let bare = BlockerResolution::new(BlockerVerdict::Retry);
    let json = serde_json::to_value(&bare).expect("serializes");
    assert_eq!(json, serde_json::json!({ "verdict": "retry" }));
    let back: BlockerResolution = serde_json::from_value(json).expect("parses");
    assert_eq!(back, bare);
    assert_eq!(back.answer, "");

    let amended = BlockerResolution::answered(BlockerVerdict::Amend, "use gpt-4o-mini");
    let json = serde_json::to_value(&amended).expect("serializes");
    let back: BlockerResolution = serde_json::from_value(json).expect("parses");
    assert_eq!(back, amended);
    assert!(back.resumes());
}

/// The step survives a round trip through JSON, because that is how it
/// reaches the resume tiers: written into the effect payload at park time,
/// read back after a restart.
#[test]
fn step_round_trips_through_json() {
    for step in [
        BlockerStep::Task {
            task_id: "task-1".to_string(),
        },
        BlockerStep::Node {
            run_id: "run-1".to_string(),
            node_id: "node-a".to_string(),
        },
    ] {
        let json = serde_json::to_string(&step).expect("step serializes");
        let back: BlockerStep = serde_json::from_str(&json).expect("step parses");
        assert_eq!(back, step);
    }
}

fn effect_of_kind(kind: &str) -> crate::ports::types::Effect {
    crate::ports::types::Effect {
        kind: kind.to_string(),
        group: crate::ports::types::EffectGroup::Other,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::Value::Null,
        agent: None,
        run_id: None,
    }
}

/// `is_blocker_effect` is a **lexical** prefix match, not a check against
/// the closed [`BlockerKind`]/[`BlockerSource`] vocabulary — an effect kind
/// that merely *starts with* `blocker.` is treated as inert and skipped at
/// execution (`CycleRunner`'s never-execute guard) whether or not it is one
/// of this module's own recognised classes. Pinned so the "blocker."
/// prefix is understood as a reserved namespace no other effect kind may
/// ever begin with, rather than a semantic check that happens to be
/// implemented as a prefix match.
#[test]
fn the_guard_is_a_bare_prefix_match_not_a_vocabulary_check() {
    assert!(is_blocker_effect(&effect_of_kind("blocker.information")));
    // A kind starting with the reserved prefix but naming no recognised
    // class is matched all the same — the guard cannot tell "a real
    // blocker" from "anything spelled with this prefix".
    assert!(is_blocker_effect(&effect_of_kind(
        "blocker.nobody_declared_this_class"
    )));
    // The boundary that actually matters: a kind that merely contains the
    // word, rather than starting with `blocker.`, is correctly NOT matched.
    assert!(!is_blocker_effect(&effect_of_kind("payment.blocker_fee")));
    assert!(!is_blocker_effect(&effect_of_kind("blocker")));
}
