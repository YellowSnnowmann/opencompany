//! Per-tenant browser OAuth for HTTP-remote MCP servers (issue #90).
//!
//! Many remote MCP servers gate access behind OAuth 2.0 (authorization-code +
//! PKCE), advertised via a `401` challenge that points at an
//! `oauth-protected-resource` document. The operator clicks *Sign in*, a
//! browser tab opens the authorization server, and the redirect lands on the
//! host's unauthenticated `/oauth/mcp/callback` route, which exchanges the code
//! for a token and stores it write-only under the company's per-server
//! credential key.
//!
//! The flow itself is tinymcp's [`OAuthFlow`]: discovery, dynamic client
//! registration, PKCE, the parked pending state, the code exchange and the
//! refresh. It runs with [`OAuthFlow::require_public_endpoints`], so every
//! discovery-supplied endpoint must be `https` on a public address. This module
//! is the host side: the qualified server id a redirect resolves back to a
//! company, and [`MaterialStore`], which maps the flow's credential map onto
//! OpenCompany's stored [`AuthMaterial::OAuth`] so tokens minted before the
//! swap keep working.
//!
//! **Security.** Every token is returned as an [`AuthMaterial::OAuth`], whose
//! [`AuthMaterial::secret_values`] enumerates the access token, refresh token,
//! and any confidential `client_secret` — so all of them feed the scrubber and
//! can never survive into an error, a health record, or agent-visible output.
//! Nothing here logs or returns a token.
//!
//! Compiled only under `feature = "mcp"`.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use openhuman_core as oh;

use oh::mcp::http_client::{AuthorizationServerMetadata, McpAuthorizationContext, McpHttpClient};
use tinymcp::registry::oauth::{OAUTH_BUNDLE_KEY, OAuthBundle, OAuthCredentialStore, OAuthFlow};

use crate::Result;
use crate::company::mcp::AuthMaterial;
use crate::error::OpenCompanyError;
use crate::ports::types::CompanyId;

/// The discovery HTTP timeout, in seconds.
const HTTP_TIMEOUT_SECS: u64 = 20;

/// The credential key the flow stores the access token under.
const ACCESS_TOKEN_KEY: &str = "Authorization";

/// The scheme the flow prefixes the access token with.
const BEARER_PREFIX: &str = "Bearer ";

/// Separates the company from the server in a qualified server id. Neither a
/// company id nor a server slug can contain it.
const SERVER_ID_SEPARATOR: char = '/';

const METADATA_FETCH_FAILED: &str = "fetching protected-resource metadata";
const NO_AUTH_REQUIRED: &str = "does not require authorization";
const ENDPOINT_REFUSED: &str = "endpoint refused";
const ENDPOINT_HAS_NO_HOST: &str = "endpoint has no host";
const INVALID_URL: &str = "invalid ";

/// The client name dynamic registration sends, shown on the authorization
/// server's consent screen.
const CLIENT_NAME: &str = "OpenCompany";

/// The console's OAuth flow: one per host, holding the authorizations parked
/// between `oauth/start` and the callback.
///
/// # Panics
///
/// When the HTTP client cannot be built, which only a broken TLS setup does.
pub fn console_flow() -> OAuthFlow {
    named_flow().require_public_endpoints()
}

fn named_flow() -> OAuthFlow {
    OAuthFlow::new(None)
        .expect("oauth http client must build")
        .with_client_name(CLIENT_NAME)
}

/// The flow refreshes run through. Refreshing parks nothing, so one flow
/// serves every company.
fn refresh_flow() -> &'static OAuthFlow {
    static FLOW: OnceLock<OAuthFlow> = OnceLock::new();
    FLOW.get_or_init(console_flow)
}

