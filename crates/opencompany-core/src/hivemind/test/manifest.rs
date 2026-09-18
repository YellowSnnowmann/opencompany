//! Tests for the [`HiveConfig`] manifest knob.

use super::super::*;
use super::fixtures::*;

#[test]
fn a_desk_with_two_members_deliberates_by_default() {
    let desk = desk_of(&three_member_manifest(), "eng").expect("three members is a room");
    assert_eq!(desk.id, "eng");
    assert_eq!(desk.name, "Engineering");
    assert_eq!(desk.description.as_deref(), Some("Ship the rollout"));
    assert_eq!(desk.member_ids(), ["planner", "scout", "critic"]);
    assert_eq!(desk.members[0].role, "Planner");
}

#[test]
fn a_single_member_desk_never_enters_the_driver() {
    let manifest = "[company]\nname = \"Acme\"\n\
         [[agent]]\nid = \"solo\"\nrole = \"Everything\"\n\
         [[group_chat]]\nid = \"eng\"\nname = \"Engineering\"\nmembers = [\"solo\"]\n";
    assert!(desk_of(manifest, "eng").is_none());
    // And saying so explicitly does not conjure a room out of one member.
    let opted_in = "[company]\nname = \"Acme\"\n\
         [[agent]]\nid = \"solo\"\nrole = \"Everything\"\n\
         [[group_chat]]\nid = \"eng\"\nname = \"Engineering\"\nmembers = [\"solo\"]\n\
         hive = { enabled = true }\n";
    assert!(desk_of(opted_in, "eng").is_none());
}

#[test]
fn an_explicit_opt_out_keeps_the_single_responder_path() {
    let manifest = format!("{}hive = {{ enabled = false }}\n", three_member_manifest());
    assert!(desk_of(&manifest, "eng").is_none());
}

#[test]
fn general_dms_and_unknown_keys_are_not_desks() {
    let manifest = three_member_manifest();
    let record = record(&manifest);
    for chat in [None, Some(""), Some("main"), Some("General")] {
        assert!(
            desk_episode(&record, chat).is_none(),
            "the company's own line is not a deliberating desk: {chat:?}"
        );
    }
    assert!(desk_episode(&record, Some("dm:planner")).is_none());
    assert!(desk_episode(&record, Some("planner")).is_none());
    assert!(desk_episode(&record, Some("nowhere")).is_none());
}

#[test]
fn the_manifest_parses_a_hive_block_and_rejects_zero_bounds() {
    let manifest = format!(
        "{}hive = {{ enabled = true, turn_budget = 6, quorum = 2, blind_round = false }}\n",
        three_member_manifest()
    );
    let desk = desk_of(&manifest, "eng").expect("an opted-in three-member desk is a room");
    assert_eq!(desk.config.turn_budget, Some(6));
    assert_eq!(desk.config.quorum, Some(2));
    assert_eq!(desk.config.blind_round, Some(false));
    let policy = desk.policy();
    assert_eq!(policy.turn_budget, 6);
    assert_eq!(policy.quorum.threshold, 2);
    assert!(!policy.blind_round);

    let problems = record(&format!(
        "{}hive = {{ quorum = 0 }}\n",
        three_member_manifest()
    ))
    .manifest
    .validate();
    assert!(
        problems.iter().any(|p| p.contains("hive.quorum = 0")),
        "{problems:?}"
    );

    let problems = record(&format!(
        "{}hive = {{ turn_budget = 0 }}\n",
        three_member_manifest()
    ))
    .manifest
    .validate();
    assert!(
        problems.iter().any(|p| p.contains("hive.turn_budget = 0")),
        "{problems:?}"
    );
}

#[test]
fn a_desk_whose_moves_table_permits_support_to_fewer_seats_than_quorum_is_refused() {
    // Only two of three seats hold `support`; `quorum = 3` needs a third
    // distinct supporter that no cooperation among these seats can produce.
    let manifest = format!(
        "{}[group_chat.hive]\nquorum = 3\n\n[group_chat.hive.moves]\n\
         planner = [\"support\", \"evidence\", \"defer\"]\n\
         scout = [\"support\", \"evidence\", \"defer\"]\n\
         critic = [\"evidence\", \"object\", \"defer\"]\n",
        three_member_manifest()
    );
    let problems = record(&manifest).manifest.validate();
    assert!(
        problems
            .iter()
            .any(|p| p.contains("`!support`") && p.contains("quorum")),
        "{problems:?}"
    );
}

