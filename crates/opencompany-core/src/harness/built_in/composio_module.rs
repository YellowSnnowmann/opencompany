//! Calls into the loaded `tinyconnectors` module, carrying one company's own
//! Composio route.
//!
//! # Why the bus and not a client
//!
//! OpenHuman v0.64.10 removed the in-process Composio clients. Its own source
//! says so: `action_tool.rs` calls `ComposioClient` "the since-removed
//! in-process client", and the direct client's `execution.rs` and
//! `tool_impl.rs` — which carried `execute_action` and `get_connection_url` —
//! are gone with it. Every Composio operation now lives in the loadable
//! `tinyconnectors` module.
//!
//! Upstream's own host entry, `oh::modules::connectors::proxy`, cannot be used
//! here. It reconciles the route from [`oh::config::Config`] — the signed-in
//! session, or a key in `config.toml` — which on this host names nobody. A
//! company's Composio credential lives in the
//! [`SecretStore`](crate::ports::SecretStore), and the managed tier's bearer is
//! resolved per company, so neither can reach the module through that door.
//!
//! What makes this work rather than a workaround is that the module was built
//! for it. `oh::modules::connectors` states the contract: "Whether the user is
//! signed in, whether they supplied their own Composio key, and which the
//! product prefers are all decisions this crate makes. They become the module's
//! configuration blob, and the module honours it rather than choosing." So this
//! builds the blob from the company's own credential and sends it over the same
//! `Configure` member upstream uses. Both routes are the module's; picking one
//! is the host's job, and here the host is per company.
//!
//! # What stays on this side
//!
//! The module's documentation is explicit that **egress policy did not move**:
//! "Apply it before calling `methods::EXECUTE`." It cannot see the reasons
//! behind a local-only refusal. [`super::composio_direct`] therefore keeps
//! `enforce_egress` and `emit_external_transfer` ahead of every execute, exactly
//! as it did when it called the client directly — moving the transport must not
//! quietly move that gate.
//!
//! # One module, one route at a time
//!
//! The module is a single process-global instance holding one route, so a
//! company's call is *configure then call*, and the two must not be split by
//! another company doing the same. [`configured`] is held across both.
//!
//! Upstream reconciles against a **private** static of its own
//! (`connectors::last_route`), which this cannot see. Were anything else to
//! configure the module, it would compare against a fingerprint this host had
//! already replaced, find it unchanged, skip its own reconfiguration, and issue
//! its calls on *this* company's route. Nothing does: `all_composio_agent_tools`
//! and `ComposioActionTool` appear nowhere in this crate, so upstream's Composio
//! agent tools are not in any belt, and no OpenCompany path reaches
//! `oh::modules::connectors` or `oh::integrations::composio::ops`. **That is a
//! standing requirement, not an accident** — route any future Composio call
//! through [`call`] rather than through either of those.
//!
//! The cost is that Composio calls across the host serialize, and the module is
//! reconfigured whenever consecutive calls come from different companies. It is
//! the same price [`super::search_byo::module`] pays for the same reason: a
//! module shaped for a single-user host, serving a multi-tenant one.
//!
//! # That contention is bounded, and by what
//!
//! A read guard is held across the member call, and tokio's `RwLock` is fair —
//! a waiting writer blocks new readers — so one company's in-flight call does
//! delay another company's, and a queue can form behind it. The question that
//! follows is how long, and the answer is not "indefinitely": every tinybus call
//! carries a deadline. `ModuleRuntime::proxy` builds the proxy through
//! `Connection::proxy`, which constructs it with `tinybus::DEFAULT_TIMEOUT` —
//! thirty seconds — and the bus states the rule outright: "Every call has a
//! deadline". So a hung module cannot hold the route; the call fails and the
//! guard drops.
//!
//! Thirty seconds is also upstream's own choice for these members. Its
//! `connectors::call` path takes the default, and it raises the deadline only
//! for `Sync` (`SLOW_MEMBER_TIMEOUT`, fifteen minutes). Any member here that
//! outgrows thirty seconds wants `Proxy::with_timeout`, not a second timeout
//! wrapped around the call: two bounds would report whichever fired first and
//! hide the bus's own message.

use std::sync::OnceLock;

use openhuman_core as oh;

