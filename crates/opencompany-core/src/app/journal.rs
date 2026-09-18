//! Where the embedded OpenHuman runtime keeps its durable agent journal.
//!
//! The vendored runtime writes every agent observation under
//! `{workspace}/tinyagents_store/`, and it resolves `{workspace}` from *its
//! own* config — which, absent an override, is a subdirectory of the user's
//! home directory. In a hosted tenant that home directory sits on the
//! **read-only root filesystem**: the store's first `create_dir_all` fails with
//! `EROFS`, and the vendored append worker reports the failure once per queued
//! event, forever, on stderr. Nothing else in the container's log survives that
//! (issue #446).
//!
//! OpenCompany's writable per-instance root is the data directory
//! ([`data_dir_from_env`](super::config::data_dir_from_env) — `/data` in a
//! tenant, where the manager mounts the persistent volume). This module points
//! the vendored resolver there through the one seam it honours — the
//! `OPENHUMAN_WORKSPACE` environment variable, which its config loader consults
//! *before* any home-directory default — and proves the resulting root is
//! writable while still in `serve`'s startup path.
//!
//! Two properties follow from doing the check at startup:
//!
//! * **It is loud once, not quiet forever.** A root that cannot be created is a
//!   misconfiguration the operator must see immediately, so [`prepare`] returns
//!   an error and `serve` aborts rather than running an agent whose work is
//!   never recorded.
//! * **The per-append flood cannot begin.** The vendored append worker has no
//!   dedup or backoff and reports straight to stderr, so no log filter on this
//!   side can bound it. Refusing to boot means the loop is never entered — the
//!   condition it would retry against cannot resolve itself while the process
//!   lives, because the mount is read-only for the lifetime of the pod.
//!
//! Only the resolved *path* is reported at startup. The only environment values
//! read here are `OPENHUMAN_WORKSPACE` and — for the temp-directory guard below
//! — the platform's temp-dir variable, and neither is echoed, so the startup
//! line cannot carry credential material.
//!
//! # The keyring rides the same root
//!
//! `pin_keyring` registers the resolved root as the vendored runtime's
//! *keyring* directory too, so secret material lands beside the journal rather
//! than in whatever its own fallback chain reaches — a chain that ends, if no
//! home directory resolves, at `/tmp` with no log line at all (issue #451). The
//! export above happens to steer it correctly today, but only because nothing
//! has touched the keyring yet; registering says it outright instead of relying
//! on startup order.

use std::path::{Path, PathBuf};

use crate::Result;
use crate::error::OpenCompanyError;

/// The vendored runtime's workspace override. Its config loader reads this
/// before consulting the home-directory default, which makes it the only seam
/// a host process has for redirecting the journal.
pub const OPENHUMAN_WORKSPACE_ENV: &str = "OPENHUMAN_WORKSPACE";

/// The data-dir subdirectory handed to the vendored runtime as its root.
const OPENHUMAN_SUBDIR: &str = "openhuman";

/// The workspace subdirectory the vendored resolver appends to the root.
const WORKSPACE_SUBDIR: &str = "workspace";

/// The store subtree the journal and kv stores live in, under the workspace.
const STORE_SUBDIR: &str = "tinyagents_store";

/// Payload for the startup write probe. Non-empty on purpose — see
/// [`ensure_writable`].
const PROBE_BYTES: &[u8] = b"opencompany journal write probe\n";

/// Where a resolved journal root came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RootSource {
    /// An operator set `OPENHUMAN_WORKSPACE`; its value is used verbatim.
    Env,
    /// Derived from the instance data directory — the normal hosted path.
    DataDir,
}

impl RootSource {
    /// The knob an operator would change to move the root, for the startup line.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Env => OPENHUMAN_WORKSPACE_ENV,
            Self::DataDir => "OPENCOMPANY_DATA_DIR",
        }
    }
}

/// A resolved (but not yet proven-writable) journal location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalRoot {
    root: PathBuf,
    source: RootSource,
}

impl JournalRoot {
    /// The directory handed to the vendored runtime as `OPENHUMAN_WORKSPACE`.
    /// Everything the journal writes lands beneath it, so this is the path
    /// whose writability decides whether observations persist.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Which knob put the root where it is.
    pub fn source(&self) -> RootSource {
        self.source
    }

    /// The store subtree the journal is expected to open,
    /// `<root>/workspace/tinyagents_store`.
    ///
    /// This mirrors the vendored resolver's ordinary result. That resolver has
    /// one legacy branch which, when a sibling `<root>/../.openhuman/config.toml`
    /// exists, treats `<root>` *itself* as the workspace and would place the
    /// store at `<root>/tinyagents_store` instead. It requires an
    /// operator-created directory that a provisioned tenant never has, and both
    /// outcomes lie inside [`root`](Self::root) — which is why the startup probe
    /// checks the root rather than this leaf.
    pub fn store_root(&self) -> PathBuf {
        self.root.join(WORKSPACE_SUBDIR).join(STORE_SUBDIR)
    }

