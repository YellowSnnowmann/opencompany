//! SSO auto-login: redeeming a platform-minted token for a session.
//!
//! # Why this exists
//!
//! A hosted company is opened from the dashboard, not from a sign-in form. The
//! owner clicks "Open" and must land **already signed in as themselves** —
//! there is no password to type on the instance, and asking for a magic link on
//! a host with no mailbox they can reach is the dead end the whole onboarding
//! flow removes (WS-A design: `opencompany-sso-onboarding-design.md`, Part 2).
//!
//! So the platform mints a short-lived, single-use token per deep-link, signs it
//! with an **SSO secret injected into the company at provision**, and hands the
//! browser `https://<slug>.opencompany.work/#/sso?token=<jwt>`. The token rides
//! the URL fragment, which the browser never sends to a server, so it survives
//! the cold-wake auto-refresh and never reaches a log. `Login.tsx` reads it and
//! POSTs it here.
//!
//! # Why the mint is decoupled from the wake
//!
//! The token is signed by the *platform* (which holds the shared secret) and
//! verified **offline** by this workload. The company does not have to be awake
//! to mint anything — a cold instance can be woken *after* the token already
//! exists, and its 5-minute TTL is far longer than the ~20s wake. This module is
//! only the redeem half: it verifies a token it did not issue.
//!
//! # What a redemption proves, and what it does
//!
//! A token carries [`SsoClaims`] `{ sub, slug, jti, iat, exp }`. It is accepted
//! only when every one of these holds:
//!
//! 1. The **HS256 signature** verifies against [`AppConfig::sso_secret`].
//! 2. It has **not expired** (`exp`).
//! 3. Its `slug` **is this company** — a token minted for company A cannot sign
//!    anyone into company B, exactly as a session cookie for A cannot.
//! 4. Its `sub` is a **standing admin** of this company
//!    ([`standing_admins`](crate::server::users::bootstrap::standing_admins) —
//!    the manifest `[users].admins` plus `OPENCOMPANY_ADMIN_EMAIL`). A signed
//!    token naming a stranger is refused: the signature proves the platform
//!    issued it, not that whoever it names owns this instance.
//! 5. Its `jti` has **not been consumed** before. Single use is enforced by an
//!    atomic `create_new` marker file under the data root (see
//!    [`ConsumedJtis`]); a replay of a still-valid token is refused.
//!
//! On success the admin is claimed (first use) or logged in (later uses) through
//! the same materialization the magic link uses
//! ([`upsert_from_eligibility`](crate::server::users::routes::upsert_from_eligibility)),
//! and a session is minted through the same choke point every login uses
//! ([`mint_session`](crate::server::users::routes::mint_session)) — so the
//! cross-origin header carrier is honored here without a second code path.
//!
//! # Off unless configured
//!
//! With no [`AppConfig::sso_secret`] the route answers `404`: a self-hosted
//! deployment that never sets `OPENCOMPANY_SSO_SECRET` exposes no SSO surface at
//! all, the same off-by-default shape the machine credentials take.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::ports::types::CompanyId;
use crate::ports::users::UserStatus;
use crate::server::users::bootstrap::standing_admins;
use crate::server::users::scope::{PublicCompany, public_scoped};

mod jti;
#[cfg(feature = "platform-jwt")]
mod verify;

pub use jti::ConsumedJtis;

/// The claims an SSO auto-login token carries.
///
/// HS256-signed by the platform, verified offline here. Registered claims only,
/// so a token stays small and a reader needs no OpenCompany-specific vocabulary
/// to reason about it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SsoClaims {
    /// The owner's email — the standing admin this token signs in.
    pub sub: String,
    /// The company slug this token is scoped to. A token is refused unless this
    /// equals the company it is redeemed against.
    pub slug: String,
    /// A unique token id, recorded on redemption so the token cannot be used
    /// twice.
    pub jti: String,
    /// Issued-at, epoch seconds. Carried for auditability; not itself a gate.
    pub iat: u64,
    /// Expiry, epoch seconds. A token past this is refused.
    pub exp: u64,
}

/// The redeem request body: the token from the `#/sso` fragment.
#[derive(Debug, Deserialize)]
struct RedeemBody {
    token: String,
}

/// Builds the SSO route fragment: `POST …/sso/redeem`.
///
/// Mounted on [`public_scoped`] like the login routes — an SSO redemption is
/// something a person does precisely because they hold no session yet, so it
/// does no auth of its own. Its authority is the signed token it verifies.
pub fn router() -> Router<AppState> {
    public_scoped("/sso/redeem", post(redeem))
}

/// `404` for a redeem attempt on a host with no SSO secret configured.
///
/// Deliberately indistinguishable from the route not existing: an unconfigured
/// deployment must not advertise an SSO surface it cannot honor, and a caller
/// probing for one learns nothing.
fn sso_disabled() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "not found", "code": "not_found" })),
    )
        .into_response()
}

/// `401` for a token that does not verify. One message for every reason —
/// bad signature, expired, wrong slug, wrong subject, replayed — so a redeem
/// endpoint cannot become an oracle for which tokens are almost-valid.
fn invalid_token() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({
            "error": "this sign-in link is not valid — it may have expired or already been used",
            "code": "invalid_sso_token",
        })),
    )
        .into_response()
}

