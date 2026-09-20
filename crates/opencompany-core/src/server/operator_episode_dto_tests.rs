//! Serialization coverage for the episode metadata a history row and an
//! `agent_reply` frame carry (plan hive-desks, Phase 4).

use crate::hive::routing::{Router, RoutingPlanDto};
use crate::ports::types::{ReplyEpisode, RoutedBy, UtteranceKind};

/// The wire shape the console binds to (`MessageEpisodeDto` in
/// `frontend/src/api/types.ts`).
#[test]
fn the_episode_metadata_reaches_the_wire_as_camel_case() {
    let episode = ReplyEpisode {
        id: "ep-1".into(),
        revision: 2,
        kind: UtteranceKind::Broadcast,
        to: Vec::new(),
        routed_by: Some(RoutedBy {
            plan: RoutingPlanDto::One {
                primary_id: "ceo".into(),
            },
            router: Router::Jev,
        }),
    };
    let wire = super::episode_json(&episode);
    assert_eq!(
        wire,
        serde_json::json!({
            "id": "ep-1",
            "revision": 2,
            "kind": "broadcast",
            "routedBy": {"plan": {"kind": "one", "primaryId": "ceo"}, "router": "jev"},
        }),
        "camelCase, `to` omitted when empty: {wire}"
    );
    let dm = ReplyEpisode {
        id: "ep-1".into(),
        revision: 3,
        kind: UtteranceKind::Dm,
        to: vec!["engineer".into()],
        routed_by: None,
    };
    let wire = super::episode_json(&dm);
    assert_eq!(wire["kind"], "dm");
    assert_eq!(wire["to"], serde_json::json!(["engineer"]));
    assert!(wire.get("routedBy").is_none());
    let done = ReplyEpisode {
        kind: UtteranceKind::CompleteEpisode,
        ..dm
    };
    assert_eq!(super::episode_json(&done)["kind"], "complete_episode");
}

/// The history DTO carries `episode` and `audience` beside the row, and
/// neither key appears on a row outside an episode.
#[test]
fn a_history_row_carries_episode_and_audience_only_when_set() {
    let mut view = crate::server::chat_history::MessageView::for_test(
        "7",
        "ceo",
        "Use the short form.",
        vec!["engineer".into()],
    );
    view.episode = Some(ReplyEpisode {
        id: "ep-1".into(),
        revision: 1,
        kind: UtteranceKind::Dm,
        to: vec!["engineer".into()],
        routed_by: None,
    });
    let dto = super::ChatHistoryMessageDto::from(view);
    let wire = serde_json::to_value(&dto).expect("the DTO serializes");
    assert_eq!(wire["episode"]["id"], "ep-1");
    assert_eq!(wire["episode"]["kind"], "dm");
    assert_eq!(wire["audience"], serde_json::json!(["engineer"]));
    assert!(wire.get("asideConversation").is_none());
    assert!(wire.get("cueText").is_none(), "text and cueText are equal: {wire}");

    let plain = crate::server::chat_history::MessageView::for_test("8", "ceo", "hi", Vec::new());
    let wire = serde_json::to_value(super::ChatHistoryMessageDto::from(plain)).unwrap();
    assert!(wire.get("episode").is_none());
    assert!(wire.get("audience").is_none());
}
