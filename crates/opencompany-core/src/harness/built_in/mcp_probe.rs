//! MCP probing and error classification (the error-hardening cell).
//!
//! This is the one place that turns a raw MCP transport failure into an
//! operator-facing [`McpHealth`] the console can render. Every message it
//! surfaces goes through [`scrub`], the security choke point for the "a
//! credential must NEVER appear in any error/health/response/agent-output"
//! invariant.
//!
//! Compiled only under `feature = "openhuman"` (the whole `harness` module is).

use std::sync::{Arc, Mutex};

use crate::company::mcp::{McpHealth, McpServerDecl, McpStatus};
use crate::harness::mcp::registry_from_decls;
use crate::mcp::decl::server_info as mcp_server_info;
use crate::mcp::policy as mcp_policy;
use crate::ports::SecretStore;
use crate::ports::now_millis;
use crate::ports::types::CompanyId;
use crate::redact::scrub;

/// The shape of an MCP failure, driving both the status tier and the operator
/// message. Derived by [`classify_mcp_error`] from the typed error (downcast)
/// and the upstream bail-string arms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailureKind {
    /// 401 with no/unknown credential — the operator hasn't supplied auth yet.
    CredentialRequired,
    /// The server advertises an OAuth challenge (browser sign-in), unsupported.
    OauthRequired,
    /// A credential WAS sent but the server refused it (wrong/expired, or 403).
    TokenRejected,
    /// The request did not complete within the timeout.
    Timeout,
    /// The host was unreachable (DNS failure, connection refused, no route).
    Unreachable,
    /// The TLS handshake failed (bad/self-signed cert, protocol mismatch).
    Tls,
    /// Reachable, but not an MCP endpoint (404 wrong path, HTML/JSON that
    /// doesn't parse as an MCP reply).
    NotMcp,
    /// The server returned a 5xx.
    ServerError,
    /// A tool call was rejected by the server (JSON-RPC error in call context).
    ToolCallRejected,
    /// The server requires OAuth, but console OAuth cannot drive it: it
    /// advertises no RFC 7591 dynamic client registration, so there is no
    /// client to mint and no sign-in to complete (issue #1260).
    ///
    /// Distinct from [`FailureKind::OauthRequired`] because the operator's next
    /// action is the opposite one — paste a static token, rather than press a
    /// Sign in button that cannot succeed.
    ///
    /// Only ever constructed by `refine_oauth_capability`, which is itself
    /// gated on `feature = "mcp"` (no `mcp` feature means no `oauth/start`
    /// route, so there is nothing to downgrade) — so this variant is
    /// legitimately unconstructed in builds without that feature.
    #[cfg_attr(not(feature = "mcp"), allow(dead_code))]
    StaticTokenRequired,
    /// Anything not otherwise recognised.
    Unknown,
}

/// The classification of one MCP failure: the coarse [`McpStatus`] tier, the
/// stable auth-hint code (when it's a credential problem), and the internal
/// [`FailureKind`] that selects the operator message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeClass {
    /// The coarse badge tier.
    pub status: McpStatus,
    /// A stable auth-failure reason code, when applicable.
    pub auth_hint: Option<String>,
    kind: FailureKind,
}

impl FailureKind {
    /// A stable machine code for this failure, used as the [`McpFailure`] and
    /// [`CompanyEvent::McpCallFailed`](crate::ports::types::CompanyEvent) status
    /// string.
    fn code(self) -> &'static str {
        match self {
            FailureKind::CredentialRequired => "credential_required",
            FailureKind::OauthRequired => "oauth_required",
            FailureKind::StaticTokenRequired => "static_token_required",
            FailureKind::TokenRejected => "token_rejected",
            FailureKind::Timeout => "timeout",
            FailureKind::Unreachable => "unreachable",
            FailureKind::Tls => "tls",
            FailureKind::NotMcp => "not_mcp",
            FailureKind::ServerError => "server_error",
            FailureKind::ToolCallRejected => "tool_call_rejected",
            FailureKind::Unknown => "error",
        }
    }

    /// The badge tier this failure maps to. Auth problems are `NeedsConfig` (a
    /// valid resting state the operator can fix), everything else is `Error`.
    fn status(self) -> McpStatus {
        match self {
            FailureKind::CredentialRequired
            | FailureKind::OauthRequired
            | FailureKind::StaticTokenRequired
            | FailureKind::TokenRejected => McpStatus::NeedsConfig,
            _ => McpStatus::Error,
        }
    }

    /// The stable auth-hint wire code, when this is a credential problem.
    fn auth_hint(self) -> Option<String> {
        match self {
            FailureKind::CredentialRequired => Some("credential_required".to_string()),
            FailureKind::OauthRequired => Some("oauth_required".to_string()),
            FailureKind::StaticTokenRequired => Some("static_token_required".to_string()),
            FailureKind::TokenRejected => Some("token_rejected".to_string()),
            _ => None,
        }
    }
}