/// `POST …/sso/redeem` — verify a platform SSO token and establish a session.
async fn redeem(
    company: PublicCompany,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RedeemBody>,
) -> Result<Response, crate::server::Rejection> {
    let runtime = company.runtime.clone();

    // Off unless configured. Read before any work so a disabled host answers
    // identically whatever the token is.
    let Some(secret) = state.config().sso_secret() else {
        return Ok(sso_disabled());
    };

    // Verify the signature, expiry, and claim shape offline. The concrete
    // verifier is feature-gated on `platform-jwt` (the `jsonwebtoken`
    // dependency); a build without it cannot honor a configured secret and says
    // so rather than accepting anything.
    let claims = match verify_token(secret, &body.token) {
        Ok(claims) => claims,
        // A build without `platform-jwt` cannot honor a configured secret; it
        // answers like an unconfigured host rather than accepting anything. The
        // arm — and the variant — exist only in that build, so the default build
        // (feature on) carries no dead code.
        #[cfg(not(feature = "platform-jwt"))]
        Err(SsoRejection::Unavailable) => return Ok(sso_disabled()),
        Err(SsoRejection::Invalid) => return Ok(invalid_token()),
    };

    // Scope: a token minted for another company must not sign anyone in here.
    // The platform mints tokens carrying the bare slug, so it is namespaced the
    // same way `runtime.id()` is — a no-op unless shared-single-DB tenant mode is
    // on, and idempotent for an already-prefixed id — before the comparison.
    // Without this a valid token is refused wherever tenant namespacing is
    // enabled, because `runtime.id()` is `<tenant>--<slug>` while the token's
    // `slug` is bare.
    let claimed = state
        .config()
        .namespaced_company_id(CompanyId::new(claims.slug.as_str()));
    if claimed.as_ref() != runtime.id().as_ref() {
        return Ok(invalid_token());
    }

    // Subject: only a standing admin of *this* company. A signed token proves the
    // platform issued it, never that its `sub` owns this instance — that is the
    // manifest's/`OPENCOMPANY_ADMIN_EMAIL`'s answer, and it is checked here.
    let standing = standing_admins(
        &crate::server::users::routes::manifest_admins(&runtime).await?,
        state.config().bootstrap_admin().as_deref(),
    );
    let subject = crate::ports::users::normalize_email(&claims.sub);
    if subject.is_empty() || !standing.contains(&subject) {
        return Ok(invalid_token());
    }

    // Single use: record the jti before minting a session. `consume` is an
    // atomic `create_new`, so two requests racing on one token cannot both win —
    // and a replay of a still-valid token finds the marker already there.
    let consumed = ConsumedJtis::new(state.home(), runtime.id())
        .consume(&claims.jti, claims.exp)
        .await?;
    if !consumed {
        return Ok(invalid_token());
    }

    // Claim the admin on first use, or log the existing account in. The `sub`
    // has already been proven a standing admin, so this materialization is the
    // claim; on later uses it is a plain read. Same path as the magic link, so a
    // passwordless admin created here is indistinguishable from one created by a
    // link.
    let now = crate::ports::now_millis();
    let user = crate::server::users::routes::upsert_from_eligibility(
        &runtime,
        &subject,
        crate::ports::users::UserRole::Admin,
        now,
    )
    .await?;

    // First use *claims* the admin (created active); a later use returns the
    // existing account as-is — which may since have been suspended. A signed,
    // unexpired token must not resurrect a deactivated admin, so this mirrors the
    // status gate every other login honors before a session is minted.
    if user.status != UserStatus::Active {
        return Ok(invalid_token());
    }

    tracing::info!(company = %runtime.id(), "sso auto-login redeemed");
    crate::server::users::routes::mint_session(&state, &runtime, &user, &headers).await
}

/// Why an SSO token was refused, before it reaches the flat `401`/`404`.
///
/// Split from the wire response so the verifier can distinguish "this build
/// cannot verify signatures" (a `404`, matching a disabled host) from "this
/// token does not verify" (a `401`) without importing axum.
enum SsoRejection {
    /// This build has no `platform-jwt` feature, so a configured secret cannot
    /// be honored. Surfaced as `404`, the same as an unconfigured host. Present
    /// only in that build; the default (feature on) can always verify, so the
    /// variant would otherwise be dead code.
    #[cfg(not(feature = "platform-jwt"))]
    Unavailable,
    /// The token failed signature, expiry, or shape verification.
    Invalid,
}

/// Verifies `token` against `secret`, returning its claims or why it was refused.
///
/// A thin dispatcher over the feature-gated verifier so the handler above stays
/// feature-agnostic: with `platform-jwt` it decodes and validates HS256; without
/// it, a configured secret is [`SsoRejection::Unavailable`].
fn verify_token(secret: &str, token: &str) -> Result<SsoClaims, SsoRejection> {
    #[cfg(feature = "platform-jwt")]
    {
        verify::verify_hs256(secret, token)
    }
    #[cfg(not(feature = "platform-jwt"))]
    {
        let _ = (secret, token);
        Err(SsoRejection::Unavailable)
    }
}

#[cfg(test)]
#[path = "sso_tests.rs"]
mod tests;
