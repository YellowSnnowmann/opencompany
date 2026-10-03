//! The OpenPanel transport, and the one function that chooses a tracker.
//!
//! [`build`] is compiled into **every** build; `HttpOpenPanelTracker` is
//! compiled only under `--features analytics`. That split is the acceptance
//! criterion of issue #1739 expressed as a type rather than as a rule: in a
//! default build there is no type here that owns an HTTP client, so "a build
//! with no opt-in emits zero outbound analytics requests" is not a behaviour
//! that could regress — the code that would make the request is not in the
//! binary.
//!
//! Under the feature, [`build`] still returns [`NullTracker`] for every
//! [`Decision::Silent`], which is what a desktop or self-hosted install resolves
//! to. `a_self_hosted_build_makes_no_request` proves that against a real local
//! collector, and `a_hosted_tenant_reports` is its positive control — without
//! the second, a zero request count would be indistinguishable from a test that
//! never sends anything at all.
//!
//! # What is here, and what is deliberately not
//!
//! Everything about *what* an event says lives in [`crate::analytics::payload`],
//! [`Envelope`] and [`Event`], which are un-gated and tested in every lane. This
//! file owns only the HTTP call: how the body is delivered, how the credential
//! is presented, and what happens when the collector does not answer. Keeping
//! that line where it is is why a content leak would be caught by the default
//! `cargo test` rather than only by the one lane that compiles `reqwest`.
//!
//! # One request per event
//!
//! **OpenPanel has no batch endpoint.** `POST /track` takes a single
//! discriminated-union object; there is no array body and no `/batch` route, so
//! the batching this module used to do has nowhere to go. See [`Inner::drain`]
//! for what replaced it and why the queue survived the batch.

use std::sync::Arc;

use crate::analytics::config::Decision;
use crate::analytics::{Envelope, NullTracker, Tracker};

/// Chooses the tracker this process will use.
///
/// The whole of the "hosted tenants only, by default" decision lands here: a
/// [`Decision::Silent`] gets a [`NullTracker`], and in a build without the
/// `analytics` feature *every* decision does, because there is nothing else to
/// return.
///
/// # A transport that cannot be built is a [`NullTracker`], never a degraded one
///
/// [`HttpOpenPanelTracker::new`] is fallible because the HTTP client it wraps is
/// where the credential header and the send timeout are configured, and both
/// are load-bearing. The obvious fallback — `reqwest::Client::default()` — is
/// the wrong answer twice: that client carries **no default headers**, so every
/// request goes out unauthenticated and is refused, and it carries **no
/// timeout**, so a slow collector parks a drain forever and `Tracker::flush`
/// waits behind it, which is exactly the shutdown block the five-second bound
/// exists to prevent. It is also not even a safe fallback in the case that
/// produces it: `Client::default()` is `Client::new()`, which is
/// `ClientBuilder::new().build().expect(…)` — the same `build` that just
/// failed, now panicking at boot instead of returning an error.
///
/// So a client that will not build disables reporting and says so once, loudly.
/// Sending nothing is a documented outcome of this module with a whole
/// vocabulary of reasons behind it; sending unauthenticated requests with no
/// timeout is not.
pub fn build(decision: &Decision, envelope: Envelope) -> Arc<dyn Tracker> {
    match decision {
        Decision::Silent(_) => Arc::new(NullTracker),
        #[cfg(feature = "analytics")]
        Decision::Report {
            endpoint,
            credentials,
        } => match http::HttpOpenPanelTracker::new(endpoint, credentials, envelope) {
            Ok(tracker) => Arc::new(tracker),
            // Routed through the same redaction the send path uses. A builder
            // error carries no request URL today, but "the dependency does not
            // print one here" is not a property this crate owns, and the
            // endpoint on the same line comes from the one helper that redacts.
            Err(error) => {
                tracing::warn!(
                    endpoint = %crate::analytics::boot::loggable_endpoint(endpoint),
                    error = %http::loggable_send_error(error),
                    "[analytics] the HTTP client for the collector could not be built, so \
                     reporting is off for this process. Nothing will be sent."
                );
                Arc::new(NullTracker)
            }
        },
        // Without the feature there is no transport to hand back. Reporting was
        // configured and the build cannot honour it, which is worth one line at
        // boot: silently ignoring an explicit `OPENCOMPANY_ANALYTICS=on` is the
        // kind of quiet no-op an operator debugs for an hour.
        #[cfg(not(feature = "analytics"))]
        Decision::Report { .. } => {
            let _ = envelope;
            tracing::info!(
                "[analytics] reporting is configured but this build was compiled without \
                 the `analytics` feature, so nothing is sent"
            );
            Arc::new(NullTracker)
        }
    }
}

#[cfg(feature = "analytics")]
pub use http::HttpOpenPanelTracker;

/// The header OpenPanel takes the client id in.
///
/// Named here rather than inline because the gated tests assert the exact
/// spelling: a header the collector does not recognise is a 401 behind a
/// `debug!`, which is the silent failure this whole module is built around.
#[cfg(feature = "analytics")]
pub const CLIENT_ID_HEADER: &str = "openpanel-client-id";

/// Names this client on the operator's own collector.
///
/// OpenPanel stores `openpanel-sdk-name` / `openpanel-sdk-version` on the event,
/// which is how an operator running one collector for several things tells this
/// traffic apart from a browser SDK's. It costs one header and answers "what is
/// writing to my project?" without anyone having to ask us.
#[cfg(feature = "analytics")]
pub const SDK_NAME_HEADER: &str = "openpanel-sdk-name";

/// The version half of the pair above.
#[cfg(feature = "analytics")]
pub const SDK_VERSION_HEADER: &str = "openpanel-sdk-version";

/// The value sent as [`SDK_NAME_HEADER`].
#[cfg(feature = "analytics")]
pub const SDK_NAME: &str = "opencompany";

/// **Event names OpenPanel refuses outright.**
///
/// `packages/constants/index.ts`, read at commit
/// `3060ca10213693cf0385be2713c8743d16733a2b`. A `track` whose `payload.name` is
/// one of these fails the collector's own zod refinement and comes back 400.
///
/// Copied here rather than merely known, because the failure it guards is the
/// one this module is least able to notice: a rejected event is a `debug!` line
/// and nothing else, so a name collision introduced years from now would look
/// exactly like a healthy instance that happens to report one fewer event. The
/// test below is a compile-time-vocabulary check against a runtime constant, and
/// it costs nothing to keep.
///
/// It is not the whole of OpenPanel's name validation — `event-blocklist.ts`
/// also rejects names over 80 characters, names containing a newline, names
/// beginning `/`, and a long anti-abuse substring list (`${`, `%{`, `../`,
/// `union select`, …). Those are asserted alongside it rather than transcribed:
/// transcribing a fifty-entry blocklist is how a copy goes stale.
pub const OPENPANEL_RESERVED_EVENT_NAMES: [&str; 2] = ["session_start", "session_end"];

#[cfg(feature = "analytics")]
mod http;

#[cfg(all(test, feature = "analytics"))]
#[path = "openpanel_collector_tests.rs"]
mod tests_collector;
#[cfg(all(test, feature = "analytics"))]
#[path = "openpanel_status_tests.rs"]
mod tests_status;
#[cfg(all(test, feature = "analytics"))]
#[path = "openpanel_transport_tests.rs"]
mod tests_transport;
