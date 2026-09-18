//! Who a parked blocker is attributed to — the teammate whose direct message
//! it surfaces in (issue #1862).
//!
//! A blocker is a question, and a question in a company is asked *by* someone.
//! [`resolve_sender`] answers "by whom", so the card lands in the operator's DM
//! with the responsible teammate rather than in an undifferentiated queue.
//!
//! It is deliberately **dumb**: it reads whatever trigger-time attribution the
//! park already carries and picks the first rung that names a real roster
//! agent. It never inspects *why* the work stopped — the gap class is #1866's
//! job, decided once in [`blockers`](crate::harness::built_in::blockers) — and
//! never promises the stop will resume, which is #1863's boundary. All it
//! decides is whose name goes on the question.

use crate::ports::types::{CompanyRecord, StartedBy};

/// The host's own last-resort identity, used when nothing else named a sender.
///
/// A reserved channel slug ([`crate::runtime::channel`]) no company can give a
/// teammate, so a blocker with no attribution still resolves to a real,
/// collision-free DM channel instead of vanishing.
pub const HOST_SENDER: &str = "workflow";

/// The trigger-time attribution a park carries into sender resolution.
///
/// Every field is optional because each park site knows a different subset: a
/// workflow run knows its [`StartedBy`], a planning pass knows the card's
/// assignee, and neither knows the other's. An all-`None` bag is honest — it
/// means "nothing was named" and resolves to the orchestrator or the host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockerSenderSignals {
    /// The [`StartedBy`] of the run this stop came from, when it came from one.
    /// Only [`StartedBy::Agent`] attributes a teammate; an operator- or
    /// schedule-started run names nobody to route to.
    pub started_by: Option<StartedBy>,
    /// The desk that owns the stopped work, resolved through its lead.
    pub owner_desk: Option<String>,
    /// The teammate the stopped card is assigned to — the planning pass's one
    /// piece of attribution.
    pub assignee: Option<String>,
}

/// Resolves the teammate a blocker is attributed to, as a roster agent id.
///
/// The rungs, first match wins:
/// 1. **The triggering agent.** A run an agent started owns its own stops.
/// 2. **The owning desk's lead.** A leadless [`Auto`] desk (issue #1835)
///    resolves `None` here — its per-message selector needs the message text a
///    park does not have — and falls through rather than guessing a member.
/// 3. **The card's assignee.** The planning pass's responsible teammate.
/// 4. **The orchestrator**, which answers anything otherwise unaddressed.
/// 5. **The host** ([`HOST_SENDER`]), so a blocker always lands somewhere real.
///
/// [`Auto`]: crate::ports::types::ResponderMode::Auto
pub fn resolve_sender(record: &CompanyRecord, signals: &BlockerSenderSignals) -> String {
    if let Some(StartedBy::Agent(id)) = &signals.started_by
        && record.is_roster_agent(id)
    {
        return id.clone();
    }
    if let Some(desk) = &signals.owner_desk
        && let Some(lead) = crate::runtime::delegation_tools::desk_lead(record, desk)
    {
        return lead;
    }
    if let Some(assignee) = &signals.assignee
        && record.is_roster_agent(assignee)
    {
        return assignee.clone();
    }
    if let Some(id) = crate::company::types::orchestrator_id(&record.effective_agents()) {
        return id.to_string();
    }
    HOST_SENDER.to_string()
}

/// The console channel a resolved sender's DM lives under — the id a park
/// stamps as its thread and a badge lands on.
pub fn dm_thread(sender: &str) -> String {
    format!("dm:{sender}")
}

#[cfg(test)]
#[path = "blocker_sender_tests.rs"]
mod tests;
