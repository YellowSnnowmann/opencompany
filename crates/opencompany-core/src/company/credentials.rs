//! How this instance obtains its TinyHumans credential.
//!
//! A hosted tenant holds **no** static secret. The platform gives its pod a
//! short-lived, audience-bound token in a file that the kubelet rewrites **in
//! place** (roughly every 8 minutes), so the credential must be *obtained* per
//! request rather than read once at boot into a `String`.
//!
//! [`TinyhumansTokenSource`] is that seam. It has exactly two tiers, in strict
//! precedence order:
//!
//! 1. **[`Tier::ProjectedFile`]** — the path named by [`TOKEN_FILE_ENV`]. The
//!    file is re-read whenever the cached copy is past its window, so a rotation
//!    is picked up without a restart. A permanent cache here would be a latent
//!    outage: the pod would keep presenting a token the cluster already rotated
//!    away from. The platform mounts it read-only as
//!    `/var/run/secrets/tinyhumans.ai/token` with a 600-second expiry; the env var
//!    carries the **path**, never a token value.
//! 2. **[`Tier::Static`]** — the long-lived [`API_KEY_ENV`] value. This keeps
//!    `docker compose` development alive and serves the (explicitly
//!    **unsupported**) self-host case. It is not going away.
//!
//! ## Expiry is read, never trusted
//!
//! The cache window is derived from the token's own `exp` claim, parsed
//! **without verifying the signature** — this process needs the expiry only, and
//! the backend is the party that verifies. A token whose `exp` cannot be read
//! (not a JWT, malformed payload, no `exp`) is never cached: correctness beats a
//! saved file read. An already-expired token is still returned — refusing to
//! send it would turn a recoverable 401 into a local outage — but likewise never
//! cached.
//!
//! ## Degrading to the static tier
//!
//! The projected tier is only selected when the named path actually exists. The
//! docker runtime injects nothing, so a leftover [`TOKEN_FILE_ENV`] pointing at
//! a path that was never mounted must fall through to the static tier rather
//! than failing every request. Once the tier *is* selected, a later read failure
//! is a hard error: silently swapping to a different identity mid-life is worse
//! than a loud failure.
//!
//! ## Rejected tokens
//!
//! Callers invalidate the cache with [`TinyhumansTokenSource::invalidate`] when
//! the backend rejects a bearer (401), so the very next request re-reads the file
//! instead of waiting out the window with a token the cluster has moved past.
//!
//! ## Redaction
//!
//! Nothing here renders the token. [`Debug`] shows the tier (and, for a
//! projected file, the path — not a secret); [`TinyhumansTokenSource::describe`]
//! is the operator-facing one-liner the doctor prints.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::Result;
use crate::app::config::EnvSource;
use crate::error::OpenCompanyError;

/// Environment variable naming the platform-projected token file. Set by the
/// platform (a kubelet-projected, audience-bound ServiceAccount token volume) —
/// never by an operator, and never present in `docker compose`.
pub const TOKEN_FILE_ENV: &str = "TINYHUMANS_TOKEN_FILE";

/// Environment variable holding a long-lived static TinyHumans API key. The
/// docker-development credential, and the unsupported self-host escape hatch.
pub const API_KEY_ENV: &str = "TINYHUMANS_API_KEY";

/// Hard ceiling on how long a projected token is cached, regardless of how much
/// TTL it claims to have left. A projected file rotates on the platform's
/// schedule, not ours, so the ceiling bounds how stale a bearer can get even if
/// a token advertises hours of validity.
pub const MAX_CACHE_WINDOW: Duration = Duration::from_secs(60);

/// Fraction of a token's remaining TTL we are willing to cache for, so a
/// re-read always happens with time left on the clock.
const TTL_FRACTION: f64 = 0.8;

/// Which tier a token was obtained from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenTier {
    /// A platform-projected file that rotates in place ([`TOKEN_FILE_ENV`]).
    ProjectedFile,
    /// A long-lived static key ([`API_KEY_ENV`]).
    Static,
}

