//! Per-tenant Composio credential + backend routing (issue #110, epic #26 Cell
//! D). Always compiled (so the console read/write plane can manage the token
//! even in the default build); the live agent tools that consume it live in the
//! feature-gated [`harness::composio`](crate::harness::composio).
//!
//! The per-tenant OAuth bearer token is **write-only**: it is set through the
//! console `PUT …/composio/token` route, stored under [`TINYHUMANS_KEY_KEY`],
//! and never returned. The read shape carries only a `tokenConfigured`
//! boolean. The token has **no environment fallback** — a missing token
//! means no tools (fail closed), never a borrowed identity. Only the backend
//! URL may be overridden from the environment.

use std::sync::Arc;

use serde::Serialize;

use crate::Result;
use crate::company::company_key;
use crate::company::credentials::{Credential, TinyhumansTokenSource};
use crate::ports::SecretStore;
use crate::ports::types::{CompanyId, SecretValue};

/// The TinyHumans bearer this company's **managed** Composio calls present — a
/// credential the *TinyHumans backend* recognises. Written by
/// `PUT …/composio/token` through [`store_token`]; read through
/// [`load_tinyhumans_key`]. Write-only over the API, never echoed.
pub const TINYHUMANS_KEY_KEY: &str = "composio/tinyhumans/key";

/// Pre-rename address of [`TINYHUMANS_KEY_KEY`] (issue #2306): its read fallback
/// only, never [`BYOK_KEY_KEY`]'s. For one release every write to that key also
/// stores the same value here, so a rolled-back binary keeps working.
///
/// DEPRECATED(keys-rework #2306): the pre-rename Composio token address;
/// replaced by [`TINYHUMANS_KEY_KEY`]; removable when the release after
/// #2306 stops the mirror write in [`write_both`]. Not `#[deprecated]`: the
/// live mirror in [`write_both`] still writes this address on every save, and
/// `clippy -D warnings` would fail on that usage.
pub const LEGACY_TOKEN_KEY: &str = "composio/token";

/// Reads `key`, falling back to `legacy_key` only when `key` holds nothing.
///
/// "Holds nothing" is *absent or blank after trim*. The value returned is the
/// stored string exactly as stored (not trimmed) — callers keep their own
/// handling. Reads never write: nothing is migrated on read.
///
/// A read error on either address **propagates** (see [`resolve_credential`]:
/// an unreadable store must not change which account a call is attributed to).
/// The legacy address is not read at all when `key` holds a value.
///
/// Private on purpose: the only callers are [`load_tinyhumans_key`] and
/// [`load_byok_key`], which fix the mapping so the two pairs can never be
/// crossed.
async fn read_with_legacy(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    key: &'static str,
    legacy_key: &'static str,
) -> Result<Option<String>> {
    if let Some(SecretValue(value)) = secrets.get(company, key).await?
        && !value.trim().is_empty()
    {
        return Ok(Some(value));
    }
    Ok(secrets
        .get(company, legacy_key)
        .await?
        .map(|SecretValue(value)| value)
        .filter(|value| !value.trim().is_empty()))
}

/// Writes `value` to `key`, then the same `value` to `legacy_key`. `value` may
/// be `""`: a clear clears both.
///
/// The legacy write exists for **one release** (issue #2306): a rolled-back
/// binary reads only `legacy_key`, and must find what this binary stored. A
/// later release stops mirroring; that is a follow-up, not part of #2306.
///
/// Order is fixed: new address first, legacy second, so a failure between the
/// two leaves the new value in place and winning on read. The legacy write is
/// unconditional — no read first, so no check-then-write window. A failed
/// legacy write is logged (key names only, never a value) and **propagated**,
/// as `search::store::store_provider_key` does, so the caller reports a failed
/// write and an admin can retry.
async fn write_both(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    key: &'static str,
    legacy_key: &'static str,
    value: &str,
) -> Result<()> {
    secrets
        .set(company, key, SecretValue(value.to_string()))
        .await?;
    // DEPRECATED(keys-rework #2306): this write is the compatibility mirror
    // to the pre-rename address (`legacy_key`); replaced by the write above
    // to `key`, the new address; removable when the release after #2306
    // stops mirroring. Not `#[deprecated]`: this write is live and required
    // on every save until then, and `clippy -D warnings` would fail on that
    // usage.
    if let Err(err) = secrets
        .set(company, legacy_key, SecretValue(value.to_string()))
        .await
    {
        tracing::error!(
            company = %company,
            key = key,
            legacy_key = legacy_key,
            "[composio] wrote the credential's new address but not its legacy address; \
             reporting the write as failed so it can be retried: {err}"
        );
        return Err(err);
    }
    Ok(())
}