use oh::integrations::composio::types::{
    ComposioAuthorizeRequest, ComposioAuthorizeResponse, ComposioConnectionsResponse,
    ComposioDeleteConnectionRequest, ComposioDeleteResponse, ComposioListToolsRequest,
    ComposioToolkitsResponse, ComposioToolsResponse,
};
use oh::modules::connectors::methods;

/// The registry id of the module that serves Composio. Upstream's own constant,
/// so a rename there is a compile error here rather than a module that is never
/// found.
pub(super) const MODULE_ID: &str = oh::modules::connectors::MODULE_ID;

/// How long a lazy module load may take before the call gives up. Matches the
/// bound [`super::search_byo::module`] uses for the same wait.
const LOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// Which Composio account a call runs as.
///
/// The two arms are the two routes the module implements. No [`Debug`]: both
/// carry a live credential, and this type exists on the request path.
#[derive(Clone)]
pub(super) enum Route {
    /// The company's own Composio API key — issue #238's BYOK tier. No platform
    /// identity and no platform bill.
    Direct { api_key: String, entity_id: String },
    /// The platform's managed Composio, reached through OpenHuman's backend with
    /// a bearer resolved for this company.
    Proxy {
        base_url: String,
        auth_token: String,
    },
}

impl Route {
    /// The configuration blob for this route, in the shape
    /// `oh::modules::connectors::module_config` builds from a `Config`.
    ///
    /// `state_dir` is deliberately absent. Upstream strips it before every
    /// reconcile because it is load-time only — the trigger archive opens once,
    /// and moving it later would strand the history already written there. This
    /// host sends no triggers at all, so the module's load-time default stands.
    ///
    /// `timezone` is pinned to UTC rather than read from the host. Upstream
    /// sends the *user's* zone so Composio results print local time beside UTC,
    /// which is right for a single-user desktop. Here the host's zone says
    /// nothing about the company whose agent is asking, and reading it would
    /// make a tenant's timestamps depend on which server their request landed
    /// on. UTC is also exactly what upstream falls back to when the lookup
    /// fails, so this is its documented behaviour-equivalent value, not a new
    /// one.
    fn blob(&self) -> serde_json::Value {
        match self {
            Self::Direct { api_key, entity_id } => serde_json::json!({
                "route": "direct",
                "api_key": api_key,
                "entity_id": entity_id,
            }),
            Self::Proxy {
                base_url,
                auth_token,
            } => serde_json::json!({
                "route": "proxy",
                "base_url": base_url,
                // The module's proxy route sends this as `Authorization:
                // Bearer`. The backend recognises a TinyHumans API key there by
                // its `tiny_` prefix, so no scheme flag is needed.
                "auth_token": auth_token,
                "timezone": "UTC",
            }),
        }
    }
}

/// A fingerprint of one route, so an unchanged one is not sent again: the same
/// company making two Composio calls in a row is the common case inside a turn,
/// and each reconfiguration is a bus round-trip.
///
/// Only the digest is kept, which is the point — comparing routes means
/// comparing bearer tokens, and a process-lifetime static holding one in
/// cleartext is a credential sitting somewhere nothing needs it. Upstream keeps
/// its own for the same stated reason. SHA-256 is deterministic and the digest
/// is never logged or reversible to the route.
fn fingerprint(route: &serde_json::Value) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(route.to_string().as_bytes()).into()
}

/// The route the module currently holds, behind a lock that distinguishes
/// *using* that route from *changing* it.
///
/// A [`RwLock`] rather than a mutex, and the distinction is load-bearing. The
/// module holds one route at a time, so a reconfiguration must exclude
/// everything — that is the write guard. But two calls that want the route
/// already loaded have no reason to wait for each other, and a mutex made them:
/// `a_slow_authorize_does_not_block_an_unrelated_toolkit` connects two toolkits
/// for one company at once, and under a mutex the second waited out the first's
/// network call. Same company, same route, nothing to serialize.
///
/// `None` means "unknown, reconfigure": either nothing has been sent yet, or a
/// `Configure` failed partway. The module is loaded through [`loader_config`],
/// which names no company, so the first call is precisely the one that must not
/// be skipped.
fn configured() -> &'static tokio::sync::RwLock<Option<[u8; 32]>> {
    static HELD: OnceLock<tokio::sync::RwLock<Option<[u8; 32]>>> = OnceLock::new();
    HELD.get_or_init(|| tokio::sync::RwLock::new(None))
}