/// The callback redirect URI: `<base>/oauth/mcp/callback`. `base` is the host's
/// public URL (`OPENCOMPANY_PUBLIC_URL`) or `http://{bind}` — see
/// [`AppConfig::host_base_url`](crate::app::AppConfig::host_base_url). This MUST
/// be the exact string registered in DCR and replayed on the token exchange, or
/// the authorization server rejects the redirect.
pub fn callback_redirect_uri(base_url: &str) -> String {
    format!("{}/oauth/mcp/callback", base_url.trim_end_matches('/'))
}

/// The server id a pending authorization is parked under: the company and the
/// server, so the callback — which carries no console session — learns both
/// from the `state` alone.
pub fn server_id(company: &CompanyId, server_name: &str) -> String {
    format!("{}{SERVER_ID_SEPARATOR}{server_name}", company.as_ref())
}

/// The company and server a [`server_id`] names.
pub fn split_server_id(server_id: &str) -> Option<(CompanyId, String)> {
    let (company, server) = server_id.rsplit_once(SERVER_ID_SEPARATOR)?;
    (!company.is_empty() && !server.is_empty())
        .then(|| (CompanyId::new(company), server.to_string()))
}

/// The flow's view of one server's stored [`AuthMaterial`].
///
/// Reads present the material as the flow stores it — the bearer under
/// `Authorization` and the refresh bookkeeping as an [`OAuthBundle`] — and a
/// write is captured as the [`AuthMaterial::OAuth`] it describes rather than
/// persisted, so the caller stores it through
/// [`store_auth`](crate::company::mcp::store_auth) exactly as before.
pub struct MaterialStore {
    endpoint: Option<String>,
    current: AuthMaterial,
    minted: Mutex<Option<AuthMaterial>>,
}

impl MaterialStore {
    /// A store over `current`, for a server at `endpoint`.
    pub fn new(endpoint: Option<String>, current: AuthMaterial) -> Self {
        Self {
            endpoint,
            current,
            minted: Mutex::new(None),
        }
    }

    /// The material the flow wrote, if it wrote any.
    pub fn minted(&self) -> Option<AuthMaterial> {
        self.minted.lock().expect("minted material").clone()
    }
}

/// `material` as the flow's credential map; empty unless it is OAuth.
pub fn credentials_of(material: &AuthMaterial) -> BTreeMap<String, String> {
    let AuthMaterial::OAuth {
        access_token,
        refresh_token,
        client_id,
        client_secret,
        token_endpoint,
        expires_at,
    } = material
    else {
        return BTreeMap::new();
    };
    let bundle = OAuthBundle {
        refresh_token: refresh_token.clone(),
        client_id: client_id.clone(),
        client_secret: client_secret.clone(),
        token_endpoint: token_endpoint.clone(),
        expires_at: *expires_at,
    };
    let mut credentials = BTreeMap::new();
    credentials.insert(
        ACCESS_TOKEN_KEY.to_string(),
        format!("{BEARER_PREFIX}{access_token}"),
    );
    if let Ok(bundle) = serde_json::to_string(&bundle) {
        credentials.insert(OAUTH_BUNDLE_KEY.to_string(), bundle);
    }
    credentials
}

/// The [`AuthMaterial::OAuth`] a credential map describes, when it holds both
/// an access token and its bundle.
pub fn material_of(credentials: &BTreeMap<String, String>) -> Option<AuthMaterial> {
    let access = credentials.get(ACCESS_TOKEN_KEY)?;
    let access = access.strip_prefix(BEARER_PREFIX).unwrap_or(access);
    let bundle: OAuthBundle = serde_json::from_str(credentials.get(OAUTH_BUNDLE_KEY)?).ok()?;
    Some(AuthMaterial::OAuth {
        access_token: access.to_string(),
        refresh_token: bundle.refresh_token,
        client_id: bundle.client_id,
        client_secret: bundle.client_secret,
        token_endpoint: bundle.token_endpoint,
        expires_at: bundle.expires_at,
    })
}

impl OAuthCredentialStore for MaterialStore {
    async fn remote_url(&self, _server_id: &str) -> tinymcp::Result<Option<String>> {
        Ok(self.endpoint.clone())
    }

