//! The seat-turn seam of the episode host: one turn as the driver asks for
//! it, what it produced, and the trait that runs it.
//!
//! Split from `driver.rs` so the loop and its contract with the harness read
//! separately: the production runner lives in `hive::seats`, tests script one.

use std::time::Duration;

use async_trait::async_trait;
use tinyhivemind::speech::Utterance;

use crate::hive::tools::HiveTurn;
use crate::ports::types::{ChatOutput, CompanyId, EventSeq, TurnStep};

/// One seat turn as the driver asks for it.
#[derive(Clone, Debug)]
pub struct SeatTurn {
    /// The company.
    pub company: CompanyId,
    /// The seat.
    pub agent_id: String,
    /// The rendered prompt (`hive::prompt`), sentinel first.
    pub message: String,
    /// The desk the turn answers on.
    pub chat_id: String,
    /// The thread root the episode runs in.
    pub thread_root: Option<EventSeq>,
    /// The message the turn answers, for the live turn stream.
    pub message_seq: Option<EventSeq>,
    /// The episode coordinates the MCP server attributes the seat's speech to.
    pub hive: HiveTurn,
    /// How long the turn may run once it holds its lock.
    pub timeout: Duration,
}

/// What one seat turn produced.
#[derive(Clone, Debug, Default)]
pub struct SeatOutcome {
    /// The reply text, used only to salvage a turn that called no tool.
    pub reply: String,
    /// The speech the seat made through the MCP server — at most one.
    pub utterances: Vec<Utterance>,
    /// The scrubbed step timeline, carried onto the reply row.
    pub steps: Vec<TurnStep>,
    /// Outputs the turn produced, carried onto the reply row.
    pub outputs: Vec<ChatOutput>,
}

/// Why a seat turn produced nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SeatFailure {
    /// The turn ran past its timeout, counted from lock acquisition.
    TimedOut,
    /// The turn errored.
    Failed(String),
}

/// Runs one seat turn. The production implementation binds to the harness
/// pool; tests script one.
#[async_trait]
pub trait SeatRunner: Send + Sync {
    /// Runs the turn and hands back what the seat said.
    async fn run_seat(&self, seat: SeatTurn) -> std::result::Result<SeatOutcome, SeatFailure>;
}
