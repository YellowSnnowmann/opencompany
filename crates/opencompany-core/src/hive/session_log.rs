//! The company journal, read as a [`SessionLog`].
//!
//! tinyhivemind projects a transcript through one primitive — "give me the
//! rows older than this cursor, newest first" — and the company journal already
//! answers exactly that shape ([`EventLog::read_before`]). What this adapter
//! adds is the narrowing: a journal carries approvals, task cards, workflow
//! runs and webhook receipts as well as chat, and an episode may only fold what
//! was actually said on its desk.
//!
//! Moved from `src/hivemind/log.rs` (plan hive-desks, Phase 4) without the
//! per-episode fold scope it carried: two episodes can no longer run in one
//! thread (`episode_store::open_episode_for` joins the second message to the
//! first's room), and what a seat has already seen is the sharing watermark
//! `hive::prompt` keeps per seat, not a boundary on the log.
//!
//! # Why the read loops
//!
//! The port's page contract forbids an empty page that still carries a cursor —
//! an empty page means the log is finished. A desk's rows can easily be a
//! hundred journal entries apart, so a single `read_before` chunk may hold no
//! chat at all, and returning that as an empty page would truncate the
//! transcript at whatever the last busy stretch of the journal happened to be.
//! The adapter therefore keeps reading raw chunks until it has at least one
//! qualifying row or the journal runs out.

use std::sync::Arc;

use tinyhivemind::{LogMessage, Sequence, SessionAuthor, SessionFuture, SessionLog, SessionPage};
use tinyhivemind_core::aside::Audience;

use crate::ports::events::EventLog;
use crate::ports::types::{CompanyEvent, CompanyId, EventSeq, StoredEvent, UtteranceKind};

/// Raw journal entries read per underlying page.
///
/// Larger than a desk's typical density so a normal transcript is one read, and
/// small enough that a company whose journal is mostly non-chat does not pull a
/// huge page to keep four rows from it.
const RAW_CHUNK: usize = 256;

/// Raw journal entries one `read_before` call will walk before giving up.
///
/// The bound exists for the pathological case only: a desk that was busy long
/// ago and silent since, behind tens of thousands of unrelated entries. Hitting
/// it returns a short (possibly empty) page with no cursor, which the
/// projection reads as "the log ends here" — a shorter transcript, never a
/// wrong one.
const RAW_SCAN: usize = 4096;

/// The company event log, projected as one desk's session log.
///
/// Scoped to a single desk on purpose. The adapter rewrites every admitted
/// row's `chat_id` to the canonical desk id, so the library's own
/// `same_conversation` check compares two spellings this host has already
/// agreed are the same one — a message addressed by display name, or in a
/// different case, still lands in the room it was meant for.
pub struct EventLogSessionLog {
    events: Arc<dyn EventLog>,
    company: CompanyId,
    desk_id: String,
    desk_name: String,
    /// The seats at this desk, for deciding whose pair channels belong to
    /// its transcript. Empty admits none, which is what every caller that
    /// does not seat a room should pass.
    seats: Vec<String>,
    /// Other desks these seats also sit at, as `(id, name)`.
    ///
    /// Read, never canonicalised: a row from one of these is reported under
    /// its **own** chat id, because the only caller asks per conversation
    /// (`gather_elsewhere`) and folding them into this desk's id would make
    /// every one of them look like a row of this room. Empty by default, so
    /// a log that is not told about them behaves exactly as it always did.
    elsewhere: Vec<(String, String)>,
}

impl std::fmt::Debug for EventLogSessionLog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EventLogSessionLog")
            .field("company", &self.company)
            .field("desk_id", &self.desk_id)
            .finish_non_exhaustive()
    }
}

impl EventLogSessionLog {
    /// Open the journal as the session log of one desk.
    #[must_use]
    pub fn new(
        events: Arc<dyn EventLog>,
        company: CompanyId,
        desk_id: String,
        desk_name: String,
        seats: Vec<String>,
    ) -> Self {
        Self {
            events,
            company,
            desk_id,
            desk_name,
            seats,
            elsewhere: Vec::new(),
        }
    }

