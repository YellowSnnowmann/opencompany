//! What an ACP session *is*, on this host.
//!
//! ACP gives a session an opaque id, a `cwd` and a set of MCP servers. None of
//! those mean here what they mean for a local harness, and the translation is
//! where the interesting decisions are.
//!
//! A session is the triple **(company, thread, optional agent)**. The thread
//! the turns land in is the desk the client asked for, or — when the client
//! pins the session to a roster member — that member's DM channel
//! (`dm:<member>`, the one chat key the cycle's routing resolves to that
//! member). Either way an ACP client and the web console looking at the same
//! thread see the same conversation, which is the whole reason to reuse the
//! thread rather than invent a parallel one.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::ports::types::CompanyId;
use crate::runtime::assignee::DM_PREFIX;

/// One live ACP session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcpSession {
    pub id: String,
    pub company: CompanyId,
    /// The thread these turns land in — the same key the console shows. A
    /// pinned session's is the pinned member's DM channel; an unpinned one's is
    /// the desk the client asked for (see [`Self::thread_key`]).
    pub chat: String,
    /// Pins the session to one roster member. `None` routes normally, through
    /// the orchestrator and the desk lead.
    pub agent_id: Option<String>,
}

impl AcpSession {
    /// The chat key a session's turns land in, given the desk the client asked
    /// for.
    ///
    /// A pinned session is answered by its member, and `responder_for` resolves
    /// a chat key to a member only through the console's own DM channel shape
    /// (`dm:<member>`, spelled by [`crate::runtime::assignee::dm_key`]). The
    /// requested desk is therefore superseded by the member's DM channel; an
    /// unpinned session keeps the desk.
    pub fn thread_key(requested_chat: &str, agent_id: Option<&str>) -> String {
        match agent_id {
            Some(id) => format!("{DM_PREFIX}{id}"),
            None => requested_chat.to_string(),
        }
    }
}

/// Why a `session/new` was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NewSessionRefusal {
    /// The client asked for MCP servers.
    McpServers,
    /// The client asked for extra directories.
    AdditionalDirectories,
}

impl NewSessionRefusal {
    /// What to tell the client. Specific, because a client that is told only
    /// "invalid params" will retry with the same request.
    pub fn message(&self) -> &'static str {
        match self {
            Self::McpServers => {
                "this host does not accept session-scoped MCP servers; configure them on the \
                 company (POST /api/v1/companies/{id}/mcp/servers) and they apply to every session"
            }
            Self::AdditionalDirectories => {
                "this host does not accept additional directories; an agent's workspace is \
                 server-side and fixed"
            }
        }
    }
}

/// Checks the parts of `session/new` this host cannot honour.
///
/// **Refused, not ignored.** Silently dropping `mcpServers` would leave a
/// client believing its tools were installed, and the model would then be asked
/// why it never called them. And they cannot be honoured: MCP servers here are
/// durable per-company configuration behind an admin gate, materialised into
/// the harness by a fingerprint rebuild. A session-scoped injection would
/// bypass the gate, force a pool rebuild per session, and leak across sessions
/// because the pool is per (company, agent).
pub fn refuse_unsupported(
    mcp_servers: &[serde_json::Value],
    additional_directories: &[serde_json::Value],
) -> Option<NewSessionRefusal> {
    if !mcp_servers.is_empty() {
        return Some(NewSessionRefusal::McpServers);
    }
    if !additional_directories.is_empty() {
        return Some(NewSessionRefusal::AdditionalDirectories);
    }
    None
}

/// What this host tells a client about the `cwd` it asked for.
///
/// ACP mandates an absolute path, and a client sends one that is meaningful on
/// **its** machine. On a remote host it names nothing. Rejecting would break
/// every stock ACP client, which always sends one; pretending to honour it
/// would break every file tool, which would resolve against a directory that
/// does not exist.
///
/// So it is accepted, ignored, and *reported* — the client is told the real
/// root in `_meta` and that its own was not used.
pub fn cwd_meta(server_workspace: &str) -> serde_json::Value {
    serde_json::json!({
        "opencompany/workspace": server_workspace,
        "opencompany/cwdIgnored": true,
    })
}

/// The most ACP sessions one connection id may hold open at once.
///
/// A `connectionId` is caller-supplied and otherwise unbounded, so a caller
/// minting one session after another on the same id would grow that
/// connection's entry forever.
pub const MAX_SESSIONS_PER_CONNECTION: usize = 32;

