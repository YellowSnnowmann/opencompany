//! One agent, one session.
//!
//! # What changed, and why
//!
//! Before this module, an agent had no continuous existence. One
//! [`oh::agent::Agent`] is held per `(company, agent_id)`, but
//! [`CompanyAgent::run_with_steer`](super::CompanyAgent::run_with_steer)
//! called `clear_history()` every time the incoming chat differed from the
//! bound one and re-seeded a window of *that desk's* transcript. So an agent
//! that answered you in a DM and then answered on `#general` was, from its own
//! point of view, two entities that happened to share a name: it could not
//! notice that the question just asked in the DM is the one it answered on the
//! desk an hour ago, and it could not hold a thought between rooms.
//!
//! This module is the replacement. The session is never cleared. Each turn the
//! agent is handed the rows it has **not yet seen**, across **every** channel
//! it can read, each stamped with where it came from.
//!
//! # The watermark, not a mailbox
//!
//! There is no queue and no per-agent inbox. `tinyhivemind` refuses to have one
//! — `NoDispatchReason::SelfMention`, `NoReferralReason::SelfMention` and
//! `UtteranceRejection::SelfRecipient` all decline self-addressing by name —
//! because the architecture is stigmergic: work leaves an attributed trace in a
//! shared, globally sequenced log, and the trace is the stimulus for the next
//! turn. Continuity is a **watermark the host owns**.
//!
//! [`AgentSessionState`] is that watermark, modelled on
//! `tinyhivemind::sharing::SharingState` **minus its `conversation` field**.
//! That field is what makes the vendored type return
//! `ReinitializeReason::ConversationChanged` on a channel switch — the exact
//! behaviour this module exists to remove — so the shape is reused and the key
//! is not.
//!
//! # Channel isolation is dropped; audience isolation is not
//!
//! Merging the channels is deliberate, and it gives up the isolation
//! `#1725` / `#1730` / `#1890` installed. What replaces it is the **cue**: every
//! delivered row is prefixed with the channel it was said on, per line, through
//! the same [`prefix_every_line`](super::chat_seed) machinery that already
//! defends attribution against forgery.
//!
//! What is **not** given up is who may read what. An aside this agent is not
//! party to still never reaches it: [`Audience::admits`] is applied here with
//! `Viewer::Agent`, exactly as `EpisodeDriver` applies it. Privacy between
//! agents is a deliberation device and never a security boundary — an operator
//! reads everything — but a peer agent's narrowing is real and stays.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::ports::EventLog;
use crate::ports::types::{CompanyEvent, CompanyId, CompanyRecord, EventSeq};
use crate::server::chat_history::{self, Channel, agent_channels};

/// How many raw journal events one delta walk may read before giving up and
/// asking for a full re-seed.
///
/// Mirrors `tinyhivemind::session::SCAN_LIMIT`. The walk counts **raw** rows,
/// not delivered ones, so an agent on one quiet desk in a busy company crosses
/// this sooner than its own traffic suggests — which is why crossing it is a
/// re-seed rather than an error.
pub const SESSION_SCAN_LIMIT: usize = 2048;

/// The most rows one delta hands over.
///
/// A turn that would be handed more than this has been away long enough that
/// the recent window is a better context than a partial replay, so it re-seeds
/// instead. Mirrors `tinyhivemind::session::SESSION_WINDOW` at desk scale,
/// widened because this window spans every channel rather than one.
pub const SESSION_DELTA_LIMIT: usize = 60;

/// How many events are read per journal page during the walk.
const EVENT_PAGE: usize = 256;

/// What this agent has already accepted, company-wide.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentSessionState {
    /// Exclusive lower boundary: every row at or below this has been handed
    /// over. `None` before the first delivery, which is the cold-boot case.
    pub watermark: Option<EventSeq>,
    /// Rows above the watermark already accepted through another path — the
    /// agent's own reply to the turn in flight, chiefly. Bounded by
    /// [`SESSION_DELTA_LIMIT`]; a state that would exceed it advances the
    /// watermark instead.
    pub present_above_watermark: BTreeSet<EventSeq>,
}

