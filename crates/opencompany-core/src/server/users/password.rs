//! Password hashing, verification, and policy.
//!
//! Passwords are an *optional* convenience alongside the magic link: a user may
//! set one to log in without waiting for mail, and a user who never sets one is
//! unaffected. [`UserRecord::password_hash`](crate::ports::UserRecord) is
//! `None` for them.
//!
//! ## Argon2id, and why the parameters are not the library defaults' business
//!
//! Hashes are Argon2id in PHC string format (`$argon2id$v=19$m=...`), which
//! embeds the algorithm, version, parameters, and salt. That is what lets
//! [`verify`] keep validating old hashes after the cost parameters are raised —
//! each hash carries the parameters it was made with.
//!
//! ## Two timing concerns, both real
//!
//! - **Verification** must not leak the password a byte at a time. `argon2`'s
//!   `verify_password` compares digests in constant time; nothing here compares
//!   a password with `==`.
//! - **Absence** must not leak. Verifying against a real hash takes ~50ms;
//!   returning early for an unknown email takes ~0ms. That difference is a
//!   user-enumeration oracle, which the magic-link path is careful not to
//!   provide, so the password path must not hand one back. [`dummy_verify`]
//!   burns the same work for an address with no account or no password.
//!
//! ## What is deliberately absent
//!
//! No composition rules (no "one uppercase, one digit"). NIST SP 800-63B
//! recommends against them: they push people toward `Password1!` and add no
//! entropy worth the friction. Length is the check that matters.

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};

use crate::error::OpenCompanyError;
use crate::server::users::token::TokenSource;

/// The shortest password accepted.
///
/// Above NIST's floor of 8. These accounts reach a company's chat, tasks, and
/// workspace, and the magic link is always available for anyone who would
/// rather not have a password at all.
pub const MIN_PASSWORD_LEN: usize = 12;

/// The longest password accepted.
///
/// Not a security limit — Argon2 has no meaningful input ceiling — but an
/// unbounded field is free CPU for whoever posts a megabyte to the login route.
pub const MAX_PASSWORD_LEN: usize = 512;

/// Bytes of salt per hash. 16 is the PHC/Argon2 recommendation.
const SALT_BYTES: usize = 16;

/// Checks a candidate password against policy.
///
/// `email` is compared against so that a password that is merely the account's
/// own address is refused — it is public, and it is the first thing anyone
/// tries.
pub fn validate(password: &str, email: &str) -> Result<(), OpenCompanyError> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(OpenCompanyError::InvalidRequest(format!(
            "a password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    if password.len() > MAX_PASSWORD_LEN {
        return Err(OpenCompanyError::InvalidRequest(format!(
            "a password may be at most {MAX_PASSWORD_LEN} bytes"
        )));
    }
    if password.trim().is_empty() {
        return Err(OpenCompanyError::InvalidRequest(
            "a password cannot be only whitespace".to_string(),
        ));
    }
    // Both sides trimmed: padding the address with spaces must not smuggle it
    // past this. The password itself is still stored untrimmed — leading and
    // trailing spaces are legitimate characters in a passphrase.
    if password.trim().eq_ignore_ascii_case(email.trim()) {
        return Err(OpenCompanyError::InvalidRequest(
            "a password cannot be your email address".to_string(),
        ));
    }
    Ok(())
}

/// Hashes `password` with Argon2id, returning a PHC string safe to store.
///
/// The salt comes from the crate's [`TokenSource`] rather than argon2's own RNG
/// feature, so there is one source of randomness in the process and tests can
/// make hashing deterministic.
pub fn hash(src: &dyn TokenSource, password: &str) -> Result<String, OpenCompanyError> {
    let mut salt_bytes = [0u8; SALT_BYTES];
    src.fill(&mut salt_bytes);
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|e| OpenCompanyError::Store(format!("password salt: {e}")))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| OpenCompanyError::Store(format!("password hash: {e}")))
}

/// Whether `password` matches the stored PHC `phc` hash.
///
/// Returns `false` — never an error — for a malformed stored hash too: a
/// corrupt record must fail closed as a wrong password, not 500 in a way that
/// distinguishes it.
pub fn verify(password: &str, phc: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(phc) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Burns the same work a real [`verify`] would, and discards it.
///
/// Call on every login where there is no hash to check — unknown address,
/// known address with no password set, suspended user — so that "no account
/// here" costs the same wall-clock as "wrong password". Without it, response
/// time answers the question the generic error message refuses to.
pub fn dummy_verify(password: &str) {
    // A fixed, valid Argon2id hash of a value nothing can log in with. Its
    // parameters match `Argon2::default()`, so the work matches a real verify.
    const DUMMY_PHC: &str = "$argon2id$v=19$m=19456,t=2,p=1$\
                             c29tZXNhbHRzb21lc2FsdA$\
                             Ik8jitpTS4/1sMkKY0YMlUj3PYm3W2v0wNKPRLGSaBM";
    let _ = verify(password, DUMMY_PHC);
}

#[cfg(test)]
#[path = "password_tests.rs"]
mod tests;
