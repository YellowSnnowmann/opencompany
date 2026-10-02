//! Whether a `needs_approval` tool mode parks, as the console is told it.
//!
//! The route that reports this held a hardcoded `false` with the condition
//! behind it written in a comment, which is the shape that goes stale silently:
//! nothing fails when the roster changes and the sentence does not.

use crate::company::Policy;

fn policy(mode: &str) -> Policy {
    Policy {
        mode: mode.to_string(),
        ..Policy::default()
    }
}

/// Every tier, because the bypass sits below the readonly brake and above the
/// classification arms — so a tier that denies more does not thereby park more.
#[test]
fn no_tier_parks_a_classified_call_on_this_build() {
    for mode in ["readonly", "supervised", "auto", "full"] {
        assert!(
            !super::approvals_park(&policy(mode)),
            "`{mode}` reported parking; the roster disables policy-generated \
             approvals, so a tool set to needs_approval runs as if allowed"
        );
    }
}

/// The reported value is the roster's own, not a second opinion about it. Fail
/// this by removing `with_policy_hitl_disabled` from one of the two and leaving
/// the other — which is exactly what a restated constant could not notice.
#[cfg(feature = "openhuman")]
#[test]
fn the_reported_answer_is_the_policy_the_roster_is_built_from() {
    let p = policy("supervised");
    assert_eq!(
        super::approvals_park(&p),
        crate::harness::built_in::roster_policy_base(&p, None).policy_hitl_enabled(),
    );
}