impl AgentSessionState {
    /// Whether `seq` has already been handed to this agent.
    fn already_seen(&self, seq: EventSeq) -> bool {
        match self.watermark {
            Some(mark) if seq <= mark => true,
            _ => self.present_above_watermark.contains(&seq),
        }
    }

    /// State after a re-seed whose recent-window context covered only the
    /// turn's own channel (`ChatSeedRequest::build` is given `incoming` and
    /// nothing else).
    ///
    /// The watermark is deliberately **not** reset to the turn's own
    /// sequence: this agent's session is company-wide, so overwriting it here
    /// would mark every row below it "already delivered" — including an
    /// older, still-unseen message on a channel the reseed never looked at
    /// (Codex P1: a greeting on desk A permanently hides an earlier unseen
    /// message on desk B). `self`'s prior watermark survives untouched, and
    /// only the turn's own message is [`accept`](Self::accept)ed, through the
    /// same compaction a delta uses — which is also what lets a true cold
    /// start (no prior watermark at all) still come out with one afterward,
    /// so the next turn can walk a delta instead of re-seeding forever.
    pub(super) fn reseeded(&self, current_message: Option<EventSeq>) -> Self {
        let mut next = Self {
            watermark: self.watermark,
            present_above_watermark: BTreeSet::new(),
        };
        if let Some(seq) = current_message {
            next.accept(seq);
        }
        next
    }

    /// Records `seq` as delivered, compacting the present set into the
    /// watermark whenever it can.
    fn accept(&mut self, seq: EventSeq) {
        self.present_above_watermark.insert(seq);
        // Compaction: while the row directly above the watermark is present,
        // the watermark can swallow it. Keeps the set small in the ordinary
        // case, where a delta is a contiguous run.
        loop {
            let next = match self.watermark {
                Some(mark) => EventSeq::new(mark.value() + 1),
                None => match self.present_above_watermark.iter().next().copied() {
                    Some(first) => first,
                    None => return,
                },
            };
            if self.present_above_watermark.remove(&next) {
                self.watermark = Some(next);
            } else {
                return;
            }
        }
    }

    /// Record a row that an alternate prompt path already showed to the agent.
    ///
    /// Hive episode turns carry their own attributed transcript. They must mark
    /// the triggering row seen without replacing the company-wide watermark;
    /// replacing it would discard an unseen DM or channel row below the trigger.
    pub(super) fn accept_seen(&mut self, seq: EventSeq) {
        self.accept(seq);
    }
}

/// One row on its way into the session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Envelope {
    /// Where in the journal it sits.
    pub seq: EventSeq,
    /// The channel it was said on, as the cue names it.
    pub channel: String,
    /// Who said it.
    pub author: String,
    /// Whether this agent is the author — its own lines are not cued as input.
    pub mine: bool,
    /// What was said.
    pub text: String,
}

/// What a turn should do about the rows it has not seen.
#[derive(Clone, Debug)]
pub enum SessionPlan {
    /// Hand these over, then commit `next_state` once the turn has accepted
    /// them. The ordering is the contract: committing first would let a failed
    /// turn leave the session believing it read something it never saw.
    Delta {
        envelopes: Vec<Envelope>,
        next_state: AgentSessionState,
    },
    /// The walk could not reach the watermark inside its bounds, or the agent
    /// has no session yet. Fall back to the recent-window seed.
    Reinitialize { reason: ReinitializeReason },
}

/// Why a delta could not be prepared.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReinitializeReason {
    /// No watermark yet — this is the agent's first turn in this process.
    ColdStart,
    /// The watermark was not crossed inside [`SESSION_SCAN_LIMIT`] raw rows.
    GapTooLarge,
    /// More unseen rows than [`SESSION_DELTA_LIMIT`]; the recent window is the
    /// better context.
    TooManyUnseen,
}

