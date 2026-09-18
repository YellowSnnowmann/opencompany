//! Capability-budget read surface (issue #108): the company's effective tier
//! plan and how much of each tier's token budget the current period has spent.
//!
//! A read-only companion to the harness gate — the console renders one row per
//! configured tier (budget, spend, remaining, whether its tools are disabled).
//! With no `[plan]` configured the response is `{ configured: false }` and the
//! console shows a "no token plan configured" note. The heavy lifting is the
//! pure math in [`crate::metering::capability`]; this handler only queries the
//! [`UsageMeter`](crate::ports::UsageMeter) for the period and projects it.

use axum::Json;
use axum::Router;
use axum::routing::get;
use serde::Serialize;

use crate::AppState;
use crate::company::credentials::{CredentialSource, TinyhumansTokenSource};
use crate::company::runtime::CompanyRuntime;
use crate::metering::capability::{CapabilityPlan, tokens_in};
use crate::ports::now_millis;
use crate::server::cognition::{CognitionState, InferenceResolution, cognition_state};
use crate::server::error::ApiError;
use crate::server::ops::{ScopedCompany, scoped};

/// Builds the capability-budget route fragment.
pub fn router() -> Router<AppState> {
    scoped("/capabilities", get(get_status))
}

/// The company's capability-budget status as the console renders it.
///
/// When no `[plan]` is configured only `configured: false` is sent; the extra
/// fields are omitted so the console can branch on presence.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CapabilityStatusDto {
    /// Whether the company has a capability plan at all.
    configured: bool,
    /// The configured built-in tier name, if any (`null` for a bare
    /// `token_budgets` plan).
    #[serde(skip_serializing_if = "Option::is_none")]
    plan: Option<String>,
    /// The budget window (`daily` / `monthly`).
    #[serde(skip_serializing_if = "Option::is_none")]
    period: Option<String>,
    /// Epoch-millis start of the current budget period (the spend window).
    #[serde(skip_serializing_if = "Option::is_none")]
    period_start_millis: Option<u64>,
    /// Total inference tokens spent by the company this period (the figure every
    /// tier threshold is compared against).
    #[serde(skip_serializing_if = "Option::is_none")]
    spent_tokens: Option<u64>,
    /// One row per configured tier, namespace-sorted. Omitted when unconfigured.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tiers: Vec<TierDto>,
    /// The plan-level **total token ceiling** (issue #188), when one is
    /// configured. Unlike the per-namespace `tiers` — a *soft* gate that only
    /// trims exec tools — crossing this is a *hard* stop: the harness refuses to
    /// dispatch further turns this period. Omitted when no ceiling is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<TotalDto>,
    /// Media generation (issue #109): whether this company **explicitly** grants
    /// the real-money `media` namespace (a `*` wildcard does NOT count). Sent
    /// regardless of whether a `[plan]` is configured, since media is opt-in per
    /// tool grant, not per plan.
    media_granted: bool,
    /// Whether the `media` feature is compiled into this build at all (the tools
    /// only exist under it). `false` lets the console show a "not in this build"
    /// state rather than implying a missing credential.
    media_in_build: bool,
    /// Whether a MANAGED media credential is resolvable from the environment on
    /// this build (feature on + env present). Never reflects a tenant secret.
    media_credential_configured: bool,
    /// Per-tenant Composio (issue #110): whether this company **explicitly**
    /// grants the `composio` namespace (a `*` wildcard does NOT count). Opt-in
    /// per tool grant, independent of a `[plan]`.
    composio_granted: bool,
    /// Whether the `composio` feature is compiled into this build at all.
    composio_in_build: bool,
    /// Chargebee billing (issue #788): whether this company **explicitly** grants
    /// the `chargebee` namespace (a `*` wildcard does NOT count). What the
    /// Settings UI reads to say whether billing tools would reach an agent even
    /// once credentials are saved.
    chargebee_granted: bool,
    /// Whether the `chargebee` feature is compiled into this build at all. The
    /// grant and the credentials can both be in place and still wire no tools if
    /// the running binary was not built with it.
    chargebee_in_build: bool,
    /// Whether a non-empty per-tenant Composio **BYO override** token is stored
    /// under `composio/tinyhumans/key` — never the token itself. Unlike media's env
    /// credential, this is a tenant secret.
    ///
    /// Deliberately narrow, and **not** the answer to "can this company reach
    /// Composio" (issue #886): the BYO slot is the first of three tiers, and on
    /// a hosted tenant the third one answers, so this reads `false` for a
    /// company whose Composio tools are wired and working. Read
    /// [`Self::composio_credential_source`] for the resolution verdict; this
    /// field is retained with its original meaning for the console surface that
    /// asks whether *this company pasted a token*.
    composio_token_configured: bool,
    /// Which tier this company's Composio credential actually resolves from
    /// (issue #886) — `attested` (the instance's platform identity), `company`
    /// (the company's own TinyHumans key), `static` (a pasted BYO token or a
    /// static instance key), or `none` (nothing resolves, so no tools are
    /// wired).
    ///
    /// Sourced from
    /// [`resolve_credential`](crate::company::composio::resolve_credential) —
    /// the same derivation the toolbelt gates on — rather than a second copy of
    /// its precedence, so the console can never name a tier the agents are not
    /// on. Matches the `credentialSource` field
    /// [`ops::composio`](crate::server::ops::composio) already reports.
    ///
    /// A **resolution** verdict, not a liveness one: `attested` says a bearer
    /// can be obtained, not that Composio answered or that any account is
    /// connected. `GET …/connections` is the axis that answers those.
    ///
    /// Omitted entirely when the secret store could not be read — an unknown
    /// answer is not `none`, and reporting a confident "no credential" for a
    /// transient store hiccup is the same class of lie #886 is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    composio_credential_source: Option<CredentialSource>,
    /// Metered web search (issue #238): whether this company **explicitly**
    /// grants the `search` namespace (a `*` wildcard does NOT count).
    search_granted: bool,
    /// Whether the harness that carries `web_search` is compiled into this
    /// build. There is no `search` Cargo feature — the tool rides the plain
    /// `openhuman` harness feature deliberately, so CI's gated lane compiles and
    /// tests it rather than a real-money surface shipping untested.
    search_in_build: bool,
    /// Whether a MANAGED search credential resolves for this company, either
    /// from its copied TinyHumans key or the deployment fallback. Omitted when
    /// the company tier cannot be read and no deployment fallback establishes
    /// a definitive `true` verdict.
    #[serde(skip_serializing_if = "Option::is_none")]
    search_credential_configured: Option<bool>,
    /// The effective managed Search owner: company first, then deployment.
    #[serde(skip_serializing_if = "Option::is_none")]
    search_credential_source: Option<CredentialSource>,
    /// The provider this company's searches actually go to — `managed`, or the
    /// slug it configured in Settings → Search and finished configuring.
    ///
    /// Reported beside the managed flag rather than folded into it because the
    /// two can disagree in both directions: a deployment with no platform
    /// credential still searches for a company that brought its own key, and a
    /// company that selected Exa and pasted nothing is still on managed. Never
    /// the key — only the slug, which is not a secret.
    search_provider: String,
    /// The company's daily `web_search` call ceiling
    /// (`[tools].search_daily_calls`, else the built-in default). Reaching it
    /// makes the tool refuse loudly rather than return an empty result set.
    search_daily_call_cap: u32,
    /// Publishing (issue #244, panel half #1192): whether this company's grants
    /// confer `publish_artifact` — the only way a file an agent wrote becomes a
    /// deliverable.
    ///
    /// **Unlike every `*_granted` field above, a bare `*` DOES confer this.**
    /// Publishing spends nothing and reaches nothing outside the company's own
    /// board, so it rides the ordinary namespace rule rather than the
    /// opt-in-by-name rule the real-money surfaces use. Sourced from
    /// [`grants_files_or_docs`](crate::company::grants_files_or_docs), which is
    /// the same predicate `build_agent`'s `wants_files` gate calls — one
    /// derivation, so this panel cannot report a capability the toolbelt does
    /// not wire.
    ///
    /// # There is deliberately no third rung
    ///
    /// Media, Composio and search each carry a credential/config flag beside
    /// their grant, because each can be granted and still wire nothing.
    /// Publishing has neither a credential nor a store toggle: the artifact
    /// store is non-optional on the runtime ops bundle and the single
    /// production `HarnessDeps` literal always sets it, so a
    /// `artifactStoreConfigured` field could only ever serialize a hardcoded
    /// `true` for every company on every deployment — a fresh instance of
    /// exactly the always-reassuring flag issue #886 was filed about. If the
    /// store ever becomes genuinely optional in production, the burden is on
    /// adding the field back with a real derivation behind it.
    publish_granted: bool,
    /// Whether the harness that carries `publish_artifact` is compiled into this
    /// build. There is no `publish` Cargo feature — the tool rides the plain
    /// `openhuman` harness feature, exactly as
    /// [`search_in_build`](Self::search_in_build) does, so do not invent one.
    publish_in_build: bool,
    /// Whether the agent-side MCP bridge is compiled into this build (issue
    /// #567). Unlike media/composio/search this is **not** a grant question: the
    /// `/mcp/servers` management routes ship in every build, so an operator can
    /// add a server, store a token and watch it probe healthy on a build that
    /// hands agents no MCP tool at all — `registry_for_agent` is pushed onto the
    /// belt behind `#[cfg(feature = "mcp")]`. The most misleading case is a
    /// build with `openhuman` but without `mcp`: live tool discovery and health
    /// probes answer for real (they ride the harness feature), so every read in
    /// the console looks correct while no agent can call the server. `false`
    /// lets the MCP surfaces state that plainly instead of the operator finding
    /// out by asking an agent and watching nothing happen.
    mcp_in_build: bool,
    /// Whether this company's teammates can actually think, and why not when
    /// they cannot (issue #1735).
    ///
    /// The wire labels are [`CognitionState`]'s own — read them there rather
    /// than from a list here. This comment carried a hand-copied list of three
    /// and was stale within two commits of the states growing to five, which is
    /// exactly the second copy of a fact that this field exists to avoid
    /// (CodeRabbit review of PR #1740).
    ///
    /// The one capability on this response that is **not** a build fact alone.
    /// `media_in_build` and its neighbours answer "was this compiled in";
    /// cognition is that question *and* "is a harness actually attached" *and*
    /// "did a model resolve at boot", and only the last is something an
    /// operator can fix without a new binary. A fifth boolean would have
    /// collapsed them, which is the same mistake as the echo reply it exists to
    /// explain — an operator told "not available" goes looking for a rebuild
    /// when a provider was one settings page away.
    ///
    /// Derived on every read from the brain the runtime is holding, never
    /// stored. See [`crate::server::cognition`].
    cognition: CognitionState,
}

