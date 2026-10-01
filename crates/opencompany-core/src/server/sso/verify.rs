//! Offline HS256 verification of SSO tokens (feature `platform-jwt`).
//!
//! Split into its own file because the `jsonwebtoken` dependency it needs is
//! gated on `platform-jwt` — the same feature [`JwtPlatformVerifier`](crate::server::platform_auth)
//! uses, already in the default set. The module above dispatches to this only
//! when the feature is on; without it a configured secret is reported as
//! unavailable rather than silently ignored.

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};

use super::{SsoClaims, SsoRejection};

/// The platform contract is a 5-minute token. A token claiming a longer life is
/// refused regardless of whether it has expired yet, so a mint-side bug or a
/// leaked key cannot sign a token that stays valid for hours or years.
const MAX_TOKEN_LIFETIME_SECS: u64 = 300;
/// Clock-skew leeway added to the lifetime cap, so an honest token minted against
/// a slightly fast platform clock is not refused at the boundary.
const CLOCK_LEEWAY_SECS: u64 = 60;

/// Verifies `token` against `secret` as an HS256 JWT and returns its claims.
///
/// The whole check is the signature, the required claim set, expiry, and a cap on
/// the token's declared lifetime:
///
/// - **Signature** against `secret`, HS256 — a token this workload could not
///   have been handed a valid signature for is refused.
/// - **Required claims** `sub`, `slug`, `jti`, `iat`, `exp` — a token missing any
///   of them is malformed and refused, so the handler never has to reason about a
///   partial claim set.
/// - **Expiry** — `jsonwebtoken` validates `exp` by default; a token past it is
///   refused here rather than downstream.
/// - **Lifetime cap** — `exp - iat` must not exceed the 5-minute contract (plus
///   leeway). `exp` alone only bounds the far edge; this bounds the blast radius
///   of any mint-side bug or key leak to ~5 minutes.
///
/// Every failure collapses to [`SsoRejection::Invalid`]: the caller renders one
/// flat `401`, so the distinction between "bad signature" and "expired" never
/// reaches the wire and cannot be probed.
pub(super) fn verify_hs256(secret: &str, token: &str) -> Result<SsoClaims, SsoRejection> {
    let mut validation = Validation::new(Algorithm::HS256);
    // Unlike the platform gate — which clears its required set because a machine
    // token may omit registered claims — an SSO token is minted by us to a fixed
    // shape, so every field is required. A token missing one is malformed, not a
    // looser variant to accept.
    validation.set_required_spec_claims(&["sub", "exp"]);
    validation.validate_exp = true;

    let claims = decode::<SsoClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map(|data| data.claims)
    .map_err(|_| SsoRejection::Invalid)?;

    // Refuse a token whose declared lifetime exceeds the contract. `saturating_sub`
    // folds a malformed `exp < iat` into `0`, which passes this cap and is then
    // refused by the expiry check above on any realistic clock.
    if claims.exp.saturating_sub(claims.iat) > MAX_TOKEN_LIFETIME_SECS + CLOCK_LEEWAY_SECS {
        return Err(SsoRejection::Invalid);
    }

    Ok(claims)
}