/// The rows this agent may read and has not yet been handed, oldest first.
///
/// `before` is exclusive and is this turn's own message: the turn appends that
/// itself, so seeding it here would duplicate it on the wire.
pub async fn prepare_delta(
    events: &Arc<dyn EventLog>,
    company: &CompanyId,
    record: &CompanyRecord,
    agent_id: &str,
    state: &AgentSessionState,
    before: Option<EventSeq>,
) -> SessionPlan {
    let Some(watermark) = state.watermark else {
        return SessionPlan::Reinitialize {
            reason: ReinitializeReason::ColdStart,
        };
    };

    let channels = agent_channels(record, agent_id);
    let mut newest_first: Vec<Envelope> = Vec::new();
    // Rows above the watermark this agent will never be handed — see the
    // comment where these are pushed, below.
    let mut skip_seen: Vec<EventSeq> = Vec::new();
    let mut scanned: usize = 0;
    let mut cursor = before;
    let mut crossed = false;

    loop {
        if scanned >= SESSION_SCAN_LIMIT {
            break;
        }
        let page = match events.read_before(company, cursor, EVENT_PAGE).await {
            Ok(page) => page,
            Err(error) => {
                tracing::warn!(
                    company = %company,
                    agent = agent_id,
                    %error,
                    "[agent-session] journal read failed; re-seeding instead of continuing"
                );
                return SessionPlan::Reinitialize {
                    reason: ReinitializeReason::GapTooLarge,
                };
            }
        };
        if page.is_empty() {
            // The log ended before the watermark. Nothing older can arrive, so
            // everything above it has been collected — treat that as crossed
            // rather than as a gap.
            crossed = true;
            break;
        }
        scanned += page.len();
        cursor = page.last().map(|event| event.seq);

        for stored in page {
            if stored.seq <= watermark {
                crossed = true;
                break;
            }
            if state.already_seen(stored.seq) {
                continue;
            }
            // Codex P1: a row above the watermark that this agent will never
            // be delivered — wrong channel, `Audience::admits` refuses it, not
            // a conversational event, or blank — still has to be marked
            // consumed. `accept` only compacts the watermark through
            // CONSECUTIVE sequences, so leaving a structurally-undeliverable
            // seq out of `next_state` pins the watermark at the row below it
            // forever: every later turn rescans from there, and once the scan
            // exceeds `SESSION_SCAN_LIMIT` it reinitializes instead of
            // completing, permanently falling back to a chat-only reseed.
            // These are safe to accept unconditionally, unlike the reseed's
            // watermark (see `AgentSessionState::reseeded`): that case skipped
            // a row that WOULD eventually be delivered on another channel;
            // this one is filtered out for this agent by construction and can
            // never become deliverable later.
            let Some(channel) = channel_for(&channels, &stored.event, record) else {
                skip_seen.push(stored.seq);
                continue;
            };
            if !readable_by(agent_id, &stored.event) {
                skip_seen.push(stored.seq);
                continue;
            }
            let Some((author, mine, text)) = body_of(agent_id, &stored.event) else {
                skip_seen.push(stored.seq);
                continue;
            };
            if text.trim().is_empty() {
                skip_seen.push(stored.seq);
                continue;
            }
            newest_first.push(Envelope {
                seq: stored.seq,
                channel,
                author,
                mine,
                text,
            });
            if newest_first.len() > SESSION_DELTA_LIMIT {
                return SessionPlan::Reinitialize {
                    reason: ReinitializeReason::TooManyUnseen,
                };
            }
        }
        if crossed {
            break;
        }
    }

    if !crossed {
        return SessionPlan::Reinitialize {
            reason: ReinitializeReason::GapTooLarge,
        };
    }

    newest_first.reverse();
    let mut next_state = state.clone();
    for envelope in &newest_first {
        next_state.accept(envelope.seq);
    }
    for seq in skip_seen {
        next_state.accept(seq);
    }
    // The turn's own message is accepted too: the turn appends it, so the next
    // delta must not hand it back.
    if let Some(seq) = before {
        next_state.accept(seq);
    }
    SessionPlan::Delta {
        envelopes: newest_first,
        next_state,
    }
}

