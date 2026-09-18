//! The [`ReadStateStore`] port: how far each person has read each channel.
//!
//! The console used to answer "is there anything new here?" entirely in the
//! browser, from a floor stamped at mount. Anything older than the instant the
//! tab opened counted as read, so a reload marked every channel caught up —
//! failing exactly the case unread exists for, and disagreeing between two
//! tabs of the same person (issue #755).
//!
//! This store holds the other half: a durable, per-person, per-channel marker
//! that survives the tab. The console still derives the *count* locally — it is
//! the only side holding the transcript — but it derives it from a floor the
//! host remembers rather than one the browser invented.
//!
//! **Per person, not per company.** Two operators on one company have read
//! different things, and a marker keyed only by channel would let one person's
//! reading clear another's badge.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::ports::types::CompanyId;

/// How far one person has read one channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelRead {
    /// The channel this marker is for, in the console's own channel-id space
    /// (`engineering`, `dm:product_manager`).
    pub channel_id: String,
    /// Milliseconds since the epoch. Everything at or before this instant is
    /// read; everything after it is not.
    ///
    /// A timestamp rather than a message id because the console's unread
    /// derivation is already a time comparison against `message.at`, and
    /// because a channel's rows come from more than one writer — there is no
    /// single sequence to point at.
    pub last_read_at: i64,
}

/// Durable read markers. One person's markers MUST be invisible to another,
/// and one company's MUST be invisible to another.
#[async_trait]
pub trait ReadStateStore: Send + Sync {
    /// Every marker this person holds in this company.
    ///
    /// A channel with no marker is absent rather than zero — "never opened" and
    /// "opened before any message existed" are different states, and only the
    /// caller knows which floor to apply to a channel it has never seen.
    ///
    /// **Ordered by `channel_id`, ascending.** Part of the contract rather than
    /// an accident of each backend: insertion order differs between a document
    /// store and a table, and a caller diffing two reads would see spurious
    /// churn. The conformance suite asserts it, so a backend that returns
    /// insertion order fails rather than passing quietly.
    async fn list(&self, company: &CompanyId, user: &str) -> Result<Vec<ChannelRead>>;

    /// Moves one channel's marker forward, and returns where it now stands.
    ///
    /// **Monotonic.** An `at` at or before the stored marker leaves it alone.
    /// Two tabs of the same person race constantly — one viewing an old channel
    /// while another reads a live one — and a late request carrying the earlier
    /// instant would otherwise resurrect messages the person has already read.
    /// Making the write a max rather than a set means the order requests land
    /// in cannot change the outcome.
    async fn mark(
        &self,
        company: &CompanyId,
        user: &str,
        channel_id: &str,
        at: i64,
    ) -> Result<ChannelRead>;
}
