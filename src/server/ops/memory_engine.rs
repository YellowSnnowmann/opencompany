//! Memory-engine selection: which engine this instance binds, chosen from the
//! console instead of only from the process environment.
//!
//! ```text
//! GET  …/memory/engine        what is bound, what is saved, what can be picked
//! POST …/memory/engine/test   probe a candidate engine without saving it
//! PUT  …/memory/engine        save it, bind it, and put it in force
//! ```
//!
//! ## Why a route and not just `OPENCOMPANY_MEMORY`
//!
//! The engine was selectable only by whoever controls the process environment,
//! which on a self-hosted box means editing a unit file and restarting. The
//! Brain page could *show* the bound engine (`docs/spec/runtime/memory-engine.md`)
//! and nothing else. This is the write half: the choice persists to the
//! `[memory]` section of `config.toml` — the instance's own second
//! configuration layer, not a company's manifest, because one instance binds
//! one engine.
//!
//! ## It applies live, and says so honestly when it cannot
//!
//! A saved-but-not-applied engine is the failure this surface exists to avoid:
//! an operator who picks Supermemory and is then shown their old memory has no
//! way to tell whether the choice landed. So [`apply`] opens the replacement
//! overlay, probes it, swaps it onto the [`AppState`], and rebuilds every
//! registered company so the new ports are actually the ones a turn will use
//! ([`rebuild_company`](crate::runtime::rebuild_company)). A host with no
//! rebuilder wired is the only case that still needs a restart, and it is
//! reported per-company rather than assumed — the same contract
//! `crate::server::setup` keeps for `auth_mode`.
//!
//! ## An engine that does not answer is refused, not bound
//!
//! Boot binds an unreachable engine and warns, because a transient vendor
//! outage must not crash-loop a tenant. An interactive apply is the opposite
//! case: the operator is right there, a typo'd URL or a revoked key is the
//! overwhelmingly likely cause, and binding it anyway would swap a working
//! engine out for a dead one. So a failed probe refuses the change and the old
//! overlay stays in force. `?force=true` is the escape hatch for an engine the
//! operator knows is down and wants bound anyway.
//!
//! ## The environment still outranks it
//!
//! `env ⟵ config.toml`, exactly as every other key resolves
//! (`docs/spec/runtime/config.md`). A hosted tenant has `OPENCOMPANY_MEMORY*`
//! injected by the control plane; accepting an edit there would write a file,
//! report success, and change nothing at the next boot. [`read`] therefore
//! reports `editable: false` with the owning layer, and [`apply`] refuses with
//! `409` rather than pretending. See
//! [`MemorySection`](crate::app::config::MemorySection).

use axum::extract::Query;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::app::config::{ConfigFile, ConfigValue, write_config_toml};
use crate::error::OpenCompanyError;
use crate::server::error::ApiError;
use crate::server::ops::scope::{AdminScopedCompany, scoped};
use crate::store::{MemoryBackend, MemorySelection, StorageSettings};

/// How long an interactive probe waits for the engine to answer.
///
/// Shorter than boot's five seconds on purpose: a human is watching this one,
/// and a hosted engine that needs more than three seconds to answer a health
/// check is not one an operator should be told is fine.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Builds the memory-engine route fragment.
pub fn router() -> Router<AppState> {
    scoped("/memory/engine", get(read).put(apply))
        .merge(scoped("/memory/engine/test", post(test_engine)))
}

/// One selectable engine, as the console draws it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineOption {
    /// What a `PUT` sends back: `store`, `supermemory`, `mem0`, `cognee`, or
    /// `null`.
    ///
    /// Deliberately flatter than the `(backend, driver)` pair the runtime
    /// takes: "embedded, driver namespace" and "remote, driver mem0" are one
    /// choice to an operator and two knobs to the host, and asking a console
    /// to model that correctly is how a UI ends up offering `remote` with no
    /// driver — a combination the host refuses at bind.
    id: &'static str,
    /// Human label for the tile.
    label: &'static str,
    /// One line on what picking it means.
    description: &'static str,
    /// Whether this build can actually construct it. A `false` tile renders
    /// disabled with `unavailableReason` rather than vanishing: an operator
    /// looking for Supermemory should learn their build lacks the feature, not
    /// conclude the product has no such thing.
    available: bool,
    /// Which Cargo feature it needs, when `available` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    unavailable_reason: Option<String>,
    /// Whether the engine needs an endpoint.
    requires_url: bool,
    /// Whether the engine needs a credential.
    requires_key: bool,
    /// Whether anything this engine stores survives a restart. `false` for
    /// `null` alone, which the console warns on.
    durable: bool,
}

