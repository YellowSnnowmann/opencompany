//! The enable/disable decision: whether this process reports at all, and where.
//!
//! Kept apart from both the transport and the payload because it is the part
//! that has to be *provably* right. It is pure — an [`EnvSource`] and a
//! [`Deployment`] in, a [`Decision`] out — so every branch of it is tested in
//! the default build, with no network and no feature flag.

use crate::analytics::types::TenantIdKey;
use crate::app::config::EnvSource;
use crate::app::deployment::Deployment;

/// Operator override: `on` forces reporting, `off` forbids it.
pub const ENABLE_ENV: &str = "OPENCOMPANY_ANALYTICS";
/// The OpenPanel client id. Half of the pair; useless on its own.
pub const CLIENT_ID_ENV: &str = "OPENCOMPANY_ANALYTICS_CLIENT_ID";
/// The OpenPanel client secret. **Configuration, never a compiled-in constant**
/// — a secret baked into a public binary is a secret everyone has, and this one
/// grants write access to the operator's collector.
pub const CLIENT_SECRET_ENV: &str = "OPENCOMPANY_ANALYTICS_CLIENT_SECRET";
/// The collector URL. **Required, with no default**, because a self-hosted
/// collector has no canonical address and guessing one means reporting to
/// somebody else's — see [`resolve`].
pub const ENDPOINT_ENV: &str = "OPENCOMPANY_ANALYTICS_ENDPOINT";
/// The secret that makes a hosted tenant's analytics id unguessable.
///
/// **Configuration, never a compiled-in constant**, and for a sharper reason
/// than the project token: a salt baked into a GPL-3.0 binary is a salt every
/// reader of the source already has, which is no salt at all. Injected by the
/// platform that provisions tenants; never given to the collector. Absent means
/// the host identifies itself by its random instance id instead — see
/// [`TenantIdKey`](crate::analytics::types::TenantIdKey).
pub const ID_KEY_ENV: &str = "OPENCOMPANY_ANALYTICS_ID_KEY";

/// An OpenPanel write client: an id and a secret, which authenticate together.
///
/// OpenPanel takes both as request **headers** — `openpanel-client-id` and
/// `openpanel-client-secret` — rather than as a field in the body, which is the
/// one structural difference from the token this replaced. It is a difference
/// worth having: a credential in a header never rides through the payload
/// builder, so no test fixture, recorded event or captured body can carry it.
///
/// A newtype rather than two bare `String`s for one reason: neither half must be
/// printed, logged, or serialized by accident. It derives **neither** `Debug`
/// nor `Serialize` — the hand-written `Debug` redacts both halves — because
/// `serde_json::to_value(&some_config)` is precisely how a credential reaches a
/// payload (issue #1741, `SecretValue`). Nothing in this module ever serializes
/// a config struct; the two values are read out explicitly, once, at the moment
/// the request headers are set.
///
/// **The id is redacted too**, although OpenPanel's own web SDK ships client ids
/// to browsers and treats them as public. The reason is local rather than
/// cryptographic: an id names the operator's project on the operator's
/// collector, this repository is GPL-3.0 and its container logs are routinely
/// pasted into public issues, and there is no line in the tree that is better
/// for having it. Redacting the half that does not need it costs nothing;
/// leaking the half that does costs everything, and a type with one printable
/// field and one redacted one is a type someone eventually prints.
#[derive(Clone, PartialEq, Eq)]
pub struct ClientCredentials {
    id: String,
    secret: String,
}

impl ClientCredentials {
    /// Wraps a client id and secret read from configuration.
    pub fn new(id: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            secret: secret.into(),
        }
    }

    /// The client id, for the one caller that puts it in a header.
    pub fn expose_id(&self) -> &str {
        &self.id
    }

    /// The client secret, for the one caller that puts it in a header.
    pub fn expose_secret(&self) -> &str {
        &self.secret
    }
}

impl std::fmt::Debug for ClientCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClientCredentials(<redacted>)")
    }
}