    /// The one-line startup report: where observations are being written, and
    /// which knob decided that.
    pub fn summary(&self) -> String {
        format!(
            "agent journal: {} (root {}, from {})",
            self.store_root().display(),
            self.root.display(),
            self.source.as_str()
        )
    }
}

/// Resolves the journal root from an `OPENHUMAN_WORKSPACE` value and the
/// instance data directory, without touching the filesystem or the environment.
///
/// An operator-set root wins so a self-hoster keeps an existing OpenHuman
/// workspace. A missing — or blank — value falls back to the data directory,
/// the one location a tenant container is guaranteed to be able to write.
///
/// Trimming decides only whether a value is blank; a value that survives that
/// test is used **verbatim**. Leading and trailing spaces are legal in a POSIX
/// path, so silently stripping them would resolve somewhere the operator did
/// not configure — the same class of surprise this module exists to remove.
pub fn resolve(env_value: Option<&str>, data_dir: &Path) -> JournalRoot {
    match env_value.filter(|value| !value.trim().is_empty()) {
        Some(value) => JournalRoot {
            root: PathBuf::from(value),
            source: RootSource::Env,
        },
        None => JournalRoot {
            root: data_dir.join(OPENHUMAN_SUBDIR),
            source: RootSource::DataDir,
        },
    }
}

/// Resolves the journal root and proves it is writable, without reading or
/// writing the process environment.
///
/// The pure-input half of [`prepare`], so the resolution and the probe can be
/// exercised without the `set_var` races that make environment mutation
/// unsuitable for parallel tests.
pub async fn prepare_with(env_value: Option<&str>, data_dir: &Path) -> Result<JournalRoot> {
    let resolved = resolve(env_value, data_dir);
    ensure_writable(resolved.root(), resolved.source()).await?;
    Ok(resolved)
}

/// Resolves the journal root, proves it is writable, and — when the root was
/// derived rather than supplied — exports it as `OPENHUMAN_WORKSPACE` so the
/// vendored config loader finds it instead of a home-directory default.
///
/// Returns the resolved root for the caller to report. An unwritable root is an
/// error: see the module docs for why that aborts startup instead of degrading.
///
/// Call this once, early in `serve`, before any company runtime is built — the
/// vendored loader reads the variable when the first agent harness is
/// constructed, and the export must already have happened.
pub async fn prepare(data_dir: &Path) -> Result<JournalRoot> {
    let raw = std::env::var(OPENHUMAN_WORKSPACE_ENV).ok();
    let resolved = prepare_with(raw.as_deref(), data_dir).await?;

    if resolved.source() == RootSource::DataDir {
        // SAFETY: `serve` calls this once during startup, before any company
        // runtime, scheduler, mailbox poller or HTTP listener exists, so no
        // other thread is reading the environment concurrently. The tokio
        // worker and blocking threads alive at this point are idle and do not
        // call `getenv`. Setting this later — from a request handler, say —
        // would race those readers and must not be done.
        unsafe { std::env::set_var(OPENHUMAN_WORKSPACE_ENV, resolved.root()) };
    }

    Ok(resolved)
}

// ---------------------------------------------------------------------------
// Keyring pin (issue #451)
// ---------------------------------------------------------------------------

/// Where the vendored keyring writes its secret material, once [`pin_keyring`]
/// has registered it.
#[cfg(feature = "openhuman")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyringPin {
    dir: PathBuf,
    source: RootSource,
    temporary: bool,
}

#[cfg(feature = "openhuman")]
impl KeyringPin {
    /// The directory the vendored keyring will resolve to.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Whether the pinned directory sits under the system temp directory — see
    /// [`pin_keyring`] for why that is reported rather than refused.
    pub fn is_temporary(&self) -> bool {
        self.temporary
    }

    /// The one-line startup report: which directory holds the credentials, and
    /// the caveat when that directory is temporary.
    pub fn summary(&self) -> String {
        let base = format!(
            "keyring: {} (pinned, from {})",
            self.dir.display(),
            self.source.as_str()
        );
        if self.temporary {
            format!(
                "{base} — WARNING: this is under the system temp directory. \
                 Credentials stored here can be removed by the OS at any time, \
                 and connected accounts will silently need reconnecting. Point \
                 {} at a durable volume for anything but local development.",
                self.source.as_str()
            )
        } else {
            base
        }
    }
}