impl ProbeClass {
    /// The stable machine code for this classification (mirrors the internal
    /// [`FailureKind`]) — the status string carried on an [`McpFailure`] and the
    /// `McpCallFailed` event.
    pub fn code(&self) -> String {
        self.kind.code().to_string()
    }
}

/// One MCP tool-call failure observed during an agent turn, pushed onto the
/// [`McpFailureQueue`] by [`crate::harness::mcp::OcMcpCallTool`] and drained by
/// the [`HarnessBrain`](crate::harness::HarnessBrain) after the turn. Every
/// string field is already scrubbed at construction — this is safe to persist,
/// return, or show an operator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpFailure {
    /// The MCP server the failing call targeted.
    pub server: String,
    /// The remote tool the agent tried to call.
    pub tool: String,
    /// A stable status code (from [`ProbeClass::code`]).
    pub status: String,
    /// The auth-failure reason code, when the failure was a credential problem.
    pub hint: Option<String>,
    /// A short, scrubbed, operator-facing message.
    pub scrubbed_message: String,
}

/// A shared, in-memory queue of MCP tool-call failures — the exact
/// [`DelegationQueue`](crate::harness::orchestrator::DelegationQueue) pattern.
/// Cheap to [`Clone`] (a shared handle); the tool built into the agent and the
/// brain that drains it see the same queue because
/// [`HarnessDeps`](crate::harness::HarnessDeps) clones share this handle.
#[derive(Clone, Default)]
pub struct McpFailureQueue {
    inner: Arc<Mutex<Vec<McpFailure>>>,
}

impl McpFailureQueue {
    /// Records a failure.
    pub fn push(&self, failure: McpFailure) {
        self.inner.lock().expect("mcp failure queue").push(failure);
    }

    /// Empties the queue (called before an orchestrator turn so a prior turn's
    /// failures never leak into this one — mirrors `DelegationQueue::clear`).
    pub fn clear(&self) {
        self.inner.lock().expect("mcp failure queue").clear();
    }

    /// Drains every queued failure (FIFO), emptying the queue.
    pub fn drain(&self) -> Vec<McpFailure> {
        let mut guard = self.inner.lock().expect("mcp failure queue");
        std::mem::take(&mut *guard)
    }

    /// The number of queued failures (test/observability).
    #[cfg(test)]
    pub fn queued(&self) -> usize {
        self.inner.lock().expect("mcp failure queue").len()
    }
}

/// Classify a raw MCP transport error into a [`ProbeClass`].
///
/// `auth_configured` is whether the server has a stored credential — it
/// disambiguates a bare 401 (credential required) from a rejected one (token
/// rejected). `in_call_context` is true when the error came from a *tool call*
/// (via [`crate::harness::mcp`]'s `OcMcpCallTool`) rather than a connect/probe;
/// only then is a JSON-RPC `MCP error:` treated as a tool-call rejection.
///
/// Order matters: the **typed** downcasts (401, reqwest) win over string arms so
/// classification survives `?`/`.context()` wrapping and never depends on
/// fragile prose for the security-relevant cases.
pub fn classify_mcp_error(
    err: &anyhow::Error,
    auth_configured: bool,
    in_call_context: bool,
) -> ProbeClass {
    let kind = classify_kind(err, auth_configured, in_call_context);
    ProbeClass {
        status: kind.status(),
        auth_hint: kind.auth_hint(),
        kind,
    }
}

