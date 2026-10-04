//! What every agent turn runs inside, whoever starts it (OC-2).
//!
//! Two callers run a company agent's turn: the pool, for an isolated turn (a
//! dispatched card, a workflow node, a copilot thread, a background task), and
//! the company hive's Coordinator, for every conversational turn — an operator
//! DM, a desk, a peer message — through the TinyHiveMind adapter's
//! `TurnHooks::wrap_turn`. Both need the same envelope around the model call,
//! and it used to live inline in `CompanyAgent::run_with_steer`:
//!
//! * **one turn per agent at a time** — the agent's `turn_lock`;
//! * **the tool executor** — the turn registered in flight on the process-wide
//!   MCP host, with a channel its native belt tools' calls are handed back on,
//!   so each call runs on *this* task where the turn's task-local queues
//!   (approval scope, publish, delegation and card claims) live;
//! * **the stop hooks** — an operator steer and the in-turn spend brake,
//!   which OpenHuman fires between tool-loop iterations;
//! * **a clean usage tap** — whatever the bridge tapped before the turn
//!   belongs to no attempt of it.
//!
//! [`TurnEnvelope`] is that envelope; [`price_usages`] and
//! [`turn_findings`] are the post-turn reads both callers make of the
//! progress stream. The admission gates a turn must pass before it costs
//! anything (the total ceiling, the monthly budget, the teammate's daily cap)
//! are [`HarnessPool::admit`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use openhuman_core as oh;
use tinyhivemind_core::embed::ConversationRef;

use super::{
    BudgetPause, CeilingGate, CeilingPause, CompanyAgent, HarnessDeps, HarnessPool,
    MonthlyBudgetGate, SpendHalt, TurnOutcome, progress_pump,
};
use crate::company::steer::SteerControl;
use crate::harness::cost::TurnUsage;
use crate::hive::tools::{HiveScope, InFlight, ToolJob};
use crate::ports::types::CompanyId;

/// The envelope one agent turn runs inside. See the module docs.
pub(crate) struct TurnEnvelope<'a> {
    agent: &'a CompanyAgent,
    hooks: Vec<Arc<dyn oh::agent::stop_hooks::StopHook>>,
    spend_brake: Option<(f64, Arc<AtomicBool>)>,
}

impl<'a> TurnEnvelope<'a> {
    /// The envelope for one of `agent`'s turns, with `steer` installed when
    /// the operator holds a control over it.
    pub(crate) fn new(agent: &'a CompanyAgent, steer: Option<&SteerControl>) -> Self {
        let mut hooks: Vec<Arc<dyn oh::agent::stop_hooks::StopHook>> = Vec::new();
        if let Some(control) = steer {
            hooks.push(Arc::new(crate::harness::steer::SteerStopHook::new(
                control.clone(),
            )));
        }
        let mut spend_brake = None;
        if let Some(cap) = agent.turn_spend_cap_usd() {
            let hook = crate::harness::spend::SpendStopHook::new(cap);
            spend_brake = Some((cap, hook.halted()));
            hooks.push(Arc::new(hook));
        }
        Self {
            agent,
            hooks,
            spend_brake,
        }
    }

    /// Whether the in-turn spend brake fired.
    pub(crate) fn spend_halted(&self) -> bool {
        self.spend_brake
            .as_ref()
            .is_some_and(|(_, halted)| halted.load(Ordering::SeqCst))
    }

    /// The spend halt to report, when the brake fired: what the turn spent
    /// against the cap it was stopped at.
    pub(crate) fn halt(&self, usages: &[TurnUsage]) -> Option<SpendHalt> {
        self.spend_brake.as_ref().and_then(|(cap_usd, halted)| {
            halted.load(Ordering::SeqCst).then(|| SpendHalt {
                agent: self.agent.agent_id.clone(),
                spent_usd: usages.iter().map(|usage| usage.cost_usd).sum(),
                cap_usd: *cap_usd,
            })
        })
    }

