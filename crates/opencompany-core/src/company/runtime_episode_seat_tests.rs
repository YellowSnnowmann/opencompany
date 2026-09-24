//! Runtime tests: approvals a hive episode seat parked under its own turn key.

use crate::ports::types::{ApprovalId, Effect, EffectGroup};
use crate::runtime::approval_park::{ApprovalParker, ParkSite};
use crate::runtime::episode_resume::turn_key;
use crate::runtime::journal::{ApprovalConversation, TaskLink};

use super::tests_approval::runtime_with_events;

pub(super) fn seat_effect(kind: &str, agent: &str, payload: serde_json::Value) -> Effect {
    Effect {
        kind: kind.to_owned(),
        group: EffectGroup::Other,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload,
        agent: Some(agent.to_owned()),
        run_id: None,
    }
}

pub(super) async fn park_for_seat(
    rt: &crate::company::runtime::CompanyRuntime,
    episode: &str,
    seat: &str,
    effect: Effect,
) -> ApprovalId {
    ApprovalParker::new(
        rt.approvals.clone(),
        rt.journal.clone(),
        rt.grants.clone(),
        rt.continuations.clone(),
        rt.events.clone(),
    )
    .park(
        &rt.id,
        effect,
        ParkSite {
            task: TaskLink::Unlinked,
            conversation: ApprovalConversation {
                thread: Some("engineering".to_owned()),
                parent: None,
            },
            turn: Some(turn_key(episode, seat)),
        },
    )
    .await
    .expect("parked")
}

#[tokio::test]
async fn a_seat_approval_names_its_episode_and_seat() {
    let (rt, _home) = runtime_with_events().await;
    park_for_seat(
        &rt,
        "ep1",
        "ceo",
        seat_effect("shell", "ceo", serde_json::json!({ "cmd": "ls" })),
    )
    .await;
    let summary = &rt.pending_approvals()[0];
    let episode = summary
        .episode
        .as_ref()
        .expect("an episode seat's approval");
    assert_eq!(episode.id, "ep1");
    assert_eq!(episode.seat, "ceo");
    assert_eq!(summary.thread.as_deref(), Some("engineering"));
    let wire = serde_json::to_value(summary).unwrap();
    assert_eq!(
        wire["episode"],
        serde_json::json!({ "id": "ep1", "seat": "ceo" })
    );
}

#[tokio::test]
async fn an_ordinary_approval_names_no_episode() {
    let (rt, _home) = runtime_with_events().await;
    super::tests_approval::seed_parked(&rt, "plain", 5_000).await;
    let summary = &rt.pending_approvals()[0];
    assert!(summary.episode.is_none());
    assert!(
        serde_json::to_value(summary)
            .unwrap()
            .get("episode")
            .is_none()
    );
}
