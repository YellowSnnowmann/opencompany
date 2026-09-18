/// The byline an agent is handed and the one the console is shipped are the
/// same string.
///
/// The console's raw view renders the cue line an agent received, and it
/// gets the author half from `MessageView::cue_author` on the session
/// route. If these two projections ever answered differently, that view
/// would be putting a name in front of an operator that the agent never
/// saw — which is the one thing it exists not to do. They are separate
/// matches in separate modules (one is `#[cfg(feature = "openhuman")]`),
/// so nothing but this test holds them together.
#[test]
fn cue_author_matches_the_envelope_the_agent_is_handed() {
    use crate::ports::types::{Actor, ActorKind};

    let cases = [
        // A signed-in person: the stable user id, never their screen name.
        Some(Actor {
            kind: ActorKind::User,
            id: "01a08d773a62-000000000034".to_string(),
        }),
        // A crossing referral, authored by a teammate: no person to name.
        Some(Actor {
            kind: ActorKind::Agent,
            id: "engineer".to_string(),
        }),
        // A machine credential, or a line journaled before attribution.
        None,
    ];
    for by in cases {
        let event = CompanyEvent::OperatorMessage {
            text: "hello".to_string(),
            by: by.clone(),
            chat: None,
            parent: None,
            deliverable: None,
            mentions: Vec::new(),
            attachments: Vec::new(),
        };
        let (author, _, _) = body_of("brand_designer", &event).expect("a body");
        assert_eq!(
            author,
            crate::server::chat_history::cue_author(&by),
            "the cue's author and the route's `cue_author` disagree for {by:?}",
        );
    }
}

use super::*;
use crate::ports::types::StoredEvent;
use async_trait::async_trait;
use futures::stream::{self, BoxStream};

/// A log that replays a fixed history, newest-first for `read_before`.
struct FixedLog(Vec<StoredEvent>);

#[async_trait]
impl EventLog for FixedLog {
    async fn append(&self, _id: &CompanyId, _e: CompanyEvent) -> crate::Result<EventSeq> {
        unreachable!("a session delta only reads")
    }
    async fn read_from(
        &self,
        _id: &CompanyId,
        seq: EventSeq,
        limit: usize,
    ) -> crate::Result<Vec<StoredEvent>> {
        Ok(self
            .0
            .iter()
            .filter(|e| e.seq >= seq)
            .take(limit)
            .cloned()
            .collect())
    }
    fn subscribe(
        &self,
        _id: &CompanyId,
    ) -> BoxStream<'static, crate::ports::events::EventStreamItem> {
        Box::pin(stream::empty())
    }
}

fn op(seq: u64, chat: &str, text: &str) -> StoredEvent {
    StoredEvent {
        seq: EventSeq::new(seq),
        company: CompanyId::new("acme"),
        event: CompanyEvent::OperatorMessage {
            text: text.to_string(),
            by: None,
            chat: Some(chat.to_string()),
            parent: None,
            deliverable: None,
            mentions: Vec::new(),
            attachments: Vec::new(),
        },
        at_millis: seq,
    }
}

fn reply(seq: u64, chat: &str, author: &str, text: &str, audience: &[&str]) -> StoredEvent {
    StoredEvent {
        seq: EventSeq::new(seq),
        company: CompanyId::new("acme"),
        event: CompanyEvent::AgentReply {
            audience: audience.iter().map(|id| id.to_string()).collect(),
            chat_id: chat.to_string(),
            agent_id: author.to_string(),
            text: text.to_string(),
            steps: Vec::new(),
            task_id: None,
            outputs: Vec::new(),
            parent: None,
            mentions: Vec::new(),
            mention_depth: 0,
        },
        at_millis: seq,
    }
}

