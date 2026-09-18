//! The provider-agnostic outbound mail adapter.
//!
//! Sending mail has two independent axes, and this module separates them:
//!
//! - **What to send** — [`OutboundEmail`], the same for every provider.
//! - **How to send it** — [`MailCredentials`], a provider-tagged enum. The tag
//!   is what makes the adapter pluggable: a stored or configured credential
//!   blob names its own transport, so nothing has to be told out-of-band which
//!   provider it belongs to.
//!
//! [`MailSender`] is the seam. It takes `MailCredentials` rather than any one
//! provider's credential type, so adding AWS SES, Resend, Postmark, or anything
//! else means adding a variant plus a sender — and because the variant makes
//! every existing `match` non-exhaustive, the compiler names each place that
//! has to account for it. That is the point of the enum over a `Box<dyn Any>`
//! style config.
//!
//! ## Adding a provider
//!
//! 1. Add a credentials struct and a [`MailCredentials`] variant for it.
//! 2. Add a [`MailProvider`] variant and map it in [`MailCredentials::provider`].
//! 3. Implement [`MailSender`] for it in its own module, behind its own feature
//!    so the default build keeps linking no network crates.
//! 4. Teach [`MailConfig::from_env`] to resolve it.
//!
//! ## Two credential scopes
//!
//! - **Host-level** ([`MailConfig::from_env`], `OPENCOMPANY_MAIL_*`): one
//!   provider for the whole host. This is what platform mail — login links —
//!   uses, because a login link is sent on the platform's behalf, not the
//!   company's.
//! - **Per-company** (the company's `SecretStore` under `__smtp`): a company's
//!   own outbound identity, used by the test-send route and per-teammate mail.
//!
//! Both flow through the same [`MailSender`]; they differ only in where the
//! credentials come from.
//!
//! ## Note on AWS SES
//!
//! SES exposes an SMTP submission endpoint, so SES already works through
//! [`MailProvider::Smtp`] by pointing the host at
//! `email-smtp.<region>.amazonaws.com` with SES SMTP credentials. A native
//! `Ses` variant would only be worth adding for things the SMTP interface
//! cannot express — configuration sets, per-message tags, richer send errors.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::app::config::{EnvSource, ProcessEnv};
use crate::error::OpenCompanyError;
use crate::ports::types::SecretValue;
use crate::server::ops::imap::ImapCredentials;
use crate::server::ops::smtp::{SmtpCredentials, SmtpSecurity};

/// Which transport a set of [`MailCredentials`] belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MailProvider {
    /// Any SMTP submission server. Also covers AWS SES, Mailgun, SendGrid, and
    /// Postmark via their SMTP endpoints.
    #[default]
    Smtp,
}

impl std::fmt::Display for MailProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MailProvider::Smtp => f.write_str("smtp"),
        }
    }
}

impl std::str::FromStr for MailProvider {
    type Err = OpenCompanyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "smtp" => Ok(MailProvider::Smtp),
            other => Err(OpenCompanyError::Config(format!(
                "unknown mail provider {other:?}; supported: smtp"
            ))),
        }
    }
}

/// Credentials for one mail provider — **secret**.
///
/// Tagged by `provider` on the wire so a stored blob is self-describing.
///
/// `Debug` used to be hand-written here because [`SmtpCredentials`] derived one
/// that printed its password. Since issue #1770 the password is a
/// [`SecretValue`], so the derive is safe at every level and the container no
/// longer has to remember — which is the whole point of guarding the field's
/// type rather than each struct that holds one.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "lowercase")]
pub enum MailCredentials {
    /// An SMTP submission server.
    Smtp(SmtpCredentials),
}

impl MailCredentials {
    /// Which transport these credentials need.
    pub fn provider(&self) -> MailProvider {
        match self {
            MailCredentials::Smtp(_) => MailProvider::Smtp,
        }
    }

    /// The envelope address mail sent with these credentials comes from.
    pub fn from_email(&self) -> &str {
        match self {
            MailCredentials::Smtp(c) => &c.from_email,
        }
    }

    /// The display name on the `From` header, if one is configured.
    pub fn from_name(&self) -> &str {
        match self {
            MailCredentials::Smtp(c) => &c.from_name,
        }
    }
}