    async fn load_credentials(
        &self,
        _server_id: &str,
    ) -> tinymcp::Result<BTreeMap<String, String>> {
        Ok(credentials_of(&self.current))
    }

    async fn store_credentials(
        &self,
        _server_id: &str,
        credentials: &BTreeMap<String, String>,
    ) -> tinymcp::Result<()> {
        *self.minted.lock().expect("minted material") = material_of(credentials);
        Ok(())
    }
}

/// Begins the browser-OAuth flow for `server_name` at `endpoint`, returning the
/// live `/authorize` URL. The pending authorization is parked on `flow` under
/// [`server_id`] until the callback's [`complete`].
///
/// Returns [`OpenCompanyError::InvalidRequest`] (a clean operator-actionable
/// error) when the server needs no sign-in, when it does not advertise dynamic
/// client registration — the operator should paste a static token instead —
/// or when a discovery-supplied endpoint is refused.
pub async fn begin(
    flow: &OAuthFlow,
    endpoint: &str,
    company_id: &CompanyId,
    server_name: &str,
    redirect_uri: &str,
) -> Result<String> {
    let store = MaterialStore::new(Some(endpoint.to_string()), AuthMaterial::None);
    let authorize_url = flow
        .begin(&store, &server_id(company_id, server_name), redirect_uri)
        .await
        .map_err(|error| begin_error(server_name, &error))?;
    log::info!(
        "[mcp-oauth] begin company={} server={server_name}",
        company_id.as_ref()
    );
    Ok(authorize_url)
}

/// The operator-facing error for a sign-in that could not begin.
fn begin_error(server_name: &str, error: &tinymcp::Error) -> OpenCompanyError {
    match error {
        tinymcp::Error::MalformedResponse { detail } if detail.contains(NO_AUTH_REQUIRED) => {
            OpenCompanyError::InvalidRequest(format!(
                "MCP server `{server_name}` does not require authorization — no OAuth sign-in is needed."
            ))
        }
        tinymcp::Error::AuthDiscovery { detail, .. }
            if detail.starts_with(METADATA_FETCH_FAILED) =>
        {
            OpenCompanyError::Harness(format!("oauth discovery failed: {error}"))
        }
        tinymcp::Error::AuthDiscovery { .. } => OpenCompanyError::InvalidRequest(format!(
            "MCP server `{server_name}` requires OAuth but does not advertise dynamic client \
             registration — paste a static API token in its credential field instead."
        )),
        tinymcp::Error::MalformedResponse { detail }
            if detail.contains(ENDPOINT_REFUSED)
                || detail.contains(ENDPOINT_HAS_NO_HOST)
                || detail.starts_with(INVALID_URL) =>
        {
            OpenCompanyError::InvalidRequest(detail.clone())
        }
        other => OpenCompanyError::Harness(format!("oauth sign-in could not start: {other}")),
    }
}

/// Completes the flow parked under `state`: exchanges `code` for a token and
/// returns the [`AuthMaterial::OAuth`] the caller stores write-only.
///
/// The parked authorization is consumed whether or not the exchange succeeds.
pub async fn complete(flow: &OAuthFlow, state: &str, code: &str) -> Result<AuthMaterial> {
    let store = MaterialStore::new(None, AuthMaterial::None);
    let server = flow
        .complete(&store, state, code)
        .await
        .map_err(|error| OpenCompanyError::Harness(format!("token request failed: {error}")))?;
    let material = store.minted().ok_or_else(|| {
        OpenCompanyError::Harness("the token exchange returned no usable token".to_string())
    })?;
    log::info!("[mcp-oauth] complete server={server} — token minted");
    Ok(material)
}