/// The stored managed-Composio TinyHumans bearer: [`TINYHUMANS_KEY_KEY`], else
/// [`LEGACY_TOKEN_KEY`]. `None` when both are absent or blank. Never reads
/// [`BYOK_KEY_KEY`] or [`LEGACY_API_KEY_KEY`].
pub async fn load_tinyhumans_key(
    company: &CompanyId,
    secrets: &dyn SecretStore,
) -> Result<Option<String>> {
    read_with_legacy(company, secrets, TINYHUMANS_KEY_KEY, LEGACY_TOKEN_KEY).await
}

/// The stored BYOK Composio API key: [`BYOK_KEY_KEY`], else
/// [`LEGACY_API_KEY_KEY`]. `None` when both are absent or blank. Never reads
/// [`TINYHUMANS_KEY_KEY`] or [`LEGACY_TOKEN_KEY`].
pub async fn load_byok_key(
    company: &CompanyId,
    secrets: &dyn SecretStore,
) -> Result<Option<String>> {
    read_with_legacy(company, secrets, BYOK_KEY_KEY, LEGACY_API_KEY_KEY).await
}

/// The tenant's shared TinyHumans API base URL (the same backend inference and
/// the rest of the app already use). Used as the Composio backend fallback when
/// `api_url` is unset, so a staging tenant's Composio calls go to staging
/// instead of the hardcoded prod default.
pub const TINYHUMANS_API_URL_ENV: &str = "TINYHUMANS_API_URL";

/// Default backend base URL for the Composio routes when the tenant API base
/// is not set. Mirrors the media backend's default host (prod).
pub const DEFAULT_BACKEND_URL: &str = "https://api.tinyhumans.ai";

/// The effective Composio backend URL: [`TINYHUMANS_API_URL_ENV`] (the
/// tenant's shared backend base) if set, else [`DEFAULT_BACKEND_URL`]. The
/// explicit per-surface override (`OPENCOMPANY_COMPOSIO_BACKEND_URL`) was
/// removed in phase 6a (issue #2306): Composio now always follows the
/// tenant's shared API base, the same way media and search already do.
pub fn backend_url_or_default(api_url: Option<String>) -> String {
    api_url
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| DEFAULT_BACKEND_URL.to_string())
}

/// Store (or rotate/clear) the per-tenant Composio token. A non-empty value
/// rotates it; an empty string clears it. Write-only — the value is never read
/// back over the API.
pub async fn store_token(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    token: &str,
) -> Result<()> {
    write_both(
        company,
        secrets,
        TINYHUMANS_KEY_KEY,
        LEGACY_TOKEN_KEY,
        token.trim(),
    )
    .await
}