/// Why a process is not reporting. Logged once at boot, so an operator who
/// *expected* analytics can tell "switched off" from "misconfigured".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Silence {
    /// The operator set `OPENCOMPANY_ANALYTICS=off`.
    OptedOut,
    /// Not a hosted tenant, and nobody opted in. **The default.**
    NotHosted,
    /// Reporting was asked for, but neither half of the collector credential is
    /// configured.
    ///
    /// Three reasons rather than one, because the three call for different
    /// edits. OpenPanel authenticates a write client with an id **and** a
    /// secret, so "no credential" and "half a credential" are different
    /// mistakes: the second is what a half-finished secret rollout looks like,
    /// and an operator staring at "no credential is configured" while
    /// `OPENCOMPANY_ANALYTICS_CLIENT_ID` is plainly set in their env file has
    /// been told something that reads as false.
    NoCredentials,
    /// A client secret is configured, but no client id.
    NoClientId,
    /// A client id is configured, but no client secret.
    NoClientSecret,
    /// A credential is configured that could not be put in an HTTP header.
    ///
    /// New with OpenPanel and worth its own reason. Mixpanel's token rode in
    /// the request **body**, where any string at all is legal JSON, so a
    /// mangled token was refused by the collector and that was the end of it.
    /// These two ride in headers, and `reqwest` will not build a request whose
    /// header value contains a control byte — so a secret that picked up a
    /// stray newline in the middle (a `kubectl create secret` on a wrapped
    /// file, most often) would otherwise install a tracker that fails to
    /// construct one single request, forever, behind a `debug!` nobody reads.
    ///
    /// The reason never quotes the value, for the same reason
    /// [`Self::UnusableEndpoint`] does not.
    UnusableCredential,
    /// Reporting was asked for, but no collector endpoint is configured.
    ///
    /// **There is deliberately no default to fall back to.** OpenPanel is
    /// self-hosted, so its address is whatever the operator runs it at, and
    /// there is no address this crate could pick that is not somebody else's
    /// collector. Defaulting would send a tenant's telemetry to a third party
    /// nobody configured — the same failure [`Self::UnusableEndpoint`] exists to
    /// prevent, arriving from the other direction. So an absent endpoint is
    /// silence with its own reason, and the reason names the variable to set.
    NoEndpoint,
    /// `OPENCOMPANY_ANALYTICS` was set to something this does not recognise.
    ///
    /// A separate reason from [`Self::OptedOut`] on purpose: an operator who
    /// typed `of` gets the outcome they meant *and* a boot line saying their
    /// value was not understood, rather than silence they cannot distinguish
    /// from a working opt-out.
    Unreadable,
    /// `OPENCOMPANY_ANALYTICS_ENDPOINT` is set to something no client could
    /// POST to — no scheme, a scheme that is not `http`/`https`, no host, or
    /// bytes this process cannot read.
    ///
    /// Silence rather than reporting, because the alternative is the failure
    /// this whole module is built to prevent: boot prints "reporting to …",
    /// the tracker is installed, and every batch dies in `reqwest` behind a
    /// `debug!` nobody has enabled. An operator reading their own logs would
    /// have no reason to look again. Naming it as a *reason* is the only thing
    /// that turns a silent misconfiguration into one line they can act on.
    ///
    /// The reason is a constant and never quotes the value: an authenticated
    /// proxy's URL is exactly where a credential lives — see
    /// `crate::analytics::boot`.
    UnusableEndpoint,
    /// `OPENCOMPANY_ANALYTICS_ENDPOINT` is a plain `http://` URL to a host that
    /// is not loopback, so the client credential would cross a network in the
    /// clear.
    ///
    /// New with OpenPanel, and it exists because of *where* the credential
    /// travels now. Mixpanel's token rode in the request body to one fixed,
    /// TLS-only address that this crate chose; there was no configuration that
    /// could downgrade it. OpenPanel's address is whatever the operator types,
    /// and its credential rides in a request **header** on every single
    /// request — so `OPENCOMPANY_ANALYTICS_ENDPOINT=http://collector.internal/track`
    /// puts a long-lived write secret on the wire, in cleartext, once per event,
    /// for the life of the tenant (CWE-319). "Internal network" is not a defence
    /// a container can verify, and this module does not get to assume one.
    ///
    /// **Loopback is the documented exception.** `http://127.0.0.1:3000/track`,
    /// `http://[::1]:3000/track` and `http://localhost:3000/track` never leave
    /// the host, so there is no wire to read; that is the shape a developer
    /// running the collector beside the workload actually uses, and every gated
    /// test in this crate. Refusing it would refuse the only http case that is
    /// genuinely safe.
    ///
    /// Silence rather than a warning-and-send, for the reason
    /// [`Self::UnusableEndpoint`] gives one level down: the alternative is a
    /// boot line nobody reads while the secret ships anyway, and a credential
    /// disclosed is not a thing an operator can un-disclose after noticing. The
    /// fix is one character in one variable, and the reason names it.
    ///
    /// The reason never quotes the value, like every other reason here.
    InsecureEndpoint,
}

