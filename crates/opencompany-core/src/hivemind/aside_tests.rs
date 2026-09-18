use super::*;
use crate::hivemind::aside::{opens_aside, party, spent_and_unsettled, surfaces};
use tinyhivemind_hive::aside::{AsidePolicy, Audience};
use tinyhivemind_hive::{Sequence, SessionAuthor, SessionMessage};

fn row(seq: u64, author: &str, content: &str, audience: Audience) -> SessionMessage {
    SessionMessage {
        sequence: Sequence(seq),
        author: SessionAuthor::Agent {
            id: author.to_owned(),
            label: author.to_owned(),
        },
        content: content.to_owned(),
        audience,
        elided: None,
    }
}

fn aside_to(who: &[&str]) -> Audience {
    Audience::Aside {
        members: who.iter().map(|s| (*s).to_owned()).collect(),
    }
}

#[test]
fn a_default_config_authorizes_nothing() {
    let config = AsideConfig::default();
    assert!(!config.enabled());
    assert_eq!(config.policy(), AsidePolicy::DEFAULT);
    assert!(config.is_default());
}

/// A disabled desk that nevertheless wrote bounds must not read as a desk
/// that is on: every bound goes to zero with the switch.
#[test]
fn a_disabled_config_discards_its_own_bounds() {
    let config = AsideConfig {
        enabled: Some(false),
        max_members: Some(3),
        max_messages: Some(9),
        ..AsideConfig::default()
    };
    assert_eq!(config.policy(), AsidePolicy::DEFAULT);
}

#[test]
fn an_enabled_config_defaults_to_a_settled_pair() {
    let policy = AsideConfig {
        enabled: Some(true),
        ..AsideConfig::default()
    }
    .policy();
    assert!(policy.enabled);
    assert_eq!(policy.max_members, 1, "a pair, not a caucus");
    assert_eq!(policy.max_messages, 2, "a question and an answer");
    assert!(policy.must_surface, "the room is owed a settlement");
    assert!(
        !policy.require_thread,
        "a hive episode runs on the desk channel; requiring a thread would \
         authorize nothing"
    );
}

#[test]
fn markers_match_on_a_word_boundary_only() {
    assert!(opens_aside("!aside @auditor is this figure right?"));
    assert!(opens_aside("  !aside @auditor"));
    assert!(opens_aside("!aside"));
    assert!(!opens_aside("!asides @auditor"));
    assert!(!opens_aside("I would !aside but cannot"));
    assert!(!opens_aside("!propose #stage"));

    assert!(surfaces(
        "!surface the auditor and I agree the figure holds"
    ));
    assert!(!surfaces("!surfaced it already"));
}

/// The party is the author plus the addressees, canonically ordered, so the
/// same pair folds to one key whichever of them wrote the row.
#[test]
fn a_party_is_order_independent_and_includes_its_author() {
    let one = party("planner", &aside_to(&["auditor"])).expect("an aside");
    let other = party("auditor", &aside_to(&["planner"])).expect("an aside");
    assert_eq!(one, other);
    assert_eq!(one, vec!["auditor".to_owned(), "planner".to_owned()]);
    assert!(
        party("planner", &Audience::Desk).is_none(),
        "a desk row is in no party"
    );
}

#[test]
fn spending_counts_only_this_partys_rows() {
    let party_key = party("planner", &aside_to(&["auditor"])).expect("an aside");
    let transcript = vec![
        row(
            1,
            "planner",
            "!aside @auditor check this",
            aside_to(&["auditor"]),
        ),
        row(
            2,
            "auditor",
            "!aside @planner it holds",
            aside_to(&["planner"]),
        ),
        // A different pair entirely: must not spend this party's budget.
        row(
            3,
            "planner",
            "!aside @scout and this?",
            aside_to(&["scout"]),
        ),
        row(4, "critic", "!propose #stage", Audience::Desk),
    ];
    let (spent, unsettled) = spent_and_unsettled(&transcript, &party_key);
    assert_eq!(spent, 2);
    assert!(unsettled, "nobody surfaced it");
}

#[test]
fn a_surface_by_either_member_settles_the_party() {
    let party_key = party("planner", &aside_to(&["auditor"])).expect("an aside");
    let mut transcript = vec![row(
        1,
        "planner",
        "!aside @auditor check this",
        aside_to(&["auditor"]),
    )];
    assert!(spent_and_unsettled(&transcript, &party_key).1);

    // The *other* member pays it back, which is enough: the room is owed
    // one settlement, not one per participant.
    transcript.push(row(
        2,
        "auditor",
        "!surface the figure holds",
        Audience::Desk,
    ));
    let (spent, unsettled) = spent_and_unsettled(&transcript, &party_key);
    assert_eq!(spent, 1, "a settlement is a desk row, not an aside row");
    assert!(!unsettled);
}

/// An ordinary desk turn by a member is not a settlement. Otherwise the
/// requirement is paid by whatever anybody happened to say next, and the
/// room never learns what the aside produced.
#[test]
fn an_ordinary_desk_turn_does_not_settle_an_aside() {
    let party_key = party("planner", &aside_to(&["auditor"])).expect("an aside");
    let transcript = vec![
        row(
            1,
            "planner",
            "!aside @auditor check this",
            aside_to(&["auditor"]),
        ),
        row(2, "planner", "!propose #stage Stage it.", Audience::Desk),
    ];
    assert!(
        spent_and_unsettled(&transcript, &party_key).1,
        "a proposal is not a settlement"
    );
}

/// A `!surface` from somebody who was never in the aside settles nothing —
/// they have nothing to report and cannot discharge a debt they do not owe.
#[test]
fn a_surface_by_a_non_member_settles_nothing() {
    let party_key = party("planner", &aside_to(&["auditor"])).expect("an aside");
    let transcript = vec![
        row(
            1,
            "planner",
            "!aside @auditor check this",
            aside_to(&["auditor"]),
        ),
        row(2, "critic", "!surface they seemed to agree", Audience::Desk),
    ];
    assert!(spent_and_unsettled(&transcript, &party_key).1);
}

/// Reopening after a settlement starts the debt again, and the spend count
/// keeps rising: the budget is per party per episode, not per aside.
#[test]
fn reopening_after_a_settlement_owes_the_room_again() {
    let party_key = party("planner", &aside_to(&["auditor"])).expect("an aside");
    let transcript = vec![
        row(
            1,
            "planner",
            "!aside @auditor check this",
            aside_to(&["auditor"]),
        ),
        row(2, "auditor", "!surface the figure holds", Audience::Desk),
        row(
            3,
            "planner",
            "!aside @auditor and the other one?",
            aside_to(&["auditor"]),
        ),
    ];
    let (spent, unsettled) = spent_and_unsettled(&transcript, &party_key);
    assert_eq!(spent, 2);
    assert!(unsettled);
}

/// The operator's own rows carry no agent author and must never be folded
/// into anybody's party or counted as anybody's settlement.
#[test]
fn operator_rows_are_in_no_party() {
    let party_key = party("planner", &aside_to(&["auditor"])).expect("an aside");
    let transcript = vec![
        SessionMessage {
            sequence: Sequence(1),
            author: SessionAuthor::Operator,
            content: "!surface".to_owned(),
            audience: Audience::Desk,
            elided: None,
        },
        row(
            2,
            "planner",
            "!aside @auditor check this",
            aside_to(&["auditor"]),
        ),
    ];
    let (spent, unsettled) = spent_and_unsettled(&transcript, &party_key);
    assert_eq!(spent, 1);
    assert!(unsettled, "an operator cannot settle an agents' aside");
}
