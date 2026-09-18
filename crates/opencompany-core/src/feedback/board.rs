//! The shared feedback board: the types the console reads and writes.
//!
//! The local feedback loop ([`super::service`]) is private by construction — an
//! operator's report is captured on their machine and, at most, forwarded to
//! the TinyHumans hub. The *board* is the other half: the public, cross-product
//! list of what everyone else asked for, the same one OpenHuman's console reads
//! (`GET /feedback`), with votes, comments and a triage status.
//!
//! Nothing here is stored locally. The board lives on the hub; this crate
//! proxies it so the console never needs the instance credential in a browser,
//! and so an unprovisioned instance degrades to "no board" instead of a CORS
//! failure against a host it cannot authenticate to.
//!
//! Wire shape: the hub speaks camelCase (`commentCount`, `myVote`), this crate's
//! HTTP surface speaks snake_case like every other route in `api.md`. The
//! translation happens once, in [`super::tinyhumans`]'s HTTP client, so the
//! console sees one vocabulary.

use serde::{Deserialize, Serialize};

/// The coarse split the hub files everything under.
///
/// Deliberately *not* [`super::types::FeedbackCategory`]: that is the local
/// taxonomy, and the hub only knows these two (see
/// [`IngestRequest::wire_type`](super::tinyhumans::IngestRequest::wire_type)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BoardKind {
    /// Something the product does not do (well enough) yet.
    Feature,
    /// Something the product does wrong.
    Bug,
}

impl BoardKind {
    /// The hub's wire token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Feature => "feature",
            Self::Bug => "bug",
        }
    }

    /// Parses a hub token, ignoring anything unrecognized.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "feature" => Some(Self::Feature),
            "bug" => Some(Self::Bug),
            _ => None,
        }
    }
}

/// Where an item sits in the hub's triage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BoardStatus {
    /// Accepted onto the board, not yet acted on.
    Open,
    /// On the roadmap.
    Planned,
    /// Shipped.
    Completed,
    /// Declined or superseded. The hub does not accept it as a *filter*, but it
    /// does return it on an item, so the console must be able to render it.
    Closed,
}

impl BoardStatus {
    /// The hub's wire token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Planned => "planned",
            Self::Completed => "completed",
            Self::Closed => "closed",
        }
    }

    /// Parses a hub token, ignoring anything unrecognized.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "planned" => Some(Self::Planned),
            "completed" => Some(Self::Completed),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }
}

/// The board orderings the hub exposes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BoardSort {
    /// Time-decayed score — the default the hub itself defaults to.
    #[default]
    Hot,
    /// Highest score first.
    Top,
    /// Newest first.
    New,
}

impl BoardSort {
    /// The hub's wire token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Top => "top",
            Self::New => "new",
        }
    }

    /// Parses a query token, ignoring anything unrecognized.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "hot" => Some(Self::Hot),
            "top" => Some(Self::Top),
            "new" => Some(Self::New),
            _ => None,
        }
    }
}

/// The hub's page-size ceiling. A larger `limit` is a 400 there, so it is
/// clamped here rather than round-tripped to be refused.
pub const MAX_LIMIT: u32 = 100;

/// The console's default page size.
pub const DEFAULT_LIMIT: u32 = 20;

/// One board query: ordering, optional filters, and a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardQuery {
    /// The ordering.
    pub sort: BoardSort,
    /// Restrict to one kind.
    pub kind: Option<BoardKind>,
    /// Restrict to one status.
    pub status: Option<BoardStatus>,
    /// 1-based page number.
    pub page: u32,
    /// Page size, clamped to [`MAX_LIMIT`].
    pub limit: u32,
}

impl Default for BoardQuery {
    fn default() -> Self {
        Self {
            sort: BoardSort::default(),
            kind: None,
            status: None,
            page: 1,
            limit: DEFAULT_LIMIT,
        }
    }
}

impl BoardQuery {
    /// Clamps `page` and `limit` into the range the hub accepts.
    ///
    /// A caller-supplied `page: 0` or `limit: 0` is a 400 at the hub; both are
    /// far more likely to be an off-by-one in a client than a deliberate ask,
    /// so they are corrected rather than propagated as a failed round trip.
    pub fn clamped(mut self) -> Self {
        self.page = self.page.max(1);
        self.limit = self.limit.clamp(1, MAX_LIMIT);
        self
    }
}

/// A vote: up, down, or retracted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "i8", into = "i8")]
pub enum VoteValue {
    /// `1`.
    Up,
    /// `-1`.
    Down,
    /// `0` — take a previous vote back.
    #[default]
    None,
}

impl VoteValue {
    /// The hub's wire integer.
    pub fn as_i8(self) -> i8 {
        match self {
            Self::Up => 1,
            Self::Down => -1,
            Self::None => 0,
        }
    }
}

impl From<VoteValue> for i8 {
    fn from(value: VoteValue) -> Self {
        value.as_i8()
    }
}

impl TryFrom<i8> for VoteValue {
    type Error = String;

    fn try_from(value: i8) -> std::result::Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Up),
            -1 => Ok(Self::Down),
            0 => Ok(Self::None),
            other => Err(format!("vote must be 1, -1, or 0 (got {other})")),
        }
    }
}

/// One item on the board, as the console renders it.
///
/// `my_vote` is resolved by the hub for the calling credential, which on this
/// runtime is the instance's own TinyHumans account: everyone using this
/// console votes as that account, so a vote is the *instance's* vote, not a
/// per-operator one. That is the same identity the local loop already files
/// under (see [`super::tinyhumans`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoardItem {
    /// The hub's id.
    pub id: String,
    /// Feature or bug.
    pub kind: BoardKind,
    /// The one-line ask.
    pub title: String,
    /// The full text.
    pub body: String,
    /// Triage status.
    pub status: BoardStatus,
    /// The author's display name, when the hub returns one.
    pub author: Option<String>,
    /// Upvote tally.
    pub upvotes: u32,
    /// Downvote tally.
    pub downvotes: u32,
    /// `upvotes - downvotes`.
    pub score: i64,
    /// How many comments the item carries.
    pub comment_count: u32,
    /// This credential's current vote.
    pub my_vote: VoteValue,
    /// The tracking issue, once one exists.
    pub issue_url: Option<String>,
    /// ISO-8601, as the hub reports it. Kept a string rather than converted to
    /// epoch millis so this crate needs no date dependency for a value it only
    /// forwards; the console parses it with `Date.parse`.
    pub created_at: String,
}

/// One comment on a board item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoardComment {
    /// The comment's id.
    pub id: String,
    /// The author's display name, when the hub returns one.
    pub author: Option<String>,
    /// The comment text.
    pub body: String,
    /// ISO-8601, as the hub reports it.
    pub created_at: String,
}

/// One page of board items.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoardPage {
    /// The items on this page, in the requested order.
    pub items: Vec<BoardItem>,
    /// How many items match the query in total, across every page.
    pub total: u32,
    /// The 1-based page this is.
    pub page: u32,
    /// The page size actually applied.
    pub limit: u32,
}

/// One item together with its comments.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoardDetail {
    /// The item.
    pub item: BoardItem,
    /// Its comments, oldest first, as the hub orders them.
    pub comments: Vec<BoardComment>,
}

#[cfg(test)]
#[path = "board_tests.rs"]
mod tests;