/// One tier's budget row.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TierDto {
    /// The exec tool namespace this tier gates.
    namespace: String,
    /// Tokens allowed this period.
    budget_tokens: u64,
    /// Tokens spent this period (company-wide — spend has no per-tier
    /// attribution, so this is the same across rows).
    spent_tokens: u64,
    /// `budget - spent`, saturating at zero.
    remaining_tokens: u64,
    /// Whether spend has reached the threshold — the tier's tools are disabled.
    exhausted: bool,
}

/// The plan-level total token ceiling row (issue #188).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TotalDto {
    /// Total tokens allowed this period before dispatch is refused.
    budget_tokens: u64,
    /// Tokens spent this period (the same company-wide figure the tiers compare
    /// against).
    spent_tokens: u64,
    /// `budget - spent`, saturating at zero.
    remaining_tokens: u64,
    /// Whether spend has reached the ceiling — the harness refuses to dispatch
    /// further turns until the period resets.
    exhausted: bool,
}

/// The opt-in-capability status flags carried on every response (media +
/// composio), independent of whether a `[plan]` is configured.
struct OptInFlags {
    media_granted: bool,
    chargebee_granted: bool,
    composio_granted: bool,
    composio_token_configured: bool,
    /// The resolved Composio credential tier (issue #886), or `None` when it
    /// could not be determined. Travels on the flags rather than being computed
    /// per DTO site because the DTO is built in two places, and a field wired
    /// into one of them alone reports honestly for a company with no plan and
    /// lies to every company that has one — the failure the issue #567 test
    /// below exists to catch.
    composio_credential_source: Option<CredentialSource>,
    search_granted: bool,
    search_credential_configured: Option<bool>,
    search_credential_source: Option<CredentialSource>,
    search_daily_call_cap: u32,
    search_provider: String,
    /// Issue #1192. Carried on the flags rather than derived per DTO site for
    /// the reason the `composio_credential_source` note above already states:
    /// the DTO is built in two places, and a field wired into one of them alone
    /// reports honestly for a company with no plan and lies to every company
    /// that has one.
    publish_granted: bool,
    /// Issue #1735. Carried here for the reason the two notes above already
    /// give: the DTO is built in two places, and a field wired into one of them
    /// alone reports honestly for a company with no plan and lies to every
    /// company that has one — which for this field would mean chat rendering
    /// the echo brain's output as a teammate's reply on exactly the companies
    /// that have a budget configured.
    cognition: CognitionState,
}

