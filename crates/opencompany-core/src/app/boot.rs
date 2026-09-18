//! Starting an instance from inside another program.
//!
//! `serve` in `src/bin/opencompany.rs` is the command-line entry point, and it
//! does several things before it ever builds a router: it resolves the data
//! root, locks it, migrates a legacy bundle nest, materializes the canonical
//! workspace layout, and prepares the agent journal. An embedder — the desktop
//! shell, which links this crate rather than spawning the binary — has to do
//! exactly the same things, and a second copy of that sequence would drift from
//! the first the moment either changed.
//!
//! So the sequence lives here, in [`prepare_instance`]. `serve` still runs its
//! own copy: it resolves `--home` and the data root separately (they can
//! diverge, which is what `store::home_divergence_warning` is for) and it takes
//! the environment-mutating half of the journal preparation, which an embedder
//! must not. Anything added to one belongs in the other until that split is
//! closed — the [`DataLayout`](crate::store::DataLayout) step below is here
//! because it was in `serve` only, so a desktop instance never created its
//! `memory/`, `store/`, `files/`, `logs/` or `tmp/` directories and never
//! cleared stale scratch on restart.
//!
//! ## The `set_var` hazard, which is why this is not just a function call
//!
//! [`journal::prepare`](crate::app::journal::prepare) exports
//! `OPENHUMAN_WORKSPACE` with `std::env::set_var`, and its `SAFETY` comment is
//! conditioned on being called "once during startup, before any company
//! runtime, scheduler, mailbox poller or HTTP listener exists, so no other
//! thread is reading the environment concurrently".
//!
//! **In a desktop process that condition is false.** Tauri has already started
//! its async runtime, its webview process and its plugin threads before any of
//! this runs, and `setenv` racing a concurrent `getenv` is undefined behaviour
//! on glibc rather than merely a stale read.
//!
//! [`prepare_journal`] therefore uses the *pure* half,
//! [`prepare_with`](crate::app::journal::prepare_with), which resolves and
//! probes the root and mutates nothing. An embedder that needs the variable
//! exported must do it in `main`, before it starts anything else —
//! [`EmbeddedInstance::journal_env`] returns exactly what to set.

use std::path::{Path, PathBuf};

use crate::Result;
use crate::app::config::{AuthMode, ConfigFile, WorkspaceConfig};
use crate::app::journal::JournalRoot;
use crate::store::DataLayout;
use crate::store::lock::HomeLock;

/// A prepared instance root: locked, migrated, and with its journal resolved.
///
/// Holds the [`HomeLock`] for as long as it lives, so the caller keeps this
/// alive for the lifetime of the instance. Dropping it hands the data root to
/// whatever asks next while this process is still writing.
#[derive(Debug)]
#[must_use = "dropping this releases the instance data root"]
pub struct EmbeddedInstance {
    home: PathBuf,
    journal: JournalRoot,
    workspace: WorkspaceConfig,
    auth_mode: Option<AuthMode>,
    _lock: HomeLock,
}

impl EmbeddedInstance {
    /// The resolved instance data root.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// The resolved agent journal root.
    pub fn journal(&self) -> &JournalRoot {
        &self.journal
    }

    /// The `[workspace]` section of the root's `config.toml`, resolved against
    /// its defaults.
    ///
    /// Returned rather than only applied, because two of its fields are not
    /// layout at all: `quota` and `git_enabled` belong on the embedder's
    /// [`AppConfig`](crate::AppConfig) exactly as `serve` puts them there. An
    /// embedder that ignores them runs with the compiled-in defaults and
    /// silently disregards an operator's `config.toml`.
    pub fn workspace(&self) -> &WorkspaceConfig {
        &self.workspace
    }