/// The engine surface: what is bound now, what is saved, and what may be picked.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineDto {
    /// The engine id currently in force — derived from the live overlay, not
    /// from the file, so a saved-but-unapplied change is visible as a
    /// difference between this and [`Self::selected`].
    active: String,
    /// The capability families the live engine negotiated at bind time.
    capabilities: Vec<String>,
    /// The last probe's verdict; absent when the engine was never probed.
    #[serde(skip_serializing_if = "Option::is_none")]
    healthy: Option<bool>,
    /// The engine id the saved selection names (the file, or the environment
    /// when it owns the choice).
    selected: String,
    /// The saved endpoint, when the selected engine has one. Topology, not a
    /// secret — the credential never comes back out.
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    /// Whether a credential is stored. The bytes are never served.
    api_key_set: bool,
    /// Which layer owns the choice: `env`, `config.toml`, or `default`.
    layer: &'static str,
    /// Whether this instance may change it from here.
    editable: bool,
    /// The file a change is written to, so the console can name it.
    config_path: String,
    /// Every engine this build knows about, selectable or not.
    options: Vec<EngineOption>,
}

/// A submitted engine choice.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EngineRequest {
    /// One of [`EngineOption::id`].
    engine: String,
    /// The endpoint, for an engine that needs one.
    #[serde(default)]
    url: Option<String>,
    /// The credential. Absent means "keep the stored one" — a console that
    /// re-sent a redacted key would otherwise save the redaction. An explicit
    /// empty string clears it.
    #[serde(default)]
    api_key: Option<String>,
}

/// Whether to bind an engine whose probe failed.
#[derive(Debug, Default, Deserialize)]
struct ApplyQuery {
    #[serde(default)]
    force: bool,
}

/// What an apply did.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppliedDto {
    /// The engine now in force.
    engine: String,
    /// Whether the probe found it usable (absent when there was nothing to
    /// probe — the base store, or the in-pod engine overlay).
    #[serde(skip_serializing_if = "Option::is_none")]
    healthy: Option<bool>,
    /// Companies whose runtime could not be rebuilt in place, and therefore
    /// still hold the previous engine's ports until the process restarts.
    /// Empty means the swap is fully in force.
    restart_required_for: Vec<String>,
    /// The file the choice was written to.
    config_path: String,
    /// The engine surface again, so the console re-renders from one answer.
    engine_state: EngineDto,
}

/// What a probe found, without saving anything.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeDto {
    /// Whether the candidate answered a health check.
    healthy: bool,
    /// The families it negotiated, so an operator sees what it does NOT do
    /// before committing to it.
    capabilities: Vec<String>,
    /// Why it failed, when it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

/// The engine catalog.
///
/// The remote ids mirror `store::memory::driver::SUPPORTED_REMOTE_DRIVERS`,
/// duplicated because that module is `tinymemory`-gated while this route is
/// always compiled — the same duplication, for the same reason, that
/// `ops::memory` keeps for the outcome label prefix. The
/// `catalog_matches_driver_registry` test under the feature keeps them honest.
fn catalog() -> Vec<EngineOption> {
    // Feature availability, resolved once. `cfg!` rather than `#[cfg]` blocks
    // so the catalog is one list in one order in every build — an option that
    // disappears entirely reads as "this product has no such engine".
    let tinymemory = cfg!(feature = "tinymemory");
    let feature = |on: bool, name: &str| {
        (!on).then(|| format!("this build was compiled without the `{name}` feature"))
    };
    vec![
        EngineOption {
            id: "store",
            label: "Built-in store",
            description: "Memory lives in this instance's own storage backend. No engine, no \
                          network call, nothing to configure.",
            available: true,
            unavailable_reason: None,
            requires_url: false,
            requires_key: false,
            durable: true,
        },
        EngineOption {
            id: "supermemory",
            label: "Supermemory",
            description: "A hosted memory engine. Your company's memory is stored by Supermemory \
                          under this instance's namespace.",
            available: tinymemory,
            unavailable_reason: feature(tinymemory, "tinymemory"),
            requires_url: true,
            requires_key: true,
            durable: true,
        },
        EngineOption {
            id: "mem0",
            label: "Mem0",
            description: "A hosted memory engine. Your company's memory is stored by Mem0 under \
                          this instance's namespace.",
            available: tinymemory,
            unavailable_reason: feature(tinymemory, "tinymemory"),
            requires_url: true,
            requires_key: true,
            durable: true,
        },
        EngineOption {
            id: "cognee",
            label: "Cognee",
            description: "A hosted memory engine with a knowledge graph. Stored by Cognee under \
                          this instance's namespace.",
            available: tinymemory,
            unavailable_reason: feature(tinymemory, "tinymemory"),
            requires_url: true,
            requires_key: true,
            durable: true,
        },
        EngineOption {
            id: "null",
            label: "No memory",
            description: "Every write is accepted and discarded, every read is empty. For \
                          testing what the company does without memory.",
            available: tinymemory,
            unavailable_reason: feature(tinymemory, "tinymemory"),
            requires_url: false,
            requires_key: false,
            durable: false,
        },
    ]
}