/// The credential this company's Composio calls present, or [`Credential::None`]
/// when none can be obtained at all.
///
/// The **one** derivation of that answer, and the reason it lives here rather
/// than in the feature-gated harness: both callers need it in every build.
/// [`TenantComposio::resolve`](crate::harness::composio::TenantComposio::resolve)
/// builds the agent-facing config from it, and the console status route
/// ([`ops::composio`](crate::server::ops::composio)) reports its
/// [`source`](Credential::source) — so the tier the console shows an operator
/// cannot disagree with the identity the agents actually present. Two functions
/// that merely *mirrored* each other's precedence would drift the first time a
/// tier was added to one of them, which is exactly the failure issue #586 exists
/// to remove.
///
/// Precedence: the company's own Composio token ([`TINYHUMANS_KEY_KEY`], the BYO
/// escape hatch) wins; otherwise the shared brokered-credential seam
/// [`company_key::resolve`] answers — the company's own TinyHumans key, else this
/// instance's platform identity, else nothing.
///
/// A store read error **propagates** rather than degrading to the next tier —
/// see [`company_key::resolve`] for why an unreadable store must not silently
/// change which account a call is attributed to.
pub async fn resolve_credential(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    token_source: Option<Arc<TinyhumansTokenSource>>,
) -> Result<Credential> {
    let byo = match load_tinyhumans_key(company, secrets).await? {
        Some(token) => Credential::from_value(token),
        None => Credential::None,
    };
    Ok(match byo {
        // The company's own Composio token always wins.
        byo @ Credential::Value(_) => byo,
        // Everything else is the shared seam's answer, so a rotated company key
        // reaches Composio the same cycle it reaches every other brokered
        // surface.
        _ => company_key::resolve(company, secrets, token_source).await?,
    })
}

/// Whether a non-empty **BYO override** token is stored under
/// [`TINYHUMANS_KEY_KEY`] — never the token itself.
///
/// ## This is not "can this company reach Composio" (issue #886)
///
/// It answers exactly one question about exactly one secret slot: did somebody
/// paste a token into the company's own [`TINYHUMANS_KEY_KEY`]. That is the *first* tier
/// of three. [`resolve_credential`] falls through it to the company's own
/// TinyHumans key and then to this instance's platform identity, and on a hosted
/// tenant it is the third tier that answers — nobody pastes a BYO token there.
/// So `false` from here is routinely true of a company whose Composio tools are
/// wired and working, which is precisely what #886 was filed about: the
/// capabilities panel reported `composioTokenConfigured: false` while agents
/// were calling `GITHUB_*` tools successfully in the same session.
///
/// **If you want to know whether Composio will work, call
/// [`resolve_credential`] and ask the returned [`Credential`] — `configured()`
/// for the boolean, [`source`](Credential::source) for the tier.** That is the
/// same derivation the toolbelt gates on
/// ([`TenantComposio::resolve`](crate::harness::composio::TenantComposio::resolve)),
/// so it cannot disagree with what the agents actually hold. Use this function
/// only where the BYO slot itself is the subject — a console field that says
/// whether *this company pasted a token*, not whether it has one.
pub async fn token_configured(company: &CompanyId, secrets: &dyn SecretStore) -> Result<bool> {
    Ok(load_tinyhumans_key(company, secrets).await?.is_some())
}

// ── Routing mode: OpenHuman-managed, or the company's own Composio account ──
//
// Everything above this line is the **managed** route: calls go to the
// OpenHuman/TinyHumans backend (`/agent-integrations/composio/*`), which owns
// the Composio API key, the billing margin and the server-enforced toolkit
// allowlist, and derives the Composio entity from the bearer it is handed.
//
// A company that holds its own Composio account can route around all of that.
// In BYOK mode the harness talks to `backend.composio.dev` directly with that
// company's `x-api-key` — nothing is proxied, nothing is billed here, and the
// providers it can connect are whatever its own Composio dashboard permits.
// This mirrors OpenHuman's own `backend` / `direct` split
// (`vendor/openhuman/crates/openhuman-core/src/integrations/composio/client.rs::create_composio_client`);
// the vocabulary here is `managed` / `byok` to match the search surface next
// door (`crate::company::search`), which made the same choice first.

