//! Host-level admin password set: the platform gives a hosted company's standing
//! admin a password, so the owner can sign in directly and not only through the
//! SSO auto-login.
//!
//! # Why this exists
//!
//! The dashboard is the source of truth for a hosted company's admin password —
//! it generates one, keeps it encrypted at rest, and reveals it on demand so an
//! owner who is signed out of the console still has a way in. For that stored
//! password to actually work the company must hold its hash; this route is how
//! the platform delivers it, without the owner ever typing it on the instance.
//!
//! # Why host-level, not company-scoped
//!
//! [`sso`](crate::server::sso) is company-scoped because the company's own
//! frontend calls it with the id it already knows. This route is called by the
//! *backend*, which knows only the slug — never the app's internal, post-wizard,
//! name-derived company id. So it takes no id in its path and resolves the
//! company from the token's `slug`, the one identifier both sides share.
//!
//! # Why a separate signing key
//!
//! The token is the same shape as an SSO login token and drawn from the same
//! `OPENCOMPANY_SSO_SECRET`. To stop one being replayed as the other — an SSO
//! token used here would set an attacker-chosen password; a set-password token
//! used at `/sso/redeem` would mint a session — the two are signed with
//! **domain-separated keys** derived from the secret under distinct labels
//! ([`derive_key`]). A token minted for one purpose cannot verify for the other,
//! and neither route has to change to defend the boundary.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;

use crate::AppState;
use crate::ports::types::CompanyId;
use crate::server::sso::{ConsumedJtis, SsoClaims};
use crate::server::users::bootstrap::{PasswordIssueContext, issue_password, standing_admins};

/// Builds the route fragment: `POST /api/v1/admin/set-password`.
pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/admin/set-password", post(set_password))
}

/// The request: the platform token that authorizes the change, and the password
/// to set. The password rides the body, never the token, so it stays out of the
/// places a JWT is apt to be logged.
#[derive(Debug, Deserialize)]
struct SetPasswordBody {
    token: String,
    password: String,
}

/// `404`, indistinguishable from the route not existing, when no SSO secret is
/// configured — the same off-by-default shape [`sso`](crate::server::sso) takes.
fn not_configured() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "not found", "code": "not_found" })),
    )
        .into_response()
}

/// `401` for any token that does not verify — one flat message so the endpoint
/// cannot become an oracle for which tokens are almost-valid.
fn invalid_token() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({
            "error": "this request is not authorized",
            "code": "invalid_set_password_token",
        })),
    )
        .into_response()
}

/// `404` with a distinct code for a host that has no company registered yet — it
/// is still in its setup wizard. Unlike [`invalid_token`]'s `401`, this tells the
/// caller the signing was fine and the company simply is not up yet, so the
/// backend can retry after setup instead of reporting a key mismatch.
fn not_ready() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": "no company is configured on this host yet",
            "code": "company_not_ready",
        })),
    )
        .into_response()
}

/// Why a set-password token was refused, before it collapses to the flat wire
/// response. Mirrors [`sso`](crate::server::sso)'s split so a build without
/// `platform-jwt` (which cannot verify signatures) answers `404` like a disabled
/// host rather than `401`.
enum Reject {
    /// This build has no `platform-jwt`, so a configured secret cannot be
    /// honored. Present only in that build; the default carries no dead variant.
    #[cfg(not(feature = "platform-jwt"))]
    Unavailable,
    /// The token failed signature, expiry, or shape verification.
    Invalid,
}

/// Verifies `token` against the domain-separated set-password key, dispatching on
/// the `platform-jwt` feature exactly as [`sso`](crate::server::sso) does.
fn verify(secret: &str, token: &str) -> Result<SsoClaims, Reject> {
    #[cfg(feature = "platform-jwt")]
    {
        verify_hs256(secret, token)
    }
    #[cfg(not(feature = "platform-jwt"))]
    {
        let _ = (secret, token);
        Err(Reject::Unavailable)
    }
}

/// The HMAC label that domain-separates the set-password signing key from the
/// SSO login key. Versioned so a future rotation is a new label, not a silent
/// reinterpretation of the old one.
#[cfg(feature = "platform-jwt")]
const SET_PASSWORD_KEY_LABEL: &[u8] = b"opencompany:admin-set-password:v1";

/// The platform contract is a 5-minute token; a token claiming a longer life is
/// refused regardless of whether it has expired yet, bounding the blast radius of
/// any mint-side bug or key leak. This route sets a credential, so a long-lived
/// token is worse here than on redeem.
#[cfg(feature = "platform-jwt")]
const MAX_TOKEN_LIFETIME_SECS: u64 = 300;
/// Clock-skew leeway added to the lifetime cap.
#[cfg(feature = "platform-jwt")]
const CLOCK_LEEWAY_SECS: u64 = 60;

