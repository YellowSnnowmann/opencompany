//! The one process-wide OpenHuman [`Runtime`] every company agent lives on.
//!
//! `openhuman_embed` allows exactly one [`Runtime`] per process: the keyring
//! master key, the RPC bearer, the global event bus and the `Once`-guarded
//! domain subscribers are process-scoped, so a second runtime would silently
//! share them while believing it had its own workspace. `RuntimeBuilder::build`
//! therefore answers `AlreadyRunning` the second time, and **agents are the
//! unit of multiplicity** — one [`openhuman_embed::Agent`] per company agent
//! (plan hive-desks, Phase 2).
//!
//! This module owns that single runtime. [`global`] builds it on first use and
//! hands out the same `Arc` afterwards. A test binary — or the desktop parity
//! suite, which boots several pools in one process — reaches the same cell, so
//! `AlreadyRunning` is mapped onto the existing runtime rather than surfaced:
//! from a caller's point of view "the runtime already exists" *is* success.
//!
//! What the runtime is booted with comes from [`RuntimeBoot`], read once from
//! the environment the `serve` boot already prepared: `OPENHUMAN_WORKSPACE`
//! (exported to `<data-dir>/openhuman` by `app::journal`, issue #446) names the
//! durable root, and the TinyHumans key (`OPENCOMPANY_INFERENCE_KEY` outranks
//! `TINYHUMANS_API_KEY`, then the token file — the same ladder
//! `app::types` resolves) is installed as the runtime's managed-inference
//! credential. No workspace in the environment means an ephemeral one, which
//! is what every unit test wants: a temp root the runtime removes when the
//! process exits, never the operator's `~/.openhuman`.

use std::path::PathBuf;
use std::sync::Arc;

use openhuman_embed::{Runtime, RuntimeError, Workspace};
use tokio::sync::OnceCell;

use crate::error::OpenCompanyError;

/// What the process-wide runtime is booted with.
///
/// Built once, by whichever caller reaches [`global`] first; later callers'
/// boots are ignored because the runtime already exists. That is deliberate:
/// the workspace and the key are process facts, not per-company ones.
#[derive(Clone, Debug, Default)]
pub struct RuntimeBoot {
    /// The durable OpenHuman root (`<data-dir>/openhuman`). `None` boots an
    /// ephemeral workspace, which is the right answer for a test binary and
    /// the wrong one for a tenant — so `serve` always sets it.
    pub workspace_dir: Option<PathBuf>,
    /// The TinyHumans API key, when one is configured. Installed into the
    /// runtime's credential store so managed inference and backend calls can
    /// authenticate; an agent with its own BYOK route never touches it.
    pub api_key: Option<String>,
    /// The TinyHumans backend base URL (`TINYHUMANS_API_URL`), when set.
    pub backend_url: Option<String>,
}

impl RuntimeBoot {
    /// Reads the boot parameters from the process environment.
    ///
    /// `OPENHUMAN_WORKSPACE` is what `app::journal` exported at `serve` boot;
    /// reading it back here rather than threading the path through every
    /// `HarnessDeps` construction site keeps the ~40 test fixtures that build
    /// deps by hand on the ephemeral path they want.
    pub fn from_env() -> Self {
        let workspace_dir = std::env::var_os(crate::app::journal::OPENHUMAN_WORKSPACE_ENV)
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty());
        let api_key = std::env::var("OPENCOMPANY_INFERENCE_KEY")
            .ok()
            .or_else(|| std::env::var("TINYHUMANS_API_KEY").ok())
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty());
        let backend_url = std::env::var("TINYHUMANS_API_URL")
            .ok()
            .map(|url| url.trim().to_string())
            .filter(|url| !url.is_empty());
        Self {
            workspace_dir,
            api_key,
            backend_url,
        }
    }

    /// An ephemeral, credential-less boot — what a unit test wants.
    pub fn ephemeral() -> Self {
        Self::default()
    }

    fn workspace(&self) -> Workspace {
        match &self.workspace_dir {
            // `Workspace::Dir(d)` puts OpenHuman's `config.toml` beside `d`
            // and its session store inside it, which is exactly the layout
            // `app::journal` describes for `<root>/workspace`.
            Some(root) => Workspace::dir(root.join("workspace")),
            None => Workspace::Ephemeral,
        }
    }
}

