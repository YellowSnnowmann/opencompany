//! One episode's card budget: three cards between its agents, no title twice.

use super::*;

#[test]
fn an_episode_opens_at_most_three_distinct_cards() {
    let cards = EpisodeCards::default();
    let budget = cards.budget("ep-1");
    assert!(budget.reserve("Draft the post").is_ok());
    assert_eq!(
        budget.reserve("draft  the POST!"),
        Err(CardRefusal::Duplicate),
        "titles are compared normalised"
    );
    assert!(budget.reserve("Edit the post").is_ok());
    assert!(budget.reserve("Publish the post").is_ok());
    assert_eq!(
        budget.reserve("Promote the post"),
        Err(CardRefusal::Full {
            cap: EPISODE_CARD_CAP
        })
    );
    budget.release("Publish the post");
    assert!(budget.reserve("Promote the post").is_ok(), "a release frees a slot");
}

#[test]
fn each_episode_has_its_own_budget_and_agents_share_it() {
    let cards = EpisodeCards::default();
    assert!(cards.budget("ep-1").reserve("Same title").is_ok());
    assert_eq!(
        cards.budget("ep-1").reserve("Same title"),
        Err(CardRefusal::Duplicate),
        "a second agent in the episode sees the first one's card"
    );
    assert!(cards.budget("ep-2").reserve("Same title").is_ok());
}