/// The environment variable naming the module artifact this host loads.
///
/// Set by the container image to the `libtinyconnectors.so` built from the
/// vendored source, which is the same revision this crate's request types come
/// from. An override is checked *before* the pinned release, for the reasons
/// [`super::search_byo::module::MODULE_PATH_ENV`] records: the tenant container
/// runs unprivileged against a read-only root filesystem and may have no
/// writable cache to install into, and an API server reaching out to a release
/// host on the first Composio call is egress nobody asked for.
///
/// Unset, loading falls back to OpenHuman's pinned release — the right default
/// for a developer machine, which is why this is not required.
pub(super) const MODULE_PATH_ENV: &str = "OPENCOMPANY_TINYCONNECTORS_MODULE";

/// The configuration the *loader* reads. Distinct from [`Route::blob`]: this one
/// only has to say that modules may load and where the artifact is, and it
/// deliberately carries no credential — the module is initialized with it and
/// then immediately configured with the company's own route.
fn loader_config() -> &'static oh::config::Config {
    static LOADER: OnceLock<oh::config::Config> = OnceLock::new();
    LOADER.get_or_init(|| {
        let mut config = oh::config::Config::default();
        config.modules.enabled = true;
        if let Some(path) = std::env::var_os(MODULE_PATH_ENV) {
            let path = std::path::PathBuf::from(path);
            if path.is_file() {
                // Downloads stay off once we have named the artifact: a missing
                // override should be a clear load failure, not a silent fetch of
                // a different build.
                config.modules.allow_download = false;
                config
                    .modules
                    .overrides
                    .push(oh::config::schema::ModuleOverride {
                        id: MODULE_ID.to_string(),
                        path: path.display().to_string(),
                    });
            } else {
                tracing::warn!(
                    env = MODULE_PATH_ENV,
                    "[composio] the configured TinyConnectors module artifact is not a file; \
                     falling back to the pinned release"
                );
            }
        }
        config
    })
}

/// Call one module member as `route`, configuring the module first if it is not
/// already holding that route.
///
/// # Errors
///
/// Returns the module's own message when Composio is unavailable (no artifact
/// for this host, downloads disabled, the module still loading), when the route
/// was refused, or when the member failed.
///
/// Note what is **not** an error, and what upstream warns about in the same
/// words: a Composio action the provider refused comes back as a successful
/// reply carrying `successful: false`. A caller that checks only for `Err` will
/// report a failed send as a success.
pub(super) async fn call<Request, Reply>(
    route: &Route,
    member: &str,
    request: Request,
) -> Result<Reply, String>
where
    Request: serde::Serialize + Send,
    Reply: serde::de::DeserializeOwned,
{
    routed_call(route, member, (request,)).await
}

/// [`call`] for a member that takes no argument.
pub(super) async fn call_bare<Reply>(route: &Route, member: &str) -> Result<Reply, String>
where
    Reply: serde::de::DeserializeOwned,
{
    routed_call(route, member, ()).await
}

/// Load the module if this is the first call, give it `route` if it is not
/// already holding it, and call `member` while holding that route.
///
/// The read guard returned by [`routed`] is held across the call, so the route
/// cannot change under a request in flight. What it does not do is block another
/// call that wants the *same* route — see [`configured`] for why that
/// distinction is the whole point.
async fn routed_call<Args, Reply>(route: &Route, member: &str, args: Args) -> Result<Reply, String>
where
    Args: serde::Serialize + Send,
    Reply: serde::de::DeserializeOwned,
{
    let blob = route.blob();
    let current = fingerprint(&blob);
    let _holding = routed(current, blob).await?;
    call_on_module(member, args).await
}

