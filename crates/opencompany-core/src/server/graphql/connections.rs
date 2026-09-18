//! Connection / domain / SMTP status reads over the [`SecretStore`] reserved
//! keys. Every value here is a **non-secret projection**: the OAuth token
//! material and the SMTP password never appear in a response.

use std::sync::Arc;

use async_graphql::SimpleObject;

use crate::company::dns::DomainStatus;
use crate::company::runtime::CompanyRuntime;
use crate::server::ops::smtp::SmtpStatus;

/// One third-party connection's state: manifest intent plus live OAuth status.
#[derive(SimpleObject)]
#[graphql(name = "ConnectionState")]
pub struct ConnectionStateGql {
    /// The provider id (e.g. `slack`, `gmail`, `github`).
    pub provider: String,
    /// Whether an OAuth token is stored for this provider.
    pub connected: bool,
    /// Which route a Connect for this provider would take on this host:
    /// `attested` (the platform runs it — nothing to register here), `static`
    /// (a token already stored for this company, or this host's own registered
    /// provider application: the self-hosted hatch), or `none` (no Connect can
    /// succeed here). A tier name, never a credential and never a path.
    pub credential_source: String,
    /// The connected account label, when known.
    pub account: Option<String>,
    /// The manifest's stated reason for wanting this connection.
    pub reason: Option<String>,
}

/// A generated DNS record for custom-domain verification. Mirrors
/// [`DnsRecord`](crate::company::dns::DnsRecord).
#[derive(SimpleObject)]
#[graphql(name = "DnsRecord")]
pub struct DnsRecordGql {
    /// The record type (`CNAME` | `TXT`).
    #[graphql(name = "type")]
    pub record_type: String,
    /// The record name/host.
    pub name: String,
    /// The record value.
    pub value: String,
    /// The record TTL.
    pub ttl: String,
}

/// Custom-domain status. Mirrors [`DomainStatus`].
#[derive(SimpleObject)]
#[graphql(name = "DomainStatus")]
pub struct DomainStatusGql {
    /// The configured domain.
    pub domain: String,
    /// Whether the domain's records have been verified.
    pub verified: bool,
    /// The DNS records the operator must publish.
    pub records: Vec<DnsRecordGql>,
}

impl From<DomainStatus> for DomainStatusGql {
    fn from(status: DomainStatus) -> Self {
        Self {
            domain: status.domain,
            verified: status.verified,
            records: status
                .records
                .into_iter()
                .map(|record| DnsRecordGql {
                    record_type: record.record_type,
                    name: record.name,
                    value: record.value,
                    ttl: record.ttl,
                })
                .collect(),
        }
    }
}

/// Non-secret SMTP status: host/port/username only — never the password.
#[derive(SimpleObject)]
#[graphql(name = "SmtpStatus")]
pub struct SmtpStatusGql {
    /// The SMTP host (empty when unconfigured).
    pub host: String,
    /// The SMTP port (0 when unconfigured).
    pub port: i32,
    /// The SMTP username (empty when unconfigured).
    pub username: String,
    /// Whether SMTP is configured.
    pub configured: bool,
}

impl From<SmtpStatus> for SmtpStatusGql {
    fn from(status: SmtpStatus) -> Self {
        Self {
            host: status.host.unwrap_or_default(),
            port: status.port.map(i32::from).unwrap_or(0),
            username: status.username.unwrap_or_default(),
            configured: status.configured,
        }
    }
}

/// Resolves `Company.connections`: manifest intent merged with OAuth status.
///
/// Delegates the whole decision to
/// [`project_connections`](crate::server::ops::connections_read::project_connections),
/// the same function the REST plane serves, and only maps the result into the
/// GraphQL type. The two planes previously ran duplicate copies of the loop —
/// which is how they were free to drift, and why a provider could answer one
/// thing over REST and another over GraphQL (issue #316).
pub(crate) async fn resolve_connections(
    runtime: &Arc<CompanyRuntime>,
) -> async_graphql::Result<Vec<ConnectionStateGql>> {
    Ok(
        crate::server::ops::connections_read::project_connections(runtime.as_ref())
            .await?
            .into_iter()
            .map(|row| ConnectionStateGql {
                credential_source: row.credential_source.as_str().to_string(),
                provider: row.provider,
                connected: row.connected,
                account: row.account,
                reason: row.reason,
            })
            .collect(),
    )
}

/// Resolves `Company.domain`, returning null when no domain is configured.
///
/// Delegates to [`ops::domain::load_domain`](crate::server::ops::domain::load_domain),
/// the same read `GET …/domain` serves, for the reason
/// [`resolve_connections`] documents: two copies of one decision are free to
/// drift, and a domain that read as verified on one plane and not the other
/// would be exactly that drift (issue #316).
pub(crate) async fn resolve_domain(
    runtime: &Arc<CompanyRuntime>,
) -> async_graphql::Result<Option<DomainStatusGql>> {
    let stored = crate::server::ops::domain::load_domain(runtime.as_ref())
        .await
        .map_err(|e| async_graphql::Error::new(format!("stored domain status is invalid: {e}")))?;
    Ok(stored.map(DomainStatusGql::from))
}

/// Resolves `Company.smtp`: the non-secret SMTP projection (never the password).
///
/// Reads through [`ops::smtp::load_credentials`](crate::server::ops::smtp::load_credentials)
/// — the same load `GET …/smtp` and the test send use — and projects it here.
/// The projection is [`SmtpStatus::from_credentials`], which is what drops the
/// password; this resolver never sees a shape that could carry one onward.
pub(crate) async fn resolve_smtp(
    runtime: &Arc<CompanyRuntime>,
) -> async_graphql::Result<SmtpStatusGql> {
    let stored = crate::server::ops::smtp::load_credentials(runtime.as_ref())
        .await
        .map_err(|e| {
            async_graphql::Error::new(format!("stored SMTP credentials are invalid: {e}"))
        })?;
    let status = stored
        .as_ref()
        .map_or_else(SmtpStatus::unconfigured, SmtpStatus::from_credentials);
    Ok(status.into())
}