impl TokenTier {
    /// The stable wire/diagnostic spelling of this tier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProjectedFile => "projected_file",
            Self::Static => "static",
        }
    }

    /// How this tier is named on the console read plane.
    pub fn credential_source(self) -> CredentialSource {
        match self {
            Self::ProjectedFile => CredentialSource::Attested,
            Self::Static => CredentialSource::Static,
        }
    }
}

impl std::fmt::Display for TokenTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a company's outbound credential comes from, as the console renders it.
/// Carries no secret and no path — just the shape of the answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CredentialSource {
    /// The pod presents a platform-minted, audience-bound identity. Nothing is
    /// stored on this instance.
    Attested,
    /// The **company's own** TinyHumans credential — the one key its admin set
    /// on this tenant (issue #586). Distinct from [`Self::Static`]: this is the
    /// company's platform identity, so every surface the backend brokers rides
    /// it, and rotating it reaches all of them at once.
    Company,
    /// A static key/token is configured — the docker-development path, or a
    /// per-provider token an operator pasted in as an escape hatch.
    Static,
    /// No credential can be obtained at all.
    None,
}

impl CredentialSource {
    /// The stable wire spelling (`attested` / `company` / `static` / `none`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Attested => "attested",
            Self::Company => "company",
            Self::Static => "static",
            Self::None => "none",
        }
    }
}

impl std::fmt::Display for CredentialSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A projected token cached until `good_until`.
struct Cached {
    token: String,
    good_until: SystemTime,
}

/// The resolved tier, with whatever state it needs to keep answering.
enum Tier {
    /// A rotating file plus the currently-cached read of it.
    ProjectedFile {
        path: PathBuf,
        cache: Mutex<Option<Cached>>,
    },
    /// A static value held in memory for the life of the process.
    Static { token: String },
}

/// How this instance obtains its TinyHumans credential — see the module docs.
///
/// Construct once and share behind an `Arc`: the projected-file tier keeps a
/// small cache, so cloning the source would clone the cache and defeat it.
pub struct TinyhumansTokenSource {
    tier: Tier,
}

impl TinyhumansTokenSource {
    /// A source that re-reads `path` as it rotates.
    pub fn projected_file(path: impl Into<PathBuf>) -> Self {
        Self {
            tier: Tier::ProjectedFile {
                path: path.into(),
                cache: Mutex::new(None),
            },
        }
    }

    /// A source over a static, long-lived key.
    pub fn static_key(token: impl Into<String>) -> Self {
        Self {
            tier: Tier::Static {
                token: token.into(),
            },
        }
    }

    /// Resolves the source from the environment, honouring the documented
    /// precedence **[`TOKEN_FILE_ENV`] > [`API_KEY_ENV`]**, or `None` when
    /// neither is configured (fail closed — the caller keeps its offline brain).
    ///
    /// The projected file wins deliberately: a pod that has been handed a cluster
    /// identity must present *that*, even if a stale static key is still lying
    /// around in its environment. It is chosen only when the named path **exists**
    /// — the docker runtime mounts nothing, so a leftover variable degrades to the
    /// static tier (with a warning) instead of failing every request.
    pub fn from_env(env: &dyn EnvSource) -> Option<Self> {
        Self::from_parts(
            env.get(TOKEN_FILE_ENV).as_deref().map(Path::new),
            env.get(API_KEY_ENV).as_deref(),
        )
    }

    /// The precedence rule itself, over already-resolved parts.
    ///
    /// [`Self::from_env`] is this function plus an environment read. Callers that
    /// already hold the resolved values — `RuntimeConfig`, which parsed them at
    /// load — must route through here rather than re-deriving the rule, because a
    /// second copy is a second chance to forget the existence check and answer
    /// `attested` for a path the runtime never mounted.
    ///
    /// Both inputs are trimmed; blank is treated as absent.
    pub fn from_parts(token_file: Option<&Path>, api_key: Option<&str>) -> Option<Self> {
        if let Some(path) = token_file {
            let path = Path::new(path.as_os_str().to_str().map(str::trim).unwrap_or_default());
            if !path.as_os_str().is_empty() {
                if path.exists() {
                    return Some(Self::projected_file(path));
                }
                tracing::warn!(
                    token_file = %path.display(),
                    "{TOKEN_FILE_ENV} names a path that does not exist; falling back to the static credential tier"
                );
            }
        }
        let key = api_key?.trim();
        if key.is_empty() {
            return None;
        }
        Some(Self::static_key(key))
    }