/// Hold the module to `current`, configuring it if it is holding anything else,
/// and hand back the guard that keeps it there.
///
/// The write guard is **downgraded** rather than released. An earlier version
/// released it and re-checked in a loop, which livelocked: two companies calling
/// concurrently each invalidated the other's fingerprint in the gap between
/// setting the route and using it, and the pair span at full CPU indefinitely —
/// the composio suite burned eighteen CPU-minutes without finishing a test.
/// [`tokio::sync::RwLockWriteGuard::downgrade`] closes the gap outright: the
/// route this just set is provably the route held when the caller calls, so
/// there is nothing to re-check and no loop to spin.
async fn routed(
    current: [u8; 32],
    blob: serde_json::Value,
) -> Result<tokio::sync::RwLockReadGuard<'static, Option<[u8; 32]>>, String> {
    // The common case: the module already holds this route, and concurrent
    // callers that want it share this guard rather than queueing.
    let held = configured().read().await;
    if *held == Some(current) {
        return Ok(held);
    }
    // Drop the read guard before waiting for the writer. The fingerprint is
    // checked again while holding that writer below, so another configuration
    // cannot change it between the check and the write.
    drop(held);

    let mut held = configured().write().await;
    if *held != Some(current) {
        // Cleared *before* the attempt, not after a failure: a `Configure` that
        // fails leaves the module holding a route this host can no longer name,
        // and the next call must reconfigure rather than assume it is this one.
        // Recording the fingerprint only on success is what keeps a failed
        // reconfiguration from running the next company's call on this route.
        *held = None;
        call_on_module::<_, serde_json::Value>(methods::CONFIGURE, (blob,))
            .await
            .map_err(|error| format!("composio route refused: {error}"))?;
        *held = Some(current);
    }
    Ok(tokio::sync::RwLockWriteGuard::downgrade(held))
}

/// Load the module if this is the first call and call one member on it.
///
/// Carries no route of its own: [`routed`] is what decides whose credential the
/// module is holding when a call lands. Written as one function rather than a
/// `proxy()` accessor so the bus's own `Proxy` type is never named here — it
/// reaches this crate only through the vendored `openhuman-core`, and naming it
/// would mean taking `tinybus` as a direct dependency to spell one return type.
async fn call_on_module<Args, Reply>(member: &str, args: Args) -> Result<Reply, String>
where
    Args: serde::Serialize + Send,
    Reply: serde::de::DeserializeOwned,
{
    oh::modules::ops::ensure_loaded_within(loader_config(), MODULE_ID, Some(LOAD_TIMEOUT))
        .await
        .map_err(oh::modules::ops::LoadError::into_message)?;
    let record = oh::modules::registry::find(MODULE_ID)
        .ok_or_else(|| format!("unknown module '{MODULE_ID}'"))?;
    oh::modules::host::runtime()
        .await
        .map_err(|error| format!("composio module bus unavailable: {error}"))?
        .proxy(record.bus_name, record.object_path)
        .map_err(|error| format!("could not reach '{MODULE_ID}': {error}"))?
        .call(member, args)
        .await
        .map_err(|error| format!("composio {member} failed: {error}"))
}

/// The platform's managed Composio account for one company.
///
/// Replaces `oh::integrations::composio::ComposioClient`, which v0.64.10
/// removed. It holds the same [`IntegrationClient`] that type did — so the
/// bearer, the backend URL and the 401 handling are unchanged — and serves the
/// five operations this crate called on it over the connector module instead of
/// over its own HTTP.
///
/// Execute is deliberately absent. The managed execute in
/// [`super::composio`] never went through `ComposioClient`: it posts to
/// `/agent-integrations/composio/execute` itself, because it carries an
/// idempotency key and a post-OAuth retry that the client surface had no way to
/// express. [`Self::inner`] is what it reaches that endpoint with, exactly as
/// before.
///
/// No [`Debug`]: [`IntegrationClient::auth_token`] is a live bearer.
#[derive(Clone)]
pub(super) struct ManagedComposio {
    inner: std::sync::Arc<oh::integrations::IntegrationClient>,
}

impl ManagedComposio {
    /// The managed client over one company's resolved backend credential.
    pub(super) fn new(inner: std::sync::Arc<oh::integrations::IntegrationClient>) -> Self {
        Self { inner }
    }

    /// The underlying backend client, for the one call that speaks to the
    /// backend directly rather than through the module.
    pub(super) fn inner(&self) -> &oh::integrations::IntegrationClient {
        &self.inner
    }