/// If `material` is an [`AuthMaterial::OAuth`] whose access token is expired or
/// within a minute of expiring, and it holds a refresh token, mints a fresh
/// access token and returns the updated material.
///
/// Returns `None` when there is nothing to refresh or the refresh fails (the
/// caller keeps the old material and lets the next 401 re-prompt sign-in — a
/// failed refresh must never brick the harness build).
pub async fn refresh(material: &AuthMaterial) -> Option<AuthMaterial> {
    refresh_with(refresh_flow(), material).await
}

/// [`refresh`] through a given flow.
pub async fn refresh_with(flow: &OAuthFlow, material: &AuthMaterial) -> Option<AuthMaterial> {
    if !matches!(material, AuthMaterial::OAuth { .. }) {
        return None;
    }
    let store = MaterialStore::new(None, material.clone());
    match flow.refresh(&store, "refresh").await {
        Ok(true) => {
            log::info!("[mcp-oauth] refreshed access token");
            store.minted()
        }
        Ok(false) => None,
        Err(error) => {
            log::warn!("[mcp-oauth] refresh failed, keeping existing token: {error}");
            None
        }
    }
}

/// How long [`supports_console_oauth`] will wait on OAuth discovery.
///
/// Short on purpose: this is a refinement of an answer the probe already has,
/// not the answer itself, so it must not dominate the probe's own budget.
const CONSOLE_OAUTH_DISCOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Whether console OAuth can actually drive this server (issue #1260).
///
/// [`begin`] answers this already, but only by attempting the flow and failing —
/// which is too late for the health probe, whose answer decides whether the
/// console offers a **Sign in** button or a credential field. A server can
/// require OAuth and still be undrivable from here: Slack's MCP endpoint
/// answers `401` with a proper resource-metadata challenge and advertises no
/// `registration_endpoint`, so there is no client for us to mint and no sign-in
/// we can complete.
///
/// **Every failure answers `true`.** Discovery is a live outbound call and can
/// fail for reasons that say nothing about the server's capabilities — a
/// timeout, a transient 5xx, a network blip. Answering `false` on one of those
/// would replace a working Sign in button with "paste a token" on a server that
/// supports sign-in perfectly well. Unsure therefore means "leave it as it
/// was" — which is why this is not [`OAuthFlow::detect`], whose discovery
/// failure reads as "static token".
pub(crate) async fn supports_console_oauth(endpoint: &str) -> bool {
    let Ok(discovered) =
        tokio::time::timeout(CONSOLE_OAUTH_DISCOVERY_TIMEOUT, discover(endpoint)).await
    else {
        return true;
    };
    match discovered {
        Ok(None) => true,
        Ok(Some(ctx)) => ctx.authorization_server_metadata.iter().any(drivable),
        Err(_) => true,
    }
}

/// Discover a server's OAuth authorization context via an unauthenticated MCP
/// `initialize` probe. `Ok(None)` means the server did not 401.
async fn discover(endpoint: &str) -> Result<Option<McpAuthorizationContext>> {
    let client = McpHttpClient::new(endpoint.to_string(), HTTP_TIMEOUT_SECS)
        .map_err(|e| OpenCompanyError::Harness(format!("oauth discovery client: {e}")))?;
    client
        .discover_authorization()
        .await
        .map_err(|e| OpenCompanyError::Harness(format!("oauth discovery failed: {e}")))
}

/// Whether [`OAuthFlow::begin`] could sign in through this authorization
/// server: authorize, token and dynamic-registration endpoints, and (when
/// grant types are listed) `authorization_code`.
fn drivable(asm: &AuthorizationServerMetadata) -> bool {
    asm.authorization_endpoint.is_some()
        && asm.token_endpoint.is_some()
        && asm.registration_endpoint.is_some()
        && (asm.grant_types_supported.is_empty()
            || asm
                .grant_types_supported
                .iter()
                .any(|g| g == "authorization_code"))
}

#[cfg(test)]
#[path = "mcp_oauth_fixture_tests.rs"]
pub(crate) mod fixture;

#[cfg(test)]
#[path = "mcp_oauth_tests.rs"]
mod tests;