/// A company where `designer` sits on `#brand` and `copy` does not.
///
/// Built from TOML rather than by struct literal, on the same reasoning
/// `hivemind::test::record` uses: the manifest this exercises is the one an
/// operator writes, and a literal would let a field drift out of the parse
/// path without any test noticing.
fn record() -> CompanyRecord {
    let manifest: crate::company::CompanyManifest = toml::from_str(
        r#"
[company]
name = "Acme"

[[agent]]
id = "designer"
role = "Designer"

[[agent]]
id = "copy"
role = "Writer"

[[group_chat]]
id = "brand"
name = "Brand"
members = ["designer"]

[[group_chat]]
id = "finance"
name = "Finance"
members = ["copy"]
"#,
    )
    .expect("test manifest parses");
    CompanyRecord {
        id: CompanyId::new("acme"),
        manifest,
        ledger: Vec::new(),
        lifecycle: "running".to_string(),
        overlay_agents: Vec::new(),
        overlay_desk_members: Vec::new(),
        overlay_desk_order: Vec::new(),
        overlay_desks: Vec::new(),
        overlay_desk_hive: Vec::new(),
        overlay_retired_agents: Vec::new(),
        overlay_agent_edits: Vec::new(),
        overlay_workflows: Vec::new(),
        overlay_budgets: Vec::new(),
        overlay_policy: None,
        overlay_tool_grants: None,
        overlay_desk_tools: Default::default(),
        disabled_workflows: Vec::new(),
        template_provenance: None,
        setup: None,
        name_confirmed: false,
        activation_completed_at: None,
        created_at_millis: None,
    }
}

fn log(events: Vec<StoredEvent>) -> Arc<dyn EventLog> {
    Arc::new(FixedLog(events))
}

/// The whole point of the module: a channel switch is no longer a reason to
/// forget. What the agent has already seen is what it does not get again —
/// not what channel it was on when it saw it.
#[tokio::test]
async fn a_delta_hands_over_only_what_this_agent_has_not_seen() {
    let events = log(vec![
        op(1, "brand", "start the hero"),
        reply(2, "brand", "designer", "on it", &[]),
        op(3, "dm:designer", "what did you decide?"),
    ]);
    let state = AgentSessionState {
        watermark: Some(EventSeq::new(2)),
        present_above_watermark: BTreeSet::new(),
    };
    let plan = prepare_delta(
        &events,
        &CompanyId::new("acme"),
        &record(),
        "designer",
        &state,
        Some(EventSeq::new(3)),
    )
    .await;
    let SessionPlan::Delta { envelopes, .. } = plan else {
        panic!("expected a delta, got {plan:?}");
    };
    // Rows 1 and 2 are below the watermark; row 3 is the turn's own message
    // and is excluded by the exclusive `before`. Nothing is left, and that
    // is correct — the agent has seen everything.
    assert!(envelopes.is_empty(), "{envelopes:?}");
}

/// A line said on a desk the agent sits on reaches it even though the turn
/// it is taking is in a different conversation. Before the session was
/// continuous this was unreachable by construction.
#[tokio::test]
async fn a_line_from_another_channel_reaches_the_session() {
    let events = log(vec![
        reply(1, "brand", "copy", "tone should be warmer", &[]),
        op(2, "dm:designer", "redo the hero"),
    ]);
    let state = AgentSessionState {
        watermark: Some(EventSeq::new(0)),
        present_above_watermark: BTreeSet::new(),
    };
    let plan = prepare_delta(
        &events,
        &CompanyId::new("acme"),
        &record(),
        "designer",
        &state,
        Some(EventSeq::new(2)),
    )
    .await;
    let SessionPlan::Delta { envelopes, .. } = plan else {
        panic!("expected a delta, got {plan:?}");
    };
    assert_eq!(envelopes.len(), 1, "{envelopes:?}");
    assert_eq!(envelopes[0].channel, "#Brand");
    assert_eq!(envelopes[0].author, "copy");
    assert!(!envelopes[0].mine);
}