impl Silence {
    /// The stable reason slug, for the boot log line.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OptedOut => "operator opted out",
            Self::NotHosted => "not a hosted tenant and no explicit opt-in",
            Self::NoCredentials => {
                "no collector credential is configured (OPENCOMPANY_ANALYTICS_CLIENT_ID \
                 and OPENCOMPANY_ANALYTICS_CLIENT_SECRET)"
            }
            Self::NoClientId => "OPENCOMPANY_ANALYTICS_CLIENT_ID is not configured",
            Self::NoClientSecret => "OPENCOMPANY_ANALYTICS_CLIENT_SECRET is not configured",
            Self::UnusableCredential => {
                "the configured collector credential contains bytes that cannot go in an \
                 HTTP header"
            }
            Self::NoEndpoint => "OPENCOMPANY_ANALYTICS_ENDPOINT is not configured",
            Self::Unreadable => "the OPENCOMPANY_ANALYTICS value is not recognised",
            Self::UnusableEndpoint => {
                "the OPENCOMPANY_ANALYTICS_ENDPOINT value is not a usable http(s) URL"
            }
            Self::InsecureEndpoint => {
                "OPENCOMPANY_ANALYTICS_ENDPOINT is a plain http:// URL to a non-loopback \
                 host, which would send the collector credential in the clear on every \
                 request; use https, or a loopback address"
            }
        }
    }
}

/// What this process will do.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    /// Send nothing. No client is constructed, so nothing *can* be sent.
    Silent(Silence),
    /// Report to `endpoint` as `credentials`.
    Report {
        /// The collector URL.
        endpoint: String,
        /// The write client this process authenticates as.
        credentials: ClientCredentials,
    },
}

impl Decision {
    /// Whether this decision reports.
    pub fn reports(&self) -> bool {
        matches!(self, Self::Report { .. })
    }
}