/// Derives the set-password signing key from the shared SSO secret:
/// `HMAC-SHA256(secret, label)`. A pseudorandom key distinct from the raw secret
/// the SSO tokens are signed with, so the two token families are not
/// interchangeable even though they share a shape and a source secret.
#[cfg(feature = "platform-jwt")]
fn derive_key(secret: &str) -> [u8; 32] {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts a key of any length");
    mac.update(SET_PASSWORD_KEY_LABEL);
    let digest = mac.finalize().into_bytes();
    let mut key = [0u8; 32];
    key.copy_from_slice(&digest);
    key
}

/// HS256 verification against the derived key. Same required-claim/expiry rules
/// as the SSO verifier; a different key is the whole point.
#[cfg(feature = "platform-jwt")]
fn verify_hs256(secret: &str, token: &str) -> Result<SsoClaims, Reject> {
    use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
    let key = derive_key(secret);
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_required_spec_claims(&["sub", "exp"]);
    validation.validate_exp = true;
    let claims = decode::<SsoClaims>(token, &DecodingKey::from_secret(&key), &validation)
        .map(|data| data.claims)
        .map_err(|_| Reject::Invalid)?;

    // Cap the declared lifetime: `exp` alone bounds only the far edge, so refuse a
    // token claiming more than the 5-minute contract (plus leeway).
    if claims.exp.saturating_sub(claims.iat) > MAX_TOKEN_LIFETIME_SECS + CLOCK_LEEWAY_SECS {
        return Err(Reject::Invalid);
    }

    Ok(claims)
}

/// `POST /api/v1/admin/set-password` — set this host's standing-admin password.
async fn set_password(
    State(state): State<AppState>,
    Json(body): Json<SetPasswordBody>,
) -> Result<Response, crate::server::Rejection> {
    // Off unless configured. Read first so a disabled host answers identically
    // whatever the token is.
    let Some(secret) = state.config().sso_secret() else {
        return Ok(not_configured());
    };

    let claims = match verify(secret, &body.token) {
        Ok(claims) => claims,
        #[cfg(not(feature = "platform-jwt"))]
        Err(Reject::Unavailable) => return Ok(not_configured()),
        Err(Reject::Invalid) => return Ok(invalid_token()),
    };

    // Resolve THIS host's company from the token's slug. The slug is namespaced
    // the same way `runtime.id()` is (a no-op off shared-single-DB, idempotent
    // otherwise), so a bare-slug token addresses the right company and a token
    // minted for another company finds no runtime here.
    let id = state
        .config()
        .namespaced_company_id(CompanyId::new(claims.slug.as_str()));
    let Some(runtime) = state.registry().get(&id) else {
        // Distinguish "no company registered on this host yet" (still in its setup
        // wizard) from a token that does not verify: with an empty registry the
        // backend should retry after setup, not read it as a signing mismatch. A
        // configured host that simply does not serve this slug stays a flat 401.
        if state.registry().is_empty() {
            return Ok(not_ready());
        }
        return Ok(invalid_token());
    };

    // Subject must be a standing admin of this company — a signed token proves
    // the platform issued it, never that its `sub` owns this instance.
    let manifest_admins = crate::server::users::routes::manifest_admins(&runtime).await?;
    let bootstrap_admin = state.config().bootstrap_admin();
    let standing = standing_admins(&manifest_admins, bootstrap_admin.as_deref());
    let subject = crate::ports::users::normalize_email(&claims.sub);
    if subject.is_empty() || !standing.contains(&subject) {
        return Ok(invalid_token());
    }

    // Single use: the same jti ledger the SSO redemption uses, so a replay — one
    // swapping in a different password included — finds the marker already there.
    let consumed = ConsumedJtis::new(state.home(), runtime.id())
        .consume(&claims.jti, claims.exp)
        .await?;
    if !consumed {
        return Ok(invalid_token());
    }

    // Set the password on the running server. `must_change = false`: the
    // dashboard manages this credential, so the owner is never forced to replace
    // a password they did not choose. `issue_password` validates the value and
    // refuses a non-admin or suspended subject.
    issue_password(
        PasswordIssueContext {
            users: runtime.users(),
            sessions: runtime.sessions(),
            login_codes: runtime.login_codes(),
            company: runtime.id(),
            manifest_admins: &manifest_admins,
            bootstrap_admin: bootstrap_admin.as_deref(),
        },
        &subject,
        &body.password,
        false,
    )
    .await?;

    Ok((StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response())
}

#[cfg(test)]
#[path = "admin_password_tests.rs"]
mod tests;
