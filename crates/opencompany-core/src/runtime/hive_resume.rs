//! Handing an operator's decision back to the company-hive agent that asked
//! (OC-2).
//!
//! A coordinator turn parks its approvals under a turn key of its own,
//! `hive-turn:{agent}:{episode}`, so the resolve path can tell a hive turn's
//! approval from a chat cycle's or a workflow run's by the key alone. The
//! Coordinator holds that agent parked until the host releases it; when the
//! last decision it waits on lands, the runtime renders every decision as a
//! [`SeatDecision::note`] and releases the agent with it
//! (`Coordinator::release_with`), which the agent reads at the top of its next
//! turn.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use crate::ports::types::ApprovalId;

/// The prefix of every coordinator turn's approval turn key.
pub const HIVE_TURN_PREFIX: &str = "hive-turn:";

/// One coordinator turn's agent and episode, as its turn key names them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HiveSeat {
    /// The manifest agent id whose turn parked.
    pub agent_id: String,
    /// The episode the turn ran for; `None` for a direct message.
    pub episode_id: Option<String>,
}

/// The approval turn key a coordinator turn's parks are recorded under.
#[must_use]
pub fn turn_key(agent_id: &str, episode_id: Option<&str>) -> String {
    format!("{HIVE_TURN_PREFIX}{agent_id}:{}", episode_id.unwrap_or_default())
}

/// The agent and episode a turn key names, or `None` for any other key.
#[must_use]
pub fn parse(turn: &str) -> Option<HiveSeat> {
    let rest = turn.strip_prefix(HIVE_TURN_PREFIX)?;
    let (agent_id, episode_id) = rest.split_once(':')?;
    if agent_id.is_empty() {
        return None;
    }
    Some(HiveSeat {
        agent_id: agent_id.to_owned(),
        episode_id: (!episode_id.is_empty()).then(|| episode_id.to_owned()),
    })
}

/// What a seat asked the operator for.
#[derive(Clone, Debug, PartialEq)]
pub enum SeatAsk {
    /// A gated tool call, re-issued under its single-use grant when approved.
    Call {
        /// The tool.
        tool: String,
        /// The exact arguments the grant admits.
        args: serde_json::Value,
    },
    /// An explicit `request_approval`.
    Request {
        /// What the seat asked to do.
        title: String,
    },
    /// An `escalate_to_human` question.
    Question {
        /// What the seat needed.
        needed: String,
    },
}

/// How the operator answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeatVerdict {
    /// Approved, or told to go ahead.
    Approved,
    /// Denied, cancelled, or left to expire.
    Denied,
    /// Told to skip the step and carry on without it.
    Skipped,
}

/// One decision on one of a seat's parked approvals.
#[derive(Clone, Debug, PartialEq)]
pub struct SeatDecision {
    /// The approval decided.
    pub approval_id: ApprovalId,
    /// What had been asked.
    pub ask: SeatAsk,
    /// What the operator decided.
    pub verdict: SeatVerdict,
    /// The operator's own words, when they wrote any.
    pub answer: String,
}

impl SeatDecision {
    /// The decision in plain words, addressed to the seat that asked.
    #[must_use]
    pub fn note(&self) -> String {
        let answer = self.answer.trim();
        let words = if answer.is_empty() {
            String::new()
        } else {
            format!(" The operator wrote: \"{answer}\"")
        };
        match (&self.ask, self.verdict) {
            (SeatAsk::Call { tool, args }, SeatVerdict::Approved) => {
                let args = serde_json::to_string(args).unwrap_or_else(|_| "{}".to_owned());
                format!(
                    "The operator approved your `{tool}` call. Make it again now with exactly \
                     these arguments: {args}.{words}"
                )
            }
            (SeatAsk::Call { tool, .. }, _) => format!(
                "The operator did not approve your `{tool}` call, so it did not run. Do not \
                 retry it; carry on without it.{words}"
            ),
            (SeatAsk::Request { title }, SeatVerdict::Approved) => format!(
                "The operator approved your request: {title}. Go ahead. Do not ask again for \
                 the same thing.{words}"
            ),
            (SeatAsk::Request { title }, _) => format!(
                "The operator denied your request: {title}. Do not do it; carry on without it \
                 or stop that part of the work.{words}"
            ),
            (SeatAsk::Question { needed }, SeatVerdict::Approved) if !answer.is_empty() => {
                format!("The operator answered your question ({needed}): \"{answer}\"")
            }
            (SeatAsk::Question { needed }, SeatVerdict::Approved) => {
                format!("The operator told you to go ahead with what you asked about ({needed}).")
            }
            (SeatAsk::Question { needed }, SeatVerdict::Skipped) => format!(
                "The operator said to skip what you asked about ({needed}) and carry on \
                 without it.{words}"
            ),
            (SeatAsk::Question { needed }, SeatVerdict::Denied) => format!(
                "The operator declined what you asked about ({needed}). Stop that part of the \
                 work.{words}"
            ),
        }
    }
}

/// The operator's own answers to escalations a hive agent raised, held until
/// the decision that carries them is assembled — which is after the answer has
/// left the blocker queue.
///
/// One per company, shared by the runtime that resolves approvals. Cloning
/// shares the state.
#[derive(Clone, Default)]
pub struct HiveAnswers {
    answers: Arc<Mutex<HashMap<ApprovalId, (SeatVerdict, String)>>>,
}

impl std::fmt::Debug for HiveAnswers {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("HiveAnswers").finish_non_exhaustive()
    }
}

impl HiveAnswers {
    /// Holds the operator's answer to `approval_id`.
    pub fn answer(&self, approval_id: &ApprovalId, verdict: SeatVerdict, answer: String) {
        self.answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(approval_id.clone(), (verdict, answer));
    }

    /// Takes the answer [`answer`](Self::answer) held for `approval_id`.
    #[must_use]
    pub fn take_answer(&self, approval_id: &ApprovalId) -> Option<(SeatVerdict, String)> {
        self.answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(approval_id)
    }
}

/// Every decision a parked agent waits on, rendered as the one release note
/// it reads at the top of its next turn.
#[must_use]
pub fn release_note(decisions: &[SeatDecision]) -> Option<String> {
    let notes: Vec<String> = decisions.iter().map(SeatDecision::note).collect();
    (!notes.is_empty()).then(|| notes.join("\n"))
}

#[cfg(test)]
#[path = "hive_resume_tests.rs"]
mod tests;
