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
use crate::ports::types::{CompanyEvent, CompanyId, EventSeq, StoredEvent};

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
    ) -> Self {
        Self {
            events,
            company,
            desk_id,
            desk_name,
        }
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

    /// Whether a stored chat key addresses this desk.
    ///
    /// Case-insensitive against both the id and the display name, which is the
    /// same latitude `CompanyRecord::resolve_desk_id` gives an operator
    /// addressing the desk in the first place.
    fn addresses_desk(&self, chat: Option<&str>) -> bool {
        chat.is_some_and(|chat| {
            chat.eq_ignore_ascii_case(&self.desk_id) || chat.eq_ignore_ascii_case(&self.desk_name)
        })
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
    fn row(&self, stored: StoredEvent) -> Option<LogMessage> {
        let sequence = Sequence(stored.seq.value());
        match stored.event {
            CompanyEvent::OperatorMessage {
                text, chat, parent, ..
            } if self.addresses_desk(chat.as_deref()) => Some(LogMessage {
                sequence,
                chat_id: Some(self.desk_id.clone()),
                parent: parent.map(|seq| Sequence(seq.value())),
                author: SessionAuthor::Operator,
                content: text,
                audience: Audience::Desk,
            }),
            CompanyEvent::AgentReply {
                chat_id,
                agent_id,
                text,
                parent,
                audience,
                ..
            } if self.addresses_desk(Some(&chat_id)) => Some(LogMessage {
                sequence,
                chat_id: Some(self.desk_id.clone()),
                parent: parent.map(|seq| Sequence(seq.value())),
                author: author_of(&agent_id),
                content: text,
                // Empty is desk-visible, which is what every row written before
                // asides existed means and what every ordinary turn means now.
                // The stored list is the addressees only; the author's own
                // admission to its row is the library's rule, not a member of
                // the set (`Audience::admits`).
                audience: if audience.is_empty() {
                    Audience::Desk
                } else {
                    Audience::Aside { members: audience }
                },
            }),
            _ => None,
        }
    }
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