/// The [`SecretStore`] key holding this company's Composio routing mode — one
/// of [`MANAGED_MODE`] or [`BYOK_MODE`].
///
/// Stored rather than inferred from "is there an API key", for the reason
/// [`crate::company::search::PROVIDER_SECRET`] is stored: the mode decides
/// which *API* the credential is presented to, and a credential sent to the
/// wrong one fails in a way that reads like a bad credential. It also lets the
/// console report the mode without reading a secret slot at all.
pub const MODE_KEY: &str = "composio/mode";

/// This company's **own** Composio API key (`ak_…`) — a credential *Composio*
/// recognises. Written by `PUT …/composio/api-key` through [`store_api_key`];
/// read through [`load_byok_key`]. Write-only over the API, never echoed.
///
/// Distinct from [`TINYHUMANS_KEY_KEY`], and not interchangeable with it: they
/// authenticate different hosts.
pub const BYOK_KEY_KEY: &str = "composio/byok/key";

/// Pre-rename address of [`BYOK_KEY_KEY`] (issue #2306): its read fallback only,
/// never [`TINYHUMANS_KEY_KEY`]'s. For one release every write to that key also
/// stores the same value here, so a rolled-back binary keeps working.
///
/// DEPRECATED(keys-rework #2306): the pre-rename Composio BYOK-key address;
/// replaced by [`BYOK_KEY_KEY`]; removable when the release after #2306
/// stops the mirror write in [`write_both`]. Not `#[deprecated]`: the live
/// mirror in [`write_both`] still writes this address on every save, and
/// `clippy -D warnings` would fail on that usage.
pub const LEGACY_API_KEY_KEY: &str = "composio/api_key";

/// Storage + wire spelling of [`ComposioMode::Managed`].
pub const MANAGED_MODE: &str = "managed";

/// Storage + wire spelling of [`ComposioMode::Byok`].
pub const BYOK_MODE: &str = "byok";

/// The Composio API host a BYOK company's calls go to directly. Non-secret, and
/// reported by the console in place of the backend URL so an operator can see
/// that the route really did change.
pub const DIRECT_BASE_URL: &str = "https://backend.composio.dev";

/// The Composio entity a BYOK company's authorizations and executes are scoped
/// to.
///
/// `default` on purpose, matching OpenHuman's own direct mode
/// (`config.composio.entity_id`) and matching what a user sees in their own
/// Composio dashboard. Scoping to the company id instead would isolate two
/// OpenCompany companies sharing one key — but it would also hide every
/// connection the operator already made in that account, which is the first
/// thing a BYOK operator looks for. The shared-account caveat is the same one
/// [`TINYHUMANS_KEY_KEY`] already carries: two companies pasting one credential
/// share one entity, and that cannot be prevented from this side.
pub const DIRECT_ENTITY_ID: &str = "default";

/// How this company reaches Composio.
///
/// Not a [`CredentialSource`](crate::company::credentials::CredentialSource):
/// that names *whose identity* a call presents, this names *which host* it is
/// presented to. A BYOK company is `Static`-sourced and `Byok`-routed; a
/// company that pasted a [`TINYHUMANS_KEY_KEY`] override is `Static`-sourced and
/// `Managed`-routed. Collapsing the two would make either question
/// unanswerable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ComposioMode {
    /// Proxied through the OpenHuman backend — the default, and the only route
    /// that needs no configuration at all.
    #[default]
    Managed,
    /// Straight to `backend.composio.dev` with the company's own API key.
    Byok,
}

