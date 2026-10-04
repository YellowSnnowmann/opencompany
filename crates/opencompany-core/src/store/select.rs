//! Config-driven storage backend selection.
//!
//! The five storage ports are the entire persistence contract; this module is
//! the one place that maps a backend *name* onto concrete port
//! implementations. `serve` (and platform provisioning) resolve a
//! [`StorageKind`] from `OPENCOMPANY_STORAGE`, open the backend once, and
//! inject the same [`StorageHandles`] into every company's `RuntimeBuilder` —
//! the kernel itself never names an engine.
//!
//! Backends behind disabled cargo features fail loudly at open time rather
//! than silently falling back to the filesystem.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use crate::Result;
use crate::app::config::{EnvSource, ProcessEnv, data_dir_from_source};
use crate::error::OpenCompanyError;
use crate::ports::artifacts::ArtifactStore;
use crate::ports::events::EventLog;
use crate::ports::inbox::InboxStore;
use crate::ports::journal::JournalStore;
use crate::ports::ledgers::LedgerStore;
use crate::ports::login_codes::LoginCodeStore;
use crate::ports::traces::TraceStore;
use crate::ports::notifications::NotificationStore;
use crate::ports::read_state::ReadStateStore;
use crate::ports::run_output::WorkflowRunOutputStore;
use crate::ports::runs::RunStore;
use crate::ports::schedule_fires::ScheduleFireStore;
use crate::ports::secrets::SecretStore;
use crate::ports::sessions::SessionStore;
use crate::ports::skills_state::SkillStateStore;
use crate::ports::store::CompanyStore;
use crate::ports::tasks::TaskStore;
use crate::ports::types::CompanyId;
use crate::ports::usage::UsageMeter;
use crate::ports::users::UserStore;
use crate::ports::workflow_revisions::WorkflowRevisionStore;
use crate::ports::workspace::WorkspaceStore;

/// Which storage backend hosts the durable ports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StorageKind {
    /// Per-company filesystem bundles (the default; no external service).
    #[default]
    Fs,
    /// One SQLite database file under the data dir (`sqlite` feature).
    Sqlite,
    /// A MongoDB database on a shared cluster (`mongodb` feature) — the
    /// multi-tenant platform backend.
    Mongodb,
}

impl StorageKind {
    /// The backend's name, for `/spec`. Stable wire strings — a client keys
    /// behaviour off these, so they are not `Debug` output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fs => "fs",
            Self::Sqlite => "sqlite",
            Self::Mongodb => "mongodb",
        }
    }

    /// Whether this backend keeps [`SecretStore`] material as **plaintext on
    /// the container's own filesystem** (issue #752).
    ///
    /// `fs` writes one plaintext file per secret under
    /// `<data-dir>/companies/<slug>/secrets/` — [`FsSecretStore`] says so in its
    /// own doc comment, and `sqlite` puts the same bytes in a database file on
    /// the same disk. `mongodb` is the only backend that keeps them out of the
    /// container, in the tenant database.
    ///
    /// This matters because of who else is on that filesystem. An agent holding
    /// `shell` runs as the same uid as the server process, in the same
    /// container, so "plaintext on disk" means "readable by a prompt-injected
    /// agent" — there is no boundary in between, and
    /// `docs/spec/security/agent-isolation.md` is explicit that none is planned
    /// inside a tenant. A repository credential parked there is a credential the
    /// agent can read and use directly, without going through any tool the host
    /// gates.
    ///
    /// New backends default to the safe answer by being added to the `true` arm
    /// unless they demonstrably keep secrets off the local disk.
    ///
    /// [`FsSecretStore`]: crate::store::FsSecretStore
    /// [`SecretStore`]: crate::ports::SecretStore
    pub fn secrets_are_plaintext_on_disk(self) -> bool {
        match self {
            Self::Fs | Self::Sqlite => true,
            Self::Mongodb => false,
        }
    }
}

/// The refusal every repository-credential gate raises on a backend that keeps
/// secrets as plaintext on the container's disk (issue #752).
///
/// One function rather than a message per call site: the bind route, the boot
/// check and the agent-build gate all refuse the *same* deployment condition,
/// and an operator who reads it in the console then reads it again in the boot
/// log should not have to work out whether they are two problems.
///
/// Written to be self-service — it names the condition, the risk in one clause,
/// and both ways out — because the operator hitting it is mid-task with a token
/// in their clipboard, and "storage backend not supported" would send them to
/// the issue tracker instead of to a fix.
pub fn plaintext_secret_refusal(kind: StorageKind) -> String {
    format!(
        "this host keeps secrets on its own filesystem (OPENCOMPANY_STORAGE={}), so a \
         repository credential would sit there in plaintext — readable by the same uid the \
         agent shell runs as, which is not a boundary this deployment has. Repository \
         credentials are refused here. Either point this host at MongoDB \
         (OPENCOMPANY_STORAGE=mongodb plus OPENCOMPANY_MONGODB_URI, which keeps secrets in \
         the tenant database), or drop the `repo` grant from the company's [tools] allow \
         list and from every agent that names it. See docs/spec/runtime/storage.md.",
        kind.as_str()
    )
}

