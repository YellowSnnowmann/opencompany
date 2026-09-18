//! [`CompanyRegistry`]: the map from [`CompanyId`] to running
//! [`CompanyRuntime`].
//!
//! One type serves both deployment shapes: the prosumer single-company case
//! (use [`sole`](CompanyRegistry::sole)) and the multi-tenant platform case
//! (address companies by id). Registry operations are fast and synchronous, so
//! it is backed by a `std::sync::RwLock`, not a tokio lock.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use crate::company::runtime::CompanyRuntime;
use crate::ports::types::CompanyId;

/// A thread-safe registry of running companies.
#[derive(Clone, Default)]
pub struct CompanyRegistry {
    inner: Arc<RwLock<HashMap<CompanyId, Arc<CompanyRuntime>>>>,
    /// Set once the host has begun shutting down (issue #986), so a company
    /// registered from here on is born quiesced.
    ///
    /// `Arc`-shared like [`inner`](Self::inner): every clone of the registry —
    /// and so every clone of `AppState` — has to see the same answer, or the
    /// handler that registers a company would consult its own private `false`.
    shutting_down: Arc<AtomicBool>,
}

impl CompanyRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the runtime registered under `id`, if any.
    pub fn get(&self, id: &CompanyId) -> Option<Arc<CompanyRuntime>> {
        self.inner
            .read()
            .expect("registry poisoned")
            .get(id)
            .cloned()
    }

    /// Registers (or replaces) the runtime for `id`.
    ///
    /// A runtime registered after [`begin_shutdown`](Self::begin_shutdown) is
    /// marked quiesced before it goes in, so it refuses every cycle and the
    /// host never starts a turn it is not going to wait for (issue #986).
    ///
    /// This is the choke point rather than the individual callers because every
    /// way a company becomes addressable — boot, `POST /api/v1/companies`, a
    /// rebuild swap — ends here. Guarding the provisioning handler alone would
    /// leave the other two, and re-scanning the registry after the drain would
    /// still race with whatever lands after the final scan.
    pub fn insert(&self, id: CompanyId, runtime: Arc<CompanyRuntime>) {
        // Under the write lock, so the flag cannot be set between the check and
        // the insert: `begin_shutdown` orders itself against this by taking the
        // same lock, and the drain snapshots the map only afterwards. A company
        // is therefore either in that snapshot or born quiesced — never neither.
        let mut map = self.inner.write().expect("registry poisoned");
        let shutting_down = self.shutting_down.load(Ordering::SeqCst);
        if shutting_down {
            runtime.mark_quiesced();
        }
        map.insert(id, runtime.clone());
        drop(map);
        if !shutting_down {
            runtime.schedule_replayed_continuations();
            // Handles dropped for the same reason a replayed continuation's is:
            // registration must not wait on a resume, and dropping a handle
            // abandons the waiting rather than the work.
            drop(runtime.schedule_replayed_blocker_resolutions());
        }
    }

    /// Marks the host as shutting down, so every subsequent
    /// [`insert`](Self::insert) registers a quiesced runtime.
    ///
    /// Deliberately one-way. Nothing resumes a registry: the process is on its
    /// way out, and a company that misses this window is registered normally by
    /// the next boot.
    pub fn begin_shutdown(&self) {
        // Taken for the ordering, not for the map: holding the write lock here
        // means no `insert` can be midway through its own check.
        let _ordering = self.inner.write().expect("registry poisoned");
        self.shutting_down.store(true, Ordering::SeqCst);
    }

    /// Whether [`begin_shutdown`](Self::begin_shutdown) has been called.
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    /// Removes and returns the runtime registered under `id`, if any.
    ///
    /// Used by the archive lifecycle transition to park a company: once removed,
    /// the company is no longer addressable and chatting it returns a 404.
    pub fn remove(&self, id: &CompanyId) -> Option<Arc<CompanyRuntime>> {
        self.inner.write().expect("registry poisoned").remove(id)
    }

    /// Removes `id` only if the runtime currently registered under it is the
    /// same allocation as `expected` (`Arc::ptr_eq`, not equal contents).
    ///
    /// The conditional counterpart to [`remove`](Self::remove), for a caller
    /// that observed one specific runtime instance — e.g. read its `status()`
    /// as `"archived"` a moment ago — and must not evict whatever now sits at
    /// `id` if something has since replaced it there. [`insert`](Self::insert)
    /// is the only writer of an already-occupied slot (a rebuild swap is the
    /// production case), and it replaces unconditionally; this is what lets a
    /// caller downstream of an unconditional `insert` still remove safely
    /// (codex review on #1943, PR comment 3894439351 — maintenance eviction
    /// used to remove "whatever is at `id`" and could deregister a live
    /// replacement instead of the archived runtime it actually confirmed).
    ///
    /// Returns the removed runtime on a match. `None` both when `id` is
    /// unregistered and when it now points to a different runtime — a caller
    /// that needs to tell those apart checks [`get`](Self::get) first.
    pub fn remove_if(
        &self,
        id: &CompanyId,
        expected: &Arc<CompanyRuntime>,
    ) -> Option<Arc<CompanyRuntime>> {
        let mut map = self.inner.write().expect("registry poisoned");
        match map.get(id) {
            Some(current) if Arc::ptr_eq(current, expected) => map.remove(id),
            _ => None,
        }
    }

    /// The ids of every registered company, sorted.
    pub fn list(&self) -> Vec<CompanyId> {
        let mut ids: Vec<CompanyId> = self
            .inner
            .read()
            .expect("registry poisoned")
            .keys()
            .cloned()
            .collect();
        ids.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
        ids
    }

    /// The number of registered companies.
    pub fn len(&self) -> usize {
        self.inner.read().expect("registry poisoned").len()
    }

    /// Whether no companies are registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The sole registered runtime, when exactly one company is registered.
    ///
    /// Powers the single-company `/api/v1/company/...` aliases; returns `None`
    /// when zero or more than one company is registered.
    pub fn sole(&self) -> Option<Arc<CompanyRuntime>> {
        let map = self.inner.read().expect("registry poisoned");
        if map.len() == 1 {
            map.values().next().cloned()
        } else {
            None
        }
    }
}

impl std::fmt::Debug for CompanyRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompanyRegistry")
            .field("companies", &self.len())
            .finish()
    }
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