impl OptInFlags {
    /// All-false — used when no company record is present.
    ///
    /// Takes the cognition state because that one is knowable without a record:
    /// the runtime is in hand either way, and which brain it holds does not
    /// depend on whether its company row loaded.
    fn none(cognition: CognitionState) -> Self {
        Self {
            cognition,
            media_granted: false,
            chargebee_granted: false,
            composio_granted: false,
            composio_token_configured: false,
            // `None` (undetermined), never `Some(CredentialSource::None)`:
            // there is no company record to resolve a credential for, which is
            // not the same answer as "no credential resolves".
            composio_credential_source: None,
            search_granted: false,
            search_credential_configured: Some(search_deployment_credential_configured()),
            search_credential_source: None,
            search_daily_call_cap: crate::company::DEFAULT_SEARCH_DAILY_CALLS,
            search_provider: crate::company::search::MANAGED_PROVIDER.to_string(),
            publish_granted: false,
        }
    }
}

/// The unconfigured response: `{ configured: false }` plus the opt-in-capability
/// flags (media + composio are opt-in per tool grant, independent of a `[plan]`).
fn unconfigured(flags: OptInFlags) -> CapabilityStatusDto {
    CapabilityStatusDto {
        configured: false,
        plan: None,
        period: None,
        period_start_millis: None,
        spent_tokens: None,
        tiers: Vec::new(),
        total: None,
        media_granted: flags.media_granted,
        media_in_build: cfg!(feature = "media"),
        media_credential_configured: media_credential_configured(),
        composio_granted: flags.composio_granted,
        composio_in_build: cfg!(feature = "composio"),
        chargebee_granted: flags.chargebee_granted,
        chargebee_in_build: cfg!(feature = "chargebee"),
        composio_token_configured: flags.composio_token_configured,
        composio_credential_source: flags.composio_credential_source,
        search_granted: flags.search_granted,
        search_in_build: cfg!(feature = "openhuman"),
        search_credential_configured: flags.search_credential_configured,
        search_credential_source: flags.search_credential_source,
        search_daily_call_cap: flags.search_daily_call_cap,
        search_provider: flags.search_provider.clone(),
        publish_granted: flags.publish_granted,
        publish_in_build: cfg!(feature = "openhuman"),
        mcp_in_build: cfg!(feature = "mcp"),
        cognition: flags.cognition,
    }
}

