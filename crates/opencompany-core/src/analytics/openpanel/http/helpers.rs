//! Pure helpers for the HTTP transport: the request headers, the cleartext and
//! collector-wide predicates, and the one redacted rendering of a send error.
//!
//! Split out of `http.rs` to keep it under the 750-line cap; nothing here holds
//! state.

use crate::analytics::config::ClientCredentials;

/// Every header this client sends on every request: the client id, marked
/// sensitive, and the two that name the client.
pub(in crate::analytics::openpanel) fn request_headers(
    credentials: &ClientCredentials,
) -> reqwest::header::HeaderMap {
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

    let sensitive = |raw: &str| {
        let mut value = HeaderValue::from_str(raw).unwrap_or_else(|_| HeaderValue::from_static(""));
        value.set_sensitive(true);
        value
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static(crate::analytics::openpanel::CLIENT_ID_HEADER),
        sensitive(credentials.expose_id()),
    );
    // Compile-time constants, both. They name this client on a collector the
    // operator may be pointing several things at.
    headers.insert(
        HeaderName::from_static(crate::analytics::openpanel::SDK_NAME_HEADER),
        HeaderValue::from_static(crate::analytics::openpanel::SDK_NAME),
    );
    if let Ok(version) = HeaderValue::from_str(env!("CARGO_PKG_VERSION")) {
        headers.insert(
            HeaderName::from_static(crate::analytics::openpanel::SDK_VERSION_HEADER),
            version,
        );
    }
    headers
}

/// Whether `endpoint` is a plain `http` URL, and so one whose safety rests
/// on the request never leaving the host.
///
/// Parsed with `url` rather than matched on a `http://` prefix, for the
/// reason `config::is_usable_endpoint` gives at length: the transport's own
/// parser is the only one whose answer is the operative one, and `HTTP://`
/// is a legal spelling that a prefix match reads as safe.
///
/// A value that does not parse answers `false`, which is the harmless
/// direction *here* — it can only leave the system proxy enabled for an
/// endpoint that `resolve` has already refused to report to, so no request
/// is ever built from it.
pub(in crate::analytics::openpanel) fn is_cleartext(endpoint: &str) -> bool {
    url::Url::parse(endpoint).is_ok_and(|parsed| parsed.scheme() == "http")
}

/// Whether `status` is the collector's answer about **itself** rather than
/// about the event that happened to be in flight.
///
/// Three statuses reach the drain that are not per-event verdicts, and they
/// split by whether they resolve on their own. A `401` and a `3xx` are
/// permanent misconfigurations, so each gets its own said-once `warn!`.
/// These are the transient half: `429` is the collector or its proxy asking
/// for less traffic, and a `5xx` is it failing to serve at all. Neither says
/// anything about the body that was posted, so every event behind it in the
/// queue would get the same answer.
///
/// Without this the drain treated them as a rejected *event* and carried on,
/// which is the worst available response to `503`: up to [`MAX_QUEUED`](super::MAX_QUEUED)
/// requests aimed at a service that has just said it is overloaded, and
/// again at the next [`FLUSH_INTERVAL`](super::FLUSH_INTERVAL), for as long as the collector stays
/// down. That is the same runaway [`Inner::report_refused_credential`](super::Inner) was
/// added to stop, arriving from the transient direction — an analytics
/// client should not be the thing that keeps an operator's collector down.
///
/// **`408` and `425` are deliberately not here.** Both are arguably
/// retryable, but neither is evidence the collector is unwell, and widening
/// this predicate costs a whole drain each time it is wrong. `4xx` other
/// than `401` and `429` stays per-event, which is the reading that loses the
/// least when it is mistaken: one dropped event rather than a whole drain.
pub(in crate::analytics::openpanel) fn is_collector_wide(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

/// The one rendering of a transport failure this module is allowed to log.
///
/// `reqwest::Error` keeps the request URL and prints it — `… for url (…)` —
/// and that URL is `OPENCOMPANY_ANALYTICS_ENDPOINT`. A self-hosted collector
/// is routinely reached through an authenticated proxy, and that is
/// precisely where the proxy's key lives: in userinfo
/// (`https://user:key@host/track`) or in the query string (`?key=…`). So a
/// collector that merely goes unreachable wrote the operator's credential
/// into container logs, on a path the boot line's redaction never touched
/// and the `ClientCredentials` redaction guards different strings from
/// entirely.
///
/// `without_url` **removes** the URL rather than rewriting it, which is why
/// this is not a second redaction surface to keep in step with
/// `boot::loggable_endpoint`. There is nothing here to diverge: the error
/// carries no URL at all, and the destination on the same log line comes
/// from that one helper, so the transport learns about a new place a URL can
/// hold a secret at the same moment the boot line does.
pub(in crate::analytics::openpanel) fn loggable_send_error(error: reqwest::Error) -> String {
    error.without_url().to_string()
}