impl std::str::FromStr for StorageKind {
    type Err = OpenCompanyError;
    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "fs" | "" => Ok(Self::Fs),
            "sqlite" => Ok(Self::Sqlite),
            "mongodb" | "mongo" => Ok(Self::Mongodb),
            other => Err(OpenCompanyError::Config(format!(
                "OPENCOMPANY_STORAGE must be 'fs', 'sqlite', or 'mongodb', got '{other}'"
            ))),
        }
    }
}

/// Durable company → tenant ownership, for shared-database platform mode.
/// Backends that can persist ownership (MongoDB today) expose it here so the
/// in-memory `AppState` map can be hydrated at boot and updated on provision.
#[async_trait]
pub trait OwnershipStore: Send + Sync {
    async fn set_owner(&self, id: &CompanyId, tenant: &str) -> Result<()>;
    async fn remove_owner(&self, id: &CompanyId) -> Result<()>;
    async fn owners(&self) -> Result<Vec<(CompanyId, String)>>;
}

/// One opened backend's implementations of every durable port, ready to be
/// injected into `RuntimeBuilder::with_stores`.
#[derive(Clone)]
pub struct StorageHandles {
    pub company: Arc<dyn CompanyStore>,
    pub events: Arc<dyn EventLog>,
    /// Compressed cycle traces and task results (not memory: `crate::memory`).
    pub traces: Arc<dyn TraceStore>,
    pub secrets: Arc<dyn SecretStore>,
    pub inbox: Arc<dyn InboxStore>,
    pub tasks: Arc<dyn TaskStore>,
    /// The company's declared ledgers and their append-only event logs.
    pub ledgers: Arc<dyn LedgerStore>,
    pub workspace: Arc<dyn WorkspaceStore>,
    pub artifacts: Arc<dyn ArtifactStore>,
    /// First-class task-run records and their step traces (#242).
    pub runs: Arc<dyn RunStore>,
    /// Per-workflow edit history for rollback (#274).
    pub workflow_revisions: Arc<dyn WorkflowRevisionStore>,
    /// Durable cross-replica scheduler fire claims (#241).
    pub schedule_fires: Arc<dyn ScheduleFireStore>,
    /// Durable, console-facing per-node run output snapshots (#596).
    pub run_outputs: Arc<dyn WorkflowRunOutputStore>,
    /// The unredacted companion of a turn's steps — reasoning text, raw tool
    /// arguments and raw tool output. Holds secrets by design; see
    /// [`crate::ports::deep_trace`].
    pub deep_trace: Arc<dyn crate::ports::deep_trace::DeepTraceStore>,
    pub usage: Arc<dyn UsageMeter>,
    pub skills: Arc<dyn SkillStateStore>,
    /// Per-person, per-channel read markers (#755).
    pub read_state: Arc<dyn ReadStateStore>,
    /// Durable notifications with per-person read state (#749).
    pub notifications: Arc<dyn NotificationStore>,
    pub users: Arc<dyn UserStore>,
    pub sessions: Arc<dyn SessionStore>,
    pub login_codes: Arc<dyn LoginCodeStore>,
    /// The runtime journal's durable sink (#726): at-most-once effect keys, the
    /// parked-approval queue, grants, and cycle brackets.
    ///
    /// Not `Option`, unlike [`ownership`](Self::ownership): a backend that
    /// cannot hold the journal cannot host a company at all, and a `None` here
    /// would be an invitation to fall back to the filesystem — which is exactly
    /// the bug (#726). On a mongodb tenant `/data` is ephemeral scratch, so a
    /// silent fs journal there loses every committed key and every parked
    /// approval the next time the container is replaced.
    pub journal: Arc<dyn JournalStore>,
    /// Present when the backend persists company → tenant ownership.
    pub ownership: Option<Arc<dyn OwnershipStore>>,
}

impl std::fmt::Debug for StorageHandles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageHandles")
            .field("ownership", &self.ownership.is_some())
            .finish_non_exhaustive()
    }
}