    /// The tier a set of resolved parts selects, without building a source.
    ///
    /// `has_static_credential` stands in for a held secret the caller must not
    /// hand over (`RuntimeConfig` keeps its key behind a redacting wrapper), so
    /// the static tier is reported from its presence rather than its value.
    pub fn source_of_parts(
        token_file: Option<&Path>,
        has_static_credential: bool,
    ) -> CredentialSource {
        match Self::from_parts(token_file, None) {
            Some(source) => source.credential_source(),
            None if has_static_credential => CredentialSource::Static,
            None => CredentialSource::None,
        }
    }

    /// Which tier this source resolved to.
    pub fn tier(&self) -> TokenTier {
        match &self.tier {
            Tier::ProjectedFile { .. } => TokenTier::ProjectedFile,
            Tier::Static { .. } => TokenTier::Static,
        }
    }

    /// How the console names this source.
    pub fn credential_source(&self) -> CredentialSource {
        self.tier().credential_source()
    }

    /// The projected token file, when this is the projected-file tier.
    pub fn token_file(&self) -> Option<&Path> {
        match &self.tier {
            Tier::ProjectedFile { path, .. } => Some(path.as_path()),
            Tier::Static { .. } => None,
        }
    }

    /// An operator-facing, credential-free one-liner for the doctor: the active
    /// tier, plus the file path when there is one. Never the token.
    pub fn describe(&self) -> String {
        match &self.tier {
            Tier::ProjectedFile { path, .. } => {
                format!("projected_file ({})", path.display())
            }
            Tier::Static { .. } => "static (set)".to_string(),
        }
    }

    /// Folds this source's *identity* into `hasher` for a roster fingerprint.
    ///
    /// The projected-file tier contributes its **tier + path**, never the token
    /// value: the whole point is that the token rotates every few minutes while
    /// the identity behind it does not. Hashing the value there would rebuild
    /// every agent's tool roster on the platform's rotation schedule. The static
    /// tier contributes its **value**, because for a static key a changed value
    /// *is* a changed identity (an operator rotated it) and must rebuild.
    pub fn hash_identity<H: std::hash::Hasher>(&self, hasher: &mut H) {
        use std::hash::Hash;
        match &self.tier {
            Tier::ProjectedFile { path, .. } => {
                0u8.hash(hasher);
                path.hash(hasher);
            }
            Tier::Static { token } => {
                1u8.hash(hasher);
                token.hash(hasher);
            }
        }
    }

    /// Drops any cached read, so the next [`Self::current`] goes back to the
    /// file. Called when the backend rejects a bearer (401): the cluster may have
    /// rotated the token early, and waiting out the cache window would keep
    /// presenting the rejected one. A no-op for the static tier.
    pub fn invalidate(&self) {
        if let Tier::ProjectedFile { cache, .. } = &self.tier {
            *cache.lock().expect("token cache poisoned") = None;
        }
    }

    /// The credential to present on the next outbound request.
    ///
    /// For the static tier this is the configured value. For the projected-file
    /// tier it is the cached read while the cache window holds, and otherwise a
    /// fresh read of the file — which is how an in-place rotation is picked up.
    pub async fn current(&self) -> Result<String> {
        self.current_at(SystemTime::now()).await
    }