    /// This company's managed account, as the module's route.
    fn route(&self) -> Route {
        Route::Proxy {
            base_url: self.inner.backend_url.clone(),
            auth_token: self.inner.auth_token.clone(),
        }
    }

    /// The toolkits this company may connect — the backend's server-enforced
    /// allowlist.
    pub(super) async fn list_toolkits(&self) -> anyhow::Result<ComposioToolkitsResponse> {
        call_bare(&self.route(), methods::LIST_TOOLKITS)
            .await
            .map_err(|error| anyhow::anyhow!("{error}"))
    }

    /// The connected accounts this company holds.
    pub(super) async fn list_connections(&self) -> anyhow::Result<ComposioConnectionsResponse> {
        call_bare(&self.route(), methods::LIST_CONNECTIONS)
            .await
            .map_err(|error| anyhow::anyhow!("{error}"))
    }

    /// The actions of `toolkits`, optionally narrowed by Composio action tags.
    ///
    /// No search term: the managed backend's endpoint never took one, so the
    /// caller applies it client-side and says so — the module's member matches
    /// that surface exactly, so nothing is lost in moving to it.
    ///
    /// `apply_user_scopes` is `false`, as upstream's own ops call sets it. The
    /// scope preference it would honour is a single-user setting this host does
    /// not store, and defaulting to `true` would hide actions against a
    /// preference that is never written — a listing narrowed by nothing anyone
    /// chose.
    pub(super) async fn list_tools(
        &self,
        toolkits: Option<&[String]>,
        tags: Option<&[String]>,
    ) -> anyhow::Result<ComposioToolsResponse> {
        let request = ComposioListToolsRequest {
            toolkits: named(toolkits),
            tags: named(tags),
            apply_user_scopes: false,
        };
        call(&self.route(), methods::LIST_TOOLS, request)
            .await
            .map_err(|error| anyhow::anyhow!("{error}"))
    }

    /// Begin an OAuth handoff and return the hosted connect URL.
    pub(super) async fn authorize(
        &self,
        toolkit: &str,
        extra_params: Option<serde_json::Value>,
    ) -> anyhow::Result<ComposioAuthorizeResponse> {
        call(
            &self.route(),
            methods::AUTHORIZE,
            ComposioAuthorizeRequest {
                toolkit: toolkit.to_string(),
                extra_params,
            },
        )
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))
    }

    /// Revoke one connected account.
    ///
    /// `clear_memory` is `false`: the memory sourced from a connection is this
    /// host's own bookkeeping — the memory targets, the identity facets,
    /// `PROFILE.md`, the `memory_sources` row — and
    /// [`super::composio`] already removes its own alongside the revoke. Asking
    /// the module to do it as well would be a second actor on the same rows.
    /// Nothing reads `memory_chunks_deleted`, so the count it leaves at zero is
    /// unobserved.
    pub(super) async fn delete_connection(
        &self,
        connection_id: &str,
    ) -> anyhow::Result<ComposioDeleteResponse> {
        call(
            &self.route(),
            methods::DELETE_CONNECTION,
            ComposioDeleteConnectionRequest {
                connection_id: connection_id.to_string(),
                clear_memory: false,
            },
        )
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))
    }
}

/// The slugs or tags a caller actually named, with blanks dropped. An empty
/// vector is the module's "not narrowed" and is skipped on the wire.
fn named(values: Option<&[String]>) -> Vec<String> {
    values
        .unwrap_or(&[])
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

/// Serializes tests that drive the module, because the route is process-global.
///
/// The module is one instance holding one route, so two tests configuring it at
/// once flip each other's route out from under an in-flight call: the one that
/// then has to reconfigure waits on the other's read guard, which is held across
/// its request. Thirty-one tests across five modules reach the module, and
/// without this the suite's result depends on which of them happen to overlap.
///
/// It serializes the *tests*, not the behaviour — each still observes its own
/// calls exactly as production makes them. What it deliberately does not model
/// is contention between companies, which is real: see this module's own notes
/// on one tenant's slow call delaying another's.
#[cfg(test)]
pub(crate) async fn route_test_guard() -> tokio::sync::MutexGuard<'static, ()> {
    static GUARD: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    GUARD
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

#[cfg(test)]
#[path = "composio_module_tests.rs"]
mod tests;