/// Whether `dir` lies inside `temp_dir`.
///
/// Component-wise by construction ([`Path::starts_with`] compares whole
/// components), so `/tmpfoo` is not treated as being under `/tmp`.
#[cfg(feature = "openhuman")]
fn under_temp_dir(dir: &Path, temp_dir: &Path) -> bool {
    dir.starts_with(temp_dir)
}

/// Registers `root` as the vendored keyring's workspace, so credential storage
/// lands beside the journal instead of wherever its own fallback chain ends up.
///
/// # Why register rather than rely on the export
///
/// The vendored resolver
/// (`keyring::store::workspace_dir_for_file_backend`) tries, in order: a
/// directory registered through its `init_workspace` seam, then
/// `OPENHUMAN_WORKSPACE`, then the user's home directory — and if it cannot
/// find a home directory at all, `/tmp`, at no log level whatsoever.
///
/// [`prepare`] already exports `OPENHUMAN_WORKSPACE`, so today a hosted tenant
/// happens to land in the data dir. That is safe **by accident**: the resolved
/// value is cached in a `OnceLock`, so whichever code touches the keyring first
/// fixes the answer for the life of the process. Nothing enforces that the
/// export happens before that first touch — it holds because of the order two
/// unrelated pieces of startup currently run in, which is not a property anyone
/// is checking when they move code around. If a touch ever lands first, secret
/// material silently goes to `$HOME` (the read-only root filesystem in a tenant)
/// or to `/tmp`, with no error and no log line to say so.
///
/// Registering explicitly removes the ordering dependency instead of restating
/// it: after this call the first branch of the resolver matches, so the env-var
/// branch, the home branch and the `/tmp` branch are all unreachable regardless
/// of what runs when. `init_workspace` is idempotent-by-first-writer (it logs at
/// debug and ignores a second call), and this is called once from `serve`.
///
/// # Loudness, not refusal
///
/// A pinned root under the system temp directory earns a `warn!` naming the path
/// and what it costs — not a refusal. Pointing `OPENCOMPANY_DATA_DIR` at a temp
/// path is a legitimate thing to do in local development and in tests; the
/// defect this addresses was never that credentials *could* be temporary, it was
/// that they could become temporary in **silence**.
///
/// # Owed upstream
///
/// The `/tmp` fallback itself still exists in `vendor/openhuman` and should be
/// removed there — a keyring that cannot resolve a durable directory should
/// fail loudly rather than quietly choosing world-readable scratch space. That
/// is a vendored-crate change and is out of scope here; this function makes the
/// fallback unreachable for `opencompany serve`, which is the half this crate
/// can own.
#[cfg(feature = "openhuman")]
pub fn pin_keyring(root: &JournalRoot) -> KeyringPin {
    let dir = root.root().to_path_buf();
    openhuman_core::security::keyring::init_workspace(&dir);

    let temporary = under_temp_dir(&dir, &std::env::temp_dir());
    if temporary {
        tracing::warn!(
            keyring = %dir.display(),
            knob = root.source().as_str(),
            "[keyring] secret material is being stored under the system temp directory; \
             the OS may remove it at any time and connected accounts will silently need \
             reconnecting — point the data directory at a durable volume outside local development",
        );
    }

    KeyringPin {
        dir,
        source: root.source(),
        temporary,
    }
}

/// Creates `root` and confirms a file can actually be written inside it.
///
/// `create_dir_all` alone is not proof: it succeeds silently when the directory
/// already exists but is not writable by this user, which is exactly the shape
/// a stale volume or a wrong `fsGroup` produces. The probe file is named per
/// process so two instances sharing a root cannot collide, and is removed
/// again — a failure to remove it is not fatal, since the write already proved
/// the point.
///
/// The probe carries real bytes rather than being empty. A zero-length write
/// allocates no blocks, so it succeeds on a volume that is full or over quota
/// while every subsequent journal append fails with `ENOSPC` — reintroducing
/// the per-append failure this module exists to make impossible.
async fn ensure_writable(root: &Path, source: RootSource) -> Result<()> {
    let fail = |stage: &str, error: std::io::Error| {
        OpenCompanyError::Config(format!(
            "agent journal root {} is not writable ({stage}: {error}); \
             refusing to start, because agent observations would be silently \
             discarded on every turn. Point {} at a writable volume — in a \
             tenant container that is the mounted data volume, not the \
             read-only root filesystem.",
            root.display(),
            source.as_str()
        ))
    };

    tokio::fs::create_dir_all(root)
        .await
        .map_err(|error| fail("create", error))?;

    let probe = root.join(format!(".opencompany-journal-probe-{}", std::process::id()));
    tokio::fs::write(&probe, PROBE_BYTES)
        .await
        .map_err(|error| fail("write", error))?;
    tokio::fs::remove_file(&probe).await.ok();

    Ok(())
}

#[cfg(test)]
#[path = "journal_tests.rs"]
mod tests;
