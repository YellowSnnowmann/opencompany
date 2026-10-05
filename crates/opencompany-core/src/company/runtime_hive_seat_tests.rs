//! Runtime tests: approvals a company-hive turn parked under its own turn key
//! release the agent with the operator's decisions rather than running a chat
//! cycle (OC-2).

use std::sync::{Arc, Mutex};

#[cfg(feature = "openhuman")]
use crate::ports::blockers::{BlockerKind, BlockerPayload, BlockerSource, BlockerVerdict};
use crate::ports::types::{
    Actor, ActorKind, ApprovalId, ApprovalOrigin, CompanyEvent, Effect, EffectGroup, Verdict,
};
use crate::runtime::approval_park::{ApprovalParker, ParkSite};
use crate::runtime::hive_resume::turn_key;
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

pub(super) async fn park_for_turn(
    rt: &crate::company::runtime::CompanyRuntime,
    episode: Option<&str>,
    agent: &str,
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
            turn: Some(turn_key(agent, episode)),
            origin: Some(ApprovalOrigin::Hive {
                agent_id: agent.to_owned(),
                episode_id: episode.map(str::to_owned),
            }),
        },
    )
    .await
    .expect("parked")
}

#[tokio::test]
async fn a_hive_approval_names_its_agent_and_episode() {
    let (rt, _home) = runtime_with_events().await;
    park_for_turn(
        &rt,
        Some("ep1"),
        "ceo",
        seat_effect("shell", "ceo", serde_json::json!({ "cmd": "ls" })),
    )
    .await;
    let summary = &rt.pending_approvals()[0];
    let hive = summary.hive.as_ref().expect("a hive turn's approval");
    assert_eq!(hive.agent_id, "ceo");
    assert_eq!(hive.episode_id.as_deref(), Some("ep1"));
    assert_eq!(summary.thread.as_deref(), Some("engineering"));
    let wire = serde_json::to_value(summary).unwrap();
    assert_eq!(
        wire["hive"],
        serde_json::json!({ "agentId": "ceo", "episodeId": "ep1" })
    );
    let parked = rt
        .events
        .read_from(&rt.id, crate::ports::types::EventSeq::new(0), usize::MAX)
        .await
        .unwrap()
        .into_iter()
        .find_map(|row| match row.event {
            CompanyEvent::ApprovalParked { origin, .. } => origin,
            _ => None,
        });
    assert_eq!(
        parked,
        Some(ApprovalOrigin::Hive {
            agent_id: "ceo".into(),
            episode_id: Some("ep1".into()),
        }),
        "the park row says whose turn is held"
    );
}

#[tokio::test]
async fn an_ordinary_approval_names_no_hive_turn() {
    let (rt, _home) = runtime_with_events().await;
    super::tests_approval::seed_parked(&rt, "plain", 5_000).await;
    let summary = &rt.pending_approvals()[0];
    assert!(summary.hive.is_none());
    assert!(serde_json::to_value(summary).unwrap().get("hive").is_none());
}

/// A brain that records the releases it is asked for and fails any cycle, so
/// a hive turn's decision that fell through to a chat turn shows up.
#[derive(Default)]
struct ReleaseRecorder {
    released: Mutex<Vec<(String, Option<String>)>>,
    cycles: Mutex<usize>,
    refuse: bool,
}

#[async_trait::async_trait]
impl crate::ports::Brain for ReleaseRecorder {
    async fn run_cycle(
        &self,
        _req: crate::ports::types::CycleRequest,
        _host: &dyn crate::ports::brain::CycleHost,
    ) -> crate::Result<crate::ports::types::CycleResult> {
        *self.cycles.lock().unwrap() += 1;
        Err(crate::error::OpenCompanyError::InvalidRequest(
            "a hive turn's decision must not run a chat cycle".into(),
        ))
    }

    async fn release_hive_agent(&self, agent_id: &str, note: Option<String>) -> bool {
        self.released
            .lock()
            .unwrap()
            .push((agent_id.to_owned(), note));
        !self.refuse
    }
}