/// Which of this agent's channels owns `event`, if any.
fn channel_for(
    channels: &[Channel],
    event: &CompanyEvent,
    record: &CompanyRecord,
) -> Option<String> {
    // Codex P1: `owns(id, name, event)` matches EITHER spelling, which is
    // right for `desk_read`/`chat_history` where `desk_id`+`desk_name` name
    // the SAME desk — but here `channels` is the whole list of desks this
    // agent sits on, checked one at a time, so a channel `{id: "ops", name:
    // "sales"}` this agent sits on would also claim an event journaled under
    // chat_id = "sales" that belongs to an ACTUAL desk named `sales` this
    // agent does not sit on. Exact ids are resolved first and alone across
    // every one of this agent's channels; a name match is only trusted once
    // it is confirmed the event's key is not some OTHER real desk's id.
    if let Some(channel) = channels
        .iter()
        .find(|channel| chat_history::owns(&channel.id, &channel.id, event))
    {
        return Some(channel.label.clone());
    }
    let collides_with_a_real_desk_id = record
        .manifest
        .group_chats
        .iter()
        .any(|chat| chat_history::owns(&chat.id, &chat.id, event))
        || record
            .overlay_desks
            .iter()
            .any(|desk| chat_history::owns(&desk.id, &desk.id, event));
    if collides_with_a_real_desk_id {
        return None;
    }
    channels
        .iter()
        .find(|channel| chat_history::owns(&channel.name, &channel.name, event))
        .map(|channel| channel.label.clone())
}

/// Whether `agent_id` may read this row at all.
///
/// The **only** narrowing this module applies, and it is not about channels: an
/// `AgentReply` carrying a non-empty `audience` is a private aside, and a peer
/// outside it reads the row elided rather than in full. Applied here so a
/// session can never be handed something the desk projection would withhold.
fn readable_by(agent_id: &str, event: &CompanyEvent) -> bool {
    match event {
        CompanyEvent::AgentReply {
            agent_id: author,
            audience,
            ..
        } => {
            audience.is_empty()
                || author == agent_id
                || audience.iter().any(|member| member == agent_id)
        }
        _ => true,
    }
}

/// The conversational body of `event`, or `None` for a row that carries none.
fn body_of(agent_id: &str, event: &CompanyEvent) -> Option<(String, bool, String)> {
    match event {
        CompanyEvent::OperatorMessage {
            text,
            attachments,
            by,
            ..
        } => Some((
            super::chat_seed::operator_label(by),
            false,
            crate::brain::medulla::effects::with_attachment_refs(text, attachments),
        )),
        CompanyEvent::AgentReply {
            agent_id: author,
            text,
            ..
        } => Some((author.clone(), author == agent_id, text.clone())),
        // `owns` also admits `DeskTaskCompleted`, a structural marker with no
        // conversational body. Not a turn; not delivered.
        _ => None,
    }
}

/// Renders a delta as the cue block prepended to a turn's message.
///
/// # Why a cued turn and not a tool result
///
/// The reference implementation this shape was taken from is deliberately
/// asymmetric: **outbound is a tool call, inbound is a plain turn carrying a
/// text cue** (`[inbound]`, `[agent]`, `[routine]`, `[Group chat: "…"]`). That
/// is the right way round here too, and cheaper: OpenHuman's resume path
/// already speaks `(role, content)`, so a cued turn needs no new plumbing,
/// whereas a synthesised tool result would need a fabricated call id with no
/// matching call and would confuse [`fold_steps`](super::steps::fold_steps).
///
/// Returns `None` for an empty delta, so an ordinary same-channel reply pays
/// nothing for this.
pub fn render_cues(envelopes: &[Envelope]) -> Option<String> {
    let lines: Vec<String> = envelopes
        .iter()
        .filter(|envelope| !envelope.mine)
        .map(|envelope| {
            format!(
                "[{} · {}] {}",
                envelope.channel,
                envelope.author,
                envelope.text.trim()
            )
        })
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "While you were away, this was said elsewhere in the company. It is \
         context, not a request — answer the message at the end of this turn.\n\n{}\n",
        lines.join("\n")
    ))
}

#[cfg(test)]
#[path = "agent_session_tests.rs"]
mod tests;