static GLOBAL: OnceCell<Arc<Runtime>> = OnceCell::const_new();

static EXECUTOR: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

/// The tokio runtime the OpenHuman runtime — and the loopback model bridge —
/// live on: process-long, on its own threads, built with the stack and
/// blocking-thread constants OpenHuman documents for a turn.
///
/// Its own runtime rather than "whichever one called first" because the
/// OpenHuman core spawns background work (the harness-init service, bus
/// subscribers) onto the runtime it is built on, and that work has to
/// outlive the caller. In a test binary the caller's runtime is one
/// `#[tokio::test]`'s, torn down when that test returns — the process-wide
/// core would then be running on a dead executor for every later test. The
/// `serve` binary builds its own big-stack runtime too; a second one here
/// costs a few idle threads and keeps the core's lifetime the process's.
///
/// Turn futures are unaffected: `Agent::turn(..).send()` is an ordinary
/// future the caller awaits on its own runtime.
pub fn executor() -> &'static tokio::runtime::Handle {
    EXECUTOR
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .worker_threads(2)
                .thread_name("openhuman-core")
                .thread_stack_size(openhuman_core::core::runtime::AGENT_WORKER_STACK_BYTES)
                .max_blocking_threads(openhuman_core::core::runtime::MAX_BLOCKING_THREADS)
                .build()
                .expect("the OpenHuman executor builds")
        })
        .handle()
}

/// The process-wide runtime, built on first call with `boot`.
///
/// Every later call returns the same `Arc` and ignores its `boot`. A build
/// that fails with `AlreadyRunning` — some other code path in this process
/// built a runtime outside this cell, which `openhuman_embed` refuses to
/// duplicate — is reported as an error naming that cause, because there is no
/// runtime handle to hand back in that case.
pub async fn global(boot: RuntimeBoot) -> crate::Result<Arc<Runtime>> {
    GLOBAL
        .get_or_try_init(|| async move {
            executor()
                .spawn(build(boot))
                .await
                .map_err(|err| OpenCompanyError::Harness(format!("build the OpenHuman runtime: {err}")))?
        })
        .await
        .cloned()
}

/// [`global`] for a caller with no async context of its own (a synchronous
/// test that builds a roster).
///
/// Refuses to run inside a tokio runtime: blocking a worker on the executor
/// is how a test binary deadlocks itself.
pub fn global_blocking(boot: RuntimeBoot) -> crate::Result<Arc<Runtime>> {
    if tokio::runtime::Handle::try_current().is_ok() {
        return Err(OpenCompanyError::Harness(
            "global_blocking called inside a tokio runtime; await `global` instead".to_string(),
        ));
    }
    executor().block_on(global(boot))
}

/// The runtime if it has already been built, without building one.
pub fn existing() -> Option<Arc<Runtime>> {
    GLOBAL.get().cloned()
}

async fn build(boot: RuntimeBoot) -> crate::Result<Arc<Runtime>> {
    let mut builder = Runtime::builder().workspace(boot.workspace());
    if let Some(key) = boot.api_key.as_deref() {
        builder = builder.api_key(key);
    }
    if let Some(url) = boot.backend_url.as_deref() {
        builder = builder.backend_url(url);
    }
    match builder.build().await {
        Ok(runtime) => {
            tracing::info!(
                workspace = ?boot.workspace_dir,
                api_key = boot.api_key.is_some(),
                "[openhuman] process-wide runtime built"
            );
            Ok(Arc::new(runtime))
        }
        Err(RuntimeError::AlreadyRunning) => Err(OpenCompanyError::Harness(
            "an OpenHuman runtime is already running in this process outside the shared cell; \
             every embedded agent must be created on `harness::openhuman_runtime::global`"
                .to_string(),
        )),
        Err(err) => Err(OpenCompanyError::Harness(format!(
            "build the OpenHuman runtime: {err}"
        ))),
    }
}

#[cfg(test)]
#[path = "openhuman_runtime_tests.rs"]
mod tests;
