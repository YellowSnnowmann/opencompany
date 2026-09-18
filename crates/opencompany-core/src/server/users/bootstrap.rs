//! Issuing the *first* password for a company, from the host.
//!
//! # Why this exists
//!
//! Every way into a new company runs through a credential the deployment may
//! not be able to deliver (#1718):
//!
//! - `POST …/auth/password` needs a session, which is what we are trying to get.
//! - `POST …/users/{id}/password` and the invite routes need an existing
//!   **admin**, and on a first boot there is none.
//! - The magic link needs a mail transport. Its code is minted and stored
//!   *hashed*, so on a host with no transport the credential exists and is
//!   unreachable.
//! - The dev echo of that code is gated on [`AppConfig::is_local_only`], which
//!   is false for exactly the hosted deployment that has this problem.
//! - The platform hub needs the hub wired.
//!
//! So a self-hosted company with no mail and no hub could not be signed into at
//! all. The console said as much — *"an admin can issue you one if you have
//! none"* — with nobody to ask.
//!
//! # Why the host, and not another route
//!
//! This is deliberately **not** reachable over HTTP. The authority it relies on
//! is possession of the process and its storage, which an operator already has
//! and a request never does. Adding an HTTP surface would mean inventing a way
//! to authenticate the one caller who cannot yet authenticate.
//!
//! # What it will not do
//!
//! It issues a password only to an address that is *already* eligible — named
//! in the manifest's `[users] admins`, or injected as the deployment's
//! bootstrap admin. It cannot invent membership, so it is not a way to add
//! someone to a company; it only makes an existing standing invite usable
//! without mail.

use std::sync::Arc;

use crate::error::OpenCompanyError;
use crate::ports::generate_id;
use crate::ports::types::CompanyId;
use crate::ports::users::{UserRecord, UserRole, UserStatus, UserStore, normalize_email};
use crate::server::users::{password, token};

/// What [`issue_password`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issued {
    /// The address the password now belongs to, normalized.
    pub email: String,
    /// Whether the account was created, as opposed to an existing one updated.
    pub created: bool,
    /// Whether the holder must replace this password before doing anything else.
    pub must_change_password: bool,
}

/// The addresses a company admits without an invite record: its manifest
/// admins, plus the deployment's bootstrap admin when one is injected.
///
/// Both are the same grant, so they are one list. Shared with the HTTP path's
/// `bootstrap_admins` rather than re-derived, so the CLI cannot come to a
/// different answer than the login route about who is eligible.
pub fn standing_admins(manifest_admins: &[String], bootstrap_admin: Option<&str>) -> Vec<String> {
    let mut admins: Vec<String> = manifest_admins
        .iter()
        .map(|a| normalize_email(a))
        .filter(|email| !email.is_empty())
        .fold(Vec::new(), |mut admins, email| {
            if !admins.contains(&email) {
                admins.push(email);
            }
            admins
        });
    if let Some(email) = bootstrap_admin
        .map(normalize_email)
        .filter(|e| !e.is_empty())
        && !admins.contains(&email)
    {
        admins.push(email);
    }
    admins
}

/// The stores and company context used to issue a password.
///
/// Keeping these related inputs together also makes the host-side operation
/// harder to call with the wrong stores or company.
pub struct PasswordIssueContext<'a> {
    /// User persistence.
    pub users: &'a Arc<dyn UserStore>,
    /// Session persistence.
    pub sessions: &'a Arc<dyn crate::ports::sessions::SessionStore>,
    /// Login-code persistence.
    pub login_codes: &'a Arc<dyn crate::ports::login_codes::LoginCodeStore>,
    /// Company receiving the password.
    pub company: &'a CompanyId,
    /// Admin addresses declared by the company manifest.
    pub manifest_admins: &'a [String],
    /// Optional deployment-provided standing admin.
    pub bootstrap_admin: Option<&'a str>,
}