fn classify_kind(err: &anyhow::Error, auth_configured: bool, in_call_context: bool) -> FailureKind {
    // 1. Typed 401 — the security-critical case. Read the typed
    //    `resource_metadata` (not the message) to decide OAuth vs static auth.
    //
    //    The standalone `McpUnauthorizedError` became `tinymcp::Error::Unauthorized`
    //    when the client was extracted. `Error` is `#[non_exhaustive]`, so match
    //    the one variant and fall through to the string rules for the rest —
    //    which is what a new variant should do here anyway.
    if let Some(resource_metadata) =
        err.chain()
            .find_map(|cause| match cause.downcast_ref::<tinymcp::Error>() {
                Some(tinymcp::Error::Unauthorized {
                    resource_metadata, ..
                }) => Some(resource_metadata),
                _ => None,
            })
    {
        return if resource_metadata.is_some() {
            FailureKind::OauthRequired
        } else if auth_configured {
            FailureKind::TokenRejected
        } else {
            FailureKind::CredentialRequired
        };
    }

    // 2. Typed transport error — timeout / connect / TLS. reqwest's `Display`
    //    leaks the full URL, but we only read its typed predicates here; the
    //    message is scrubbed before it ever surfaces.
    if let Some(req) = err
        .chain()
        .find_map(|cause| cause.downcast_ref::<reqwest::Error>())
    {
        if req.is_timeout() {
            return FailureKind::Timeout;
        }
        if looks_like_tls(err) {
            return FailureKind::Tls;
        }
        if req.is_connect() {
            return FailureKind::Unreachable;
        }
        return FailureKind::Unreachable;
    }

    // 3. String arms over the upstream bail! messages (whole chain).
    let full = format!("{err:#}");
    // 3. Typed non-success status. Read the variant rather than the rendered
    //    text: the transport's own `Display` is `mcp http {status} from …`,
    //    which the string rule below only catches because it is spelled
    //    case-insensitively. A caller holding the error itself should not
    //    depend on that.
    if let Some(status) =
        err.chain()
            .find_map(|cause| match cause.downcast_ref::<tinymcp::Error>() {
                Some(tinymcp::Error::Http { status, .. }) => Some(*status),
                _ => None,
            })
    {
        return status_kind(status, auth_configured);
    }

    if let Some(code) = http_status_in(&full) {
        return status_kind(code, auth_configured);
    }
    if full.contains("Failed to parse MCP JSON response") {
        return FailureKind::NotMcp;
    }
    if in_call_context && full.contains("MCP error:") {
        return FailureKind::ToolCallRejected;
    }
    if looks_like_tls(err) {
        return FailureKind::Tls;
    }
    FailureKind::Unknown
}

/// What an HTTP status code means for a server we were trying to reach.
fn status_kind(code: u16, auth_configured: bool) -> FailureKind {
    match code {
        401 => {
            if auth_configured {
                FailureKind::TokenRejected
            } else {
                FailureKind::CredentialRequired
            }
        }
        403 => FailureKind::TokenRejected,
        404 => FailureKind::NotMcp,
        500..=599 => FailureKind::ServerError,
        _ => FailureKind::Unknown,
    }
}

