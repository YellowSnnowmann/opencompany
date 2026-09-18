//! The [`SessionStore`] port: durable, revocable proof that a user logged in.
//!
//! A session is minted when a user redeems a login code and is carried by the
//! console in an `HttpOnly` cookie. The plaintext token is handed to the browser
//! exactly once and is never written down: only its hash reaches this port (see
//! [`SessionRecord::token_hash`]). A dump of this store therefore cannot be
//! replayed as anyone.
//!
//! Sessions are per-company, like every other port. That is not merely tidiness:
//! in local development one process serves many companies from one origin, so a
//! session minted for company A must be unusable against company B. Keying every
//! method by [`CompanyId`] is what makes that structural rather than a check
//! someone might forget.
//!
//! Session records are credential material and must stay out of
//! `opencompany export` — the export path covers the company/event/memory/context
//! ports only, and this port must not be added to it.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::ports::types::CompanyId;

/// What kind of client holds a session.
///
/// A device is not a second credential *system* — it is the same session record
/// with a longer life and a name on it. That is a deliberate choice over a
/// separate `DeviceStore` port, and the reasons are worth stating because the
/// alternative looks tidier than it is:
///
/// - **Revocation stays one lever.** [`SessionStore::delete_for_user`] is what
///   suspension and admin password reset call. A separate device table would be
///   a second thing every one of those paths must remember to clear, and the
///   failure mode of forgetting is a suspended user whose desktop keeps working.
/// - **The security properties are already proven here.** Hash-only storage,
///   lookup *by* hash, per-company partitioning, liveness, the per-request user
///   re-read. A parallel port would have to re-establish every one of them.
/// - **It costs no storage change.** All three backends persist this record as
///   JSON (fs to an array file, sqlite to a `session_json` column, mongo via
///   serde), so an additive `#[serde(default)]` field needs no migration.
///
/// What a device genuinely needs that a browser session does not — a much
/// longer life, a human-chosen name, and the ability for a route to tell the
/// two apart — is exactly this enum plus [`SessionRecord::label`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionKind {
    /// A browser holding an `HttpOnly` cookie. The default, so every record
    /// written before this field existed reads back as what it was.
    #[default]
    Browser,
    /// A paired client presenting the session as a header, because a
    /// `SameSite=Lax` cookie is never sent cross-site.
    Device,
}

impl SessionKind {
    /// Whether this is a paired device rather than a browser.
    pub fn is_device(self) -> bool {
        matches!(self, SessionKind::Device)
    }
}

/// One logged-in session: a browser tab, or a paired device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    /// Stable id for the session within the company. Safe to show a user when
    /// listing their sessions, and to revoke by — unlike the token.
    pub id: String,
    /// Lowercase hex SHA-256 of the session token.
    ///
    /// The plaintext token exists only in the response that minted it and in the
    /// browser's cookie jar. Never store, log, or return it.
    pub token_hash: String,
    /// The [`UserRecord::id`](crate::ports::UserRecord) this session authenticates.
    pub user_id: String,
    /// Epoch-millis timestamp of when the session was minted.
    pub created_at_millis: u64,
    /// Epoch-millis timestamp after which the session is refused.
    pub expires_at_millis: u64,
    /// The `User-Agent` that minted the session, so a user can recognize a
    /// session when revoking it. Untrusted, display-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    /// Whether a browser or a paired device holds this session.
    ///
    /// `#[serde(default)]` so every record written before devices existed reads
    /// back as [`SessionKind::Browser`], which is what it is.
    #[serde(default)]
    pub kind: SessionKind,
    /// The human-chosen name for a paired device ("Ada's MacBook"), shown when
    /// listing and revoking. `None` for a browser session, which is identified
    /// by its [`user_agent`](Self::user_agent) instead.
    ///
    /// Untrusted and display-only: it is whatever the pairing client sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl SessionRecord {
    /// Whether the session is still valid at `now_millis`.
    pub fn is_live(&self, now_millis: u64) -> bool {
        now_millis < self.expires_at_millis
    }
}

/// The company's durable session table. Company A's sessions MUST be invisible
/// to company B.
///
/// Note there is no `touch`/activity tracking: recording a session's last use
/// would mean a store write on every authenticated request. `UserRecord`'s
/// `last_seen_at_millis` records sign-ins instead, which costs one write per
/// login and answers the question anyone actually asks.
///
/// Lookup by token hash is on every authenticated request's hot path and must be
/// indexed, not scanned.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Inserts a new session.
    async fn create(&self, company: &CompanyId, session: &SessionRecord) -> Result<()>;
    /// Fetches a session by its token hash.
    ///
    /// Returns the record even when expired; callers decide, so that an expired
    /// session is distinguishable from an unknown one for purging. Authentication
    /// paths must check [`SessionRecord::is_live`].
    async fn find_by_token_hash(
        &self,
        company: &CompanyId,
        token_hash: &str,
    ) -> Result<Option<SessionRecord>>;
    /// Lists a user's live and expired sessions, most-recently-created first.
    async fn list_for_user(&self, company: &CompanyId, user_id: &str)
    -> Result<Vec<SessionRecord>>;
    /// Revokes one session by id; returns whether one was removed.
    async fn delete(&self, company: &CompanyId, id: &str) -> Result<bool>;
    /// Revokes every session belonging to a user; returns how many were removed.
    ///
    /// This is the lever behind suspending or deleting a user: without it, a
    /// removed user keeps working until their cookie happens to expire.
    async fn delete_for_user(&self, company: &CompanyId, user_id: &str) -> Result<u64>;
    /// Drops sessions that expired at or before `now_millis`; returns how many.
    async fn purge_expired(&self, company: &CompanyId, now_millis: u64) -> Result<u64>;
}

#[cfg(test)]
#[path = "sessions_tests.rs"]
mod tests;