impl ComposioMode {
    /// The stable wire spelling (`managed` / `byok`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Managed => MANAGED_MODE,
            Self::Byok => BYOK_MODE,
        }
    }

    /// Whether `raw` names BYOK. Everything else — including an empty slot, a
    /// typo, and a mode from some future shape — reads as [`Self::Managed`].
    ///
    /// Unknown-means-managed rather than unknown-is-an-error because this is
    /// read on the roster path: a hand-edited slot must not be able to take a
    /// company's Composio tools away, and managed is the route that works
    /// without anything being stored.
    ///
    /// ## Why `direct` is accepted too
    ///
    /// Three repos in this org spell this split three ways: OpenHuman says
    /// `direct` / `backend`, TinyMemory says `direct` / `proxied`, and this one
    /// says `byok` / `managed`. Only [`BYOK_MODE`] is ever *written* here, so
    /// the alias is not load-bearing today — it is there because of what the
    /// fallback above would otherwise do to the one spelling somebody is most
    /// likely to reach for.
    ///
    /// `direct` falling through to managed would be silent and wrong in the
    /// specific way this whole surface refuses: a company that asked to act
    /// through its own Composio account would act through the platform's
    /// instead. (It would not *leak* the key — managed resolution reads
    /// [`TINYHUMANS_KEY_KEY`] and the company key, never [`BYOK_KEY_KEY`], so the
    /// stored Composio key would simply go unread — but the routing surprise is the
    /// part that matters.) Nothing is gained by making the org's own other
    /// spelling of "the company's own account" mean its opposite here.
    ///
    /// The managed spellings need no aliases: `backend` and `proxied` already
    /// land on [`Self::Managed`] through the fallback, which is what they mean.
    pub fn parse(raw: &str) -> Self {
        let raw = raw.trim();
        if raw.eq_ignore_ascii_case(BYOK_MODE) || raw.eq_ignore_ascii_case("direct") {
            Self::Byok
        } else {
            Self::Managed
        }
    }

    /// Whether this mode talks to Composio directly.
    pub fn is_byok(self) -> bool {
        matches!(self, Self::Byok)
    }
}

impl std::fmt::Display for ComposioMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// This company's stored routing mode, or [`ComposioMode::Managed`].
pub async fn load_mode(company: &CompanyId, secrets: &dyn SecretStore) -> Result<ComposioMode> {
    Ok(secrets
        .get(company, MODE_KEY)
        .await?
        .map(|SecretValue(raw)| ComposioMode::parse(&raw))
        .unwrap_or_default())
}

/// Store (or rotate/clear) this company's own Composio API key **and the mode
/// that goes with it**, returning the resulting mode.
///
/// The two are written together on purpose. A mode without a key is a company
/// with no Composio tools (see [`resolve_access`], which fails closed rather
/// than borrowing the platform identity), and a key without a mode is a
/// credential nothing reads — both are states an operator can reach only if
/// this function lets them. A non-empty value therefore sets the key and
/// selects [`ComposioMode::Byok`]; an empty one clears the key and returns the
/// company to [`ComposioMode::Managed`].
///
/// The writes are ordered by **direction**, not fixed key-then-mode: whichever
/// order leaves a failed second write inert, rather than in the outage this
/// whole function exists to rule out.
///
/// Selecting BYOK writes the key first. If the mode write then fails, the
/// company is still `Managed` holding an unread key — inert, since managed
/// resolution never looks at [`BYOK_KEY_KEY`].
///
/// Clearing writes the mode first. A fixed key-then-mode order would write the
/// *empty* key first here — and if the mode write then failed, the company
/// would stay `Byok` (its old mode, unwritten) with an empty key, which
/// [`resolve_access`] resolves to [`Credential::None`]: the exact "BYOK mode,
/// no key" outage the key-first rule above exists to avoid, reached from the
/// other direction. Writing the mode first instead leaves a failed second
/// write as `Managed` holding a stale-but-present key — inert, for the same
/// reason as the set direction.
pub async fn store_api_key(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    api_key: &str,
) -> Result<ComposioMode> {
    let api_key = api_key.trim();
    let mode = if api_key.is_empty() {
        ComposioMode::Managed
    } else {
        ComposioMode::Byok
    };
    if mode.is_byok() {
        // 1. BYOK_KEY_KEY = key   2. LEGACY_API_KEY_KEY = key   3. MODE_KEY = "byok"
        // The mode flip is LAST: until it lands the company is still on its old
        // route, so a failure at 1 or 2 leaves a managed company managed.
        write_both(company, secrets, BYOK_KEY_KEY, LEGACY_API_KEY_KEY, api_key).await?;
        secrets
            .set(company, MODE_KEY, SecretValue(mode.as_str().to_string()))
            .await?;
    } else {
        // 1. MODE_KEY = "managed"   2. BYOK_KEY_KEY = ""   3. LEGACY_API_KEY_KEY = ""
        // The mode flip is FIRST: a failure at 2 or 3 leaves a managed company
        // holding a stale BYOK key, which managed resolution never reads.
        secrets
            .set(company, MODE_KEY, SecretValue(mode.as_str().to_string()))
            .await?;
        write_both(company, secrets, BYOK_KEY_KEY, LEGACY_API_KEY_KEY, "").await?;
    }
    Ok(mode)
}