/// Sets `email`'s password in `company`, creating the account if the address is
/// eligible and has none.
///
/// `require_change` flags the account so the holder must replace the password
/// before doing anything else — the same treatment an admin-issued temporary
/// password gets, and the right default when the operator and the eventual
/// holder are different people.
pub async fn issue_password(
    context: PasswordIssueContext<'_>,
    email: &str,
    plaintext: &str,
    require_change: bool,
) -> Result<Issued, OpenCompanyError> {
    let PasswordIssueContext {
        users,
        sessions,
        login_codes,
        company,
        manifest_admins,
        bootstrap_admin,
    } = context;
    let email = normalize_email(email);
    if email.is_empty() {
        return Err(OpenCompanyError::InvalidRequest(
            "an email address is required".into(),
        ));
    }

    // Validated before anything is written, and against the address it will
    // belong to — `validate` refuses a password that contains its own email.
    password::validate(plaintext, &email)?;

    let existing = users.find_user_by_email(company, &email).await?;

    // Existing accounts may only be reset while retaining their administrative
    // role. A removed standing grant does not erase historical admin status.
    if let Some(existing) = existing.as_ref()
        && existing.role != UserRole::Admin
    {
        return Err(OpenCompanyError::InvalidRequest(format!(
            "{email} is not an admin account and cannot receive a host password reset"
        )));
    }

    // A suspended account cannot sign in at all — the password login path
    // refuses every non-active user — so committing a new password here would
    // only claim a success that can never be used. Refuse it outright instead
    // of persisting an unusable credential.
    if let Some(existing) = existing.as_ref()
        && existing.status != UserStatus::Active
    {
        return Err(OpenCompanyError::InvalidRequest(format!(
            "{email} is suspended and cannot receive a host password reset"
        )));
    }

    // Eligibility is only consulted when there is no account yet. An address
    // that already holds one keeps it even if the manifest later stops naming
    // them: removing someone is `status`, not a silent inability to reset.
    if existing.is_none() {
        let admins = standing_admins(manifest_admins, bootstrap_admin);
        if !admins.contains(&email) {
            return Err(OpenCompanyError::InvalidRequest(format!(
                "{email} is not a standing admin of `{}`, so there is no account to issue a \
                 password for. Add the address to the manifest's [users] admins, or set the \
                 deployment's bootstrap admin, and try again. This command makes an existing \
                 grant usable without mail; it does not create one.",
                company.as_ref()
            )));
        }
    }

    let hash = password::hash(&token::OsTokens, plaintext)?;
    let now = crate::ports::now_millis();
    let created = existing.is_none();

    let user = match existing {
        Some(mut user) => {
            user.password_hash = Some(hash);
            user.must_change_password = require_change;
            user.updated_at_millis = now;
            user
        }
        None => UserRecord {
            // The same id scheme the login path mints, so an account created
            // here is indistinguishable from one created by a magic link.
            id: generate_id(),
            email: email.clone(),
            display_name: None,
            avatar: None,
            // Eligibility above proved this address is a standing *admin*;
            // there is no other role this path can mint.
            role: UserRole::Admin,
            status: UserStatus::Active,
            password_hash: Some(hash),
            must_change_password: require_change,
            created_at_millis: now,
            last_seen_at_millis: None,
            updated_at_millis: now,
        },
    };

    // Revoke old credentials before the password commit. If either revocation
    // fails, no new password is persisted and the old credential state remains
    // the only usable state.
    if !created {
        sessions.delete_for_user(company, &user.id).await?;
    }
    login_codes.delete_for_email(company, &user.email).await?;
    users.upsert_user(company, &user).await?;
    // A newly minted account is the same materialization the login path
    // produces on redemption, so mark any outstanding invite redeemed the same
    // way — a manifest admin's bootstrapped invite record otherwise reads as
    // still pending beside an account that now exists. A bootstrap admin with
    // no invite record is a no-op.
    if created && let Some(mut invite) = users.find_invite_by_email(company, &email).await? {
        invite.accepted_at_millis = Some(now);
        users.upsert_invite(company, &invite).await?;
    }
    Ok(Issued {
        email,
        created,
        must_change_password: require_change,
    })
}

#[cfg(test)]
#[path = "bootstrap_tests.rs"]
mod tests;
