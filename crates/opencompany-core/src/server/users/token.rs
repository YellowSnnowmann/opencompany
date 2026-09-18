//! Minting and hashing the two secrets in the user-auth flow.
//!
//! There are exactly two: the **login code** carried in a magic link, and the
//! **session token** carried in a cookie. Both follow the same three rules.
//!
//! ## 1. They come from the OS CSPRNG, never from `generate_id`
//!
//! [`generate_id`](crate::ports::generate_id) is epoch-millis plus a counter —
//! excellent for record ids, and completely predictable. Anyone who can guess
//! roughly when a link was minted could enumerate its id. Secrets come from
//! [`TokenSource`] instead.
//!
//! ## 2. Only their hashes are stored
//!
//! The plaintext exists in exactly one place — the email that was sent, or the
//! browser's cookie jar — and is never written down. The stores hold
//! [`sha256_hex`] output, so a dump of the database (or of a backup, or of an
//! export) cannot be replayed as anyone.
//!
//! ## 3. Lookup is *by* the hash, so nothing is ever compared
//!
//! Both stores find a record by hashing what the caller presented and looking
//! that up. There is no "fetch the record, then compare the secret" step, which
//! is why this module has no constant-time comparison: there is no comparison.
//! Forging a hit would require a SHA-256 preimage.
//!
//! That property is bought with entropy, and it is why the login code is a
//! 256-bit token in a link rather than six digits to type. A six-digit code
//! *must* be looked up by email and compared, which then needs constant-time
//! equality, an attempt budget, and an atomic counter in all three backends to
//! resist a 10⁶ brute force. The link avoids all of it.

use sha2::{Digest, Sha256};

use crate::server::platform_auth::b64url_encode;

/// How long a magic link stays redeemable.
///
/// Short, because the link *is* the credential: it sits in a mailbox, may be
/// forwarded, and lands in the browser history of whoever clicks it. Long
/// enough to survive mail-delivery lag and a distracted human.
pub const LOGIN_CODE_TTL_MILLIS: u64 = 15 * 60 * 1000;

/// How long a session stays valid once minted.
///
/// Absolute, not sliding: extending on every request would mean a store write
/// per request, which on the fs backend is a whole-file rewrite. Revocation is
/// the lever for cutting a session short, not expiry.
pub const SESSION_TTL_MILLIS: u64 = 14 * 24 * 60 * 60 * 1000;

/// How long a paired device's session stays valid.
///
/// Much longer than a browser session, and for a different threat model. A
/// browser session is short because a browser is a shared, long-lived,
/// attack-exposed surface that its owner may walk away from. A paired device is
/// a specific machine its owner deliberately enrolled, holding its token in the
/// OS keychain rather than a cookie jar.
///
/// Absolute, like [`SESSION_TTL_MILLIS`], for the same reason: sliding expiry
/// would be a store write per request. A year is long enough that re-pairing is
/// not a recurring annoyance, and short enough that a device someone forgot
/// they enrolled does not stay a credential forever. Revocation remains the
/// lever for cutting one short.
pub const DEVICE_TTL_MILLIS: u64 = 365 * 24 * 60 * 60 * 1000;

/// How many random bytes back each secret. 32 bytes = 256 bits, which is why
/// guessing is not a threat model and the codes need no attempt budget.
const TOKEN_BYTES: usize = 32;

/// A source of cryptographically secure random bytes.
///
/// A seam, so tests can mint reproducible tokens without `unsafe` or global
/// state. Production always uses [`OsTokens`].
pub trait TokenSource: Send + Sync {
    /// Fills `out` with unpredictable bytes.
    fn fill(&self, out: &mut [u8]);
}

/// The real source: the operating system's CSPRNG.
#[derive(Clone, Copy, Debug, Default)]
pub struct OsTokens;

impl TokenSource for OsTokens {
    fn fill(&self, out: &mut [u8]) {
        // A CSPRNG failure means the OS cannot give us randomness. There is no
        // safe degraded behavior — falling back to anything predictable would
        // hand out forgeable credentials — so refuse loudly instead.
        getrandom::fill(out).expect("the OS CSPRNG is unavailable; cannot mint a secret");
    }
}

/// Mints an opaque session token: 256 bits, base64url, 43 chars.
///
/// Returned to the browser once and never stored; persist
/// [`sha256_hex`] of it instead.
pub fn mint_session_token(src: &dyn TokenSource) -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    src.fill(&mut bytes);
    b64url_encode(&bytes)
}

/// Mints a login code for a magic link: 256 bits, base64url, 43 chars.
///
/// Deliberately the same shape as a session token. It is URL-safe with no
/// escaping, which matters because it is pasted straight into a link.
pub fn mint_login_code(src: &dyn TokenSource) -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    src.fill(&mut bytes);
    b64url_encode(&bytes)
}

/// Lowercase-hex SHA-256 of `input`. The only form of a secret that is stored.
pub fn sha256_hex(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        // Infallible: writing to a String never fails.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
#[path = "token_tests.rs"]
mod tests;
