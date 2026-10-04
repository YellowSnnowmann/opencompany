//! The hooks' bookkeeping that does not need a live agent: where a turn
//! answers, and that a turn the hooks never prepared completes untouched.

use std::collections::HashMap;

use tinyhivemind_hives::{EpisodeContext, TurnDisposition};

use super::*;

fn hooks() -> HiveHooks {
    let roster = Arc::new(HiveRoster::default());
    roster.install(HashMap::from([(
        "acme--writer".to_string(),
        "writer".to_string(),
    )]));
    HiveHooks::new(
        CompanyId::new("acme"),
        Arc::new(HiveAgents::default()),
        roster,
        Arc::new(TurnMetaBoard::default()),
    )
}

fn scope(destination: Destination) -> TurnScope {
    TurnScope {
        agent_id: "acme--writer".into(),
        episode: matches!(destination, Destination::Hive(_)).then(|| EpisodeContext {
            episode_id: "ep-1".into(),
            hive_id: "content".into(),
            thread: None,
            brief: String::new(),
        }),
        message_ids: vec!["op:1".into()],
        senders: vec![tinyhivemind_hives::HOST_ID.into()],
        destination,
        thread: None,
    }
}

#[test]
fn a_hive_turn_answers_in_its_desk_and_a_direct_one_in_the_agents_dm() {
    let hooks = hooks();
    assert_eq!(
        hooks.chat_of(&scope(Destination::Hive("content".into()))),
        "content"
    );
    assert_eq!(
        hooks.chat_of(&scope(Destination::Agent("acme--writer".into()))),
        "dm:writer"
    );
}

#[test]
fn an_agent_with_no_seat_runs_bare_and_completes() {
    let hooks = hooks();
    let scope = scope(Destination::Agent("acme--writer".into()));
    assert_eq!(hooks.prepare(&scope), TurnOptions::default());
    assert!(hooks.progress(&scope).is_none(), "no pump without a seat");
    assert_eq!(
        hooks.after_turn(&scope, None).expect("after_turn"),
        TurnDisposition::Completed
    );
}

#[test]
fn the_agent_map_drops_a_handle_on_remove() {
    let agents = HiveAgents::default();
    assert!(agents.get("acme--writer").is_none());
    assert!(agents.remove("acme--writer").is_none());
    assert!(agents.ids().is_empty());
}
