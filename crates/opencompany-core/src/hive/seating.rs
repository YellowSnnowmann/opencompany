//! What an episode lends a teammate for the turns it runs as a seat.
//!
//! A teammate's belt is composed once per turn by the factory its
//! [`AgentSpec`](openhuman_embed::AgentSpec) carries, and that factory is
//! built when the agent is registered -- long before any episode opens. So an
//! episode cannot hand its tools to the agent directly. It leaves them here,
//! under the conversation its seat will run in, and the factory looks them up
//! when a turn on that conversation arrives.
//!
//! This is what lets one teammate be both itself and a seat without existing
//! twice. Before, an episode built a second session around its tools; now the
//! tools reach the handle the pool already holds.
//!
//! # Why a conversation id is the key
//!
//! Because it is what the factory is told. A turn arrives with a
//! [`TurnContext`](openhuman_core::agent::TurnContext) naming the agent and
//! the conversation, and nothing else -- deliberately, since a belt keyed on
//! anything the host keeps mutably would be a race the moment one agent
//! serves two conversations.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use tinyhivemind_openhuman::EpisodeBeltSource;

/// The belts episodes have lent one teammate, by conversation.
///
/// Cheap to clone: the map is shared, so the copy the factory closed over at
/// registration is the copy an episode writes to later.
#[derive(Clone, Default)]
pub struct EpisodeBelts {
    lent: Arc<Mutex<HashMap<String, EpisodeBeltSource>>>,
}

impl std::fmt::Debug for EpisodeBelts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let lent = self.lent.lock().unwrap_or_else(PoisonError::into_inner);
        f.debug_struct("EpisodeBelts")
            .field("conversations", &lent.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl EpisodeBelts {
    /// Lend `source` to every turn that runs on `conversation`.
    ///
    /// Replaces what was there. A conversation is one episode's, and an
    /// episode that opens again on the same id means the new one.
    pub fn lend(&self, conversation: impl Into<String>, source: EpisodeBeltSource) {
        self.lent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(conversation.into(), source);
    }

    /// The belt lent to `conversation`, if an episode is running there.
    ///
    /// `None` for an ordinary turn, which is most of them: a teammate
    /// answering its operator is not seated in anything.
    #[must_use]
    pub fn lent_to(&self, conversation: Option<&str>) -> Option<EpisodeBeltSource> {
        let conversation = conversation?;
        self.lent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(conversation)
            .cloned()
    }

    /// Take the belt back when the episode ends.
    pub fn reclaim(&self, conversation: &str) {
        self.lent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(conversation);
    }
}