    /// [`Self::current`] against an explicit clock, so the cache window can be
    /// exercised deterministically in tests.
    pub async fn current_at(&self, now: SystemTime) -> Result<String> {
        let (path, cache) = match &self.tier {
            Tier::Static { token } => return Ok(token.clone()),
            Tier::ProjectedFile { path, cache } => (path, cache),
        };

        // Scoped so the guard is dropped before the await below.
        {
            let guard = cache.lock().expect("token cache poisoned");
            if let Some(cached) = guard.as_ref()
                && cached.good_until > now
            {
                return Ok(cached.token.clone());
            }
        }

        let raw = tokio::fs::read_to_string(path).await.map_err(|e| {
            OpenCompanyError::Config(format!(
                "could not read the projected TinyHumans token file {}: {e}",
                path.display()
            ))
        })?;
        let token = raw.trim().to_string();
        if token.is_empty() {
            return Err(OpenCompanyError::Config(format!(
                "the projected TinyHumans token file {} is empty",
                path.display()
            )));
        }

        // A token we cannot date, or one already past `exp`, is never cached.
        let window = cache_window(now, unverified_jwt_exp(&token));
        *cache.lock().expect("token cache poisoned") = window.map(|window| Cached {
            token: token.clone(),
            good_until: now + window,
        });
        Ok(token)
    }
}

/// One resolved outbound credential: nothing, a value someone gave us, or a
/// [`TinyhumansTokenSource`] that must be read per request.
///
/// This is the seam that keeps a rotating token from being flattened into a
/// `String` at build time. Anything holding a credential holds *this*, resolves
/// it with [`Self::current`] on the request path, and asks
/// [`Self::configured`]/[`Self::source`] for status — so a rotation never needs a
/// rebuild and status never needs the value.
#[derive(Clone, Default)]
pub enum Credential {
    /// No credential — omit the bearer entirely (e.g. a keyless local Ollama).
    #[default]
    None,
    /// A value held in memory: a per-provider token an operator pasted into the
    /// secret store, or a BYOK key. A changed value is a changed identity.
    Value(String),
    /// The **company's own** TinyHumans credential, read from its secret store
    /// (issue #586). Behaves like [`Self::Value`] on the request path — it is a
    /// held string, and a changed value is a changed identity — but reports
    /// itself as [`CredentialSource::Company`] so the console can say *whose*
    /// identity a brokered call presents rather than only that one exists.
    Company(String),
    /// A platform token source. Read per request; the value it yields rotates
    /// while the identity behind it does not.
    Source(std::sync::Arc<TinyhumansTokenSource>),
}

impl Credential {
    /// A credential from a stored value, treating blank as "not configured".
    pub fn from_value(value: impl Into<String>) -> Self {
        let value = value.into();
        if value.trim().is_empty() {
            Self::None
        } else {
            Self::Value(value)
        }
    }

    /// The company's own TinyHumans credential, treating blank as "not
    /// configured". The seam a brokered surface resolves its company identity
    /// through — see [`company_key`](crate::company::company_key).
    pub fn from_company_key(value: impl Into<String>) -> Self {
        let value = value.into();
        if value.trim().is_empty() {
            Self::None
        } else {
            Self::Company(value)
        }
    }

    /// A credential over a shared token source.
    pub fn from_source(source: std::sync::Arc<TinyhumansTokenSource>) -> Self {
        Self::Source(source)
    }

    /// Whether a credential can be obtained — the non-secret status the read
    /// planes surface. Never returns the value.
    pub fn configured(&self) -> bool {
        !matches!(self, Self::None)
    }

    /// Which tier this credential comes from, as the console names it.
    pub fn source(&self) -> CredentialSource {
        match self {
            Self::None => CredentialSource::None,
            Self::Value(_) => CredentialSource::Static,
            Self::Company(_) => CredentialSource::Company,
            Self::Source(source) => source.credential_source(),
        }
    }

    /// The bearer to present on the next request, or `None` to omit the header.
    /// A [`Self::Source`] is read here — that is what makes a rotation take
    /// effect without a rebuild.
    pub async fn current(&self) -> Result<Option<String>> {
        let token = match self {
            Self::None => return Ok(None),
            Self::Value(value) | Self::Company(value) => value.clone(),
            Self::Source(source) => source.current().await?,
        };
        let token = token.trim().to_string();
        Ok(if token.is_empty() { None } else { Some(token) })
    }

