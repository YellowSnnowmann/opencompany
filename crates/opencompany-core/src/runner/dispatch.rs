//! Handing a company's turn to a runner on someone else's machine.
//!
//! ## The spike, and why `Proxy`/`Conductor` is not used
//!
//! The ACP Rust SDK ships a proxy layer, and the obvious question was whether
//! it could carry this. It cannot, and the reason is structural rather than a
//! missing feature.
//!
//! `agent-client-protocol-conductor` takes `components: Vec<String>` — "a list
//! of commands to chain together; **the final command must be the agent**" —
//! and spawns that chain when `initialize` arrives. The topology is a fixed
//! linear chain to exactly **one** upstream, fixed per connection. It is built
//! for *adding capabilities* to one agent (inject an MCP server, prepend a
//! preamble), and it does that well.
//!
//! What dispatch needs is the other shape entirely: many downstream clients,
//! many upstream runners, and the upstream chosen **per session** at
//! `session/new` from whichever runner currently holds the scope. A fixed chain
//! cannot express "this session goes to Ada's laptop and that one to Bob's",
//! and there is no point in the conductor's lifecycle where that choice could
//! be made.
//!
//! So the routing is ours. That costs less than it sounds, because the pieces
//! already exist: [`AcpRunTurn`](crate::harness::acp_run_turn) folds an ACP
//! turn into a [`TurnOutcome`], and it takes an
//! [`AcpAgent`](crate::harness::acp_run_turn::AcpAgent) port. A runner is just
//! another implementation of that port.
//!
//! ## Session ids are rewritten, not forwarded
//!
//! A runner mints its own session ids, and so does this host. Forwarding either
//! side's id to the other would mean two namespaces sharing a keyspace — and
//! the first collision between two runners' `sess-1` would silently cross two
//! companies' turns. [`SessionMap`] keeps them apart.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::Result;
use crate::error::OpenCompanyError;
use crate::harness::acp_run_turn::{AcpAgent, AcpTurn};
use crate::ports::types::CompanyId;
use crate::runner::registry::RunnerRegistry;

/// One runner's wire, as dispatch needs it.
///
/// A port for the same reason [`AcpAgent`] is: the socket belongs to the server
/// lane, and dispatch should be testable without one.
#[async_trait]
pub trait RunnerLink: Send + Sync {
    /// Opens a session on the runner, returning **its** session id.
    async fn open_session(&self, runner_id: &str, scope: &str) -> Result<String>;

    /// Runs one turn on an already-open runner session.
    async fn prompt(&self, runner_id: &str, runner_session: &str, message: &str)
    -> Result<AcpTurn>;

    /// Forwards a cancel. Advisory, like every ACP cancel.
    async fn cancel(&self, runner_id: &str, runner_session: &str) -> Result<()>;
}

/// Maps this host's session keys onto runner-side session ids.
#[derive(Debug, Default)]
pub struct SessionMap {
    /// `(runner_id, host session key)` → the runner's own session id.
    inner: Mutex<HashMap<(String, String), String>>,
}

impl SessionMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, runner_id: &str, host_key: &str) -> Option<String> {
        self.inner
            .lock()
            .expect("session map poisoned")
            .get(&(runner_id.to_string(), host_key.to_string()))
            .cloned()
    }

    pub fn insert(&self, runner_id: &str, host_key: &str, runner_session: String) {
        self.inner.lock().expect("session map poisoned").insert(
            (runner_id.to_string(), host_key.to_string()),
            runner_session,
        );
    }

    /// Drops every session a runner held.
    ///
    /// Called when a runner detaches. Without it, a runner that reconnects
    /// would be handed session ids from its previous life — ids its new process
    /// has never heard of, so every turn would fail in a way that looks like a
    /// protocol bug rather than a stale mapping.
    pub fn forget_runner(&self, runner_id: &str) {
        self.inner
            .lock()
            .expect("session map poisoned")
            .retain(|(id, _), _| id != runner_id);
    }

    pub fn len(&self) -> usize {
        self.inner.lock().expect("session map poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An [`AcpAgent`] backed by whichever runner currently holds the scope.
pub struct RunnerDispatch<L: RunnerLink> {
    registry: std::sync::Arc<RunnerRegistry>,
    link: L,
    sessions: SessionMap,
    /// Injected so tests are not at the mercy of the clock.
    now: fn() -> u64,
}

impl<L: RunnerLink> RunnerDispatch<L> {
    pub fn new(registry: std::sync::Arc<RunnerRegistry>, link: L) -> Self {
        Self {
            registry,
            link,
            sessions: SessionMap::new(),
            now: crate::ports::now_millis,
        }
    }

    #[cfg(test)]
    fn with_clock(mut self, now: fn() -> u64) -> Self {
        self.now = now;
        self
    }

    /// Picks a runner for `scope`, or explains why there is none.
    ///
    /// The message distinguishes "nothing is attached" from "something is
    /// attached but cannot work", because those have completely different
    /// answers — start the desktop, versus sign the harness in.
    fn choose(&self, scope: &str) -> Result<String> {
        let now = (self.now)();
        if let Some(runner) = self.registry.available_for(scope, now).into_iter().next() {
            return Ok(runner.runner_id);
        }
        let attached = self
            .registry
            .list()
            .into_iter()
            .filter(|r| r.scopes.iter().any(|s| s == scope))
            .collect::<Vec<_>>();
        Err(OpenCompanyError::InvalidRequest(if attached.is_empty() {
            format!("no runner is attached for {scope}")
        } else if attached.iter().any(|r| r.is_live(now)) {
            format!("the runner for {scope} has no signed-in harness")
        } else {
            format!("the runner for {scope} has stopped reporting")
        }))
    }
}

#[async_trait]
impl<L: RunnerLink> AcpAgent for RunnerDispatch<L> {
    /// `observer` is ignored, and can only be ignored here: [`RunnerLink`]
    /// hands back a whole [`AcpTurn`] when the remote turn is over, so there
    /// is no per-update stream on this side to tee. Making it observable is a
    /// change to the runner *wire* — the socket would have to forward each
    /// `session/update` as it arrives instead of the turn's transcript at the
    /// end — not something this fold can synthesise. Until then an ACP turn on
    /// a runner shows its steps when it finishes, exactly as it did before,
    /// while a local one shows them live.
    async fn prompt(
        &self,
        _company: &CompanyId,
        session_key: &str,
        message: &str,
        _observer: Option<&crate::ports::acp::AcpObserver>,
    ) -> Result<AcpTurn> {
        // Chosen per turn rather than pinned at session start: a runner can go
        // away between turns, and re-choosing is how the next turn lands on
        // whatever replaced it instead of failing against a dead one.
        let runner_id = self.choose(session_key)?;

        let runner_session = match self.sessions.get(&runner_id, session_key) {
            Some(existing) => existing,
            None => {
                let opened = self.link.open_session(&runner_id, session_key).await?;
                self.sessions
                    .insert(&runner_id, session_key, opened.clone());
                opened
            }
        };

        self.link.prompt(&runner_id, &runner_session, message).await
    }

    async fn cancel(&self, _company: &CompanyId, session_key: &str) -> Result<()> {
        let runner_id = self.choose(session_key)?;
        let Some(runner_session) = self.sessions.get(&runner_id, session_key) else {
            // Nothing open to cancel. Not an error: a cancel racing the first
            // turn of a session is ordinary, and failing it would surface as a
            // scary message for a no-op.
            return Ok(());
        };
        self.link.cancel(&runner_id, &runner_session).await
    }
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod tests;
