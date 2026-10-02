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

use crate::error::OpenCompanyError;
use crate::ports::types::CompanyId;
use crate::server::users::token::sha256_hex;

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