/// Which tier this company's Composio credential resolves from (issue #886), or
/// `None` when the secret store could not be read.
///
/// Asks
/// [`resolve_access`](crate::company::composio::resolve_access) rather
/// than restating its precedence. The three-tier managed resolution — BYO
/// `composio/tinyhumans/key`, then the company's own TinyHumans key, then this instance's
/// platform identity — and the BYOK route that bypasses all three, are the
/// *same* answer
/// [`TenantComposio::resolve`](crate::harness::composio::TenantComposio::resolve)
/// gates the toolbelt on, and the whole point of #886 is that this panel had a
/// second, one-tier copy of the question that disagreed with it. There must be
/// exactly one derivation, and this is not it — it is a caller of it.
///
/// Takes the instance identity **already resolved** rather than an `&dyn
/// EnvSource`, mirroring
/// [`ops::composio`](crate::server::ops::composio)'s `credential_source_for`: a
/// trait object with no `Send + Sync` bound held across the await below makes
/// the whole handler future non-`Send`, which axum rejects. Passing the resolved
/// value also keeps the tier matrix testable without mutating the process
/// environment.
///
/// A store error yields `None` and a warning, never `Some(CredentialSource::None)`.
/// The rest of `/capabilities` is budget and tier data with nothing to do with
/// Composio, so failing the whole response would be the wrong trade — but
/// answering "no credential" for a transient hiccup would send an operator to
/// paste a token they already have, which is the #886 failure in the other
/// direction. An omitted field is the only honest "we do not know".
async fn composio_credential_source(
    runtime: &CompanyRuntime,
    token_source: Option<std::sync::Arc<TinyhumansTokenSource>>,
) -> Option<CredentialSource> {
    match crate::company::composio::resolve_access(
        runtime.id(),
        runtime.secrets().as_ref(),
        token_source,
    )
    .await
    {
        Ok(access) => Some(access.credential.source()),
        Err(err) => {
            tracing::warn!(
                company = %runtime.id(),
                error = %err,
                "[capabilities] could not resolve the Composio credential tier; omitting \
                 `composioCredentialSource` rather than reporting a confident `none`"
            );
            None
        }
    }
}