/// One outbound message handed to a [`MailSender`].
#[derive(Clone, Debug)]
pub struct OutboundEmail {
    /// Recipient address.
    pub to: String,
    /// Subject line.
    pub subject: String,
    /// Plain-text body.
    pub body: String,
}

/// The outbound-send seam.
///
/// Implementations are per-provider and select on the [`MailCredentials`]
/// variant. Mockable so every calling route is exercised offline; real
/// transports are feature-gated so the default build links no network crates.
#[async_trait]
pub trait MailSender: Send + Sync {
    /// Sends `email` using `creds`. An error means the message was not accepted.
    ///
    /// A sender handed credentials for a provider it does not implement must
    /// return [`OpenCompanyError::Config`] rather than panic — the binary may
    /// simply have been built without that provider's feature.
    async fn send(
        &self,
        creds: &MailCredentials,
        email: &OutboundEmail,
    ) -> Result<(), OpenCompanyError>;
}

/// One inbound message produced by a [`MailReceiver`]. Plain-text body (v1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboundEmail {
    pub from_name: String,
    pub from_email: String,
    pub subject: String,
    pub body: String,
}

/// One fetched-but-not-yet-acked message: the parsed [`InboundEmail`] plus the
/// IMAP UID it was fetched under. The UID (not a sequence number, which shifts
/// on expunge) is what [`MailReceiver::mark_seen`] takes, so filing and acking
/// stay correctly paired even if the mailbox changes between the two calls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchedEmail {
    pub uid: u32,
    pub email: InboundEmail,
}

/// The inbound-fetch seam. Implementations fetch *new* (unseen) messages
/// without marking them seen, and only do so once the caller confirms the
/// message was durably filed — see [`MailReceiver::mark_seen`]. Mockable so the
/// poller is exercised offline; the real transport is feature-gated.
#[async_trait]
pub trait MailReceiver: Send + Sync {
    /// Fetches messages not yet marked `\Seen`, without setting that flag
    /// (`UID FETCH ... BODY.PEEK[]` on the real transport) — a message stays
    /// unseen, and so is re-fetched, until [`MailReceiver::mark_seen`] is
    /// called for its UID.
    async fn fetch_new(
        &self,
        creds: &ImapCredentials,
    ) -> Result<Vec<FetchedEmail>, OpenCompanyError>;

    /// Marks `uids` `\Seen`. Callers must only pass UIDs of messages that have
    /// already been durably filed — this is the "ack" half of the fetch, kept
    /// separate so a storage failure never causes an unfiled message to be
    /// marked seen (and thus lost: it would never be re-fetched).
    async fn mark_seen(
        &self,
        creds: &ImapCredentials,
        uids: &[u32],
    ) -> Result<(), OpenCompanyError>;
}

/// Offline mock: returns queued batches, one per `fetch_new` call, counts
/// calls, and records every UID passed to `mark_seen`.
pub struct RecordingMailReceiver {
    batches: Mutex<std::collections::VecDeque<Vec<FetchedEmail>>>,
    calls: std::sync::atomic::AtomicUsize,
    marked: Mutex<Vec<u32>>,
}

impl RecordingMailReceiver {
    pub fn new() -> Self {
        Self {
            batches: Mutex::new(std::collections::VecDeque::new()),
            calls: Default::default(),
            marked: Mutex::new(Vec::new()),
        }
    }
    /// Queue a batch to be returned by the next `fetch_new`.
    pub fn push_batch(&self, batch: Vec<FetchedEmail>) {
        self.batches.lock().expect("poisoned").push_back(batch);
    }
    pub fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::Relaxed)
    }
    /// Every UID passed to `mark_seen` so far, in call order.
    pub fn marked(&self) -> Vec<u32> {
        self.marked.lock().expect("poisoned").clone()
    }
}

impl Default for RecordingMailReceiver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MailReceiver for RecordingMailReceiver {
    async fn fetch_new(
        &self,
        _creds: &ImapCredentials,
    ) -> Result<Vec<FetchedEmail>, OpenCompanyError> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(self
            .batches
            .lock()
            .expect("poisoned")
            .pop_front()
            .unwrap_or_default())
    }

    async fn mark_seen(
        &self,
        _creds: &ImapCredentials,
        uids: &[u32],
    ) -> Result<(), OpenCompanyError> {
        self.marked
            .lock()
            .expect("poisoned")
            .extend_from_slice(uids);
        Ok(())
    }
}

