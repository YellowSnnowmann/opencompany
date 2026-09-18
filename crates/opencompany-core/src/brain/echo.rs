//! [`EchoBrain`]: the offline, dependency-free default [`Brain`].
//!
//! It has zero TinyAgents dependency and is the Phase-1 default cognition seam.
//! Each cycle it echoes every operator message back, emits one trivial
//! `Other`-group effect to exercise the [`CycleHost`] approval path, and
//! records a single [`CompressedTrace`]. It guarantees at least one channel
//! response per cycle, satisfying the runtime's ≥1-response invariant cheaply.

use async_trait::async_trait;

use crate::Result;
use crate::ports::brain::{Brain, Cognition, CycleHost, ECHO_PATH, UsageMetering};
use crate::ports::types::{
    CompanyEvent, CompressedTrace, CycleRequest, CycleResult, Effect, EffectGroup, OutboundMessage,
    TokenUsage,
};

/// The offline echo brain: turns operator messages into acknowledgements.
#[derive(Clone, Copy, Debug, Default)]
pub struct EchoBrain;

impl EchoBrain {
    /// Creates an echo brain.
    pub fn new() -> Self {
        Self
    }

    /// The trivial effect emitted each cycle to prove the gate path is wired.
    fn heartbeat_effect() -> Effect {
        Effect {
            kind: "echo.noop".to_string(),
            group: EffectGroup::Other,
            amount_usd: None,
            established_thread: false,
            first_time_counterparty: false,
            payload: serde_json::Value::Null,
            agent: None,
            run_id: None,
        }
    }
}

#[async_trait]
impl Brain for EchoBrain {
    async fn run_cycle(&self, req: CycleRequest, host: &dyn CycleHost) -> Result<CycleResult> {
        let mut channel_responses = Vec::new();
        for event in &req.events {
            if let CompanyEvent::OperatorMessage { text, .. } = event {
                channel_responses.push(OutboundMessage {
                    message_id: None,
                    task_id: None,
                    outputs: Vec::new(),
                    channel: "operator".to_string(),
                    agent: None,
                    text: format!("You said: {text}"),
                    steps: Vec::new(),
                    reply_to: None,
                    mentions: Vec::new(),
                });
            }
            if let CompanyEvent::WebhookReceived { channel, .. } = event {
                let (text, reply_to) = (format!("webhook on {channel}"), None);
                channel_responses.push(OutboundMessage {
                    message_id: None,
                    task_id: None,
                    outputs: Vec::new(),
                    channel: channel.clone(),
                    agent: None,
                    text,
                    steps: Vec::new(),
                    reply_to,
                    mentions: Vec::new(),
                });
            }
        }
        if channel_responses.is_empty() {
            channel_responses.push(OutboundMessage {
                message_id: None,
                task_id: None,
                outputs: Vec::new(),
                channel: "operator".to_string(),
                agent: None,
                text: "Acknowledged.".to_string(),
                steps: Vec::new(),
                reply_to: None,
                mentions: Vec::new(),
            });
        }

        // Exercise the approval/effect path; the disposition is informational
        // for the echo brain, so we don't branch on it.
        let _ = host.emit_effect(Self::heartbeat_effect()).await?;

        let trace = CompressedTrace::now(
            req.cycle_id.clone(),
            format!("echo cycle handled {} event(s)", req.events.len()),
        );

        Ok(CycleResult {
            channel_responses,
            new_traces: vec![trace],
            ledger_deltas: Vec::new(),
            token_usage: TokenUsage::default(),
        })
    }

    /// No model is ever called here, so there is nothing to meter: a zero Usage
    /// reading on this path is the truth, not a missing hook. Surfacing that is
    /// what lets an operator tell "no inference configured" from "metering
    /// broken" (issue #174).
    fn cognition(&self) -> Cognition {
        Cognition {
            path: ECHO_PATH,
            provider: "none",
            // No model is called on this path, so there is no model to name.
            model: None,
            metering: UsageMetering::None,
        }
    }
}

#[cfg(test)]
#[path = "echo_tests.rs"]
mod tests;
