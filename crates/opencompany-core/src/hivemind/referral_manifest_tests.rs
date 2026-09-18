//! Tests for the `[[group_chat]].hive.referral` manifest surface: parsing,
//! validation of policies that could never fire, and the pair-exchange close
//! line's line-counting (split out of `referral_prompt_tests.rs`, issue
//! tracked alongside the rest of the referral test split).

use super::referral;
use super::referral_fixtures_tests::*;
use super::test::{desk_of, record};

// ---------------------------------------------------------------------------
// The manifest
// ---------------------------------------------------------------------------

/// The referral block is parsed off `[[group_chat]].hive`, not invented here.
#[test]
pub(super) fn the_manifest_parses_a_referral_block() {
    let manifest = two_desks(
        "hive = { referral = { enabled = true, max_hops = 3, reach = \"channels\", \
         returns = false, peer_cap = 4 } }",
    );
    let desk = desk_of(&manifest, "eng").expect("a room");
    let referral = &desk.config.referral;
    assert!(referral.enabled());
    assert_eq!(referral.peer_cap(), 4);
    let policy = referral.policy();
    assert_eq!(policy.max_hops, 3);
    assert!(!policy.returns);
    assert!(policy.reach.crosses());
    assert!(
        !policy.reach.addresses_desks(),
        "`channels` lets a turn run elsewhere without making `@#desk` mean anything"
    );
}

/// Every check here catches a policy that would be *silently* inert. A desk
/// that asks nothing looks exactly like a desk whose members had nothing to
/// ask, so a typo has to be a validation error rather than a quiet no-op.
#[test]
pub(super) fn the_manifest_refuses_a_referral_policy_that_could_never_fire() {
    let problems = |hive: &str| record(&two_desks(hive)).manifest.validate();

    let found = problems("hive = { referral = { enabled = true, reach = \"everywhere\" } }");
    assert!(
        found.iter().any(|p| p.contains("hive.referral.reach")),
        "{found:?}"
    );

    for key in ["max_hops", "peer_cap"] {
        let found = problems(&format!(
            "hive = {{ referral = {{ enabled = true, {key} = 0 }} }}"
        ));
        assert!(
            found
                .iter()
                .any(|p| p.contains(&format!("hive.referral.{key} = 0"))),
            "{key}: {found:?}"
        );
    }

    // A round trip is two hops. One hop plus `returns` describes a question
    // whose answer is thrown away, which is worse than refusing it.
    let found = problems("hive = { referral = { enabled = true, max_hops = 1 } }");
    assert!(
        found.iter().any(|p| p.contains("a round trip is two hops")),
        "{found:?}"
    );
    // Declared one-way, it is a policy somebody meant.
    let found = problems("hive = { referral = { enabled = true, max_hops = 1, returns = false } }");
    assert!(!found.iter().any(|p| p.contains("round trip")), "{found:?}");

    // And the ordinary opted-in block is accepted.
    assert!(problems(REFERRING).is_empty(), "{:?}", problems(REFERRING));
}

/// `left` counts the lines that come AFTER this one, so only zero closes.
///
/// The closing arm matched `0 | 1`, which handed the second-to-last speaker
/// "this is the last line of this exchange — close it" and then let another
/// line run anyway. At `pair_messages = 4` — what `retail_co` ships — that is
/// the third row of every four-row exchange being told to conclude while the
/// fourth was still coming (CodeRabbit, #2341).
#[test]
fn only_the_final_line_of_a_pair_exchange_is_told_to_close() {
    let exchange = vec![
        ("planner".to_string(), "what is the lag budget?".to_string()),
        ("sre".to_string(), "which path?".to_string()),
    ];

    let last = referral::pair_turn_prompt("sre", &exchange, 0);
    assert!(
        last.contains("This is the last line"),
        "a speaker with nothing after it closes: {last}"
    );

    let penultimate = referral::pair_turn_prompt("sre", &exchange, 1);
    assert!(
        !penultimate.contains("This is the last line"),
        "one more line is still coming, so this speaker must not be told to close: {penultimate}"
    );
    assert!(
        penultimate.contains("1 more line in this exchange"),
        "and it is told how much room is left, in the singular: {penultimate}"
    );

    let early = referral::pair_turn_prompt("sre", &exchange, 3);
    assert!(
        early.contains("3 more lines"),
        "the plural still reads correctly: {early}"
    );
}