    /// Also read these desks, as `(id, name)`, without folding them into
    /// this one.
    ///
    /// The library asks for them by conversation when it builds a seat's
    /// brief (`EpisodeBrief::elsewhere`): a seat that sits at two desks is
    /// shown the newest rows of the other as context. It reads them through
    /// this log, so a log that admits only its own desk answers nothing and
    /// the seat is told it is in nothing else -- which is what every seat
    /// was told before this existed.
    pub fn also_read(&mut self, desks: Vec<(String, String)>) {
        self.elsewhere = desks;
    }

    /// The seats this log serves, for a caller deciding which other desks
    /// they sit at.
    #[must_use]
    pub fn seats(&self) -> &[String] {
        &self.seats
    }

    /// The desk this log is scoped to.
    #[must_use]
    pub fn desk_id(&self) -> &str {
        &self.desk_id
    }

    /// The desk's display name.
    #[must_use]
    pub fn desk_name(&self) -> &str {
        &self.desk_name
    }

    /// The conversation this log projects, for the sharing walk.
    #[must_use]
    pub fn conversation(&self, thread_root: Option<Sequence>) -> tinyhivemind::Conversation {
        tinyhivemind::Conversation {
            desk_id: self.desk_id.clone(),
            desk_name: self.desk_name.clone(),
            thread_root,
        }
    }

    /// Whether a stored chat key addresses this desk, or one of the private
    /// conversations its seats hold.
    ///
    /// Case-insensitive against both the id and the display name, which is the
    /// same latitude `CompanyRecord::resolve_desk_id` gives an operator
    /// addressing the desk in the first place.
    ///
    /// A conversation two seats open with `ask` is written to their own pair
    /// channel, so that the room's timeline stays the room's. It is still
    /// part of this desk's transcript: the seat that asked cannot finish
    /// until it is answered, and the seat asked is turned inside it. Admitted
    /// rows are reported under the desk id like every other, and the library
    /// narrows them by thread root and audience exactly as it already does —
    /// which is why a desk turn still cannot read them.
    ///
    /// **Both seats must sit at this desk.** A pair channel is minted from
    /// two roster ids and says nothing about where they were talking, so
    /// admitting one on the strength of its name alone would pull another
    /// desk's private exchange into this transcript.
    fn addresses_desk(&self, chat: Option<&str>) -> bool {
        chat.is_some_and(|chat| {
            chat.eq_ignore_ascii_case(&self.desk_id)
                || chat.eq_ignore_ascii_case(&self.desk_name)
                || self.addresses_a_seat_pair(chat)
                || self.addresses_elsewhere(chat)
        })
    }

    /// The id a row is reported under: its own when it came from a desk this
    /// log merely reads, and this desk's otherwise.
    fn reported_chat(&self, chat: &str) -> String {
        self.elsewhere
            .iter()
            .find(|(id, name)| chat.eq_ignore_ascii_case(id) || chat.eq_ignore_ascii_case(name))
            .map_or_else(|| self.desk_id.clone(), |(id, _)| id.clone())
    }

    /// Whether `chat` is one of the other desks these seats sit at.
    fn addresses_elsewhere(&self, chat: &str) -> bool {
        self.elsewhere
            .iter()
            .any(|(id, name)| chat.eq_ignore_ascii_case(id) || chat.eq_ignore_ascii_case(name))
    }

    /// Whether `chat` is the pair channel of two seats of this desk.
    fn addresses_a_seat_pair(&self, chat: &str) -> bool {
        let Some((one, two)) = super::referral::pair_seats(chat) else {
            return false;
        };
        self.seats.iter().any(|seat| seat == one) && self.seats.iter().any(|seat| seat == two)
    }