/// The most ACP sessions this host holds open at once, across every
/// connection. A circuit breaker on total memory, not a business limit.
pub const MAX_SESSIONS_TOTAL: usize = 8_192;

/// How long an ACP session may sit unused before [`SessionRegistry::sweep_expired`]
/// reclaims it. This surface has no heartbeat, so the bound has to be
/// generous enough to outlast a normal gap between prompts — one working day.
pub const SESSION_TTL_MILLIS: u64 = 24 * 60 * 60 * 1000;

/// How often [`SessionSweeper`] checks for expired sessions.
///
/// Deliberately much shorter than [`SESSION_TTL_MILLIS`]: a session that goes
/// idle right after one sweep tick is not stale enough for the *next* tick
/// (a full [`SESSION_TTL_MILLIS`] later) to reclaim either, so sweeping once
/// per TTL lets a session squat its cap slot for up to twice its own TTL. An
/// hourly cadence bounds that overshoot to about an hour instead.
pub const SESSION_SWEEP_INTERVAL_MILLIS: u64 = 60 * 60 * 1000;

const _: () = assert!(
    SESSION_SWEEP_INTERVAL_MILLIS < SESSION_TTL_MILLIS,
    "sweeping no more often than the TTL reintroduces the up-to-2x-TTL overshoot this constant exists to bound",
);

/// Why [`SessionRegistry::open`] refused a `session/new`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenSessionRefusal {
    /// The connection id is already bound to a different caller, or this
    /// caller never opened it.
    NotOwned,
    /// The connection already holds [`MAX_SESSIONS_PER_CONNECTION`] sessions.
    PerConnectionCap,
    /// The host already holds [`MAX_SESSIONS_TOTAL`] sessions.
    TotalCap,
}

impl OpenSessionRefusal {
    pub fn message(&self) -> &'static str {
        match self {
            // Deliberately the same wording as an unrecognized connection id:
            // telling the two apart would let a caller learn that a foreign
            // connection id exists by probing it.
            Self::NotOwned => "unknown ACP connection",
            Self::PerConnectionCap => "too many open ACP sessions on this connection",
            Self::TotalCap => "too many open ACP sessions on this host",
        }
    }
}

#[derive(Debug)]
struct SessionEntry {
    session: Arc<AcpSession>,
    last_used_millis: u64,
}

/// One connection's sessions, plus who is allowed to address it.
#[derive(Debug)]
struct Connection {
    owner: String,
    sessions: HashMap<String, SessionEntry>,
}

/// The live sessions on this host, keyed by connection so a disconnect can
/// sweep them.
///
/// A connection id is bound to whichever caller first opens a session on it
/// (see [`SessionRegistry::open`]); every other method refuses an id it does
/// not recognize as that same caller's, rather than acting on whatever the
/// request claims.
#[derive(Debug, Default)]
pub struct SessionRegistry {
    by_connection: Mutex<HashMap<String, Connection>>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens a session on `connection`, binding it to `owner` if this is the
    /// first session opened on that id. Refuses an id already bound to a
    /// different owner, and refuses once either cap is hit.
    pub fn open(
        &self,
        connection: &str,
        owner: &str,
        session: AcpSession,
        now_millis: u64,
    ) -> Result<Arc<AcpSession>, OpenSessionRefusal> {
        let mut by_connection = self
            .by_connection
            .lock()
            .expect("session registry poisoned");
        if let Some(existing) = by_connection.get(connection) {
            if existing.owner != owner {
                return Err(OpenSessionRefusal::NotOwned);
            }
            if existing.sessions.len() >= MAX_SESSIONS_PER_CONNECTION {
                return Err(OpenSessionRefusal::PerConnectionCap);
            }
        }
        let total: usize = by_connection.values().map(|c| c.sessions.len()).sum();
        if total >= MAX_SESSIONS_TOTAL {
            return Err(OpenSessionRefusal::TotalCap);
        }
        let session = Arc::new(session);
        let conn = by_connection
            .entry(connection.to_string())
            .or_insert_with(|| Connection {
                owner: owner.to_string(),
                sessions: HashMap::new(),
            });
        conn.sessions.insert(
            session.id.clone(),
            SessionEntry {
                session: Arc::clone(&session),
                last_used_millis: now_millis,
            },
        );
        Ok(session)
    }

