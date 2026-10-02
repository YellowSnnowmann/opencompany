//! What opened an episode: whether its message read as a question, and,
//! for a resumed episode, which operator row that was.
//!
//! Split out of `conducted.rs` to keep that file under the 750-line cap.

use super::{HiveDispatcher, Trigger};
use crate::ports::types::{CompanyEvent, EventSeq, StoredEvent};

impl Trigger {
    /// Whether the operator's words read as a question.
    #[must_use]
    pub fn is_question(&self) -> bool {
        !self.carried_on
            && crate::company::task_intent::triage_message_detailed(
                crate::runtime::delegation::operator_words(&self.text),
            )
            .triage
            .is_answer()
    }
}

impl HiveDispatcher {
    /// The operator row a resumed episode opened at, and whether it read as a
    /// question, from the episode's own `EpisodeOpened` row.
    ///
    /// A row that cannot be read falls back to the thread root and to not
    /// answering, which is how a resumed episode ran before either was known.
    pub(super) async fn opening_of(
        &self,
        rows: &[StoredEvent],
        thread_root: Option<EventSeq>,
    ) -> (EventSeq, bool) {
        let fallback = thread_root.unwrap_or(EventSeq::new(0));
        let Some(opened_at) = rows.iter().find_map(|row| match &row.event {
            CompanyEvent::EpisodeOpened { opened_by_seq, .. } => {
                Some(EventSeq::new(*opened_by_seq))
            }
            _ => None,
        }) else {
            return (fallback, false);
        };
        let text = match self.events.read_from(&self.record.id, opened_at, 1).await {
            Ok(found) => found.into_iter().find_map(|row| match row.event {
                CompanyEvent::OperatorMessage { text, .. } if row.seq == opened_at => Some(text),
                _ => None,
            }),
            Err(error) => {
                tracing::warn!(%error, "[hive] could not read the message a resumed episode answers");
                None
            }
        };
        let answering = text.is_some_and(|text| {
            Trigger {
                seq: opened_at,
                text,
                parent: None,
                mentions: Vec::new(),
                carried_on: false,
            }
            .is_question()
        });
        (opened_at, answering)
    }
}

#[cfg(test)]
#[path = "opening_tests.rs"]
mod tests;