    /// Runs `body` as this agent's one turn: under its turn lock, registered
    /// in flight on `surface` (and `hive`, for a coordinator turn) with its
    /// tool calls executed on this task, inside its stop hooks.
    pub(crate) async fn run<T, F>(
        &self,
        surface: ConversationRef,
        hive: Option<HiveScope>,
        body: F,
    ) -> T
    where
        F: std::future::Future<Output = T>,
    {
        let agent = self.agent;
        let _turn = agent.turn_lock.lock().await;
        let (job_tx, mut job_rx) = tokio::sync::mpsc::channel::<ToolJob>(8);
        let in_flight = agent.mcp.in_flight();
        let mut registration = InFlight::new(
            agent.company.clone(),
            agent.runtime_id.clone(),
            agent.agent_id.clone(),
            surface,
        )
        .with_executor(job_tx.clone());
        if let Some(hive) = hive {
            registration = registration.with_hive(hive);
        }
        // The lock is held, so nothing else of this agent's should be
        // registered; a caller that registered first regardless keeps its own
        // entry and this turn only lends it the executor.
        let ticket = match in_flight.begin(registration) {
            Ok(ticket) => Some(ticket),
            Err(_) => {
                in_flight.with(&agent.runtime_id, |turn| {
                    turn.executor = Some(job_tx.clone());
                });
                None
            }
        };
        drop(job_tx);
        let served = agent.mcp.agent(&agent.runtime_id);
        let serve_jobs = async {
            while let Some(job) = job_rx.recv().await {
                let turn = in_flight.snapshot(&agent.runtime_id);
                let result = match &served {
                    Some(served) => served.serve_call(&job.tool, job.arguments, turn).await,
                    None => serde_json::json!({
                        "content": [{ "type": "text", "text": format!(
                            "refused: '{}' is not served for this agent", job.tool
                        ) }],
                        "isError": true,
                    }),
                };
                let _ = job.reply.send(result);
            }
            // The registry's sender outlives the turn, so this loop ends only
            // if the entry was dropped under us; never let it end the select.
            std::future::pending::<()>().await;
        };
        // Anything left on the taps belongs to no attempt of ours.
        let _ = agent.bridge.take_usage();
        let _ = agent.bridge.take_errors();
        let body = oh::agent::stop_hooks::with_stop_hooks(self.hooks.clone(), Box::pin(body));
        let out = tokio::select! {
            biased;
            out = body => out,
            () = serve_jobs => unreachable!("the tool-job loop never completes"),
        };
        match ticket {
            Some(ticket) => {
                let _ = ticket.finish();
            }
            None => {
                in_flight.with(&agent.runtime_id, |turn| turn.executor = None);
            }
        }
        out
    }
}

/// Prices attempts whose bridge tap saw tokens but no price — or nothing at
/// all — from the turn's own progress stream.
///
/// The bridge tap is authoritative for tokens and for a charged amount the
/// provider reported. A provider that reports tokens but no price (the
/// managed backend's billing meta is absent on a BYOK route, and on every
/// scripted double) leaves the cost at zero there, while the runtime's own
/// `TurnCostUpdated` carries its catalogue estimate — the figure the in-turn
/// spend brake fired on — so that estimate stands in for the price, and for
/// everything when the tap saw nothing. `TurnCostUpdated` is a cumulative
/// rollup, so the stream's last one prices the turn; it supplies the price
/// only, and only to an attempt that burned something.
pub(crate) fn price_usages(
    agent_id: &str,
    usages: &mut [TurnUsage],
    events: &[oh::agent::progress::AgentProgress],
) {
    if !usages
        .iter()
        .any(|usage| usage.is_zero() || usage.cost_usd == 0.0)
    {
        return;
    }
    let segments = progress_pump::attempt_event_segments(events, usages.len());
    let rollup = progress_pump::last_observed_turn_cost(events);
    for (usage, segment) in usages.iter_mut().zip(segments) {
        let Some(observed) = progress_pump::last_observed_turn_cost(segment) else {
            if !usage.is_zero()
                && usage.cost_usd == 0.0
                && let Some(rollup) = rollup.as_ref()
                && rollup.cost_usd > 0.0
            {
                usage.cost_usd = rollup.cost_usd;
            }
            continue;
        };
        if usage.is_zero() {
            tracing::info!(
                agent = %agent_id,
                input_tokens = observed.input_tokens,
                output_tokens = observed.output_tokens,
                cost_usd = observed.cost_usd,
                "[turn] an attempt published no totals; metering the spend observed on its own \
                 progress-stream segment"
            );
            *usage = observed;
        } else if usage.cost_usd == 0.0 && observed.cost_usd > 0.0 {
            usage.cost_usd = observed.cost_usd;
        }
    }
}