/// How a company reaches Composio: the route, and the credential that route
/// presents.
///
/// Returned as a pair rather than resolved twice because the two answers must
/// agree — a console reporting BYOK while the agents present a platform bearer
/// is the exact drift [`resolve_credential`]'s own docs exist to prevent.
pub struct ComposioAccess {
    /// Which host the calls go to.
    pub mode: ComposioMode,
    /// What they authenticate with: a Composio API key under
    /// [`ComposioMode::Byok`], otherwise whatever [`resolve_credential`]
    /// resolves.
    pub credential: Credential,
}

impl ComposioAccess {
    /// The non-secret endpoint this access reaches — [`DIRECT_BASE_URL`] for
    /// BYOK, the resolved backend URL otherwise. Safe to surface on the console
    /// read plane.
    pub fn endpoint(&self, backend_url: &str) -> String {
        match self.mode {
            ComposioMode::Byok => DIRECT_BASE_URL.to_string(),
            ComposioMode::Managed => backend_url.to_string(),
        }
    }
}

/// The route and credential this company's Composio calls use.
///
/// **Managed** defers wholly to [`resolve_credential`] — the BYO backend token,
/// then the company's TinyHumans key, then the instance identity — so nothing
/// about the default path changes by adding this.
///
/// **BYOK** reads [`BYOK_KEY_KEY`] (falling back to [`LEGACY_API_KEY_KEY`]) and
/// nothing else. It deliberately does *not* fall back to the managed tiers when
/// the key is missing or blank: a company that asked to act through its own
/// Composio account and silently acted through the platform's instead would
/// connect providers into the wrong tenant
/// and bill the wrong party. [`Credential::None`] here means no tools this
/// cycle, which is the same fail-closed answer an absent managed credential
/// gets.
///
/// A store read error **propagates**, for the reason it propagates in
/// [`resolve_credential`]: an unreadable store must not be able to change which
/// account a call is attributed to.
pub async fn resolve_access(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    token_source: Option<Arc<TinyhumansTokenSource>>,
) -> Result<ComposioAccess> {
    let mode = load_mode(company, secrets).await?;
    let credential = match mode {
        ComposioMode::Managed => resolve_credential(company, secrets, token_source).await?,
        ComposioMode::Byok => match load_byok_key(company, secrets).await? {
            Some(key) => Credential::from_value(key),
            None => Credential::None,
        },
    };
    if mode.is_byok() && !credential.configured() {
        tracing::warn!(
            company = %company,
            "[composio] this company is in BYOK mode with no Composio API key stored; \
             withholding tools rather than presenting the platform identity"
        );
    }
    Ok(ComposioAccess { mode, credential })
}

