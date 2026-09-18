use super::carried_proposal;
use tinyhivemind_hive::{Sequence, SessionAuthor, SessionMessage};

fn msg(seq: u64, content: &str) -> SessionMessage {
    SessionMessage {
        sequence: Sequence(seq),
        author: SessionAuthor::Agent {
            id: "engineer".to_string(),
            label: "Engineer".to_string(),
        },
        content: content.to_string(),
        elided: None,
        audience: tinyhivemind_hive::aside::Audience::Desk,
    }
}

#[test]
fn takes_the_opening_proposal_and_strips_the_grammar() {
    let transcript = vec![
        msg(
            1,
            "thinking about this\n!propose #lazy-load each section, so the page only pays for what is opened",
        ),
        msg(2, "!support #lazy-load ^1 agreed"),
    ];
    assert_eq!(
        carried_proposal(&transcript, "lazy-load").as_deref(),
        Some("each section, so the page only pays for what is opened"),
        "the marker and topic are stripped, and prose above the move is ignored"
    );
}

/// `#lazy` must not answer for `#lazy-load`: a plain prefix match would
/// report the wrong decision, which is worse than reporting none.
#[test]
fn a_topic_that_merely_prefixes_another_does_not_match() {
    let transcript = vec![msg(1, "!propose #lazy-load defer every section")];
    assert_eq!(carried_proposal(&transcript, "lazy"), None);
}

#[test]
fn a_topic_never_proposed_in_the_window_is_absent() {
    let transcript = vec![msg(1, "!support #stage ^0 no proposal survives here")];
    assert_eq!(carried_proposal(&transcript, "stage"), None);
}
/// The live failure this rule exists for: two members filed opposite
/// answers under one id named after the QUESTION, and the fold counted
/// both `!propose` traces as backing the same topic — a reported quorum
/// of two for a decision they disagreed about.
#[test]
fn a_second_proposal_under_one_id_is_caught() {
    let floor = vec![msg(
        1,
        "!propose #decide-one lazy-load each section because settings pages are single-purpose",
    )];
    assert_eq!(
        super::reuses_a_topic(
            "!propose #decide-one load everything up front, because sections are small forms",
            &floor,
        )
        .as_deref(),
        Some("decide-one"),
    );
    // Supporting it is the right move and is never corrected.
    assert_eq!(
        super::reuses_a_topic("!support #decide-one ^1 agreed", &floor),
        None
    );
    // A genuinely new option is not a reuse.
    assert_eq!(
        super::reuses_a_topic(
            "!propose #load-upfront one fetch, instant switching",
            &floor
        ),
        None
    );
}

/// `#lazy` and `#lazy-load` are different topics — a prefix match would
/// correct a member for proposing something nobody had proposed.
#[test]
fn a_topic_that_merely_prefixes_another_is_not_a_reuse() {
    let floor = vec![msg(1, "!propose #lazy-load defer every section")];
    assert_eq!(
        super::reuses_a_topic("!propose #lazy do it later", &floor),
        None
    );
}