/// Resolves the decision.
///
/// The order matters and is the whole policy:
///
/// 1. `OPENCOMPANY_ANALYTICS=off` wins over everything. An operator switching
///    it off must not be overruled by a deployment kind, a token, or a future
///    default.
/// 2. A value that is set but unrecognised resolves to **silence**, whatever
///    the deployment. The deployment default is reserved for a switch that is
///    *absent*. Falling an unreadable value through to the default meant a
///    hosted tenant whose operator typed `OPENCOMPANY_ANALYTICS=of` kept
///    reporting — a typo in the opt-out direction silently ignored, which is
///    the one direction that must never be silently ignored.
/// 3. Otherwise reporting is on **only** for [`Deployment::HostedTenant`], or
///    when an operator explicitly sets `OPENCOMPANY_ANALYTICS=on`. Decision 1
///    of #1739: silence is the default and reporting is the exception, so a
///    self-hosted or desktop install that has said nothing sends nothing.
/// 4. **Both halves of the client credential are required.** OpenPanel
///    authenticates a write client with an id and a secret together, so one
///    without the other is a misconfiguration rather than a partial
///    configuration, and the reason says which half is missing — see
///    [`Silence::NoClientId`].
/// 5. **An endpoint is required, and there is no default.** The collector is
///    self-hosted; its address is whatever the operator runs it at. A default
///    would be somebody else's collector, and quietly reporting to a third
///    party nobody configured is the accident the endpoint check below already
///    refuses to make in the other direction.
/// 6. And the endpoint has to be one a client could post to. A decision that
///    says [`Decision::Report`] is a promise the boot line then repeats out
///    loud, so an endpoint that cannot be sent to is silence with a reason,
///    not reporting — see [`is_usable_endpoint`].
/// 7. **And one the credential can safely cross.** The client secret is a
///    request header on every request, so a plain `http://` endpoint to a
///    non-loopback host puts it on the wire in cleartext once per event. That
///    is silence with its own reason too — see [`is_secure_endpoint`] and
///    [`Silence::InsecureEndpoint`].
pub fn resolve(deployment: Deployment, env: &dyn EnvSource) -> Decision {
    // Read through `get_os`, not `get`. [`EnvSource::get`] maps a non-Unicode
    // value to `None`, which here would read as "the operator said nothing" and
    // leave a hosted tenant reporting — the same failure as the unreadable
    // spelling below, arriving by a different route. The trait's own docs point
    // a reader that must tell *malformed* from *unset* at `get_os` for exactly
    // this reason.
    //
    // Blank is still absent, for the same reason a blank token is: a variable
    // set to whitespace is a variable nobody meant to set. See [`non_blank`].
    let switch = match env.get_os(ENABLE_ENV) {
        Some(raw) => match raw.into_string() {
            Ok(value) => {
                let value = value.trim().to_ascii_lowercase();
                if value.is_empty() { None } else { Some(value) }
            }
            Err(_) => return Decision::Silent(Silence::Unreadable),
        },
        None => None,
    };

    match switch.as_deref() {
        Some("off" | "false" | "0" | "no") => return Decision::Silent(Silence::OptedOut),
        Some("on" | "true" | "1" | "yes") => {}
        // Set, but not a spelling of yes or no. Both directions of that typo
        // are now silence: it was never an opt-in, and — since it reached a
        // hosted tenant's deployment default and kept reporting — it must not
        // be a failed opt-*out* either. Silence is the safe answer to "I cannot
        // tell what you asked for", and the boot line says which value it could
        // not read.
        Some(_) => return Decision::Silent(Silence::Unreadable),
        None => {
            if deployment != Deployment::HostedTenant {
                return Decision::Silent(Silence::NotHosted);
            }
        }
    }

    // A non-Unicode half already fails closed on its own: `get` maps it to
    // `None` and the match below reports it missing. Reporting *less* than was
    // configured is always the safe direction for a credential.
    let credentials = match (
        non_blank(env, CLIENT_ID_ENV),
        non_blank(env, CLIENT_SECRET_ENV),
    ) {
        (Some(id), Some(secret)) => {
            if !is_header_safe(&id) || !is_header_safe(&secret) {
                return Decision::Silent(Silence::UnusableCredential);
            }
            ClientCredentials::new(id, secret)
        }
        (None, None) => return Decision::Silent(Silence::NoCredentials),
        (None, Some(_)) => return Decision::Silent(Silence::NoClientId),
        (Some(_), None) => return Decision::Silent(Silence::NoClientSecret),
    };

    // Read through `get_os`, like the switch, so that bytes this process cannot
    // decode are *unusable* rather than *absent*. The two now resolve to
    // different reasons, and an operator who mistyped their proxy URL should be
    // told the value was unreadable rather than that they never set one.
    //
    // There is no fallback in either arm. Reporting to a default collector an
    // operator never named is worse than reporting nothing at all: it is
    // telemetry leaving for an address nobody chose, and no amount of reading
    // the boot line would reveal it, because the line would name a destination
    // that is real.
    let endpoint = match env.get_os(ENDPOINT_ENV) {
        None => return Decision::Silent(Silence::NoEndpoint),
        Some(raw) => match raw.into_string() {
            Err(_) => return Decision::Silent(Silence::UnusableEndpoint),
            Ok(value) => match value.trim() {
                // Blank is absent, as it is for the credential and the switch.
                "" => return Decision::Silent(Silence::NoEndpoint),
                // Shape before transport security, and the order matters for
                // the reason an operator is given: a value that does not parse
                // has no host to judge, and "this will not parse" sends them
                // somewhere different from "this would leak the secret".
                configured if !is_usable_endpoint(configured) => {
                    return Decision::Silent(Silence::UnusableEndpoint);
                }
                configured if !is_secure_endpoint(configured) => {
                    return Decision::Silent(Silence::InsecureEndpoint);
                }
                configured => configured.to_string(),
            },
        },
    };

    Decision::Report {
        endpoint,
        credentials,
    }
}