/// The HTTP status code embedded in an upstream `MCP HTTP {status} — …` message,
/// if present.
///
/// Matched case-insensitively: the extracted client renders the same fact as
/// `mcp http {status} from …`, and this is the path that classifies an error
/// which has crossed an RPC boundary and arrived as text with no variant left
/// to read.
fn http_status_in(text: &str) -> Option<u16> {
    let lowered = text.to_ascii_lowercase();
    let offset = lowered.find("mcp http ")? + "mcp http ".len();
    let digits: String = text[offset..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// Whether the error chain smells like a TLS/certificate failure.
fn looks_like_tls(err: &anyhow::Error) -> bool {
    let full = format!("{err:#}").to_ascii_lowercase();
    [
        "tls",
        "certificate",
        "handshake",
        "ssl",
        "self-signed",
        "self signed",
    ]
    .iter()
    .any(|needle| full.contains(needle))
}

/// A short, actionable, operator-facing message for a classified failure. The
/// caller MUST pass the result through [`scrub`] before persisting or surfacing
/// it — `err` may embed a body or URL.
pub fn operator_message(server: &str, class: &ProbeClass, err: &anyhow::Error) -> String {
    match class.kind {
        FailureKind::CredentialRequired => format!(
            "MCP server '{server}' needs a credential. Add its API token (or query-parameter key) in its Token field, then Test again."
        ),
        FailureKind::OauthRequired => format!(
            "MCP server '{server}' needs OAuth sign-in — click Sign in on this server to authorize it."
        ),
        FailureKind::StaticTokenRequired => format!(
            "MCP server '{server}' requires OAuth, but it doesn't offer the automatic client registration this console signs in with. Paste a static API token in its Token field, then Test again."
        ),
        FailureKind::TokenRejected => format!(
            "MCP server '{server}' rejected the credential — it's wrong or expired. Update it and Test again."
        ),
        FailureKind::Timeout => format!(
            "MCP server '{server}' didn't respond in time. Check the endpoint is reachable, or raise its timeout."
        ),
        FailureKind::Unreachable => format!(
            "Couldn't reach MCP server '{server}'. Check the endpoint URL is correct and the host is online."
        ),
        FailureKind::Tls => format!(
            "MCP server '{server}' has a TLS/certificate problem — the secure connection couldn't be established."
        ),
        FailureKind::NotMcp => format!(
            "MCP server '{server}' didn't respond as an MCP endpoint. Check the URL points at the server's MCP path."
        ),
        FailureKind::ServerError => format!(
            "MCP server '{server}' returned a server error. It's likely down — try again later."
        ),
        FailureKind::ToolCallRejected => format!(
            "MCP server '{server}' rejected the call: {err}. It may need different arguments or credentials — tell the operator, don't retry blindly."
        ),
        FailureKind::Unknown => format!("MCP server '{server}' couldn't be used: {err}."),
    }
}

/// Probe a single server end-to-end: build a one-server registry (auth
/// **included** — unlike upstream's no-auth test probe), list its tools
/// (inheriting the injection-safety filter), and classify the outcome into a
/// **scrubbed** [`McpHealth`].
///
/// A disabled decl is probed as if enabled (the caller decides whether to probe;
/// probing must reflect the configured auth regardless of the exposed flag).
/// Probes a server, then persists everything the probe learned: the scrubbed
/// health record, and — on a successful listing — the tool inventory and what
/// the server said about itself.
///
/// One function because every caller wants all of it, and two transcriptions of
/// "probe, then persist" is how these records come to describe different
/// moments.
///
/// No write can fail the probe. A store that refuses leaves the caller with a
/// health record it can still answer with, which is strictly better than
/// reporting the server unreachable because a write failed.
///
/// A failed probe leaves the previous inventory and description in place. The
/// tools a server offered before an outage are the best available answer during
/// one, and clearing them would empty the console's row set and silently drop
/// every tool a stored tier default reaches.
pub async fn probe_and_record(
    company: &CompanyId,
    decl: &McpServerDecl,
    secrets: &dyn SecretStore,
) -> McpHealth {
    let outcome = probe_server_outcome(decl).await;
    let health = outcome.health;
    let _ = crate::company::mcp::save_health(company, &decl.name, &health, secrets).await;
    if let Some(listing) = outcome.listing {
        let inventory = mcp_policy::inventory_from_discovery(
            listing
                .iter()
                .map(|(name, description)| (name.as_str(), description.as_deref())),
            health.checked_at_millis,
        );
        let _ = mcp_policy::save_tool_inventory(
            company,
            secrets,
            &mcp_policy::tool_inventory_key(&decl.name),
            &inventory,
        )
        .await;
    }
    if let Some(server_info) = outcome.server_info {
        let mut info = mcp_server_info::from_server_info(&server_info);
        if let Some(source) = mcp_server_info::icon_source(&server_info) {
            info.icon_data_url = mcp_server_info::fetch_icon(&source).await;
        }
        let stored = mcp_server_info::load(company, &decl.name, secrets).await;
        info.title = info.title.or(stored.title);
        info.website_url = info.website_url.or(stored.website_url);
        info.icon_data_url = info.icon_data_url.or(stored.icon_data_url);
        let _ = mcp_server_info::save(company, &decl.name, &info, secrets).await;
    }
    health
}

pub async fn probe_server(decl: &McpServerDecl) -> McpHealth {
    probe_server_outcome(decl).await.health
}

/// Everything one probe learned about a server.
pub struct ProbeOutcome {
    /// The scrubbed health record, always present.
    pub health: McpHealth,
    /// The tools the listing returned, `Some` only on a successful listing. An
    /// empty vector and a failed probe are different facts: the first says the
    /// server offers nothing, the second says nobody asked it.
    pub listing: Option<Vec<(String, Option<String>)>>,
    /// The `serverInfo` block the handshake settled on, `Some` only when the
    /// probe got that far. Untyped by the protocol, so it is carried raw and
    /// read by [`mcp_server_info`](crate::mcp::decl::server_info).
    pub server_info: Option<serde_json::Value>,
}

/// [`probe_server`], keeping what the probe learned beyond the badge.
///
/// The discovery pass already happens — the plain probe computes a count from
/// this same listing and drops the names. A caller that persists an inventory
/// needs them, and a second probe to re-fetch what was just discarded would be
/// a second round trip and a second chance for the two to disagree. The server's
/// own description is read back from the transport's cached `initialize`.
pub async fn probe_server_outcome(decl: &McpServerDecl) -> ProbeOutcome {
    let secrets = decl.auth.secret_values();
    let auth_configured = decl.auth.is_configured();

    let mut probe = decl.clone();
    probe.enabled = true; // the probe reflects config, not the exposed flag
    let registry = registry_from_decls(std::slice::from_ref(&probe));

    match registry.list_tools(&decl.name).await {
        Ok(tools) => {
            let count = tools.len() as u32;
            let listing = tools
                .iter()
                .map(|tool| (tool.name.clone(), tool.description.clone()))
                .collect();
            ProbeOutcome {
                health: McpHealth {
                    status: McpStatus::Ok,
                    message: format!(
                        "{count} tool{} available.",
                        if count == 1 { "" } else { "s" }
                    ),
                    tool_count: count,
                    checked_at_millis: now_millis(),
                    auth_hint: None,
                },
                listing: Some(listing),
                server_info: registry
                    .initialize(&decl.name)
                    .await
                    .ok()
                    .map(|initialized| initialized.server_info),
            }
        }
        Err(err) => {
            // The transport reports its own error type now. Wrap it so the
            // classifier keeps seeing one `anyhow::Error` chain — it downcasts
            // through that chain for both the typed 401 and the typed reqwest
            // transport failure, and `anyhow` preserves `source()`.
            let err = anyhow::Error::new(err);
            let class = classify_mcp_error(&err, auth_configured, false);
            // Issue #1260: "wants OAuth" and "we can sign in" are two different
            // questions, and only the second decides whether the console should
            // offer a Sign in button. Asked here rather than in
            // `classify_mcp_error`, which is pure and synchronous by design —
            // this needs a live discovery call.
            let class = refine_oauth_capability(&decl.endpoint, class).await;
            let message = scrub(&operator_message(&decl.name, &class, &err), &secrets);
            ProbeOutcome {
                health: McpHealth {
                    status: class.status,
                    message,
                    tool_count: 0,
                    checked_at_millis: now_millis(),
                    auth_hint: class.auth_hint,
                },
                listing: None,
                server_info: None,
            }
        }
    }
}

/// Downgrade an `oauth_required` verdict to `static_token_required` when console
/// OAuth provably cannot drive this server (issue #1260).
///
/// Only ever consulted for [`FailureKind::OauthRequired`], so a healthy server
/// pays nothing: this runs on a server that already answered `401`.
#[cfg(feature = "mcp")]
async fn refine_oauth_capability(endpoint: &str, class: ProbeClass) -> ProbeClass {
    if class.kind != FailureKind::OauthRequired
        || crate::company::mcp_oauth::supports_console_oauth(endpoint).await
    {
        return class;
    }
    ProbeClass {
        status: FailureKind::StaticTokenRequired.status(),
        auth_hint: FailureKind::StaticTokenRequired.auth_hint(),
        kind: FailureKind::StaticTokenRequired,
    }
}

/// Without the `mcp` feature there is no `oauth/start` route for the console to
/// call, so there is no Sign in button to withdraw and nothing to refine.
#[cfg(not(feature = "mcp"))]
async fn refine_oauth_capability(_endpoint: &str, class: ProbeClass) -> ProbeClass {
    class
}

#[cfg(test)]
#[path = "mcp_probe_tests.rs"]
mod tests;
