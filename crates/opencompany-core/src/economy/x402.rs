//! x402 payment challenges and Ed25519-signed authorizations.
//!
//! When a counterparty gates a skill behind payment it answers `402` with a
//! challenge naming the `amount`, `recipient`, `asset`, and `network`. The payer
//! signs an **authorization** over a canonical payload with the same Ed25519
//! identity key it uses for SIWX, then posts it to the settlement endpoints.
//! This module only *builds and verifies* authorizations — no on-chain
//! submission happens here (that is a documented SDK gap).
//!
//! ## Canonical byte layout (golden, versioned)
//!
//! ```text
//! tiny.place-x402-v1\n
//! <agentId>\n
//! <amount>\n
//! <recipient>\n
//! <asset>\n
//! <network>\n
//! <nonce>\n
//! <timestamp>
//! ```
//!
//! Isolated in [`canonical_bytes`] so it is a one-function change to reconcile
//! with the real tiny.place server when reachable.
//!
//! ## Single use is enforced, not merely documented
//!
//! The signature covers the whole payload including the nonce, so a payer
//! cannot re-point one authorization at different work — but nothing about a
//! signature stops the *same* authorization being presented again. [`verify`]
//! therefore takes the spent-nonce set and the current time, and refuses both a
//! nonce it has already seen and an authorization older than
//! [`MAX_AGE_SECS`]. Neither is optional, because a verified-but-unspent
//! authorization is a bearer token: one signature buying unlimited work.
//!
//! Bounding acceptance by age is what makes forgetting a nonce safe. The spent
//! set prunes on the same constant, so a nonce is dropped only once the
//! authorization carrying it would be refused on age anyway, and there is no
//! window in which a replay outlives the memory of it.
//!
//! ## The nonce comes from the OS CSPRNG
//!
//! The nonce is signed into the payload above, so a counterparty's replay check
//! is only as good as the value's unpredictability and uniqueness. It is
//! therefore minted by [`mint_nonce`] from 256 bits of OS randomness through
//! the same
//! [`TokenSource`](crate::server::users::token::TokenSource) seam the user-auth
//! secrets use — **not** from
//! [`generate_id`](crate::ports::generate_id), whose epoch-millis-plus-counter
//! shape is guessable from a prior value and repeats across processes that
//! start in the same millisecond.

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::economy::signer::{LocalSigner, verify_b58};
use crate::economy::siwx::{NonceCache, SKEW_SECS};
use crate::error::OpenCompanyError;
use crate::server::platform_auth::b64url_encode;
use crate::server::users::token::{OsTokens, TokenSource};

/// The domain-separation tag pinning the x402 canonical layout version.
pub const X402_DOMAIN: &str = "tiny.place-x402-v1";

/// How long an authorization stays acceptable, and therefore how long its nonce
/// is remembered as spent. Ten minutes.
///
/// Twice the SIWX clock-skew tolerance. An authorization only ever arrives
/// inside a SIWX-signed request, and that request is already refused unless its
/// own timestamp is within [`SKEW_SECS`] of the verifier's clock, so this
/// covers a payer at the far edge of tolerated clock offset plus a full
/// challenge → authorize → resend round trip — a round trip that in practice
/// takes under a second. Anything older is a request the transport layer would
/// have turned away, so accepting it buys the payer nothing and costs the spent
/// set unbounded memory.
pub const MAX_AGE_SECS: i64 = 2 * SKEW_SECS;

/// A payment challenge parsed from a counterparty's `402` response body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct X402Challenge {
    /// The amount due, as a decimal string (e.g. `"25.00"`).
    pub amount: String,
    /// The recipient address to pay.
    pub recipient: String,
    /// The settlement asset (e.g. `"USDC"`).
    pub asset: String,
    /// The settlement network (e.g. `"solana"`).
    pub network: String,
}

impl X402Challenge {
    /// Parses a challenge from a `402` JSON body.
    ///
    /// Accepts either a flat object (`{amount, recipient, asset, network}`) or
    /// the x402 `{ "accepts": [ { … } ] }` envelope, and tolerates the common
    /// field aliases `maxAmountRequired`/`payTo`.
    pub fn from_body(v: &serde_json::Value) -> Result<Self> {
        let obj = v.get("accepts").and_then(|a| a.get(0)).unwrap_or(v);

        let amount = string_field(obj, &["amount", "maxAmountRequired"]).ok_or_else(|| {
            OpenCompanyError::InvalidRequest("x402 challenge is missing `amount`".into())
        })?;
        let recipient = string_field(obj, &["recipient", "payTo"]).ok_or_else(|| {
            OpenCompanyError::InvalidRequest("x402 challenge is missing `recipient`".into())
        })?;
        let asset = string_field(obj, &["asset"]).unwrap_or_else(|| "USDC".to_string());
        let network = string_field(obj, &["network"]).unwrap_or_else(|| "solana".to_string());

        Ok(Self {
            amount,
            recipient,
            asset,
            network,
        })
    }
}

/// A signed x402 payment authorization, ready to POST to `/payments/verify`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct X402Authorization {
    /// The payer's base58 `agentId`.
    #[serde(rename = "agentId")]
    pub agent_id: String,
    /// The amount authorized. May exceed the challenge amount for an `upto`
    /// delegated-signer grant.
    pub amount: String,
    /// The recipient address.
    pub recipient: String,
    /// The settlement asset.
    pub asset: String,
    /// The settlement network.
    pub network: String,
    /// A single-use nonce: 256 bits of OS randomness, base64url, 43 chars.
    ///
    /// [`verify`] rejects any value that is not exactly [`NONCE_LEN`]
    /// base64url characters before it ever reaches the shared
    /// [`NonceCache`], so a counterparty cannot grow the cache's memory
    /// footprint by signing an oversized nonce. See [`mint_nonce`].
    pub nonce: String,
    /// The authorization timestamp, epoch seconds.
    pub timestamp: i64,
    /// The base58 Ed25519 signature over [`canonical_bytes`].
    #[serde(rename = "signature")]
    pub signature_b58: String,
}

