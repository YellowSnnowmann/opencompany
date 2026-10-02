//! The call into the loaded TinySearch module, carrying one company's own
//! credential.
//!
//! # Why the bus and not a tool
//!
//! OpenHuman v0.64.10 moved the search engines into the loadable `tinysearch`
//! module and deleted the per-engine tool types [`super`] used to construct.
//! Its replacement, `oh::search::TinySearchTool`, cannot be used here: it calls
//! `oh::modules::search::execute_tool`, whose first act is
//! `reload_config_from_paths` — the configuration is re-read **from disk**, so
//! the only credential that can reach the module through that door is one
//! written into `config.toml`. Company keys live in the
//! [`SecretStore`](crate::ports::SecretStore), and they stay there.
//!
//! Everything under that wrapper is public, so this module takes the same path
//! it does, minus the reload: build the private configuration, hand it to the
//! module, call it. The key travels in the module configuration, which is the
//! channel the module documents for secrets — never in a method call.
//!
//! # One module, one configuration at a time
//!
//! The module is a single process-global instance holding one configuration, so
//! a company's search is *reconfigure then call*, and the two must not be split
//! by another company doing the same. [`with_module_lock`] is OpenHuman's own
//! answer to that and is `pub(super)` there, so this keeps its own lock to the
//! same discipline. That is sufficient rather than hopeful only because nothing
//! else in this crate calls `oh::modules::search` or wires `TinySearchTool`:
//! the managed surface in [`search`](crate::harness::search) posts to the
//! backend itself, and no other caller exists. **That is a standing
//! requirement, not an accident**: OpenHuman's lock and this one cannot see each
//! other, so a second caller arriving through `oh::modules::search` would
//! reintroduce exactly the interleaving this lock exists to prevent. Route any
//! future search through [`execute`] rather than through that wrapper.
//!
//! The cost is that BYO searches across the host serialize, and the module is
//! reconfigured whenever consecutive calls come from different companies. That
//! is the price of a module shaped for a single-user host; it buys the engines,
//! the fallback ladder and upstream's own rendering.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use openhuman_core as oh;
use tinysearch_bus::{
    BackendConfig, ExecuteToolRequest, ExecuteToolResponse, PresentationConfig, PresentationMode,
    ProviderConfig, ProviderRoute, SearchConfig, names,
};
use tokio::sync::Mutex;

use super::TenantSearch;

/// The registry id of the module that serves search. OpenHuman's own constant
/// is `pub`, and taking it from there means a rename upstream is a compile
/// error here rather than a module that is never found.
pub(super) const MODULE_ID: &str = oh::modules::search::MODULE_ID;

/// How long a lazy module load may take before the call gives up and reports
/// that search is not ready. Matches OpenHuman's own bound for the same wait.
const LOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// The private configuration that makes exactly one provider usable, with
/// exactly one company's key.
///
/// Only the configured provider is present, so the module's catalogue offers
/// only that provider's tools and the role ladder has nowhere else to fall back
/// to. `PresentationMode::AllTools` is what reproduces today's belt: each
/// provider tool under its own upstream name, with
/// [`byo_search_tools`](super::byo_search_tools) aliasing the canonical one to
/// `web_search`.
///
/// `backend` is left at its default. A BYO provider is always
/// [`ProviderRoute::Direct`] — the managed route is the other half of issue
/// #238 and bills the platform, which is the one thing this path must not do.
pub(super) fn configuration(tenant: &TenantSearch) -> SearchConfig {
    let mut settings = ProviderConfig {
        enabled: true,
        route: ProviderRoute::Direct,
        credential: tenant.api_key.clone(),
        max_results: Some(super::DEFAULT_MAX_RESULTS as u64),
        timeout_secs: Some(super::TIMEOUT_SECS),
        ..Default::default()
    };
    if tenant.provider == "searxng" {
        // A self-hosted instance has no credential and is addressed by URL.
        // Resolution guarantees the endpoint for this provider.
        settings.base_url = tenant.endpoint.clone();
        settings.default_language = Some(super::SEARXNG_LANGUAGE.to_string());
    }
    SearchConfig {
        enabled: true,
        backend: BackendConfig::default(),
        providers: BTreeMap::from([(tenant.provider.clone(), settings)]),
        presentation: PresentationConfig {
            mode: PresentationMode::AllTools,
            ..Default::default()
        },
    }
}

/// A fingerprint of one encoded configuration, so an unchanged one is not sent
/// again: the same company searching twice in a row is the common case inside a
/// turn, and each reconfiguration is a bus round-trip that can land while the
/// module is still applying the previous one.
///
/// Hashes the credential along with everything else, which is the point — a
/// rotated key must reconfigure. The hash is never logged, and `DefaultHasher`
/// is only ever compared against another value from this same process.
fn fingerprint(configuration: &serde_json::Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    configuration.to_string().hash(&mut hasher);
    hasher.finish()
}

/// The lock held across reconfigure-and-call, carrying the fingerprint of the
/// configuration the module currently holds. See the module docs for why one
/// lock has to span both.
///
/// `None` means "unknown, reconfigure": either nothing has been sent yet, or a
/// reinitialization failed partway. Note this is the opposite of OpenHuman's
/// reading of `None` — it may skip the first call because the module was loaded
/// with the very configuration it is about to use, whereas ours is loaded with
/// [`loader_config`], which deliberately carries no company key. For this host
/// the first call is precisely the one that must not be skipped.
fn configured() -> &'static Mutex<Option<u64>> {
    static HELD: OnceLock<Mutex<Option<u64>>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(None))
}