async fn runtime_with_brain(
    brain: Arc<ReleaseRecorder>,
) -> (
    Arc<crate::company::runtime::CompanyRuntime>,
    tempfile::TempDir,
) {
    runtime_with_channels(brain, Vec::new()).await
}

async fn runtime_with_channels(
    brain: Arc<ReleaseRecorder>,
    channels: Vec<Arc<dyn crate::ports::ChannelAdapter>>,
) -> (
    Arc<crate::company::runtime::CompanyRuntime>,
    tempfile::TempDir,
) {
    let home = tempfile::tempdir().expect("tempdir");
    let manifest: crate::company::types::CompanyManifest = toml::from_str(
        r#"
        [company]
        name = "Acme"

        [[agent]]
        id = "ceo"
        role = "Chief"

        [policy]
        mode = "supervised"
        "#,
    )
    .expect("manifest");
    let rt = crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest)
        .with_brain(brain)
        .with_channels(channels)
        .build()
        .await
        .expect("runtime");
    (Arc::new(rt), home)
}

fn operator() -> Actor {
    Actor {
        kind: ActorKind::Operator,
        id: "owner".into(),
    }
}

#[tokio::test]
async fn an_approved_call_releases_its_agent_with_the_decision_not_a_chat_cycle() {
    let brain = Arc::new(ReleaseRecorder::default());
    let (rt, _home) = runtime_with_brain(brain.clone()).await;
    let id = park_for_turn(
        &rt,
        Some("ep1"),
        "ceo",
        seat_effect("send_email", "ceo", serde_json::json!({ "to": "a@b.c" })),
    )
    .await;
    rt.resolve_approval(&id, Verdict::Approve, operator())
        .await
        .expect("resolved");
    let released = brain.released.lock().unwrap().clone();
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].0, "ceo");
    let note = released[0].1.as_deref().expect("a release note");
    assert!(note.contains("approved your `send_email` call"), "{note}");
    assert!(
        note.contains("a@b.c"),
        "the note carries the arguments: {note}"
    );
    assert!(
        rt.grants.peek(&id).is_some(),
        "the agent redeems the single-use grant itself"
    );
    assert_eq!(*brain.cycles.lock().unwrap(), 0, "no chat cycle ran");
}

#[cfg(feature = "openhuman")]
#[tokio::test]
async fn each_blocker_verdict_releases_a_question_with_its_distinct_decision() {
    let cases = [
        (
            BlockerVerdict::Retry,
            "",
            "The operator told you to go ahead with what you asked about (what you asked).",
        ),
        (
            BlockerVerdict::Amend,
            "use staging",
            "The operator answered your question (what you asked): \"use staging\"",
        ),
        (
            BlockerVerdict::Skip,
            "",
            "The operator said to skip what you asked about (what you asked) and carry on without it.",
        ),
        (
            BlockerVerdict::Cancel,
            "",
            "The operator declined what you asked about (what you asked). Stop that part of the work.",
        ),
    ];

    for (index, (verdict, answer, expected)) in cases.into_iter().enumerate() {
        let brain = Arc::new(ReleaseRecorder::default());
        let (rt, _home) = runtime_with_brain(brain.clone()).await;
        let payload = BlockerPayload {
            kind: BlockerKind::Information,
            source: BlockerSource::AgentQuestion,
            step: None,
            reason: format!("which cluster for case {index}?"),
            needed: "the cluster name".to_owned(),
            group_key: None,
        };
        let id = park_for_turn(
            &rt,
            Some("ep1"),
            "ceo",
            seat_effect(
                "blocker.information",
                "ceo",
                serde_json::to_value(payload).unwrap(),
            ),
        )
        .await;
        let (receipt, follow_up) = rt
            .apply_blocker_reply_spawned(std::slice::from_ref(&id), &id, verdict, answer, None)
            .await
            .expect("applies the blocker verdict");
        assert_eq!(receipt.outcome(), "settled");
        crate::company::runtime::join_follow_up(follow_up)
            .await
            .expect("releases the parked seat");

        let released = brain.released.lock().unwrap().clone();
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].0, "ceo");
        let note = released[0].1.as_deref().expect("release note");
        assert_eq!(note, expected, "wrong note for {verdict:?}");
    }
}