/// How many random bytes back an authorization nonce. 32 bytes = 256 bits,
/// matching the user-auth secrets, so two mints colliding is not a scenario.
const NONCE_BYTES: usize = 32;

/// The exact length of a [`mint_nonce`] output: unpadded base64url of
/// [`NONCE_BYTES`] bytes.
const NONCE_LEN: usize = (NONCE_BYTES * 4).div_ceil(3);

/// Mints an authorization nonce: 256 bits from `src`, base64url, 43 chars.
///
/// A pure function of the source bytes — no clock, no counter, no process
/// state — which is the property that makes one nonce say nothing about the
/// next, and makes two processes minting in the same millisecond differ.
pub fn mint_nonce(src: &dyn TokenSource) -> String {
    let mut bytes = [0u8; NONCE_BYTES];
    src.fill(&mut bytes);
    b64url_encode(&bytes)
}

/// Whether `nonce` has the exact shape [`mint_nonce`] produces: [`NONCE_LEN`]
/// unpadded base64url characters. [`verify`] enforces this before the nonce
/// ever reaches the shared [`NonceCache`], so a counterparty cannot grow that
/// cache's memory footprint by signing an authorization around an oversized
/// nonce.
fn has_nonce_shape(nonce: &str) -> bool {
    nonce.len() == NONCE_LEN
        && nonce
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Builds the canonical bytes an x402 authorization signs. See module docs.
pub fn canonical_bytes(
    agent_id: &str,
    amount: &str,
    recipient: &str,
    asset: &str,
    network: &str,
    nonce: &str,
    timestamp: i64,
) -> Vec<u8> {
    format!(
        "{X402_DOMAIN}\n{agent_id}\n{amount}\n{recipient}\n{asset}\n{network}\n{nonce}\n{timestamp}"
    )
    .into_bytes()
}

/// Signs an authorization paying exactly the challenged amount.
pub fn authorize(signer: &LocalSigner, ch: &X402Challenge, now: i64) -> X402Authorization {
    authorize_amount(signer, ch, ch.amount.clone(), now)
}

/// Signs a delegated-signer `upto` authorization capped at `cap`, letting the
/// counterparty settle any amount up to the cap.
pub fn authorize_upto(
    signer: &LocalSigner,
    ch: &X402Challenge,
    cap: &str,
    now: i64,
) -> X402Authorization {
    authorize_amount(signer, ch, cap.to_string(), now)
}

fn authorize_amount(
    signer: &LocalSigner,
    ch: &X402Challenge,
    amount: String,
    now: i64,
) -> X402Authorization {
    let agent_id = signer.agent_id();
    let nonce = mint_nonce(&OsTokens);
    let msg = canonical_bytes(
        &agent_id,
        &amount,
        &ch.recipient,
        &ch.asset,
        &ch.network,
        &nonce,
        now,
    );
    let signature_b58 = signer.sign_b58(&msg);
    X402Authorization {
        agent_id,
        amount,
        recipient: ch.recipient.clone(),
        asset: ch.asset.clone(),
        network: ch.network.clone(),
        nonce,
        timestamp: now,
        signature_b58,
    }
}

/// Verifies an authorization and spends its nonce, so one signature buys one
/// task.
///
/// Enforces, in order: the nonce's shape, the signature against the declared
/// `agentId`, freshness within [`MAX_AGE_SECS`], and single use of the nonce
/// against `spent`.
///
/// `spent` and `now` are parameters rather than something a caller may choose
/// to consult. A signature proves who authorized the payment, not that the
/// payment has not already been collected; a caller that could verify without
/// spending would be treating the authorization as a bearer token, which is
/// precisely the bug this signature shape exists to prevent.
///
/// The signature is checked before the nonce is spent, so an unverifiable
/// authorization cannot burn a nonce — otherwise anyone who observed a payer's
/// nonce could spend it on their behalf with a forged signature.
pub fn verify(auth: &X402Authorization, spent: &NonceCache, now: i64) -> Result<()> {
    if !has_nonce_shape(&auth.nonce) {
        return Err(OpenCompanyError::InvalidRequest(
            "x402 authorization nonce is not a valid mint_nonce value".into(),
        ));
    }

    let msg = canonical_bytes(
        &auth.agent_id,
        &auth.amount,
        &auth.recipient,
        &auth.asset,
        &auth.network,
        &auth.nonce,
        auth.timestamp,
    );
    verify_b58(&auth.agent_id, &msg, &auth.signature_b58)?;

    if now.abs_diff(auth.timestamp) > MAX_AGE_SECS as u64 {
        return Err(OpenCompanyError::InvalidRequest(format!(
            "x402 authorization timestamp is outside the ±{MAX_AGE_SECS}s window"
        )));
    }

    if !spent.check_and_insert(&auth.nonce, now, auth.timestamp)? {
        return Err(OpenCompanyError::InvalidRequest(
            "x402 authorization nonce has already been spent (replay)".into(),
        ));
    }

    Ok(())
}

fn string_field(obj: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = obj.get(*key).and_then(|v| v.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

#[cfg(test)]
#[path = "x402_tests.rs"]
mod tests;