    /// Looks up a session for `owner`, refusing a connection id it does not
    /// hold. Renews the session's idle TTL on a hit; evicts and refuses one
    /// already past [`SESSION_TTL_MILLIS`] instead — otherwise a lookup that
    /// lands in the gap between two [`SessionSweeper`] ticks would revive an
    /// already-expired session merely by touching it.
    pub fn get(
        &self,
        connection: &str,
        owner: &str,
        id: &str,
        now_millis: u64,
    ) -> Option<Arc<AcpSession>> {
        let mut by_connection = self
            .by_connection
            .lock()
            .expect("session registry poisoned");
        let conn = by_connection.get_mut(connection)?;
        if conn.owner != owner {
            return None;
        }
        if conn
            .sessions
            .get(id)
            .is_some_and(|e| Self::expired(e, now_millis))
        {
            conn.sessions.remove(id);
            if conn.sessions.is_empty() {
                by_connection.remove(connection);
            }
            return None;
        }
        let entry = conn.sessions.get_mut(id)?;
        entry.last_used_millis = now_millis;
        Some(Arc::clone(&entry.session))
    }

    /// Looks up a session for `owner` without renewing its idle TTL, refusing
    /// a connection id it does not hold. Evicts and refuses one already past
    /// [`SESSION_TTL_MILLIS`], for the same reason [`Self::get`] does — a
    /// caller cannot authorize itself to act on a session that has already
    /// aged out, no matter which lookup finds it.
    ///
    /// For a caller that still has to run `authorize_address` on the
    /// session's company before it may act on it — the ACP transport's
    /// `session/delete` and `session/prompt`. Renewing the TTL via [`Self::get`]
    /// before that check can still refuse the caller would let a tenant whose
    /// access to one company under it was revoked keep a session's cap slot
    /// alive indefinitely, by repeatedly presenting it and losing the
    /// authorization check every time.
    pub fn peek(
        &self,
        connection: &str,
        owner: &str,
        id: &str,
        now_millis: u64,
    ) -> Option<Arc<AcpSession>> {
        let mut by_connection = self
            .by_connection
            .lock()
            .expect("session registry poisoned");
        let conn = by_connection.get_mut(connection)?;
        if conn.owner != owner {
            return None;
        }
        if conn
            .sessions
            .get(id)
            .is_some_and(|e| Self::expired(e, now_millis))
        {
            conn.sessions.remove(id);
            if conn.sessions.is_empty() {
                by_connection.remove(connection);
            }
            return None;
        }
        Some(Arc::clone(&conn.sessions.get(id)?.session))
    }

    /// Whether `entry` has sat idle longer than [`SESSION_TTL_MILLIS`].
    fn expired(entry: &SessionEntry, now_millis: u64) -> bool {
        now_millis.saturating_sub(entry.last_used_millis) > SESSION_TTL_MILLIS
    }

    /// Renews a session's idle TTL, once the caller's authorization to act on
    /// it is already confirmed (typically via a prior [`Self::peek`]).
    ///
    /// A silent no-op for a connection or session `owner` does not hold, or
    /// that vanished between the authorization check and this call — nothing
    /// here needs a second refusal for a session that already will not run.
    pub fn touch(&self, connection: &str, owner: &str, id: &str, now_millis: u64) {
        let mut by_connection = self
            .by_connection
            .lock()
            .expect("session registry poisoned");
        if let Some(conn) = by_connection.get_mut(connection)
            && conn.owner == owner
            && let Some(entry) = conn.sessions.get_mut(id)
        {
            entry.last_used_millis = now_millis;
        }
    }

    /// Every session on a connection `owner` holds, for `session/list`.
    /// `None` when the connection is unknown or belongs to someone else.
    pub fn list(
        &self,
        connection: &str,
        owner: &str,
        now_millis: u64,
    ) -> Option<Vec<Arc<AcpSession>>> {
        let by_connection = self
            .by_connection
            .lock()
            .expect("session registry poisoned");
        let conn = by_connection.get(connection)?;
        if conn.owner != owner {
            return None;
        }
        Some(
            conn.sessions
                .values()
                .filter(|entry| !Self::expired(entry, now_millis))
                .map(|entry| Arc::clone(&entry.session))
                .collect(),
        )
    }