/// The [`SecretStore`] key holding this company's per-toolkit default
/// connections — a JSON object `{"gmail": "ca_123"}` written by the console
/// (issue #820).
///
/// Stored the way `inference/config` is: one small JSON blob per company,
/// alongside the credential it qualifies, rather than a new port. It is a
/// *preference*, not a secret — the ids in it are already handed to the console
/// by `GET …/composio/connections`, and are useless without the bearer that
/// scopes them. It lives in the secret store because that is the one per-company
/// key/value plane this repo has, and because keeping it beside
/// [`TINYHUMANS_KEY_KEY`] means a company's Composio state moves, backs up and
/// is deleted as one thing.
pub const DEFAULTS_KEY: &str = "composio/defaults";

/// This company's chosen connection per toolkit: `gmail` → a Composio connection
/// id (issue #820).
///
/// Absent for a toolkit means **no company has expressed an intent**, and the
/// execute path then sends no connection id at all, leaving the resolution to
/// Composio exactly as before. That absence is the ordinary case and is not a
/// degraded one — one account per toolkit needs no choice — so nothing here
/// invents a default from the connection list. A default that the product does
/// not actually make would be a claim the harness could not honour, which is the
/// failure #820 was filed about.
pub type ComposioDefaults = std::collections::BTreeMap<String, String>;

/// This company's stored per-toolkit defaults, or an empty map.
///
/// A blob that will not parse is treated as *no defaults* rather than an error:
/// the only writer is [`set_default`] / [`clear_default`], so unparseable means
/// hand-edited or from a future shape, and the honest response on the agent path
/// is to fall back to Composio's own resolution rather than to withhold the
/// tools. It is logged, not swallowed silently.
pub async fn load_defaults(
    company: &CompanyId,
    secrets: &dyn SecretStore,
) -> Result<ComposioDefaults> {
    let Some(SecretValue(raw)) = secrets.get(company, DEFAULTS_KEY).await? else {
        return Ok(ComposioDefaults::new());
    };
    if raw.trim().is_empty() {
        return Ok(ComposioDefaults::new());
    }
    match serde_json::from_str::<ComposioDefaults>(&raw) {
        Ok(defaults) => Ok(defaults
            .into_iter()
            .map(|(toolkit, id)| (toolkit.trim().to_ascii_lowercase(), id.trim().to_string()))
            .filter(|(toolkit, id)| !toolkit.is_empty() && !id.is_empty())
            .collect()),
        Err(err) => {
            tracing::warn!(
                company = %company,
                error = %err,
                "[composio] stored connection defaults did not parse; treating this company as \
                 having expressed no preference"
            );
            Ok(ComposioDefaults::new())
        }
    }
}

/// Pin `toolkit` to `connection_id`, replacing whatever it named before, and
/// return the resulting map.
///
/// The caller is responsible for checking that the id names a connection this
/// company actually holds — see
/// [`set_default_connection`](crate::harness::composio::set_default_connection),
/// which is the only path the console reaches this through. Storing an id blind
/// would let a typo silently redirect every send for a toolkit to nothing.
pub async fn set_default(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    toolkit: &str,
    connection_id: &str,
) -> Result<ComposioDefaults> {
    let mut defaults = load_defaults(company, secrets).await?;
    defaults.insert(
        toolkit.trim().to_ascii_lowercase(),
        connection_id.trim().to_string(),
    );
    save_defaults(company, secrets, &defaults).await?;
    Ok(defaults)
}

/// Drop `toolkit`'s pin — back to letting Composio resolve the account — and
/// return the resulting map.
pub async fn clear_default(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    toolkit: &str,
) -> Result<ComposioDefaults> {
    let mut defaults = load_defaults(company, secrets).await?;
    defaults.remove(&toolkit.trim().to_ascii_lowercase());
    save_defaults(company, secrets, &defaults).await?;
    Ok(defaults)
}

/// Drop every pin naming `connection_id`, and report whether anything went.
///
/// Called when an account is revoked: a pin to a connection that no longer
/// exists would be sent on the next execute and refused by Composio, turning a
/// disconnect of the *other* account into a broken toolkit.
pub async fn forget_connection(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    connection_id: &str,
) -> Result<bool> {
    let connection_id = connection_id.trim();
    let mut defaults = load_defaults(company, secrets).await?;
    let before = defaults.len();
    defaults.retain(|_, id| id != connection_id);
    if defaults.len() == before {
        return Ok(false);
    }
    save_defaults(company, secrets, &defaults).await?;
    Ok(true)
}