    /// One journal entry as a session row, or `None` when it is not desk chat.
    ///
    /// Authorship is the whole point of the conversion, and it is three-way:
    ///
    /// - an operator message is [`SessionAuthor::Operator`], whatever human
    ///   sent it — the room reads it as the task, not as a participant;
    /// - a teammate's reply is [`SessionAuthor::Agent`], and its id is what the
    ///   quorum fold counts distinct supporters by, so it has to be the roster
    ///   id rather than the label;
    /// - a reply authored by one of this host's reserved, unmintable ids —
    ///   [`HIVE_REFERRAL_AUTHOR`](super::referral::HIVE_REFERRAL_AUTHOR), a
    ///   workflow report, an owner-fallback report — is
    ///   [`SessionAuthor::System`]: a seat reads "the content desk answered"
    ///   rather than a teammate that never sat here.
    ///
    /// One committed row is deliberately not chat: the conclusion the
    /// conductor mints when a conversation ends (see [`is_conclusion`]).
    fn row(&self, stored: StoredEvent) -> Option<LogMessage> {
        let sequence = Sequence(stored.seq.value());
        match stored.event {
            CompanyEvent::OperatorMessage { text, chat, .. }
                if self.addresses_desk(chat.as_deref()) =>
            {
                Some(LogMessage {
                    sequence,
                    chat_id: Some(self.reported_chat(chat.as_deref().unwrap_or(&self.desk_id))),
                    // An operator's thread is the console's, never a
                    // conversation the library models (`conversation_root`).
                    parent: None,
                    author: SessionAuthor::Operator,
                    content: text,
                    audience: Audience::Desk,
                })
            }
            CompanyEvent::AgentReply {
                chat_id,
                agent_id,
                text,
                parent,
                audience,
                episode,
                ..
            } if self.addresses_desk(Some(&chat_id)) && !is_conclusion(episode.as_ref()) => {
                Some(LogMessage {
                    sequence,
                    chat_id: Some(self.reported_chat(&chat_id)),
                    parent: self.conversation_root(&chat_id, parent, episode.map(|e| e.kind)),
                    author: author_of(&agent_id),
                    content: text,
                    audience: self.audience_of(&chat_id, &agent_id, audience),
                })
            }
            _ => None,
        }
    }

    /// The root a row is reported under: the ask that opened the conversation
    /// it was said in, or none.
    ///
    /// This host threads every row an episode writes under the operator
    /// message that opened it -- the console's shape, where an episode is one
    /// thread of the room. The library reads a desk at channel level as each
    /// root and its **first** reply, a rule written for a desk where a thread
    /// is a conversation; read through it, a busy episode is the operator's
    /// ask and one reply, with every later row of every seat withheld from
    /// every seat. The conversations the library models are the pair
    /// channels, rooted at an ask. So a desk row is reported as a root, an
    /// ask as the root of its own conversation, and only a row said inside
    /// one keeps its parent -- the answers, and the conclusion the host
    /// threads under the ask (`DeskHost::commit`).
    fn conversation_root(
        &self,
        chat: &str,
        parent: Option<EventSeq>,
        kind: Option<UtteranceKind>,
    ) -> Option<Sequence> {
        if !self.addresses_a_seat_pair(chat) || matches!(kind, Some(UtteranceKind::Ask)) {
            return None;
        }
        parent.map(|seq| Sequence(seq.value()))
    }

    /// Who a row is addressed to.
    ///
    /// Empty is desk-visible, which is what every row written before asides
    /// existed means and what every ordinary turn means now. The stored list
    /// is the addressees only; the author's own admission to its row is the
    /// library's rule, not a member of the set (`Audience::admits`).
    ///
    /// A pair channel is private to the two seats it names, so a row in one
    /// is an aside to the other seat whether or not its author wrote that
    /// down. The answer inside a conversation carries no audience of its own;
    /// reported as desk-visible it is one promotion away from a third seat's
    /// transcript, and once the ask is a root its first reply is promoted.
    fn audience_of(&self, chat: &str, author: &str, stored: Vec<String>) -> Audience {
        let mut members = stored;
        if let Some((one, two)) = super::referral::pair_seats(chat) {
            for seat in [one, two] {
                if seat != author && !members.iter().any(|member| member == seat) {
                    members.push(seat.to_owned());
                }
            }
        }
        if members.is_empty() {
            Audience::Desk
        } else {
            Audience::Aside { members }
        }
    }
}