#[tokio::test]
async fn an_agent_waits_for_every_decision_before_it_is_released() {
    let brain = Arc::new(ReleaseRecorder::default());
    let (rt, _home) = runtime_with_brain(brain.clone()).await;
    let first = park_for_turn(
        &rt,
        None,
        "ceo",
        seat_effect("send_email", "ceo", serde_json::json!({ "n": 1 })),
    )
    .await;
    let second = park_for_turn(
        &rt,
        None,
        "ceo",
        seat_effect("send_email", "ceo", serde_json::json!({ "n": 2 })),
    )
    .await;
    rt.resolve_approval(&first, Verdict::Approve, operator())
        .await
        .unwrap();
    assert!(brain.released.lock().unwrap().is_empty());
    rt.resolve_approval(&second, Verdict::Deny, operator())
        .await
        .unwrap();
    let released = brain.released.lock().unwrap().clone();
    assert_eq!(released.len(), 1);
    let note = released[0].1.as_deref().unwrap();
    assert_eq!(
        note.lines().count(),
        2,
        "both decisions in one note: {note}"
    );
}

#[tokio::test]
async fn an_explicit_request_decision_retires_its_continuation() {
    let brain = Arc::new(ReleaseRecorder::default());
    let (rt, _home) = runtime_with_brain(brain.clone()).await;
    let id = park_for_turn(
        &rt,
        Some("ep1"),
        "ceo",
        seat_effect(
            crate::ports::types::REQUEST_APPROVAL_EFFECT_KIND,
            "ceo",
            serde_json::json!({ "title": "email the client" }),
        ),
    )
    .await;
    rt.resolve_approval(&id, Verdict::Approve, operator())
        .await
        .unwrap();
    let note = brain.released.lock().unwrap()[0].1.clone().unwrap();
    assert!(note.contains("email the client"), "{note}");
    assert!(
        rt.grants.peek_continuation(&id).is_none(),
        "the continuation is retired, so nothing replays it as a chat turn"
    );
    assert!(rt.journal.replayed_approval_continuations().is_empty());
    assert_eq!(*brain.cycles.lock().unwrap(), 0);
}

#[tokio::test]
async fn an_expired_hive_approval_releases_the_agent_as_denied() {
    let brain = Arc::new(ReleaseRecorder::default());
    let (rt, _home) = runtime_with_brain(brain.clone()).await;
    let id = park_for_turn(
        &rt,
        Some("ep1"),
        "ceo",
        seat_effect("send_email", "ceo", serde_json::json!({})),
    )
    .await;
    rt.retire_approval(
        &id,
        crate::runtime::journal::ExpiryReason::Ttl,
        crate::ports::now_millis(),
    )
    .await
    .expect("retired");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if !brain.released.lock().unwrap().is_empty() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "an expiry releases the agent"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let note = brain.released.lock().unwrap()[0].1.clone().unwrap();
    assert!(note.contains("did not approve"), "{note}");
    assert_eq!(*brain.cycles.lock().unwrap(), 0);
}

#[tokio::test]
async fn a_release_nobody_takes_tells_the_operator() {
    let brain = Arc::new(ReleaseRecorder {
        refuse: true,
        ..ReleaseRecorder::default()
    });
    let operator_line =
        crate::runtime::channel::RecordingChannel::new(crate::runtime::channel::OPERATOR_CHANNEL);
    let (rt, _home) =
        runtime_with_channels(brain.clone(), vec![Arc::new(operator_line.clone())]).await;
    let id = park_for_turn(
        &rt,
        None,
        "ceo",
        seat_effect("send_email", "ceo", serde_json::json!({})),
    )
    .await;
    rt.resolve_approval(&id, Verdict::Approve, operator())
        .await
        .unwrap();
    let announced = operator_line
        .sent()
        .iter()
        .any(|message| message.text.contains("no longer waiting"));
    assert!(announced, "the operator is told the decision went nowhere");
}