/// Whether `raw` is something a client could actually POST a batch to: an
/// absolute `http`/`https` URL with a host.
///
/// This is the check that stops [`resolve`] promising what the transport cannot
/// deliver. `OPENCOMPANY_ANALYTICS_ENDPOINT=collector.internal/track` — a
/// hostname written without a scheme, which is how anyone would first write it
/// — resolved to [`Decision::Report`]: boot said "reporting to
/// collector.internal/track", the tracker was installed, and every send failed
/// with `RelativeUrlWithoutBase` behind a `debug!` line. Nothing an operator
/// would ever see said the endpoint was the problem.
///
/// It matters more now than it did, because there is no default endpoint to
/// fall back to: every reporting deployment types this variable by hand.
///
/// **Parsed with `url`, the same crate `reqwest` parses with, rather than
/// approximated.** The first version of this check hand-rolled the grammar to
/// avoid what it wrongly believed would be a new dependency — `url` has been an
/// unconditional one since issue #673, added there with the rule this check
/// should have followed: it must be *the same* parser `reqwest` uses, because
/// "a grant key computed by a second, hand-rolled reader is a bypass waiting to
/// be found". The hand-rolled version accepted five shapes `reqwest` rejects
/// outright:
/// `http://[::1/track` (unclosed bracket), `http://host:99999/track` and
/// `:65536` (port out of range), `http://host:abc/track`,
/// `http://host:8080:9090/track`, and `http://999.999.999.999/track`. Each one
/// resolved to `Report` and then dropped every batch — the exact failure the
/// check exists to prevent, reintroduced by the check itself. The IPv4-shaped-
/// host rule (`127.0.0.1.5` is rejected, `exa_mple.com` is not) is the tell
/// that the tail here is unbounded: an approximation of a grammar this fiddly
/// is a standing source of the same bug. One parser, and it is the transport's
/// own.
///
/// Two things are still checked beyond parsing, because `url` is happy with
/// both and `reqwest` is not:
///
/// * **the scheme.** `url` parses `ftp://collector.internal/track` and
///   `reqwest` will even *build* a request from it; the send then fails with
///   "URL scheme is not allowed". Measured, not assumed.
/// * **a non-empty host**, defensively. No input has been found where `url`
///   returns a parsed `http`/`https` URL with an empty host — `https://` is a
///   parse error, and `http:///track` is *not* the counter-example it looks
///   like, because `url` normalizes it to `http://track/`, taking the first
///   path segment as the host. The guard stays because "there is somewhere to
///   connect to" is the property actually being asserted, and it should not
///   rest on a normalization rule holding forever.
///
/// This asks a different question from the endpoint redaction in
/// `crate::analytics::boot` — that one is about what may be *printed* — so the
/// two are not two halves of one rule.
fn is_usable_endpoint(raw: &str) -> bool {
    let Ok(parsed) = url::Url::parse(raw) else {
        return false;
    };
    matches!(parsed.scheme(), "http" | "https")
        && parsed.host_str().is_some_and(|host| !host.is_empty())
}