async fn save_defaults(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    defaults: &ComposioDefaults,
) -> Result<()> {
    // An empty map is stored as an empty string rather than `{}`, matching how
    // every other value here is cleared: `SecretStore` has no delete, and the
    // loader already reads empty as "nothing pinned".
    let raw = if defaults.is_empty() {
        String::new()
    } else {
        serde_json::to_string(defaults).map_err(|err| {
            crate::error::OpenCompanyError::Store(format!(
                "could not serialize composio defaults: {err}"
            ))
        })?
    };
    secrets.set(company, DEFAULTS_KEY, SecretValue(raw)).await
}

/// One provider in the catalog the console renders, carrying the backend's own
/// display metadata rather than a bare slug (issue #600).
///
/// ## Why this lives here and not in the harness
///
/// It is produced by `harness::composio::list_catalog_toolkits` and consumed by
/// the always-compiled status route, and the harness compiles only under the
/// `openhuman` feature. Same reason [`TINYHUMANS_KEY_KEY`] and
/// [`backend_url_or_default`] live here: the console plane must keep working in
/// a default build that links none of the live tools.
///
/// ## Why it is not `composio_catalog::CatalogToolkit`
///
/// That type describes the same backend entry for an *agent*, and it drops the
/// logo URL on purpose — a URL a model can never act on costs tokens to no end.
/// The logo and the categories are the entire point of this one: they are what
/// let 123 providers be a browsable grid instead of 123 stacked rows. It also
/// carries no connected flag, because the console learns that from
/// `GET …/composio/connections` — live per-company state, not catalog data.
///
/// Serialized straight into the status DTO, so these field names are the
/// console's wire contract.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    /// Toolkit slug, e.g. `googlecalendar`. The key every host call is made
    /// with, and the only field the backend always publishes.
    pub slug: String,
    /// Human-readable name, e.g. `Google Calendar`. Empty when the backend
    /// published none — the console then falls back to its own typography.
    pub name: String,
    /// One-line description. Empty when unpublished. The console searches it
    /// alongside the name, so an operator who knows what a provider *does* can
    /// find it without knowing what it is called.
    pub description: String,
    /// Composio-hosted logo URL. `None` when unpublished.
    pub logo: Option<String>,
    /// Composio's own category names, e.g. `["productivity", "email"]`.
    ///
    /// Forwarded **verbatim** and uninterpreted. The console buckets them by
    /// substring, and it does so precisely because that means a Composio
    /// integration added tomorrow lands in the right group with no code change
    /// on either side of this wire.
    pub categories: Vec<String>,
}

impl CatalogEntry {
    /// An entry for a provider the backend published a slug and nothing else
    /// for.
    ///
    /// Three real callers, so "no metadata" is a first-class state rather than
    /// a reason to drop the provider: a manifest allowlist (hand-written slugs,
    /// and the catalog is deliberately never consulted for it), the fallback
    /// list (which exists *because* the metadata could not be fetched), and a
    /// backend predating the dynamic catalog (which sends no `catalog[]` at
    /// all). The console renders all three with its own typography.
    pub fn from_slug(slug: impl Into<String>) -> Self {
        Self {
            slug: slug.into(),
            ..Self::default()
        }
    }
}

#[cfg(test)]
#[path = "composio_tests_addresses.rs"]
mod tests_addresses;
#[cfg(test)]
#[path = "composio_tests_backend.rs"]
mod tests_backend;
#[cfg(test)]
#[path = "composio_tests_pins.rs"]
mod tests_pins;
#[cfg(test)]
#[path = "composio_tests_writes.rs"]
mod tests_writes;
