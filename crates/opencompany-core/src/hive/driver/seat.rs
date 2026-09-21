//! The seat-turn seam of the episode host: one turn as the driver asks for
//! it, what it produced, and the trait that runs it.
//!
//! Split from `driver.rs` so the loop and its contract with the harness read
//! separately: the production runner lives in `hive::seats`, tests script one.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tinyhivemind::speech::Utterance;

use crate::hive::tools::HiveTurn;
use crate::ports::types::{ChatOutput, CompanyId, EventSeq, TurnOutcome, TurnStep};

/// The turn bracket a seat turn is journaled under, called by whoever holds
/// the agent's turn lock (plan hive-desks, Phase 8).
///
/// `TurnStarted` and `TurnSettled` / `TurnFailed` are the per-seat lane the
/// console draws and the brackets `opencompany measure` counts concurrency
/// from — and the one invariant the measurement checks is that one agent's
/// brackets never overlap. That is only true if both ends are written while
/// the agent's `turn_lock` is held: the round proposes a shared seat on two
/// desks at once and the lock serialises the turns, so a bracket opened
/// before the lock or closed after it is released overlaps its sibling on
/// the journal exactly where the runtime did not. The runner therefore
/// hands the bracket down (`SeatTurn::bracket`) and the pool calls
/// [`started`](Self::started) once it holds the lock and
/// [`settled`](Self::settled) before it lets go. A scripted runner calls
/// both itself.
#[async_trait]
pub trait SeatBracket: Send + Sync + fmt::Debug {
    /// The agent's lock is held; the turn is about to run.
    async fn started(&self);
    /// The turn returned, with the lock still held. `error` is the failure
    /// reason for a `failed` / `timed_out` outcome.
    async fn settled(&self, outcome: TurnOutcome, error: Option<String>);
}

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
    /// The journal bracket the lock holder writes around the turn.
    pub bracket: Option<Arc<dyn SeatBracket>>,
}

impl SeatTurn {
    /// Opens the bracket, when the turn carries one.
    pub async fn bracket_started(&self) {
        if let Some(bracket) = &self.bracket {
            bracket.started().await;
        }
    }

    /// Closes the bracket with the outcome the turn's result classifies to:
    /// `committed` for a turn that spoke, `no_utterance` for one that did
    /// not, `failed` / `timed_out` for an error.
    pub async fn bracket_settled(&self, result: &std::result::Result<SeatOutcome, SeatFailure>) {
        let Some(bracket) = &self.bracket else { return };
        let (outcome, error) = match result {
            Ok(turn) if turn.utterances.is_empty() => (TurnOutcome::NoUtterance, None),
            Ok(_) => (TurnOutcome::Committed, None),
            Err(SeatFailure::TimedOut) => (
                TurnOutcome::TimedOut,
                Some("the seat turn ran past its timeout".to_string()),
            ),
            Err(SeatFailure::Failed(error)) => (TurnOutcome::Failed, Some(error.clone())),
        };
        bracket.settled(outcome, error).await;
    }
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
