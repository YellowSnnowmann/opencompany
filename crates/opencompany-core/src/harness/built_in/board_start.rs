//! Starting a board card from inside a turn — the one write an agent could not
//! reach.
//!
//! # The gap this closes
//!
//! `spawn_task` opens a card in [`COLUMN_TODO`](crate::ports::tasks::COLUMN_TODO)
//! and stops. Dispatch is edge-fired on the `todo → in_progress` transition inside
//! [`CompanyRuntime::upsert_task`](crate::company::CompanyRuntime::upsert_task),
//! and that function is reachable only from the REST boundary — the console's
//! column drag. Every in-process path writes through the plain
//! [`TaskStore`](crate::ports::TaskStore) port, which deliberately has no runtime
//! handle, so it cannot fire the edge. `AssignTaskTool` says as much in its own
//! docs: *"It does not (re)dispatch."*
//!
//! The consequence was that **nothing but a human could start a card.** An agent
//! could open one, assign it, and watch it sit there.
//!
//! # Why a handle, and why a narrow one
//!
//! The circularity is the same one `run_workflow` has: the tool is built from
//! [`HarnessDeps`](crate::harness::HarnessDeps), while the thing it must reach is
//! built from the deps. [`WorkflowRunnerHandle`](super::orchestrator::WorkflowRunnerHandle)
//! solves it with a fillable `Weak` cell — empty on deps, filled once the owner
//! exists, upgraded on demand — and this is the same device.
//!
//! What differs is the width. The runner handle hands back the runner itself; this
//! one hands back a **capability**, not the runtime. A tool that could reach
//! `CompanyRuntime` could reach everything on it; one that holds a
//! [`BoardStarter`] can do exactly one thing, and the trait names it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use crate::ports::TaskRecord;
use crate::ports::types::CompanyId;

/// One handle per company, reached by id rather than threaded.
///
/// Threading it would mean a field on
/// [`HarnessDeps`](crate::harness::HarnessDeps), and that struct is built at
/// fifty-odd sites — almost all of them tests. A new field there costs a
/// fifty-file diff to express something neither the deps nor those tests have an
/// opinion about. `company_write_lock` has the same shape of problem and answers
/// it the same way, so this follows it rather than inventing a second convention.
static HANDLES: Mutex<Option<HashMap<CompanyId, BoardStarterHandle>>> = Mutex::new(None);

/// The board-start handle for `company`, created empty on first ask.
///
/// Both ends call this: the builder, to give the tool a handle to read, and the
/// registry, to fill it once the runtime is behind an `Arc`. They get the same
/// cell because it is keyed on the company and nothing else.
#[must_use]
pub fn for_company(company: &CompanyId) -> BoardStarterHandle {
    // Never `expect`: a poisoned lock here would panic inside whatever is
    // registering a company, and this crate's convention is that such a panic
    // reaches an axum handler with no `CatchPanicLayer`. Recovering the guard
    // keeps the map usable, which is strictly better than a 500 for a cell whose
    // worst failure mode is an unwired tool that refuses in its own turn.
    let mut map = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.get_or_insert_with(HashMap::new)
        .entry(company.clone())
        .or_default()
        .clone()
}

/// The one write that fires the dispatch edge.
#[async_trait::async_trait]
pub trait BoardStarter: Send + Sync {
    /// Persists `card` through the write site that edge-fires dispatch.
    ///
    /// The caller has already set the column; this is only about *which* write
    /// path the record takes, because the plain port cannot dispatch.
    async fn start(&self, card: &TaskRecord) -> crate::Result<()>;
}

/// A shared, fillable handle to the company's [`BoardStarter`].
///
/// Holds a [`Weak`], so deps → handle → starter → deps is not a strong cycle: the
/// strong reference lives on the runtime and the tool upgrades on demand. Empty
/// until filled, and always empty where nothing wired one — in which case the tool
/// refuses in its own turn rather than reporting a start that never happened.
#[derive(Clone, Default)]
pub struct BoardStarterHandle {
    inner: Arc<Mutex<Option<Weak<dyn BoardStarter>>>>,
}

impl BoardStarterHandle {
    /// Points the handle at `starter`, **replacing** whatever it held.
    ///
    /// Replacing and not write-once, because this cell outlives the runtime it
    /// points at. `WorkflowRunnerHandle` can be a `OnceLock` since it lives on
    /// deps and a rebuild mints fresh deps with a fresh cell; this one is keyed on
    /// the company and survives the swap. Write-once here would leave a rebuilt
    /// company pointing at its predecessor's shim — whose `Weak` is dead — so
    /// `start_task` would read as unwired forever after the first rebuild.
    pub fn set(&self, starter: &Arc<dyn BoardStarter>) {
        *self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::downgrade(starter));
    }

    /// The wired starter, or `None` when none was attached or its owner is gone.
    #[must_use]
    pub fn get(&self) -> Option<Arc<dyn BoardStarter>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade)
    }
}

impl std::fmt::Debug for BoardStarterHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoardStarterHandle")
            .field("wired", &self.get().is_some())
            .finish()
    }
}

#[cfg(test)]
#[path = "board_start_tests.rs"]
mod tests;