/// Looks an engine id up in the catalog.
fn option_for(engine: &str) -> Option<EngineOption> {
    catalog().into_iter().find(|o| o.id == engine)
}

/// The `(backend, driver)` pair an engine id resolves to.
///
fn split_engine(engine: &str) -> Option<(MemoryBackend, Option<&'static str>)> {
    match engine {
        "store" => Some((MemoryBackend::Store, None)),
        "supermemory" => Some((MemoryBackend::Remote, Some("supermemory"))),
        "mem0" => Some((MemoryBackend::Remote, Some("mem0"))),
        "cognee" => Some((MemoryBackend::Remote, Some("cognee"))),
        "null" => Some((MemoryBackend::Null, None)),
        _ => None,
    }
}

/// The inverse of [`split_engine`]: the console-facing id for a selection.
///
/// Total rather than fallible: a selection assembled outside this module (an
/// environment naming `remote` with no driver, say) still has to render, and
/// falling back to the backend's own name says something true rather than
/// erroring a read that is only describing what is there.
fn engine_id(selection: &MemorySelection) -> String {
    match (selection.backend, selection.driver.as_deref()) {
        (MemoryBackend::Store, _) => "store".to_string(),
        (MemoryBackend::Remote, Some(driver)) => driver.to_string(),
        (MemoryBackend::Remote, None) => "remote".to_string(),
        (MemoryBackend::Null, _) => "null".to_string(),
    }
}

/// The saved selection and which layer owns it.
fn saved_selection(state: &AppState) -> Result<(MemorySelection, &'static str), OpenCompanyError> {
    if StorageSettings::memory_is_env_owned() {
        let settings = StorageSettings::from_env()?;
        return Ok((
            MemorySelection {
                backend: settings.memory_backend,
                driver: settings.memory_driver,
                url: settings.memory_url,
                api_key: settings.memory_api_key,
            },
            "env",
        ));
    }
    let file = ConfigFile::load(state.config_root())?;
    let section = file.map(|f| f.memory).unwrap_or_default();
    let named = section
        .backend
        .as_deref()
        .is_some_and(|b| !b.trim().is_empty());
    let selection = MemorySelection::from_section(&section)?;
    Ok((selection, if named { "config.toml" } else { "default" }))
}

/// Renders the whole engine surface.
fn snapshot(state: &AppState) -> Result<EngineDto, OpenCompanyError> {
    let (selection, layer) = saved_selection(state)?;
    let live = state.memory_overlay();
    // The live overlay is the honest answer to "what is bound"; its absence
    // means the base store serves memory, which is exactly `store`.
    let (active, capabilities, healthy) = match &live {
        Some(overlay) => (
            engine_id(&MemorySelection {
                backend: overlay.descriptor.backend,
                driver: Some(overlay.descriptor.driver_id.clone()),
                url: None,
                api_key: None,
            }),
            overlay.descriptor.capabilities.clone(),
            overlay.descriptor.healthy,
        ),
        None => ("store".to_string(), Vec::new(), None),
    };
    Ok(EngineDto {
        active,
        capabilities,
        healthy,
        selected: engine_id(&selection),
        url: selection.url.clone(),
        api_key_set: selection.api_key.is_some(),
        layer,
        editable: layer != "env",
        config_path: state
            .config_root()
            .join("config.toml")
            .display()
            .to_string(),
        options: catalog(),
    })
}

