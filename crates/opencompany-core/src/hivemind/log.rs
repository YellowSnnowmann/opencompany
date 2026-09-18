//! The company journal, read as a [`SessionLog`].
//!
//! tinyhivemind projects a transcript through one primitive — "give me the
//! rows older than this cursor, newest first" — and the company journal already
//! answers exactly that shape ([`EventLog::read_before`]). What this adapter
//! adds is the narrowing: a journal carries approvals, task cards, workflow
//! runs and webhook receipts as well as chat, and an episode may only fold what
//! was actually said on its desk.
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

use tinyhivemind_hive::aside::Audience;
use tinyhivemind_hive::{
    LogMessage, Sequence, SessionAuthor, SessionFuture, SessionLog, SessionPage,
};

use super::scope::EpisodeScope;
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
    /// The running episode's fold boundary, when this log is scoped to one
    /// (see [`EpisodeScope`]). `None` — the default — admits every row this
    /// desk's own filter passes, which is every reader this adapter had
    /// before concurrent episodes needed narrowing at all.
    scope: Option<Arc<EpisodeScope>>,
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
            scope: None,
        }
    }

    /// Narrow every row this log returns to `scope`'s fold boundary: shared
    /// context at or below its trigger, plus whatever that one episode
    /// instance has itself appended.
    ///
    /// Without this, two episodes deliberating in the same thread read
    /// identical rows above their respective triggers — including each
    /// other's turns — because nothing about the desk id or the thread root
    /// tells them apart. See `EpisodeScope`'s module doc.
    #[must_use]
    pub fn with_scope(mut self, scope: Arc<EpisodeScope>) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Whether a stored chat key addresses this desk.
    ///
    /// Case-insensitive against both the id and the display name, which is the
    /// same latitude [`CompanyRecord::resolve_desk_id`] gives an operator
    /// addressing the desk in the first place.
    ///
    /// [`CompanyRecord::resolve_desk_id`]: crate::ports::types::CompanyRecord::resolve_desk_id
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
    ///   [`HIVE_REPORT_AUTHOR`](super::HIVE_REPORT_AUTHOR), a workflow report,
    ///   an owner-fallback report — is [`SessionAuthor::System`]. That
    ///   distinction is load-bearing rather than cosmetic: a system row must
    ///   never be counted as a supporter, and it must stay visible through a
    ///   blind round, both of which follow from the author variant alone.
    fn row(&self, stored: StoredEvent) -> Option<LogMessage> {
        if let Some(scope) = &self.scope
            && !scope.admits(stored.seq)
        {
            return None;
        }
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
    agent_id == super::HIVE_REPORT_AUTHOR
        || agent_id == super::HIVE_FAILURE_AUTHOR
        || agent_id == super::HIVE_REFERRAL_AUTHOR
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
                    .map_err(|error| Box::new(error) as tinyhivemind_hive::SourceError)?;
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