/// Host-level outbound mail configuration.
///
/// Resolved once, at boot, from `OPENCOMPANY_MAIL_*`. This is the platform's
/// own mail identity — the one login links are sent from. A company's own
/// outbound credentials live in its `SecretStore` instead, so a tenant is never
/// handed the platform's mail credential.
#[derive(Clone, Debug)]
pub struct MailConfig {
    /// The credentials to send platform mail with.
    pub credentials: MailCredentials,
}

impl MailConfig {
    /// Resolves host-level mail configuration from the environment.
    ///
    /// Returns `Ok(None)` when no provider is configured — mail is optional and
    /// its absence degrades a surface rather than failing a boot. But a
    /// *partial* configuration is an error, not a silent `None`: a deployment
    /// that set some of `OPENCOMPANY_MAIL_*` meant to have working mail, and
    /// discovering the typo when the first login link silently fails to arrive
    /// is worse than refusing here.
    pub fn from_env() -> Result<Option<Self>, OpenCompanyError> {
        Self::from_env_source(&ProcessEnv)
    }

    /// Resolves host-level mail configuration from an injected source.
    pub fn from_env_source(env: &dyn EnvSource) -> Result<Option<Self>, OpenCompanyError> {
        let var = |key: &str| env.get(key).filter(|v| !v.trim().is_empty());

        let provider: MailProvider = match var("OPENCOMPANY_MAIL_PROVIDER") {
            Some(raw) => raw.parse()?,
            // No provider named, but other mail vars present: default to smtp
            // rather than making PROVIDER=smtp boilerplate for the common case.
            None if var("OPENCOMPANY_MAIL_HOST").is_some() => MailProvider::Smtp,
            None => return Ok(None),
        };

        match provider {
            MailProvider::Smtp => {
                let missing = |key: &str| {
                    OpenCompanyError::Config(format!(
                        "{key} is required for OPENCOMPANY_MAIL_PROVIDER=smtp"
                    ))
                };
                let host =
                    var("OPENCOMPANY_MAIL_HOST").ok_or_else(|| missing("OPENCOMPANY_MAIL_HOST"))?;
                let from_email = var("OPENCOMPANY_MAIL_FROM_EMAIL")
                    .ok_or_else(|| missing("OPENCOMPANY_MAIL_FROM_EMAIL"))?;
                let port = match var("OPENCOMPANY_MAIL_PORT") {
                    Some(raw) => raw.parse::<u16>().map_err(|_| {
                        OpenCompanyError::Config(format!(
                            "OPENCOMPANY_MAIL_PORT must be a port number, got {raw:?}"
                        ))
                    })?,
                    None => 587,
                };
                let security = match var("OPENCOMPANY_MAIL_SECURITY") {
                    Some(raw) => match raw.trim().to_lowercase().as_str() {
                        "none" => SmtpSecurity::None,
                        "starttls" => SmtpSecurity::Starttls,
                        "ssl" | "tls" | "smtps" => SmtpSecurity::Ssl,
                        other => {
                            return Err(OpenCompanyError::Config(format!(
                                "unknown OPENCOMPANY_MAIL_SECURITY {other:?}; \
                                 supported: none, starttls, ssl"
                            )));
                        }
                    },
                    None => SmtpSecurity::default(),
                };
                Ok(Some(Self {
                    credentials: MailCredentials::Smtp(SmtpCredentials {
                        host,
                        port,
                        security,
                        username: var("OPENCOMPANY_MAIL_USERNAME").unwrap_or_default(),
                        password: SecretValue(var("OPENCOMPANY_MAIL_PASSWORD").unwrap_or_default()),
                        from_name: var("OPENCOMPANY_MAIL_FROM_NAME").unwrap_or_default(),
                        from_email,
                    }),
                }))
            }
        }
    }
}

/// A managed tenant's OWN mailbox identity, injected by the manager as
/// `OPENCOMPANY_MAIL_*`. Distinct from the host-level `OPENCOMPANY_MAIL_HOST/...`
/// platform-mail read by `MailConfig` (login links). Seeds the company's SMTP
/// send credentials AND the IMAP poller config.
#[derive(Clone)]
pub struct TenantMailboxConfig {
    pub address: String,
    pub smtp: SmtpCredentials,
    pub imap: ImapCredentials,
}

