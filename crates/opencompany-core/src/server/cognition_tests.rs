use super::*;
use crate::ports::brain::{ECHO_PATH, HARNESS_PATH};

use InferenceResolution::{Nothing, Resolved, Unreadable};

/// The whole matrix in one place — including the `unavailable` arm that a
/// lane enabling the feature could otherwise reach only by constructing a
/// harness-less runtime.
#[test]
fn the_states_are_derived_from_the_path_the_harness_and_the_resolution() {
    for path in [HARNESS_PATH, "hosted", "sidecar", "custom"] {
        assert_eq!(
            cognition_state(path, true, Nothing),
            CognitionState::Configured,
            "{path} runs a model, whatever the config says",
        );
    }
    assert_eq!(
        cognition_state(ECHO_PATH, true, Nothing),
        CognitionState::Unconfigured,
        "a harness is attached and nothing is set: a provider really is one \
         settings page away",
    );
    assert_eq!(
        cognition_state(ECHO_PATH, true, Resolved),
        CognitionState::RestartRequired,
        "a provider resolves, so the operator has already chosen one",
    );
    assert_eq!(
        cognition_state(ECHO_PATH, true, Unreadable),
        CognitionState::Undetermined,
        "the config could not be read, so nothing saved is known to help",
    );
    for resolution in [Nothing, Resolved, Unreadable] {
        assert_eq!(
            cognition_state(ECHO_PATH, false, resolution),
            CognitionState::Unavailable,
            "with no harness, what the config resolved to changes no advice",
        );
    }
}

/// The regression this exists for: a build with no harness, sitting on the
/// echo brain, must never report itself as able to think. That is the state
/// the console renders `"You said: …"` under a teammate's name in.
#[test]
fn a_build_with_no_harness_never_reports_itself_configured() {
    for reachable in [true, false] {
        for resolution in [Nothing, Resolved, Unreadable] {
            assert_ne!(
                cognition_state(ECHO_PATH, reachable, resolution),
                CognitionState::Configured,
            );
        }
    }
}

/// A harness that is compiled in but never attached must not be sold to the
/// operator as a settings problem (codex review of PR #1740).
///
/// This is the case `cfg!(feature = "openhuman")` alone gets wrong. An
/// embedder that skips [`crate::app::harness::attach`] — the shipped
/// desktop-shell bug that module was written to end — leaves an `openhuman`
/// binary whose companies hold no pool. Saying `unconfigured` there points
/// the operator at Settings → Inference, and nothing they save moves that
/// runtime off the echo brain. The input is reachability precisely so this
/// arm exists; asserting it here is what stops a later "simplification"
/// back to the feature flag.
#[test]
fn a_compiled_in_harness_that_is_not_attached_is_not_a_settings_problem() {
    assert_eq!(
        cognition_state(ECHO_PATH, false, Nothing),
        CognitionState::Unavailable,
    );
    assert_ne!(
        cognition_state(ECHO_PATH, false, Nothing),
        CognitionState::Unconfigured,
        "no attached pool: Settings → Inference is a dead end here",
    );
}

/// An unreadable config is not a missing one (codex review of PR #1740).
///
/// `ops::inference` already refuses this promise from the other side: its
/// `unreadable_inference_config_is_not_restartable` regression builds a
/// reachable harness over a failing `SecretStore` and asserts
/// `RunnerGap::NotWired`, "not `InferenceRequired`", because saving cannot
/// resolve a configuration the host cannot read. Chat pointing that same
/// operator at Settings → Inference would make the promise that route
/// declines to make, on the same runtime, in the same breath.
#[test]
fn an_unreadable_config_is_not_sold_as_a_missing_one() {
    assert_eq!(
        cognition_state(ECHO_PATH, true, Unreadable),
        CognitionState::Undetermined,
    );
    assert_ne!(
        cognition_state(ECHO_PATH, true, Unreadable),
        CognitionState::Unconfigured,
        "an unreadable config is no evidence that saving one would help (#266)",
    );
    // And it is not the harness's fault either — one is attached, so
    // naming a rebuild would be a plain falsehood.
    assert_ne!(
        cognition_state(ECHO_PATH, true, Unreadable),
        CognitionState::Unavailable,
    );
}

/// A configured provider that is not live yet is not an unconfigured one
/// (codex review of PR #1740).
///
/// The most likely way to reach the echo brain in practice, and the one
/// where getting it wrong is rudest: the operator followed this banner's own
/// link, chose a provider, saved it — and brain selection happens once, in
/// `RuntimeBuilder::build`, so the company keeps the echo brain until its
/// runtime is rebuilt. Reporting `unconfigured` sends them back to the page
/// they just came from to redo work they did correctly. `ops::inference`
/// calls this same state `restartRequired` (issue #266).
#[test]
fn a_saved_provider_awaiting_a_restart_is_not_unconfigured() {
    assert_eq!(
        cognition_state(ECHO_PATH, true, Resolved),
        CognitionState::RestartRequired,
    );
    assert_ne!(
        cognition_state(ECHO_PATH, true, Resolved),
        CognitionState::Unconfigured,
        "a provider resolves: telling them to choose one is telling them to \
         repeat themselves",
    );
}

/// A path this module has never heard of is cognition until proven
/// otherwise. The alternative — allow-listing the working paths — would
/// report "cannot think" for the next brain someone adds, and the symptom
/// (a banner on a company that is thinking perfectly well) points nowhere
/// near this function.
#[test]
fn an_unknown_path_is_treated_as_cognition() {
    assert_eq!(
        cognition_state("some-brain-added-later", false, Unreadable),
        CognitionState::Configured,
    );
}

#[test]
fn the_wire_labels_match_the_serde_renaming() {
    for state in [
        CognitionState::Configured,
        CognitionState::Unconfigured,
        CognitionState::Unavailable,
        CognitionState::RestartRequired,
        CognitionState::Undetermined,
    ] {
        assert_eq!(
            serde_json::to_value(state).expect("serialize"),
            serde_json::Value::String(state.as_str().to_string()),
        );
    }
}