/// `GET …/memory/engine`.
async fn read(
    company: AdminScopedCompany,
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<Json<EngineDto>, ApiError> {
    // The company is resolved for authorization only: the engine is an
    // instance-level choice, and every company on this host shares it.
    let _ = company.id();
    Ok(Json(snapshot(&state)?))
}

/// Refuses an engine this build cannot construct, before its fields are
/// checked.
///
/// Ahead of [`selection_from`] on purpose: telling an operator their endpoint
/// is missing, when the engine could not be bound with any endpoint, sends
/// them to fix the wrong thing.
fn ensure_available(engine: &str) -> Result<(), OpenCompanyError> {
    let option = option_for(engine.trim()).ok_or_else(|| unknown_engine(engine))?;
    if option.available {
        return Ok(());
    }
    Err(OpenCompanyError::Conflict(format!(
        "{} cannot be bound here: {}.",
        option.label,
        option
            .unavailable_reason
            .unwrap_or_else(|| "it is not compiled into this build".to_string())
    )))
}

/// The refusal for an id the catalog does not carry, listing what may be
/// picked instead.
fn unknown_engine(engine: &str) -> OpenCompanyError {
    OpenCompanyError::InvalidRequest(format!(
        "`{}` is not a memory engine this host knows. Pick one of: {}.",
        engine.trim(),
        catalog()
            .iter()
            .map(|o| o.id)
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Turns a submitted choice into a validated [`MemorySelection`], carrying the
/// stored credential forward when the request omits one.
///
/// Field rules only — whether this *build* can bind the engine is
/// [`ensure_available`]'s question, kept separate so these rules are the same
/// in every feature set and are therefore testable in every lane.
fn selection_from(
    request: &EngineRequest,
    stored: &MemorySelection,
) -> Result<MemorySelection, OpenCompanyError> {
    let engine = request.engine.trim();
    let option = option_for(engine).ok_or_else(|| unknown_engine(engine))?;
    let (backend, driver) = split_engine(engine).ok_or_else(|| {
        OpenCompanyError::InvalidRequest(format!("`{engine}` has no runtime mapping"))
    })?;

    let trimmed = |value: Option<&String>| {
        value
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let url = trimmed(request.url.as_ref());
    // Absent means keep; present-but-empty means clear. A console that re-sent
    // the redacted key it was shown would otherwise store the redaction as the
    // credential, which fails at the next call with an authentication error
    // nobody can trace back to this screen.
    let api_key = match &request.api_key {
        None => stored.api_key.clone(),
        Some(raw) if raw.trim().is_empty() => None,
        Some(raw) => Some(raw.trim().to_string()),
    };

    // Refused here rather than at bind, because the bind-time messages name
    // environment variables an operator working from the console never set.
    if option.requires_url && url.is_none() {
        return Err(OpenCompanyError::InvalidRequest(format!(
            "{} needs an endpoint URL.",
            option.label
        )));
    }
    if option.requires_key && api_key.is_none() {
        return Err(OpenCompanyError::InvalidRequest(format!(
            "{} needs an API key.",
            option.label
        )));
    }
    if let Some(url) = &url
        && !(url.starts_with("http://") || url.starts_with("https://"))
    {
        return Err(OpenCompanyError::InvalidRequest(
            "the endpoint must be an http:// or https:// URL.".to_string(),
        ));
    }

    Ok(MemorySelection {
        backend,
        driver: driver.map(str::to_string),
        // Carried only for the engines that use them: leaving a stale URL on a
        // selection that switched to the built-in store would write dead keys
        // into `config.toml` and confuse the next reader of the file.
        url: option.requires_url.then_some(url).flatten(),
        api_key: option.requires_key.then_some(api_key).flatten(),
    })
}

/// Opens a candidate selection and probes it, without touching live state.
///
/// `Ok(None)` is the `store` default — nothing to open, nothing to probe.
async fn open_and_probe(
    selection: &MemorySelection,
) -> Result<Option<crate::store::MemoryOverlay>, OpenCompanyError> {
    let settings = StorageSettings::from_env()?.with_memory_selection(selection.clone());
    let Some(mut overlay) = crate::store::open_memory_overlay(&settings)? else {
        return Ok(None);
    };
    overlay.refresh_health(PROBE_TIMEOUT).await;
    Ok(Some(overlay))
}

/// `POST …/memory/engine/test` — probe a candidate without saving it.
async fn test_engine(
    company: AdminScopedCompany,
    axum::extract::State(state): axum::extract::State<AppState>,
    Json(request): Json<EngineRequest>,
) -> Result<Json<ProbeDto>, ApiError> {
    let _ = company.id();
    let (stored, _) = saved_selection(&state)?;
    ensure_available(&request.engine)?;
    let selection = selection_from(&request, &stored)?;
    // A failed *open* is a configuration answer, not a 500: a bad URL, a
    // driver this build cannot construct, a missing credential. The operator
    // is testing precisely to find that out, so it comes back as a verdict
    // rather than an error page.
    match open_and_probe(&selection).await {
        Ok(None) => Ok(Json(ProbeDto {
            healthy: true,
            capabilities: Vec::new(),
            detail: None,
        })),
        Ok(Some(overlay)) => Ok(Json(ProbeDto {
            // `None` means there was no provider seam to ask (the in-pod
            // engine overlay). It opened, so it is usable.
            healthy: overlay.descriptor.healthy.unwrap_or(true),
            capabilities: overlay.descriptor.capabilities.clone(),
            detail: (overlay.descriptor.healthy == Some(false))
                .then(|| "the engine did not answer a health check".to_string()),
        })),
        Err(error) => Ok(Json(ProbeDto {
            healthy: false,
            capabilities: Vec::new(),
            detail: Some(error.to_string()),
        })),
    }
}

/// `PUT …/memory/engine` — save it, bind it, and put it in force.
async fn apply(
    company: AdminScopedCompany,
    axum::extract::State(state): axum::extract::State<AppState>,
    Query(query): Query<ApplyQuery>,
    Json(request): Json<EngineRequest>,
) -> Result<Json<AppliedDto>, ApiError> {
    let _ = company.id();
    let (stored, layer) = saved_selection(&state)?;
    if layer == "env" {
        return Err(ApiError(OpenCompanyError::Conflict(
            "the memory engine on this host is set by `OPENCOMPANY_MEMORY`, which outranks \
             config.toml. Writing it here would have no effect at the next boot — change the \
             deployment's environment instead."
                .to_string(),
        )));
    }
    ensure_available(&request.engine)?;
    let selection = selection_from(&request, &stored)?;

    // Bind the replacement BEFORE anything is written or swapped. A refusal
    // here leaves the instance exactly as it was, which is the whole reason
    // this route probes at all.
    let overlay = open_and_probe(&selection).await?;
    if !query.force
        && let Some(overlay) = &overlay
        && overlay.descriptor.healthy == Some(false)
    {
        return Err(ApiError(OpenCompanyError::Conflict(format!(
            "`{}` did not answer a health check, so it was not bound and your current engine is \
             untouched. Check the endpoint and the API key.",
            request.engine.trim()
        ))));
    }

    // Persist next, for the reason `server::setup` persists before mutating:
    // a write failure must not leave a live host reflecting a change its
    // configuration does not record.
    let dir = state.config_root().to_path_buf();
    let edits: Vec<(&'static str, ConfigValue)> = vec![
        (
            "memory.backend",
            ConfigValue::Str(selection.backend.as_str().to_string()),
        ),
        (
            "memory.driver",
            selection
                .driver
                .clone()
                .map_or(ConfigValue::Unset, ConfigValue::Str),
        ),
        (
            "memory.url",
            selection
                .url
                .clone()
                .map_or(ConfigValue::Unset, ConfigValue::Str),
        ),
        (
            "memory.api_key",
            selection
                .api_key
                .clone()
                .map_or(ConfigValue::Unset, ConfigValue::Str),
        ),
    ];
    let config_path = write_config_toml(&dir, &edits)?;

    let healthy = overlay.as_ref().and_then(|o| o.descriptor.healthy);
    state.set_memory_overlay(overlay);

    // Companies already in the registry hold the previous engine's ports on
    // their cached runtime, so rebuild them in place. Reported per-company
    // rather than assumed either way: "restart required" must name what is
    // genuinely still pending, never a guess.
    let mut restart_required_for = Vec::new();
    for id in state.registry().list() {
        match crate::runtime::rebuild_company(&state, &id).await {
            Ok(_) => tracing::info!(company = %id, "rebuilt for the new memory engine"),
            Err(error) => {
                tracing::warn!(
                    company = %id, %error,
                    "could not rebuild for the new memory engine; it needs a restart",
                );
                restart_required_for.push(id.as_ref().to_string());
            }
        }
    }

    Ok(Json(AppliedDto {
        engine: engine_id(&selection),
        healthy,
        restart_required_for,
        config_path: config_path.display().to_string(),
        engine_state: snapshot(&state)?,
    }))
}

#[cfg(test)]
mod test;