/// Whether the client credential can cross `raw` without being readable on the
/// wire: `https`, or `http` to a loopback host.
///
/// This asks a different question from [`is_usable_endpoint`] — that one is
/// "can a client send here at all", this one is "may this client's secret go
/// there" — and they are kept apart because they resolve to different reasons
/// and send an operator to different edits.
///
/// The rule exists because of where the OpenPanel credential travels.
/// Mixpanel's token rode in the body of a request to one fixed `https` address
/// this crate chose; no configuration could downgrade it. OpenPanel's address
/// is typed by the operator and its secret is a **request header on every
/// request**, so `http://collector.internal/track` writes a long-lived write
/// credential to the network in cleartext once per event, forever
/// ([CWE-319](https://cwe.mitre.org/data/definitions/319.html)). A container
/// cannot verify anyone's claim that the network in between is private, so this
/// does not assume it.
///
/// **Loopback is the exception, and it is a real one.** Traffic to
/// `127.0.0.0/8`, `::1` or `localhost` **does not leave the host**: it goes over
/// the host's loopback interface and reaches no link anyone else is on, so there
/// is no wire between machines for it to be read off. It is also how the
/// collector is run beside the workload in development and in every gated test
/// in this crate. Refusing it would refuse the one `http` case that is actually
/// safe.
///
/// That is deliberately narrower than "nobody can see it", because the narrower
/// claim is the true one: a sufficiently privileged local process can capture
/// `lo`. It does not weaken the exception, though — anything with that access on
/// a tenant's host can already read the process environment the credential was
/// loaded from, so the capture gains it nothing it did not have. The property
/// this rests on is that the secret never crosses a network **between hosts**,
/// which is what CWE-319 is about.
///
/// `localhost` is matched **by exact name**, not by suffix. RFC 6761 reserves
/// `*.localhost` for loopback as well, and a resolver may honour that — but
/// "may" is not a property this check can rest a credential on, and the strict
/// subset is the safe direction: it can only refuse an endpoint that would have
/// worked, loudly, with a named reason and a one-character fix. Widening it
/// later costs nothing; narrowing it after a secret has shipped costs the
/// secret.
///
/// Matched on `Url::host()` rather than on the raw string, so that
/// `http://127.0.0.1:3000/track`, `http://[::1]/track` and
/// `http://user@localhost/track` are all judged on the host `url` actually
/// parsed out, and a value like `http://127.0.0.1.evil.example/track` — which
/// merely *starts* with a loopback address — is not.
pub(crate) fn is_secure_endpoint(raw: &str) -> bool {
    let Ok(parsed) = url::Url::parse(raw) else {
        return false;
    };
    // `Url` lower-cases the scheme while parsing, so `HTTPS://…` arrives here
    // as `https` and needs no case handling of its own.
    if parsed.scheme() == "https" {
        return true;
    }
    match parsed.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

/// Whether `raw` is something the transport could put in an HTTP header.
///
/// **Deliberately a strict subset of what `http::HeaderValue` accepts**, not an
/// approximation of it: every byte must be printable ASCII with no space
/// (`0x21..=0x7E`). `HeaderValue` is more permissive — it takes space, tab and
/// the whole `0xA0..=0xFF` range — so anything this accepts, `reqwest` accepts,
/// and the subset direction is the safe one. A check that were merely
/// *approximate* could accept a value the transport then refuses, which is
/// exactly the "boot said reporting and nothing was ever sent" failure
/// [`is_usable_endpoint`] exists to prevent; a strict subset cannot.
///
/// It is written here rather than deferred to `reqwest` for the reason
/// [`Silence::UnusableCredential`] gives: this module is un-gated and un-feature
/// -flagged on purpose, so the whole decision is provable in the default build,
/// with no network and no `reqwest` in the graph (`--no-default-features` drops
/// it entirely). The gated transport test asserts the subset claim against
/// `HeaderValue::from_str` itself, so the two cannot drift apart silently.
///
/// The cost of being strict is refusing a credential OpenPanel would have
/// accepted. An OpenPanel client id and secret are generated opaque tokens —
/// this has never been observed to reject one — and the failure is loud, named
/// and reversible, which is the direction to be wrong in.
fn is_header_safe(raw: &str) -> bool {
    !raw.is_empty() && raw.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}

/// The tenant-identity key, if this deployment configured one.
///
/// Read through `get`, not `get_os`, and that is deliberate rather than an
/// oversight of the rule the switch and the endpoint follow: there is no
/// unsafe direction to fail into here. A key that cannot be read is treated as
/// absent, and absent means the host falls back to its random instance id —
/// which is *more* private than any keyed digest, not less. The distinction
/// `get_os` buys elsewhere ("malformed must not read as unset") only matters
/// when unset is the dangerous answer, and here it is the safe one.
pub fn tenant_id_key(env: &dyn EnvSource) -> Option<TenantIdKey> {
    non_blank(env, ID_KEY_ENV).and_then(TenantIdKey::new)
}

/// A configured value, trimmed, or `None` when there is nothing left of it.
///
/// [`EnvSource::get`] already drops an *empty* value, but not a whitespace-only
/// one, and the difference is not academic: a token mounted from a file arrives
/// with a trailing newline more often than not. Untrimmed, a hosted tenant whose
/// token is `"\n"` resolves to [`Decision::Report`], the boot line says
/// "reporting to …", and every batch is refused by the collector — the failure
/// mode #1739 added that line to prevent.
///
/// The endpoint is trimmed by [`resolve`] itself rather than here, because it
/// has to be read through [`EnvSource::get_os`] to tell an unreadable value from
/// an absent one.
///
/// The same trim-and-filter the rest of the tree applies to environment values
/// (`src/bin/opencompany.rs`).
fn non_blank(env: &dyn EnvSource, key: &str) -> Option<String> {
    env.get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
#[path = "config_credential_tests.rs"]
mod tests_credential;
#[cfg(test)]
#[path = "config_endpoint_tests.rs"]
mod tests_endpoint;
