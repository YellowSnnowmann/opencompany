//! The production [`SeatRunner`]: one seat turn over the harness pool.
//!
//! A seat turn is an ordinary pool turn — the same tools, the same memory
//! loop, the same metering and run row, the same live tool frames on the
//! desk — with three things the pool learns from the seat scope this runner
//! sets around it (`delegation::seat_turn`): the episode and round the MCP
//! server attributes the seat's speech to, the timeout counted from the
//! moment the agent's turn lock is held, and the outbox the seat's one
//! utterance comes back through. Everything else about running an agent
//! stays where it is, which is the point: a seat differs from a chat
//! responder in the prompt it is handed and in nothing else.

use std::sync::Arc;

use async_trait::async_trait;

use crate::company::steer::SteerControl;
use crate::hive::driver::{SeatFailure, SeatOutcome, SeatRunner, SeatTurn};
use crate::runtime::delegation::{ChatTarget, RunTurn, SeatTurnScope, with_seat_turn};

/// Runs seat turns on a harness [`RunTurn`] lane.
pub struct HarnessSeatRunner {
    /// The lane the turns run on.
    pub run_turn: Arc<dyn RunTurn>,
}

#[async_trait]
impl SeatRunner for HarnessSeatRunner {
    async fn run_seat(&self, seat: SeatTurn) -> std::result::Result<SeatOutcome, SeatFailure> {
        let scope = Arc::new(SeatTurnScope::new(seat.hive.clone(), seat.timeout));
        let control = SteerControl::new();
        let chat = ChatTarget::in_thread(Some(seat.chat_id.as_str()), seat.thread_root)
            .answering(seat.message_seq);
        let outcome = with_seat_turn(
            Arc::clone(&scope),
            self.run_turn.run_steered(
                &seat.company,
                &seat.agent_id,
                &seat.message,
                &control,
                chat,
                None,
            ),
        )
        .await;
        let turn = match outcome {
            Ok(turn) => turn,
            Err(_) if scope.timed_out() => return Err(SeatFailure::TimedOut),
            Err(error) => return Err(SeatFailure::Failed(error.to_string())),
        };
        // A pause, a spend halt or an abnormal stop is host-authored copy in
        // `reply`, not the seat's answer: folding it as a turn that spoke
        // would journal that copy under the seat's own name.
        if let Some(pause) = &turn.budget_paused {
            return Err(SeatFailure::Failed(format!(
                "paused for lack of inference budget: {}",
                pause.summary
            )));
        }
        if let Some(halt) = &turn.halted_for_spend {
            return Err(SeatFailure::Failed(format!(
                "halted for spend: ${:.2} against a cap of ${:.2}",
                halt.spent_usd, halt.cap_usd
            )));
        }
        if let Some(reason) = &turn.abnormal_stop {
            return Err(SeatFailure::Failed(format!("did not complete its turn: {reason}")));
        }
        Ok(SeatOutcome {
            reply: turn.reply,
            utterances: scope.take_outbox(),
            steps: turn.steps,
            outputs: Vec::new(),
        })
    }
}