/// Whether the deployment-level MANAGED search fallback is available.
fn search_deployment_credential_configured() -> bool {
    #[cfg(feature = "openhuman")]
    {
        use crate::app::config::ProcessEnv;
        crate::harness::provider::search_backend_from_env(&ProcessEnv).is_some()
    }
    #[cfg(not(feature = "openhuman"))]
    {
        false
    }
}

/// Whether MANAGED search can resolve for this company through the same two
/// tiers as the request path: its copied TinyHumans key first, then the
/// deployment credential. Off the harness feature there is no search tool.
async fn search_credential_source(runtime: &CompanyRuntime) -> Option<CredentialSource> {
    #[cfg(feature = "openhuman")]
    {
        match crate::company::search::load_managed_key(runtime.id(), runtime.secrets().as_ref())
            .await
        {
            Ok(Some(_)) => Some(CredentialSource::Company),
            Ok(None) => Some(
                crate::company::TinyhumansTokenSource::from_env(&crate::app::config::ProcessEnv)
                    .map(|source| source.credential_source())
                    .unwrap_or(CredentialSource::None),
            ),
            Err(err) => {
                tracing::warn!(
                    company = %runtime.id(),
                    error = %err,
                    "[capabilities] could not resolve the managed Search credential tier; omitting its source and configured verdict"
                );
                None
            }
        }
    }
    #[cfg(not(feature = "openhuman"))]
    {
        let _ = runtime;
        Some(CredentialSource::None)
    }
}

/// Whether a MANAGED media credential (issue #109) is resolvable from the
/// environment on this build. `true` only under the `media` feature with a
/// credential present — env-only, never a tenant secret, matching the harness's
/// fail-closed resolution. Off the feature this is always `false`.
fn media_credential_configured() -> bool {
    #[cfg(feature = "media")]
    {
        use crate::app::config::ProcessEnv;
        crate::harness::provider::media_backend_from_env(&ProcessEnv).is_some()
    }
    #[cfg(not(feature = "media"))]
    {
        false
    }
}