impl std::fmt::Debug for TenantMailboxConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately narrower than a derive, which would be safe on its own
        // since #1770 made both passwords `SecretValue`s: this renders one line
        // naming the two hosts rather than two nested credential structs.
        f.debug_struct("TenantMailboxConfig")
            .field("address", &self.address)
            .field("smtp_host", &self.smtp.host)
            .field("imap_host", &self.imap.host)
            .finish_non_exhaustive()
    }
}

impl TenantMailboxConfig {
    /// `Ok(None)` when unconfigured (no `OPENCOMPANY_MAIL_ADDRESS`); a *partial*
    /// injection is a hard error.
    pub fn from_env() -> Result<Option<Self>, OpenCompanyError> {
        Self::from_env_source(&ProcessEnv)
    }

    /// Resolves a tenant mailbox from an injected source.
    pub fn from_env_source(env: &dyn EnvSource) -> Result<Option<Self>, OpenCompanyError> {
        let var = |k: &str| env.get(k).filter(|v| !v.trim().is_empty());
        let Some(address) = var("OPENCOMPANY_MAIL_ADDRESS") else {
            return Ok(None);
        };
        let need = |k: &str| {
            var(k).ok_or_else(|| {
                OpenCompanyError::Config(format!(
                    "{k} is required when OPENCOMPANY_MAIL_ADDRESS is set"
                ))
            })
        };
        let port = |k: &str| -> Result<u16, OpenCompanyError> {
            need(k)?
                .parse::<u16>()
                .map_err(|_| OpenCompanyError::Config(format!("{k} must be a port number")))
        };
        let user = need("OPENCOMPANY_MAIL_USER")?;
        let password = SecretValue(need("OPENCOMPANY_MAIL_PASSWORD")?);
        let smtp_port = port("OPENCOMPANY_MAIL_SMTP_PORT")?;
        // The injected env carries no SECURITY var, so derive it from the port:
        // 465 = implicit TLS (SMTPS); everything else (587, 25, custom) = STARTTLS.
        // STARTTLS on 465 fails the handshake, so this must match the port.
        let security = if smtp_port == 465 {
            SmtpSecurity::Ssl
        } else {
            SmtpSecurity::Starttls
        };
        let smtp = SmtpCredentials {
            host: need("OPENCOMPANY_MAIL_SMTP_HOST")?,
            port: smtp_port,
            security,
            username: user.clone(),
            password: password.clone(),
            from_name: String::new(),
            from_email: address.clone(),
        };
        let imap = ImapCredentials {
            host: need("OPENCOMPANY_MAIL_IMAP_HOST")?,
            port: port("OPENCOMPANY_MAIL_IMAP_PORT")?,
            username: user,
            password,
        };
        Ok(Some(Self {
            address,
            smtp,
            imap,
        }))
    }
}

/// An offline mock sender that records every send and never fails. Used by
/// tests and any offline deployment.
#[derive(Clone, Default)]
pub struct RecordingMailSender {
    sent: Arc<Mutex<Vec<(String, OutboundEmail)>>>,
    presented: Arc<Mutex<Vec<MailCredentials>>>,
}

impl RecordingMailSender {
    /// Creates an empty recording sender.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every `(from_email, email)` sent so far.
    pub fn sent(&self) -> Vec<(String, OutboundEmail)> {
        self.sent.lock().expect("mail sender poisoned").clone()
    }

    /// The credentials each send actually presented, in order.
    ///
    /// Recorded alongside [`sent`](Self::sent) rather than folded into it, so
    /// existing callers keep their tuple. It exists because "which password
    /// reached the transport" is otherwise unobservable from a test, and that
    /// is precisely the question a patching `PUT …/smtp` has to answer: a save
    /// that omitted the password must still send under the stored one.
    pub fn presented(&self) -> Vec<MailCredentials> {
        self.presented.lock().expect("mail sender poisoned").clone()
    }
}

#[async_trait]
impl MailSender for RecordingMailSender {
    async fn send(
        &self,
        creds: &MailCredentials,
        email: &OutboundEmail,
    ) -> Result<(), OpenCompanyError> {
        self.sent
            .lock()
            .expect("mail sender poisoned")
            .push((creds.from_email().to_string(), email.clone()));
        self.presented
            .lock()
            .expect("mail sender poisoned")
            .push(creds.clone());
        Ok(())
    }
}

#[cfg(test)]
#[path = "mailer_tests.rs"]
mod tests;