/// A desk this agent is not on is not in its session, however busy it is.
/// Merging the channels widened what an agent reads; it did not make the
/// company one room.
#[tokio::test]
async fn a_desk_this_agent_is_not_on_stays_out() {
    let events = log(vec![
        reply(1, "finance", "copy", "the invoice is out", &[]),
        op(2, "dm:designer", "anything from finance?"),
    ]);
    let state = AgentSessionState {
        watermark: Some(EventSeq::new(0)),
        present_above_watermark: BTreeSet::new(),
    };
    let plan = prepare_delta(
        &events,
        &CompanyId::new("acme"),
        &record(),
        "designer",
        &state,
        Some(EventSeq::new(2)),
    )
    .await;
    let SessionPlan::Delta {
        envelopes,
        next_state,
    } = plan
    else {
        panic!("expected a delta, got {plan:?}");
    };
    assert!(envelopes.is_empty(), "{envelopes:?}");
    // Codex P1: neither row produced an envelope, but both were scanned —
    // and both must still be marked consumed. Before the fix, `accept`
    // was never called for either, so the watermark stayed pinned at its
    // starting value forever: every later turn would rescan the same two
    // non-deliverable rows, and once that rescan crossed
    // `SESSION_SCAN_LIMIT` it would reinitialize instead of completing,
    // permanently falling back to a chat-only reseed.
    assert_eq!(
        next_state.watermark,
        Some(EventSeq::new(2)),
        "scanned-but-nondeliverable rows must still advance the watermark: {next_state:?}"
    );
}

/// The one narrowing that survives. Channel isolation was dropped
/// deliberately; audience isolation is not, and a private exchange this
/// agent is not party to must never arrive in its context.
#[tokio::test]
async fn an_aside_this_agent_is_not_party_to_never_arrives() {
    let events = log(vec![
        reply(
            1,
            "brand",
            "copy",
            "between us: the client hated it",
            &["ceo"],
        ),
        reply(2, "brand", "copy", "and to you: ship it", &["designer"]),
        op(3, "brand", "where are we?"),
    ]);
    let state = AgentSessionState {
        watermark: Some(EventSeq::new(0)),
        present_above_watermark: BTreeSet::new(),
    };
    let plan = prepare_delta(
        &events,
        &CompanyId::new("acme"),
        &record(),
        "designer",
        &state,
        Some(EventSeq::new(3)),
    )
    .await;
    let SessionPlan::Delta { envelopes, .. } = plan else {
        panic!("expected a delta, got {plan:?}");
    };
    let texts: Vec<&str> = envelopes.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(texts, vec!["and to you: ship it"], "{envelopes:?}");
}

/// A session with no watermark has nothing to be a delta against, and says
/// so rather than replaying the company's whole history into a cold agent.
#[tokio::test]
async fn a_cold_session_asks_for_a_seed() {
    let plan = prepare_delta(
        &log(vec![op(1, "brand", "hello")]),
        &CompanyId::new("acme"),
        &record(),
        "designer",
        &AgentSessionState::default(),
        Some(EventSeq::new(2)),
    )
    .await;
    assert!(
        matches!(
            plan,
            SessionPlan::Reinitialize {
                reason: ReinitializeReason::ColdStart
            }
        ),
        "{plan:?}"
    );
}

/// An agent away long enough that the replay would be worse than the
/// recent window gets the recent window.
#[tokio::test]
async fn too_much_unseen_falls_back_to_a_seed() {
    let mut events = Vec::new();
    for seq in 1..=(SESSION_DELTA_LIMIT as u64 + 5) {
        events.push(reply(seq, "brand", "copy", &format!("line {seq}"), &[]));
    }
    let state = AgentSessionState {
        watermark: Some(EventSeq::new(0)),
        present_above_watermark: BTreeSet::new(),
    };
    let plan = prepare_delta(
        &log(events),
        &CompanyId::new("acme"),
        &record(),
        "designer",
        &state,
        None,
    )
    .await;
    assert!(
        matches!(
            plan,
            SessionPlan::Reinitialize {
                reason: ReinitializeReason::TooManyUnseen
            }
        ),
        "{plan:?}"
    );
}