    /// Forwards a rejection (401) to the underlying source so the next request
    /// re-reads it. A no-op for a value we were handed.
    pub fn invalidate(&self) {
        if let Self::Source(source) = self {
            source.invalidate();
        }
    }

    /// Folds this credential's *identity* into `hasher` for a roster
    /// fingerprint — see [`TinyhumansTokenSource::hash_identity`] for why a
    /// projected source contributes its path rather than its rotating value.
    pub fn hash_identity<H: std::hash::Hasher>(&self, hasher: &mut H) {
        use std::hash::Hash;
        match self {
            Self::None => 0u8.hash(hasher),
            Self::Value(value) => {
                1u8.hash(hasher);
                value.hash(hasher);
            }
            Self::Source(source) => {
                2u8.hash(hasher);
                source.hash_identity(hasher);
            }
            // Hashed by **value**, like `Value`: the company key is a static
            // credential its admin rotates deliberately, not one the cluster
            // rotates on a timer, so a new value really is a new identity and
            // the roster must rebuild. That is what makes a console rotation
            // reach every surface wired to this credential on the next cycle —
            // Composio today (issue #586).
            Self::Company(value) => {
                3u8.hash(hasher);
                value.hash(hasher);
            }
        }
    }
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("Credential(<unset>)"),
            Self::Value(_) => f.write_str("Credential(<redacted>)"),
            Self::Company(_) => f.write_str("Credential(company <redacted>)"),
            Self::Source(source) => write!(f, "Credential({})", source.describe()),
        }
    }
}

impl std::fmt::Debug for TinyhumansTokenSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.tier {
            Tier::ProjectedFile { path, .. } => f
                .debug_struct("TinyhumansTokenSource")
                .field("tier", &"projected_file")
                .field("path", path)
                .finish(),
            Tier::Static { token } => f
                .debug_struct("TinyhumansTokenSource")
                .field("tier", &"static")
                .field(
                    "token",
                    &if token.is_empty() {
                        "<unset>"
                    } else {
                        "<redacted>"
                    },
                )
                .finish(),
        }
    }
}

/// How long a token read at `now` may be cached: 80% of its remaining TTL,
/// capped at [`MAX_CACHE_WINDOW`]. `None` — meaning "do not cache" — when the
/// expiry is unknown, already passed, or so close that no window is left.
fn cache_window(now: SystemTime, expiry: Option<SystemTime>) -> Option<Duration> {
    let remaining = expiry?.duration_since(now).ok()?;
    let window = remaining.mul_f64(TTL_FRACTION).min(MAX_CACHE_WINDOW);
    if window.is_zero() { None } else { Some(window) }
}

/// The `exp` claim of a JWT, read **without verifying the signature**.
///
/// This process needs the expiry to schedule a re-read; the backend is the party
/// that verifies the token, so validating here would buy nothing and would need
/// the cluster's public keys. `None` for anything that is not a three-segment
/// JWT with a numeric `exp` — the caller then declines to cache.
pub fn unverified_jwt_exp(token: &str) -> Option<SystemTime> {
    let mut segments = token.trim().split('.');
    let _header = segments.next()?;
    let payload = segments.next()?;
    // A JWS has exactly three segments; anything else is not a token we can read.
    segments.next()?;
    if segments.next().is_some() {
        return None;
    }
    let claims: serde_json::Value = serde_json::from_slice(&base64url_decode(payload)?).ok()?;
    let exp = claims.get("exp")?.as_u64()?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(exp))
}

/// Decodes unpadded base64url (RFC 4648 §5) — the JWT segment encoding.
/// `None` on any character outside the alphabet.
fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            // Tolerate the padded form even though JWT segments omit it.
            b'=' => break,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
#[path = "credentials_tests.rs"]
mod tests;