/// What a finished turn's progress stream and pause slots say about how it
/// stopped.
#[derive(Debug, Default)]
pub(crate) struct TurnFindings {
    /// It paused at the tool-iteration cap (never alongside a spend halt).
    pub(crate) hit_iteration_cap: bool,
    /// The in-turn spend brake stopped it.
    pub(crate) halted_for_spend: Option<SpendHalt>,
    /// The inference budget ran out under it.
    pub(crate) budget_paused: Option<BudgetPause>,
    /// It hit the per-turn wall-clock ceiling.
    pub(crate) ceiling_paused: Option<CeilingPause>,
}

/// Reads [`TurnFindings`] off a finished turn.
///
/// #988: a spend halt reads `hit_iteration_cap == false` — the halt is the
/// more specific account of why the turn stopped, and a step pause would
/// invite "continue" on a budget that has already run out.
pub(crate) fn turn_findings(
    agent_id: &str,
    events: &[oh::agent::progress::AgentProgress],
    halted_for_spend: Option<SpendHalt>,
    budget_summary: Option<String>,
    ceiling: Option<(String, std::time::Duration)>,
) -> TurnFindings {
    let raw_iteration_cap = progress_pump::hit_iteration_cap(events);
    let hit_iteration_cap =
        progress_pump::reportable_iteration_cap(raw_iteration_cap, halted_for_spend.is_some());
    if hit_iteration_cap {
        tracing::info!(
            agent = %agent_id,
            "[turn] paused at the tool-iteration cap; the reply is a resumable checkpoint, not a \
             finished answer"
        );
    }
    if let Some(halt) = &halted_for_spend {
        tracing::info!(
            agent = %agent_id,
            spent_usd = halt.spent_usd,
            cap_usd = halt.cap_usd,
            "[turn] halted at the in-turn spend cap; the reply stops short of the work it was doing"
        );
    }
    let budget_paused = budget_summary.map(|summary| BudgetPause {
        agent: agent_id.to_string(),
        summary,
    });
    if let Some(pause) = &budget_paused {
        tracing::info!(
            agent = %agent_id,
            "[turn] paused for lack of inference budget/credits: {}",
            pause.summary
        );
    }
    let ceiling_paused = ceiling.map(|(summary, elapsed)| CeilingPause {
        agent: agent_id.to_string(),
        elapsed,
        summary,
    });
    TurnFindings {
        hit_iteration_cap,
        halted_for_spend,
        budget_paused,
        ceiling_paused,
    }
}

/// What a turn holds while it runs once [`HarnessPool::admit`] let it in: the
/// total-ceiling reservation and the monthly-budget serialization guard, both
/// released when it drops.
pub(crate) struct Admission {
    _ceiling: Option<crate::metering::TokenReservation>,
    _monthly: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl HarnessPool {
    /// The admission gates a turn of `agent`'s must pass before it costs
    /// anything: the company's total token ceiling, its monthly budget, and
    /// the teammate's own daily spend cap (issue #304). `Err` is the turn's
    /// whole outcome — a refusal the operator reads, with no model call made.
    pub(crate) async fn admit(
        &self,
        company: &CompanyId,
        agent: &CompanyAgent,
        deps: &HarnessDeps,
    ) -> Result<Admission, Box<TurnOutcome>> {
        let agent_id = agent.agent_id.as_str();
        let ceiling = match Self::total_ceiling_refusal(company, agent_id, deps).await {
            CeilingGate::Admitted(reservation) => reservation,
            CeilingGate::Refused(refusal) => return Err(Box::new(refusal)),
        };
        let monthly = match self.monthly_budget_refusal(company, agent_id, deps).await {
            MonthlyBudgetGate::Admitted(guard) => guard,
            MonthlyBudgetGate::Refused(refusal) => return Err(refusal),
        };
        if let Some(refusal) = super::daily_cap_refusal(company, agent, deps).await {
            return Err(Box::new(refusal));
        }
        Ok(Admission {
            _ceiling: ceiling,
            _monthly: monthly,
        })
    }
}

#[cfg(test)]
#[path = "turn_envelope_tests.rs"]
mod tests;