/// The watermark swallows a contiguous run rather than growing a set, so a
/// busy session does not carry an ever-longer list of individual sequences.
#[test]
fn accepting_a_run_compacts_into_the_watermark() {
    let mut state = AgentSessionState::default();
    state.accept(EventSeq::new(1));
    state.accept(EventSeq::new(2));
    state.accept(EventSeq::new(3));
    assert_eq!(state.watermark, Some(EventSeq::new(3)));
    assert!(state.present_above_watermark.is_empty());
}

/// Codex P1: a chat-only reseed's recent-window seed covers only the
/// incoming channel. Overwriting the watermark with the turn's own
/// sequence — as this used to — would mark an OLDER, still-unseen row on
/// some OTHER channel "already delivered" underneath it, and it would
/// never reach the agent. `reseeded` must leave the prior watermark where
/// it was: a row below it stays exactly as seen or unseen as it already
/// was, and only the turn's own message is newly accepted.
#[test]
fn reseed_does_not_swallow_an_older_unseen_row_on_another_channel() {
    // Desk B's message at seq 6 has never been delivered.
    let before_reseed = AgentSessionState {
        watermark: Some(EventSeq::new(5)),
        present_above_watermark: BTreeSet::new(),
    };
    // A greeting on desk A, seq 7, triggers a chat-only reseed.
    let after_reseed = before_reseed.reseeded(Some(EventSeq::new(7)));
    assert_eq!(
        after_reseed.watermark,
        Some(EventSeq::new(5)),
        "the prior watermark must survive the reseed unchanged"
    );
    assert!(
        !after_reseed.already_seen(EventSeq::new(6)),
        "desk B's still-unseen row must not become 'delivered' by a reseed \
         that never showed it to the agent"
    );
    assert!(
        after_reseed.already_seen(EventSeq::new(7)),
        "the turn's own message, which the seed DID show the agent, is accepted"
    );
}

#[test]
fn an_isolated_hive_turn_marks_only_its_trigger_seen() {
    let mut state = AgentSessionState {
        watermark: Some(EventSeq::new(5)),
        present_above_watermark: BTreeSet::new(),
    };
    state.accept_seen(EventSeq::new(7));

    assert_eq!(state.watermark, Some(EventSeq::new(5)));
    assert!(!state.already_seen(EventSeq::new(6)));
    assert!(state.already_seen(EventSeq::new(7)));
}

/// A true cold start (no watermark at all yet) must still come out of its
/// first reseed WITH a watermark — otherwise `session.watermark.is_none()`
/// keeps tripping the reseed branch forever and the agent can never walk a
/// delta.
#[test]
fn reseed_from_a_true_cold_start_still_establishes_a_watermark() {
    let cold = AgentSessionState::default();
    let after_reseed = cold.reseeded(Some(EventSeq::new(3)));
    assert_eq!(after_reseed.watermark, Some(EventSeq::new(3)));
    assert!(after_reseed.present_above_watermark.is_empty());
}

/// The agent's own lines are in the session — it said them — but they are
/// not cued as things it needs to read. Cueing them would have the model
/// answering its own last reply.
#[test]
fn the_cue_block_leaves_out_this_agents_own_lines() {
    let mine = Envelope {
        seq: EventSeq::new(1),
        channel: "#Brand".to_string(),
        author: "designer".to_string(),
        mine: true,
        text: "on it".to_string(),
    };
    let theirs = Envelope {
        seq: EventSeq::new(2),
        channel: "#Brand".to_string(),
        author: "copy".to_string(),
        mine: false,
        text: "warmer".to_string(),
    };
    assert!(render_cues(std::slice::from_ref(&mine)).is_none());
    let rendered = render_cues(&[mine, theirs]).expect("a peer line is cued");
    assert!(rendered.contains("[#Brand · copy] warmer"), "{rendered}");
    assert!(!rendered.contains("on it"), "{rendered}");
}