/// Connection settings for [`open_storage`]. `fs` needs nothing beyond the
/// runtime's home directory (handled by the builder's defaults), so it yields
/// `None` handles.
#[derive(Clone, Default)]
pub struct StorageSettings {
    pub kind: StorageKind,
    /// MongoDB connection string (`OPENCOMPANY_MONGODB_URI`).
    pub mongodb_uri: Option<String>,
    /// MongoDB database name (`OPENCOMPANY_MONGODB_DB`); the hosting layer
    /// sets a per-tenant name (e.g. `oc-<tenant>`) on a shared cluster.
    pub mongodb_db: Option<String>,
    /// Tenant identity for shared-single-DB deployments
    /// (`OPENCOMPANY_TENANT_ID`). When set, company ids are namespaced with
    /// this value so that many tenants sharing one logical database never
    /// collide on the `companies` unique index. Unset means the id-namespacing
    /// no-op: single-tenant / db-per-tenant behavior is unchanged.
    pub tenant_id: Option<String>,
}

impl std::fmt::Debug for StorageSettings {
    /// Renders everything except the MongoDB connection string.
    ///
    /// `StorageSettings` is printed at boot (`src/bin/opencompany.rs`), so a
    /// derived `Debug` would put a credential-bearing connection string into
    /// the startup log of every tenant container.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageSettings")
            .field("kind", &self.kind)
            .field("mongodb_uri", &self.mongodb_uri.as_ref().map(|_| "<set>"))
            .field("mongodb_db", &self.mongodb_db)
            .field("tenant_id", &self.tenant_id)
            .finish()
    }
}

/// Parses env var `key` into `T`. Absent → `Ok(None)` (the caller applies its
/// default); a set-but-non-UTF-8 value is a hard [`OpenCompanyError::Config`]
/// rather than a silent fallback to the default.
fn parse_env<T>(env: &dyn EnvSource, key: &str) -> Result<Option<T>>
where
    T: std::str::FromStr<Err = OpenCompanyError>,
{
    match env.get_os(key) {
        Some(raw) => match raw.into_string() {
            Ok(raw) => Ok(Some(raw.parse()?)),
            Err(_) => Err(OpenCompanyError::Config(format!(
                "{key} is set but is not valid UTF-8"
            ))),
        },
        None => Ok(None),
    }
}