/// Reads this company's cognition state off the runtime it actually holds.
///
/// The third input is only consulted on the one degraded path, and that is
/// deliberate rather than incidental: `/capabilities` is a console read that
/// gets polled, and `inference_resolution` costs a manifest load plus a
/// secret-store resolve. A company that is thinking pays neither, because its
/// brain answers the question before either is needed. The placeholder passed
/// in the other arm is never read — `cognition_state` has already returned by
/// then, and its ordering is asserted.
async fn cognition_for(runtime: &CompanyRuntime) -> CognitionState {
    let path = runtime.cognition().path;
    let harness_reachable = crate::server::ops::inference::harness_reachable(runtime);
    // Short-circuit: with a real brain, or with no harness at all, the config
    // read cannot change the answer — see `cognition_state`'s own ordering.
    let resolution = if path == crate::ports::brain::ECHO_PATH && harness_reachable {
        crate::server::ops::inference::inference_resolution(runtime).await
    } else {
        InferenceResolution::Nothing
    };
    cognition_state(path, harness_reachable, resolution)
}

/// Resolves the capability-budget status DTO for a company.
async fn effective_status(runtime: &CompanyRuntime) -> Result<CapabilityStatusDto, ApiError> {
    // Issue #1735. Read off the brain this runtime is actually holding, before
    // anything else can fail: a company whose record will not load still has a
    // brain, and "can a teammate answer me" is the one question on this
    // response that must never degrade to a reassuring default.
    //
    // Reachability, not `cfg!(feature = "openhuman")`: the feature says the
    // harness was compiled in, not that this runtime was handed a pool, and an
    // embedder that skips `app::harness::attach` gets exactly that (the shipped
    // desktop-shell bug that module exists to end). Reporting `unconfigured`
    // there would point the operator at Settings → Inference, which cannot move
    // that runtime off the echo brain — the dead end `ops::inference`'s own
    // `restart_pending`/`runner_gap_for` already gate on this same predicate to
    // avoid (issues #266, #514). Borrowing that function rather than re-deriving
    // it is what keeps the two surfaces from disagreeing about one company.
    let cognition = cognition_for(runtime).await;
    let record = runtime.store().load(runtime.id()).await.map_err(ApiError)?;
    let Some(record) = record else {
        return Ok(unconfigured(OptInFlags::none(cognition)));
    };
    // Media + composio are opt-in per tool grant (explicit namespace, never `*`)
    // and live on the manifest regardless of whether a `[plan]` is configured.
    let search_credential_source = search_credential_source(runtime).await;
    let flags = OptInFlags {
        cognition,
        media_granted: crate::company::grants_media_explicit(&record.manifest.tools.allow),
        chargebee_granted: crate::company::grants_chargebee_explicit(&record.manifest.tools.allow),
        composio_granted: crate::company::grants_composio_explicit(&record.manifest.tools.allow),
        // Degrade to "unconfigured" on a transient secret-store error rather
        // than failing the whole /capabilities response (budget/tier data is
        // unrelated to Composio). Mirrors other opt-in-credential probes.
        composio_token_configured: crate::company::composio::token_configured(
            runtime.id(),
            runtime.secrets().as_ref(),
        )
        .await
        .unwrap_or(false),
        // Issue #886: the field above answers only whether a BYO token was
        // pasted, which on a hosted tenant is `false` for a company whose
        // Composio tools work. This one asks the resolver what the toolbelt
        // will actually present.
        //
        // The instance identity is read straight from the process environment
        // here, as `ops::composio` and `ops::company_key` already do. It would
        // be better held once on `CompanyRuntime` — this is the fourth
        // `from_env` call site on a console read path — but inventing that
        // accessor is a wider change than this fix, so it is left as a
        // follow-up rather than half-done here.
        composio_credential_source: composio_credential_source(
            runtime,
            TinyhumansTokenSource::from_env(&crate::app::config::ProcessEnv)
                .map(std::sync::Arc::new),
        )
        .await,
        // Issue #238: search is opt-in per tool grant like media/composio, and
        // its daily cap lives on `[tools]` rather than `[plan]` — a call
        // ceiling, not a token budget — so both travel with the plan-independent
        // flags.
        search_granted: crate::company::grants_search_explicit(&record.manifest.tools.allow),
        search_credential_source,
        search_credential_configured: search_credential_source
            .map(|source| source != CredentialSource::None),
        search_daily_call_cap: record
            .manifest
            .tools
            .search_daily_calls
            .unwrap_or(crate::company::DEFAULT_SEARCH_DAILY_CALLS),
        // Which provider the company's own settings point at. Degrades to the
        // managed answer on a transient secret-store error rather than failing
        // the whole /capabilities response, like the Composio probe above — the
        // panel's other cards are unrelated to search.
        search_provider: crate::company::search::resolve_effective_provider(
            runtime.id(),
            runtime.secrets().as_ref(),
        )
        .await
        .unwrap_or_else(|_| crate::company::search::MANAGED_PROVIDER.to_string()),
        // Issue #245: opt-in per tool grant like the three above, and read from
        // the same manifest field, so the repositories card can tell an operator
        // which half of the setup is missing.
        // Issue #1192: the same predicate `build_agent`'s `wants_files` gate
        // calls, so the panel's verdict and the wired toolbelt cannot disagree.
        // Note the shape difference from its four neighbours above — this one is
        // NOT `_explicit`, because a bare `*` confers publishing.
        publish_granted: crate::company::grants_files_or_docs(&record.manifest.tools.allow),
    };
    let manifest_plan = &record.manifest.plan;
    let Some(plan) = CapabilityPlan::from_manifest(manifest_plan) else {
        return Ok(unconfigured(flags));
    };

    let now = now_millis();
    let since = plan.period.period_start_millis(now);
    let samples = runtime
        .usage()
        .query(runtime.id(), since)
        .await
        .map_err(ApiError)?;
    let spent = tokens_in(&samples);

    let tiers = plan
        .status(spent)
        .into_iter()
        .map(|tier| TierDto {
            namespace: tier.namespace,
            budget_tokens: tier.budget,
            spent_tokens: tier.spent,
            remaining_tokens: tier.remaining,
            exhausted: tier.exhausted,
        })
        .collect();

    // The plan-level total ceiling (issue #188): present only when the manifest
    // set `[plan].total_tokens`. This is the hard gate the harness enforces by
    // refusing dispatch — the console renders it alongside the soft per-namespace
    // tiers.
    let total = plan.total_status(spent).map(|status| TotalDto {
        budget_tokens: status.budget,
        spent_tokens: status.spent,
        remaining_tokens: status.remaining,
        exhausted: status.exhausted,
    });

    Ok(CapabilityStatusDto {
        configured: true,
        plan: manifest_plan.name.clone(),
        period: Some(plan.period.as_str().to_string()),
        period_start_millis: Some(since),
        spent_tokens: Some(spent),
        tiers,
        total,
        media_granted: flags.media_granted,
        media_in_build: cfg!(feature = "media"),
        media_credential_configured: media_credential_configured(),
        composio_granted: flags.composio_granted,
        composio_in_build: cfg!(feature = "composio"),
        chargebee_granted: flags.chargebee_granted,
        chargebee_in_build: cfg!(feature = "chargebee"),
        composio_token_configured: flags.composio_token_configured,
        composio_credential_source: flags.composio_credential_source,
        search_granted: flags.search_granted,
        search_in_build: cfg!(feature = "openhuman"),
        search_credential_configured: flags.search_credential_configured,
        search_credential_source: flags.search_credential_source,
        search_daily_call_cap: flags.search_daily_call_cap,
        search_provider: flags.search_provider.clone(),
        publish_granted: flags.publish_granted,
        publish_in_build: cfg!(feature = "openhuman"),
        mcp_in_build: cfg!(feature = "mcp"),
        cognition: flags.cognition,
    })
}

/// `GET …/capabilities` — the company's capability-budget status.
async fn get_status(company: ScopedCompany) -> Result<Json<CapabilityStatusDto>, ApiError> {
    Ok(Json(effective_status(company.runtime.as_ref()).await?))
}

#[cfg(test)]
#[path = "capabilities_a_company_on_the_tests.rs"]
mod tests_a_company_on_the;
#[cfg(feature = "openhuman")]
#[cfg(test)]
#[path = "capabilities_managed_search_tests.rs"]
mod tests_managed_search;
#[cfg(test)]
#[path = "capabilities_the_composio_verdict_walks_tests.rs"]
mod tests_the_composio_verdict_walks;