    /// The root's `config.toml` `auth_mode`, if it names one.
    ///
    /// Returned for the same reason as [`Self::workspace`] and with more at
    /// stake. `serve` gets this layer for free through `AppConfig::load`; an
    /// embedder that builds its `AppConfig` by hand — the desktop shell does —
    /// gets it only if something hands it over, and without it the host-wide
    /// sign-in mode is whatever the embedder compiled in. That matters because
    /// the [first-run setup flow](crate::server::setup) *writes* this key and
    /// tells the operator it takes effect: a shell that ignored the file would
    /// honour their choice until they quit and silently discard it after.
    ///
    /// `None` means the file named no mode, leaving the decision to the
    /// embedder — which is not the same as `Some(AuthMode::Email)`, the value
    /// each company's own `[users].mode` defaults to further down the stack.
    pub fn auth_mode(&self) -> Option<AuthMode> {
        self.auth_mode
    }

    /// The `(name, value)` an embedder must export **before** starting any
    /// other thread, in place of the `set_var` [`prepare`] would have done.
    ///
    /// See the module docs: doing this from `main` is safe, and doing it from
    /// here would not be.
    ///
    /// [`prepare`]: crate::app::journal::prepare
    pub fn journal_env(&self) -> (&'static str, &Path) {
        (
            crate::app::journal::OPENHUMAN_WORKSPACE_ENV,
            self.journal.root(),
        )
    }
}

/// Resolves, locks and prepares an instance data root.
///
/// The order is load-bearing:
///
/// 1. **Resolve** the root, so everything below agrees on where it is.
/// 2. **Lock** it, before reading or writing anything. Migration rewrites the
///    bundle layout, and two processes migrating one root concurrently is the
///    worst moment to discover they were sharing it.
/// 3. **Migrate** the legacy nest, exactly as `serve` does.
/// 4. **Materialize** the canonical workspace layout — `memory/`, `store/`,
///    `files/`, `logs/`, `tmp/` — clearing the ephemeral `tmp/` scratch first
///    when `[workspace].clear_tmp_on_startup` allows it (the default). Before
///    the journal, because both write under the root and a layout failure is
///    the cheaper of the two to hit.
/// 5. **Prepare** the journal, proving its root is writable — an unwritable one
///    aborts here rather than at the first agent turn.
///
/// One root, not two. An embedder passes the single directory it owns, so the
/// company bundles, the `config.toml` and the workspace layout all resolve
/// under it — the aligned shape
/// [`home_divergence_warning`](crate::store::home_divergence_warning) is silent
/// about. `serve` is the caller that can split them, with `--home` pointing the
/// bundles somewhere other than `OPENCOMPANY_DATA_DIR`, and it keeps its own
/// copy of this sequence for that reason.
pub async fn prepare_instance(home: Option<PathBuf>) -> Result<EmbeddedInstance> {
    let home = crate::store::resolve_home(home)?;
    let lock = crate::store::lock::acquire(&home)?;
    // The announced variant, same as `serve`: a migration that moved bundles is
    // something an operator should see in the log rather than infer later.
    crate::store::migrate::migrate_legacy_nest_announced(&home)?;
    // Read after the migration: an un-migrated install's `config.toml` may
    // still be one level down, and moving it up is what the step above does.
    let file = ConfigFile::load(&home)?;
    let workspace = file
        .as_ref()
        .map(|file| file.workspace.clone())
        .unwrap_or_default()
        .resolve();
    // Parsed here rather than by the embedder, so an unparseable value aborts
    // the boot it was configured for instead of being silently ignored by one
    // caller and honoured by another. The same refusal `serve` makes.
    let auth_mode = file
        .as_ref()
        .and_then(|file| file.auth_mode.as_deref())
        .map(str::parse::<AuthMode>)
        .transpose()?;
    DataLayout::new(&home)
        .ensure(workspace.clear_tmp_on_startup)
        .await?;
    let journal = prepare_journal(&home).await?;

    Ok(EmbeddedInstance {
        home,
        journal,
        workspace,
        auth_mode,
        _lock: lock,
    })
}

/// Resolves and validates the journal root **without touching the environment**.
///
/// The non-mutating half of `journal::prepare`. See the module docs for why an
/// embedder must not take the mutating one.
async fn prepare_journal(home: &Path) -> Result<JournalRoot> {
    let existing = std::env::var(crate::app::journal::OPENHUMAN_WORKSPACE_ENV).ok();
    crate::app::journal::prepare_with(existing.as_deref(), home).await
}

#[cfg(test)]
#[path = "boot_tests.rs"]
mod tests;
