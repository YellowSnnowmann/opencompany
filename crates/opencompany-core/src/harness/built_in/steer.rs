//! [`SteerStopHook`]: the openhuman [`StopHook`] that honors an operator steer
//! between tool-loop iterations (issue #111).
//!
//! OpenHuman's stop hooks fire **between** iterations of an agent's tool-call
//! loop — never mid-tool-call — and a [`StopDecision::Stop`] pauses the run
//! gracefully before the next provider call
//! ([`oh::agent::stop_hooks`]). This hook wraps one
//! [`SteerControl`](crate::company::steer::SteerControl): while no operator
//! action pends it returns [`Continue`](StopDecision::Continue); the moment the
//! operator pauses / cancels / redirects the run, the shared control flips and
//! the next `check` stops the turn. The disposition site then reads the action
//! off the same control and moves the card.
//!
//! One control is registered per steerable turn (task-local, installed via
//! [`with_stop_hooks`](oh::agent::stop_hooks::with_stop_hooks) around the turn),
//! so two concurrent turns never see each other's steer.
//!
//! Compiled only under `feature = "openhuman"` (the whole `harness` module is).

use async_trait::async_trait;

use openhuman_core as oh;

use oh::agent::stop_hooks::{StopDecision, StopHook, TurnState};

use crate::company::steer::SteerControl;

/// A [`StopHook`] that stops the turn as soon as its [`SteerControl`] carries a
/// pending operator action.
pub struct SteerStopHook {
    control: SteerControl,
}

impl SteerStopHook {
    /// Builds a hook over a turn's shared steer control.
    pub fn new(control: SteerControl) -> Self {
        Self { control }
    }
}

#[async_trait]
impl StopHook for SteerStopHook {
    fn name(&self) -> &str {
        "steer"
    }

    async fn check(&self, _ctx: &TurnState<'_>) -> StopDecision {
        if self.control.requested() {
            StopDecision::Stop {
                reason: "operator steer requested".to_string(),
            }
        } else {
            StopDecision::Continue
        }
    }
}

#[cfg(test)]
#[path = "steer_tests.rs"]
mod tests;