#[test]
fn a_desk_at_exactly_quorum_many_eligible_supporters_is_accepted() {
    // Three seats hold `support` and `quorum` asks for exactly three. That is
    // fragile — it carries only by unanimity among the three, and one grounded
    // `!object` silencing any of them leaves nobody to replace what was
    // silenced — but fragile is not impossible, and it is a shape an operator
    // is deliberately allowed to ask for: `HivePolicy::from_config` clamps an
    // explicit `quorum` to `1..=count`, so `quorum = 3` on a desk of three is
    // honoured as written. The `.min(count - 1)` next to it governs the
    // *default* threshold only — what a desk that named no number gets — and
    // is not a ban on unanimity for a desk that named one.
    //
    // This test exists as the boundary marker for the check above: an earlier
    // draft refused this shape and thereby outlawed the desk
    // `tests/hivemind_e2e.rs` deliberately exercises (`quorum = 3`, three
    // seats, no `moves` table). Validation enforces the crate's policy; it
    // does not get to invent a stricter one.
    let manifest = format!(
        "{}[group_chat.hive]\nquorum = 3\n\n[group_chat.hive.moves]\n\
         planner = [\"support\", \"evidence\", \"defer\"]\n\
         scout = [\"support\", \"evidence\", \"defer\"]\n\
         critic = [\"support\", \"object\", \"evidence\", \"defer\"]\n",
        three_member_manifest()
    );
    let problems = record(&manifest).manifest.validate();
    assert!(
        !problems.iter().any(|p| p.contains("can never carry")),
        "unanimity among the eligible seats is reachable, so it is the \
         operator's call and not this check's: {problems:?}"
    );
}

#[test]
fn a_desk_with_one_seat_of_slack_beyond_quorum_is_accepted() {
    // Three seats hold `support`, `quorum` asks for only two: one eligible
    // seat is free to sit any given topic out, so this is not unanimity and
    // must pass.
    let manifest = format!(
        "{}[group_chat.hive]\nquorum = 2\n\n[group_chat.hive.moves]\n\
         planner = [\"support\", \"evidence\", \"defer\"]\n\
         scout = [\"support\", \"evidence\", \"defer\"]\n\
         critic = [\"support\", \"object\", \"evidence\", \"defer\"]\n",
        three_member_manifest()
    );
    let problems = record(&manifest).manifest.validate();
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn a_member_who_may_only_propose_counts_as_an_eligible_supporter() {
    // `critic` holds `propose` but never `support`. `tinyhivemind_hive`
    // counts a `!propose` as its own author's support unconditionally
    // (`quorum::standings` gates `require_grounded`/`require_evidential` on
    // `TraceKind::Support` only), so `critic` can still add itself as a
    // distinct supporter — of its own proposal, or by re-proposing a topic
    // already on the floor. Without it, `planner` and `scout` alone equal
    // `quorum`, which this check refuses.
    let manifest = format!(
        "{}[group_chat.hive]\nquorum = 2\n\n[group_chat.hive.moves]\n\
         planner = [\"support\", \"evidence\", \"defer\"]\n\
         scout = [\"support\", \"evidence\", \"defer\"]\n\
         critic = [\"propose\", \"evidence\", \"defer\"]\n",
        three_member_manifest()
    );
    let problems = record(&manifest).manifest.validate();
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn an_unnamed_member_counts_as_an_eligible_supporter() {
    // `critic` is not named in `hive.moves` at all, so it keeps every move,
    // `support` included. Without it only `planner` and `scout` could ever
    // support — exactly `quorum`, which this check refuses — so this desk
    // passes only because the unnamed member is counted.
    let manifest = format!(
        "{}[group_chat.hive]\nquorum = 2\n\n[group_chat.hive.moves]\n\
         planner = [\"support\", \"evidence\", \"defer\"]\n\
         scout = [\"support\", \"evidence\", \"defer\"]\n",
        three_member_manifest()
    );
    let problems = record(&manifest).manifest.validate();
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn a_member_with_an_empty_moves_list_counts_as_an_eligible_supporter() {
    // An empty `hive.moves` entry is read as "every move", same as an
    // unnamed member. Without `critic` counting, `planner` and `scout` alone
    // equal `quorum`, which this check refuses — so this desk passes only
    // because the empty-list member is counted too.
    let manifest = format!(
        "{}[group_chat.hive]\nquorum = 2\n\n[group_chat.hive.moves]\n\
         planner = [\"support\", \"evidence\", \"defer\"]\n\
         scout = [\"support\", \"evidence\", \"defer\"]\n\
         critic = []\n",
        three_member_manifest()
    );
    let problems = record(&manifest).manifest.validate();
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn a_desk_that_never_deliberates_is_not_checked_for_reachable_quorum() {
    // A one-member desk (no `members` list at all here) never opens a hive
    // episode, whatever `hive.quorum` says — so an empty `hive.moves` and a
    // desk of zero declared seats must not read as "quorum unreachable".
    let manifest = "[company]\nname = \"X\"\n\
         [[group_chat]]\nid = \"content\"\nname = \"Content desk\"\n";
    let problems = record(manifest).manifest.validate();
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn the_derived_policy_scales_with_the_room() {
    let policy = |members: usize| HivePolicy::from_config(&HiveConfig::default(), members).episode;
    // A pair can only ever need one supporter — the other one — so a majority
    // that still leaves somebody outside it is exactly one.
    assert_eq!(policy(2).quorum.threshold, 1);
    assert_eq!(policy(3).quorum.threshold, 2);
    assert_eq!(policy(4).quorum.threshold, 3);
    assert_eq!(policy(5).quorum.threshold, 3);
    assert_eq!(policy(3).turn_budget, 9);
    assert!(policy(3).blind_round);
    // An operator's number is honoured, but never one the room could not meet.
    let over = HiveConfig {
        quorum: Some(99),
        ..HiveConfig::default()
    };
    assert_eq!(
        HivePolicy::from_config(&over, 3).episode.quorum.threshold,
        3
    );
}