/// Whether this row is the conductor concluding a conversation.
///
/// The conductor mints one when an exchange ends: a `Dm` to the asker whose
/// message **restates the askee's own last line**, verbatim. Both rows are in
/// the same thread now (`DeskHost::commit`), so every reader that gets one
/// gets the other directly above it, and a live run measured the paragraph
/// three times in one 6.4k-character prompt -- once in the desk delta, once
/// in the conversation transcript, and once as this restatement of it.
///
/// So it is journaled and not projected. The row is the ledger's proof that
/// the asker was answered (`ledger.answered`), and the console reads the
/// journal directly, so neither loses anything; what it stops being is a
/// second copy in every brief and every rebuilt seat history that carries
/// the conversation.
///
/// Safe to key on the kind: `dm` is unserved in the speech vocabulary, so no
/// seat can call it and a `Dm` commit has exactly one author -- this
/// conductor, concluding. A row from before episodes carries no kind at all
/// and is chat like any other.
fn is_conclusion(episode: Option<&crate::ports::types::ReplyEpisode>) -> bool {
    episode.is_some_and(|episode| matches!(episode.kind, UtteranceKind::Dm))
}

/// Reserved reply authors this host journals under, which no roster id can
/// spell (all are hyphenated, and every id minter rejects a hyphen).
fn is_system_author(agent_id: &str) -> bool {
    super::referral::is_hive_author(agent_id)
        || agent_id == crate::ports::SYSTEM_AUTHOR
        || agent_id == crate::runtime::channel::WORKFLOW_REPLY_AUTHOR
        || agent_id == crate::runtime::channel::OWNER_FALLBACK_REPORT_AUTHOR
}

/// The session author for a journaled reply.
///
/// The label is the id. The adapter holds no roster, deliberately: it is opened
/// for one desk and reads rows written by teammates who may since have left it.
/// A seated member's real name is applied by the episode prompt, which does
/// hold the roster.
fn author_of(agent_id: &str) -> SessionAuthor {
    if is_system_author(agent_id) {
        return SessionAuthor::System {
            kind: agent_id.to_owned(),
            label: agent_id.to_owned(),
        };
    }
    SessionAuthor::Agent {
        id: agent_id.to_owned(),
        label: agent_id.to_owned(),
    }
}

impl SessionLog for EventLogSessionLog {
    fn read_before(&self, before: Option<Sequence>, limit: usize) -> SessionFuture<'_> {
        Box::pin(async move {
            let mut cursor = before.map(|sequence| EventSeq::new(sequence.0));
            let mut messages: Vec<LogMessage> = Vec::new();
            let mut scanned = 0_usize;

            while scanned < RAW_SCAN {
                let chunk = RAW_CHUNK.min(RAW_SCAN - scanned);
                let raw = self
                    .events
                    .read_before(&self.company, cursor, chunk)
                    .await
                    .map_err(|error| Box::new(error) as tinyhivemind::SourceError)?;
                if raw.is_empty() {
                    // The journal is finished; there is no older page.
                    return Ok(SessionPage {
                        messages,
                        next_before: None,
                    });
                }
                scanned += raw.len();
                // A short chunk is the tail of the journal. Read before the
                // rows are consumed, and acted on only *after* the limit check
                // below: a chunk that both filled the caller's page and ran out
                // is still a page with rows left behind it, and reporting no
                // cursor there would silently truncate the transcript at
                // whatever the page happened to end on.
                let tail = raw.len() < chunk;
                // Newest-first, so the last entry read is the oldest one seen.
                cursor = raw.last().map(|stored| stored.seq);
                for stored in raw {
                    if messages.len() == limit {
                        break;
                    }
                    if let Some(row) = self.row(stored) {
                        messages.push(row);
                    }
                }
                if messages.len() == limit {
                    break;
                }
                if tail {
                    return Ok(SessionPage {
                        messages,
                        next_before: None,
                    });
                }
            }

            // Stopped on the caller's limit or the scan bound with rows in
            // hand. The cursor has to be no newer than the oldest row returned
            // — the page validator checks exactly that — so it is the oldest
            // row itself rather than the oldest entry scanned, which may be
            // older still and would silently skip whatever lies between them.
            let next_before = messages.last().map(|message| message.sequence);
            Ok(SessionPage {
                messages,
                next_before,
            })
        })
    }
}

#[cfg(test)]
#[path = "session_log_tests.rs"]
mod tests;