    /// Drops one session, for ACP's `session/delete`.
    ///
    /// Returns whether the session existed. Deleting a session that was never
    /// there — or addressing a connection `owner` does not hold — is a silent
    /// no-op, exactly as ACP specifies for an opaque id: absence says nothing
    /// useful.
    pub fn remove(&self, connection: &str, owner: &str, id: &str) -> bool {
        let mut by_connection = self
            .by_connection
            .lock()
            .expect("session registry poisoned");
        let Some(conn) = by_connection.get_mut(connection) else {
            return false;
        };
        if conn.owner != owner {
            return false;
        }
        let removed = conn.sessions.remove(id).is_some();
        // A connection's last session going also takes the connection key
        // (and its owner binding) with it — otherwise `session/new` +
        // `session/delete` over fresh caller-controlled connection ids grows
        // the host-wide map by one empty entry per connection, forever.
        if conn.sessions.is_empty() {
            by_connection.remove(connection);
        }
        removed
    }

    /// Drops every session a connection `owner` holds that `authorized`
    /// approves, under one lock, and reports the ids actually removed. A
    /// connection `owner` does not hold is left untouched — reports none.
    ///
    /// `authorized` is a per-session check, not a blanket one, because
    /// `owner` names a tenant rather than an authorization scope: two
    /// platform credentials for the same tenant can carry different company
    /// allow-lists (`PlatformClaims::companies`), and a connection can hold
    /// sessions opened under either. Removing every session on the
    /// connection unconditionally would let a narrowly-scoped credential
    /// close sessions for companies outside its own allow-list, just because
    /// it shares an owner string with whichever credential opened them.
    /// Checked under the same lock as the removal so a concurrent
    /// `session/new` cannot land in a gap between the check and the removal.
    pub fn close_connection(
        &self,
        connection: &str,
        owner: &str,
        mut authorized: impl FnMut(&CompanyId) -> bool,
    ) -> Vec<String> {
        let mut by_connection = self
            .by_connection
            .lock()
            .expect("session registry poisoned");
        let Some(conn) = by_connection.get_mut(connection) else {
            return Vec::new();
        };
        if conn.owner != owner {
            return Vec::new();
        }
        let mut removed = Vec::new();
        conn.sessions.retain(|id, entry| {
            if authorized(&entry.session.company) {
                removed.push(id.clone());
                false
            } else {
                true
            }
        });
        if conn.sessions.is_empty() {
            by_connection.remove(connection);
        }
        removed
    }

    /// Forgets every session idle past [`SESSION_TTL_MILLIS`], and every
    /// connection that leaves empty. Returns how many sessions were reclaimed.
    pub fn sweep_expired(&self, now_millis: u64) -> usize {
        let mut by_connection = self
            .by_connection
            .lock()
            .expect("session registry poisoned");
        let mut removed = 0;
        by_connection.retain(|_, conn| {
            let before = conn.sessions.len();
            conn.sessions.retain(|_, entry| {
                now_millis.saturating_sub(entry.last_used_millis) <= SESSION_TTL_MILLIS
            });
            removed += before - conn.sessions.len();
            !conn.sessions.is_empty()
        });
        removed
    }
}

/// Periodically reclaims ACP sessions idle past [`SESSION_TTL_MILLIS`].
///
/// Mirrors [`crate::server::presence::PresenceSweeper`]: this registry is
/// host-global, not scoped to a registered company, so it gets its own
/// always-on task rather than riding the per-company maintenance ticker.
pub struct SessionSweeper {
    registry: Arc<SessionRegistry>,
}

impl SessionSweeper {
    pub fn new(registry: Arc<SessionRegistry>) -> Self {
        Self { registry }
    }

    /// Runs until `shutdown` is notified, sweeping every
    /// [`SESSION_SWEEP_INTERVAL_MILLIS`].
    pub fn spawn(self, shutdown: Arc<Notify>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let notified = shutdown.notified();
            tokio::pin!(notified);
            loop {
                tokio::select! {
                    _ = &mut notified => break,
                    _ = tokio::time::sleep(Duration::from_millis(SESSION_SWEEP_INTERVAL_MILLIS)) => {
                        self.registry.sweep_expired(crate::ports::now_millis());
                    }
                }
            }
        })
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