/// Reads a boolean opt-in env flag. Truthy values (case-insensitive, trimmed):
/// `1`, `true`, `yes`, `on`. Anything else — including unset — is `false`.
fn env_flag(env: &dyn EnvSource, key: &str) -> bool {
    env.get(key)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

impl StorageSettings {
    /// Reads the CLI-surface storage env vars (`OPENCOMPANY_STORAGE`,
    /// `OPENCOMPANY_MONGODB_URI`, `OPENCOMPANY_MONGODB_DB`,
    /// `OPENCOMPANY_TENANT_ID`).
    pub fn from_env() -> Result<Self> {
        Self::from_env_source(&ProcessEnv)
    }

    /// Resolves storage settings from an injected environment source.
    pub fn from_env_source(env: &dyn EnvSource) -> Result<Self> {
        let kind: StorageKind = parse_env(env, "OPENCOMPANY_STORAGE")?.unwrap_or_default();
        let non_empty = |key: &str| env.get(key);
        Ok(Self {
            kind,
            mongodb_uri: non_empty("OPENCOMPANY_MONGODB_URI"),
            mongodb_db: non_empty("OPENCOMPANY_MONGODB_DB"),
            tenant_id: non_empty("OPENCOMPANY_TENANT_ID"),
        })
    }
}

/// Opens the selected backend once. `Ok(None)` means "use the builder's fs
/// defaults"; a selected-but-unavailable backend is an error, never a silent
/// fs fallback.
pub async fn open_storage(
    settings: &StorageSettings,
    data_dir: &Path,
) -> Result<Option<StorageHandles>> {
    match settings.kind {
        StorageKind::Fs => Ok(None),
        StorageKind::Sqlite => open_sqlite(data_dir),
        StorageKind::Mongodb => open_mongodb(settings).await,
    }
}

/// The bundle-command environment refusals (`export` / `import`), extracted
/// from the bin's `live_ports` so they execute under this module's tests —
/// the first cut left them in the binary, where no CI lane runs tests and a
/// mutation (`if false &&`) went green (the #1279 review's finding).
///
/// One deployment per bundle: with a non-default environment an explicit
/// `--home` is refused rather than mixed in; `null` is refused in both
/// directions; shared-single-DB tenant mode is refused (bundle ops write no
/// owner rows). Under the fs+store default every check passes and `--home`
/// means exactly what it always has.
pub fn refuse_bundle_env(settings: &StorageSettings, home_was_flagged: bool) -> crate::Result<()> {
    let live = settings.kind != StorageKind::Fs || settings.memory_backend != MemoryBackend::Store;
    if settings.memory_backend == MemoryBackend::Null {
        return Err(crate::error::OpenCompanyError::Config(
            "OPENCOMPANY_MEMORY=null retains nothing: an export would capture no memory and an \
             import would discard every record while reporting success. Unset OPENCOMPANY_MEMORY \
             for bundle operations."
                .into(),
        ));
    }
    if live && home_was_flagged {
        return Err(crate::error::OpenCompanyError::Config(format!(
            "--home names an fs data set, but this environment selects storage `{}` and memory \
             `{}` — the bundle would mix two deployments. Unset OPENCOMPANY_STORAGE and \
             OPENCOMPANY_MEMORY* to operate on the fs home, or drop --home to operate on the \
             live deployment.",
            settings.kind.as_str(),
            settings.memory_backend.as_str()
        )));
    }
    if live
        && settings
            .tenant_id
            .as_deref()
            .is_some_and(|t| !t.trim().is_empty())
    {
        return Err(crate::error::OpenCompanyError::Config(
            "shared-single-DB tenant mode (OPENCOMPANY_TENANT_ID) namespaces company ids and \
             owner rows at the app layer; bundle operations write neither. Run them without \
             tenant mode, from the manager path."
                .into(),
        ));
    }
    Ok(())
}

#[cfg(feature = "sqlite")]
fn open_sqlite(data_dir: &Path) -> Result<Option<StorageHandles>> {
    let store = Arc::new(crate::store::SqliteStore::open(
        data_dir.join("opencompany.db"),
    )?);
    Ok(Some(StorageHandles {
        company: store.clone(),
        events: store.clone(),
        traces: store.clone(),
        secrets: store.clone(),
        inbox: store.clone(),
        tasks: store.clone(),
        ledgers: store.clone(),
        workspace: store.clone(),
        artifacts: store.clone(),
        runs: store.clone(),
        workflow_revisions: store.clone(),
        schedule_fires: store.clone(),
        run_outputs: store.clone(),
        deep_trace: store.clone(),
        usage: store.clone(),
        skills: store.clone(),
        read_state: store.clone(),
        notifications: store.clone(),
        users: store.clone(),
        sessions: store.clone(),
        login_codes: store.clone(),
        journal: store,
        ownership: None,
    }))
}

#[cfg(not(feature = "sqlite"))]
fn open_sqlite(_data_dir: &Path) -> Result<Option<StorageHandles>> {
    Err(OpenCompanyError::Config(
        "OPENCOMPANY_STORAGE=sqlite requires a build with the `sqlite` feature".into(),
    ))
}

#[cfg(feature = "mongodb")]
async fn open_mongodb(settings: &StorageSettings) -> Result<Option<StorageHandles>> {
    let uri = settings.mongodb_uri.as_deref().ok_or_else(|| {
        OpenCompanyError::Config(
            "OPENCOMPANY_STORAGE=mongodb requires OPENCOMPANY_MONGODB_URI".into(),
        )
    })?;
    let db = settings.mongodb_db.as_deref().unwrap_or("opencompany");
    let store = Arc::new(crate::store::MongoStore::connect(uri, db).await?);
    Ok(Some(StorageHandles {
        company: store.clone(),
        events: store.clone(),
        traces: store.clone(),
        secrets: store.clone(),
        inbox: store.clone(),
        tasks: store.clone(),
        ledgers: store.clone(),
        workspace: store.clone(),
        artifacts: store.clone(),
        runs: store.clone(),
        workflow_revisions: store.clone(),
        schedule_fires: store.clone(),
        run_outputs: store.clone(),
        deep_trace: store.clone(),
        usage: store.clone(),
        skills: store.clone(),
        read_state: store.clone(),
        notifications: store.clone(),
        users: store.clone(),
        sessions: store.clone(),
        login_codes: store.clone(),
        journal: store.clone(),
        ownership: Some(store),
    }))
}

#[cfg(not(feature = "mongodb"))]
async fn open_mongodb(_settings: &StorageSettings) -> Result<Option<StorageHandles>> {
    Err(OpenCompanyError::Config(
        "OPENCOMPANY_STORAGE=mongodb requires a build with the `mongodb` feature".into(),
    ))
}

#[cfg(feature = "mongodb")]
#[async_trait]
impl OwnershipStore for crate::store::MongoStore {
    async fn set_owner(&self, id: &CompanyId, tenant: &str) -> Result<()> {
        crate::store::MongoStore::set_owner(self, id, tenant).await
    }
    async fn remove_owner(&self, id: &CompanyId) -> Result<()> {
        crate::store::MongoStore::remove_owner(self, id).await
    }
    async fn owners(&self) -> Result<Vec<(CompanyId, String)>> {
        crate::store::MongoStore::owners(self).await
    }
}
#[cfg(test)]
#[path = "select_config_tests.rs"]
mod tests_config;