/// The environment variable naming the module artifact this host loads.
///
/// Set by the container image to the `libtinysearch.so` built from the vendored
/// source, which is the same revision this crate's `tinysearch-bus` comes from.
/// An override is checked *before* the pinned release, so a host that sets it
/// never reaches the download path — which matters here for two reasons beyond
/// reproducibility: the tenant container runs unprivileged against a read-only
/// root filesystem and may have no writable cache directory to install into,
/// and an API server reaching out to a release host on the first search is
/// egress nobody asked for.
///
/// Unset, loading falls back to OpenHuman's pinned release. That is the right
/// default for a developer machine and is why this is not required.
pub(super) const MODULE_PATH_ENV: &str = "OPENCOMPANY_TINYSEARCH_MODULE";

/// The configuration the *loader* reads. Distinct from [`configuration`]: this
/// one only has to say that modules may load and where the artifact is, and it
/// deliberately carries no credential — the module is initialized with it and
/// then immediately reconfigured with the company's own.
///
/// Built once. `Config::default()` resolves the OpenHuman directory through the
/// environment and the user's home directory, which is not work a search should
/// repeat.
fn loader_config() -> &'static oh::config::Config {
    static LOADER: OnceLock<oh::config::Config> = OnceLock::new();
    LOADER.get_or_init(|| {
        let mut config = oh::config::Config::default();
        config.modules.enabled = true;
        if let Some(path) = std::env::var_os(MODULE_PATH_ENV) {
            let path = std::path::PathBuf::from(path);
            if path.is_file() {
                // Downloads stay off once we have named the artifact: a missing
                // override should be a clear load failure, not a silent fetch
                // of a different build.
                config.modules.allow_download = false;
                // Not re-exported at `oh::config`; `schema` is the public home of the
                // config schema types.
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
                    "[search] the configured TinySearch module artifact is not a file; \
                     falling back to the pinned release"
                );
            }
        }
        config
    })
}

/// Execute one catalogue tool against the company's own provider.
///
/// # Errors
///
/// Returns the module's message when search is unavailable (no artifact for
/// this host, downloads disabled, the module still loading) or when the
/// provider itself failed. Callers render it with
/// `oh::search::tools::user_facing_error`, so a classified failure reaches the
/// model as advice rather than as a stack of transport nouns.
pub(super) async fn execute(
    tenant: &TenantSearch,
    request: ExecuteToolRequest,
) -> Result<ExecuteToolResponse, String> {
    #[cfg(test)]
    if let Some(response) = TEST_RESPONSE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .await
        .take()
    {
        *TEST_CALL.get_or_init(|| Mutex::new(None)).lock().await = Some((
            tenant.provider.clone(),
            tenant.api_key.clone(),
            request.name.clone(),
        ));
        return Ok(response);
    }

    let configuration = serde_json::to_value(configuration(tenant))
        .map_err(|error| format!("search configuration could not be encoded: {error}"))?;
    let current = fingerprint(&configuration);

    // Held across both the reconfiguration and the call: a second company
    // reconfiguring between them would run this search on its key.
    let mut held = configured().lock().await;

    oh::modules::ops::ensure_loaded_within(loader_config(), MODULE_ID, Some(LOAD_TIMEOUT))
        .await
        .map_err(oh::modules::ops::LoadError::into_message)?;
    let runtime = oh::modules::host::runtime()
        .await
        .map_err(|error| format!("search module bus unavailable: {error}"))?;
    if *held != Some(current) {
        // Cleared *before* the attempt, not after a failure: a reinitialization
        // that fails leaves the module holding a configuration this host can no
        // longer name, and the next call must reconfigure rather than assume it
        // is this one. Recording the fingerprint only on success is what keeps a
        // failed refresh from running the next company's search on this key.
        *held = None;
        runtime
            .connection()
            .reinitialize_module(MODULE_ID, configuration)
            .await
            .map_err(|error| format!("search module configuration refresh failed: {error}"))?;
        *held = Some(current);
    }
    runtime
        .proxy(names::INTERFACE, names::OBJECT_PATH)
        .map_err(|error| format!("search module proxy unavailable: {error}"))?
        .call(names::methods::EXECUTE_TOOL, (request,))
        .await
        .map_err(|error| format!("search ExecuteTool failed: {error}"))
}

/// Supply a one-shot module response for a harness-level BYO search test.
///
/// The test still builds the real catalogue tool and sends it through the
/// supervised harness turn; only the dynamic module boundary is replaced, so
/// CI does not need a platform-specific module artifact or a provider key.
#[cfg(test)]
pub(super) async fn set_test_response(response: ExecuteToolResponse) {
    *TEST_RESPONSE.get_or_init(|| Mutex::new(None)).lock().await = Some(response);
}

#[cfg(test)]
pub(super) async fn take_test_call() -> Option<(String, Option<String>, String)> {
    TEST_CALL
        .get_or_init(|| Mutex::new(None))
        .lock()
        .await
        .take()
}

#[cfg(test)]
static TEST_RESPONSE: OnceLock<Mutex<Option<ExecuteToolResponse>>> = OnceLock::new();
#[cfg(test)]
static TEST_CALL: OnceLock<Mutex<Option<(String, Option<String>, String)>>> = OnceLock::new();

#[cfg(test)]
#[path = "module_tests.rs"]
mod tests;
