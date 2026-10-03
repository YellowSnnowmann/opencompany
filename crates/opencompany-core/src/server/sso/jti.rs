//! Single-use enforcement for SSO tokens: a consumed-`jti` marker under `/data`.
//!
//! # Why a filesystem marker, and not a store port
//!
//! Single use has to be *atomic* — two requests racing on one token must not
//! both mint a session — and it has to hold across every storage backend a
//! deployment might pick. A new [`Store`](crate::ports::store) port would be the
//! heavyweight answer: three backend implementations (fs, sqlite, mongodb) for a
//! record that is a short-lived boolean. `create_new` gives the same guarantee
//! with none of that. It fails with [`AlreadyExists`](std::io::ErrorKind::AlreadyExists)
//! when the file is already there, and the OS makes the check-and-create one
//! step — the same property [`LoginCodeStore::consume`](crate::ports::login_codes::LoginCodeStore::consume)
//! promises, without a port to reimplement per backend. The token's own `exp` is
//! the real credential lifetime; the marker only has to outlast the window a
//! valid token could be replayed in.
//!
//! # Why the jti is hashed into the filename
//!
//! A `jti` is attacker-influenced (it arrives inside a token the caller holds),
//! so it must never reach a path verbatim: a `jti` of `../../etc/x` would write
//! outside the marker directory. Every jti is SHA-256'd to a fixed hex string
//! before it becomes a filename, so the path is `[0-9a-f]{64}` by construction —
//! no separators, no traversal, and one file per distinct token.

use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::OpenCompanyError;
use crate::ports::types::CompanyId;
use crate::server::users::token::sha256_hex;

/// Retain replay markers for one minute after token expiry to cover verifier
/// clock leeway.
const MARKER_EXPIRY_GRACE_SECS: u64 = 60;
/// Avoid walking the marker directory on every redemption.
const PRUNE_INTERVAL_SECS: u64 = 60;
static LAST_PRUNE_AT: AtomicU64 = AtomicU64::new(0);

/// The consumed-`jti` marker directory for one company, under the data root.
///
/// Constructed per redeem rather than held on state: it owns only a path, the
/// work is a couple of filesystem calls on a cold path, and keeping it local
/// means nothing has to thread a handle through the router.
pub struct ConsumedJtis {
    dir: PathBuf,
}

impl ConsumedJtis {
    /// The marker directory for `company` beneath `home` (the `--home`/`/data`
    /// data root).
    ///
    /// Namespaced by company id so two companies served from one process (local
    /// development) cannot see each other's consumed tokens. The id is used as a
    /// path component; it is the runtime's own id, minted by the process, not
    /// caller input — so unlike the jti it needs no sanitizing.
    pub fn new(home: &std::path::Path, company: &CompanyId) -> Self {
        Self {
            dir: home
                .join("sso")
                .join(company.as_ref())
                .join("consumed_jtis"),
        }
    }

    /// Whether a successful redemption has already recorded `jti`.
    ///
    /// Used by the empty-host setup session: the signed token is the temporary
    /// credential, but it becomes usable there only after the redemption route
    /// has consumed it and returned that session to the caller.
    pub async fn is_consumed(&self, jti: &str) -> Result<bool, OpenCompanyError> {
        let path = self.dir.join(sha256_hex(jti));
        match tokio::fs::metadata(&path).await {
            Ok(_) => Ok(true),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
            Err(source) => Err(io_err(&path, source)),
        }
    }

    /// Atomically records `jti` as consumed, returning whether this call is the
    /// one that consumed it.
    ///
    /// `true` exactly once per distinct jti: the first call creates the marker
    /// and returns `true`; every later call — a replay, or a lost race — finds it
    /// present and returns `false`. `exp` is written into the marker so a later
    /// prune can tell a dead marker from a live one without decoding anything.
    ///
    /// A missing parent directory is created first; any *other* I/O error
    /// propagates rather than being read as "already consumed", so a full or
    /// read-only disk fails the redemption loudly instead of silently refusing a
    /// valid token.
    pub async fn consume(&self, jti: &str, exp: u64) -> Result<bool, OpenCompanyError> {
        let path = self.dir.join(sha256_hex(jti));
        tokio::fs::create_dir_all(&self.dir)
            .await
            .map_err(|source| io_err(&self.dir, source))?;

        let now = unix_time_secs();
        if should_prune(now)
            && let Err(err) = self.prune_expired_at(now).await
        {
            tracing::warn!("sso: could not prune expired consumed jti markers: {err}");
        }

        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(_) => {
                // Best-effort record of the expiry for pruning. A write failure
                // here does not un-consume the token — the marker's existence is
                // the guarantee — so it is logged, not surfaced.
                if let Err(err) = tokio::fs::write(&path, exp.to_string()).await {
                    tracing::warn!("sso: could not stamp consumed jti marker: {err}");
                }
                Ok(true)
            }
            Err(err) if err.kind() == ErrorKind::AlreadyExists => Ok(false),
            Err(source) => Err(io_err(&path, source)),
        }
    }

    /// Removes markers whose token expiry and verifier leeway have passed.
    /// Markers with missing or unreadable expiry stamps are kept: pruning must
    /// not turn an uncertain marker into a replayable token.
    async fn prune_expired_at(&self, now: u64) -> Result<(), OpenCompanyError> {
        let mut entries = tokio::fs::read_dir(&self.dir)
            .await
            .map_err(|source| io_err(&self.dir, source))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|source| io_err(&self.dir, source))?
        {
            let path = entry.path();
            let Ok(stamp) = tokio::fs::read_to_string(&path).await else {
                continue;
            };
            let Ok(exp) = stamp.parse::<u64>() else {
                continue;
            };
            if exp.saturating_add(MARKER_EXPIRY_GRACE_SECS) < now
                && let Err(source) = tokio::fs::remove_file(&path).await
                && source.kind() != ErrorKind::NotFound
            {
                tracing::warn!("sso: could not remove expired consumed jti marker: {source}");
            }
        }
        Ok(())
    }
}

fn unix_time_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn should_prune(now: u64) -> bool {
    let mut previous = LAST_PRUNE_AT.load(Ordering::Relaxed);
    loop {
        if now.saturating_sub(previous) < PRUNE_INTERVAL_SECS {
            return false;
        }
        // This atomic only suppresses redundant directory scans; it publishes
        // no associated data, so relaxed ordering is sufficient. The strong
        // form avoids spurious retries while contending callers refresh the
        // observed timestamp below.
        match LAST_PRUNE_AT.compare_exchange(previous, now, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return true,
            Err(actual) => previous = actual,
        }
    }
}

/// Maps a filesystem failure on a marker path to the crate's store-I/O error,
/// which renders as a `500`. A disk that cannot record single-use is an
/// unexpected server fault, not a bad request — a valid token is refused loudly
/// rather than silently.
fn io_err(path: &std::path::Path, source: std::io::Error) -> OpenCompanyError {
    OpenCompanyError::StoreIo {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
#[path = "jti_tests.rs"]
mod tests;
