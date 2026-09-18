//! Wiring analytics into a booting host: one function, called once.
//!
//! Kept out of `src/bin/opencompany.rs` so it is testable. The binary's `serve`
//! arm is ~200 lines of sequencing that nothing else runs, and a decision this
//! consequential — whether a GPL-3.0 install reports — should not only be
//! exercised by starting a real server.

use std::sync::Arc;

use crate::analytics::config::{Decision, resolve};
use crate::analytics::types::{Envelope, OpaqueId};
use crate::analytics::{DeferredTracker, Event, Tracker, openpanel};
use crate::app::AppState;
use crate::app::config::EnvSource;
use crate::app::deployment::Deployment;

/// Chooses this process's tracker, installs it behind `handle`, and reports
/// `instance_started`.
///
/// Called **after** the host's companies are registered, for two reasons that
/// both come down to reporting what is true rather than what was configured:
///
/// * the context envelope names the cognition path, and that is a property of
///   the brain a runtime *builds* — see [`DeferredTracker`] for why the handle
///   is handed out before the tracker exists;
/// * `instance_started` carries the company count, which is not known until
///   they have been registered or adopted.
///
/// Returns the decision so the caller can say, in one line, why a host that an
/// operator expected to report is not reporting.
pub fn install(state: &AppState, handle: &DeferredTracker, env: &dyn EnvSource) -> Decision {
    let deployment = Deployment::from_env(env);
    let decision = resolve(deployment, env);

    // Identity, in the order #1739's decision 2 sets out: the tenant slug
    // (hashed) when the platform named one, else this host's own random
    // instance id. Never the company name, and never anything derived from the
    // hostname or the bind address — `crate::app::instance` argues that at
    // length for the same id on the same grounds.
    let id = identify(state, env);

    // The cognition seam the rest of the tree already uses, rather than a second
    // derivation of "which brain is this host on?" beside the code that picks
    // one.
    //
    // **This is a host-level label, not a per-company one, and the difference is
    // real.** Inference is configured per company, so `serve --company a
    // --company b` with one configured and one on the echo fallback gives two
    // cognition paths and one envelope — the first registered runtime answers
    // for both. `Envelope::set_cognition` then makes the label the *most
    // recently observed* rather than the first, which is right for the case it
    // exists for (a host whose one company is provisioned or rebuilt after
    // boot) and no more correct than the first for a genuinely mixed host.
    //
    // Making it per-company means moving cognition off the envelope's
    // super-properties and onto `turn_finished` and `turn_metered` themselves,
    // which changes the payload shape #1739 shipped — an analytics-contract
    // decision rather than a defect fix, and one `instance_started` (which has
    // no company) does not fit. Raised on PR #1751 and left for its own change.
    //
    // A host with no companies yet reports the default descriptor, which
    // honestly says `custom`/`unknown` until `observe_cognition` corrects it.
    let cognition = state
        .registry()
        .list()
        .first()
        .and_then(|id| state.registry().get(id))
        .map(|runtime| runtime.cognition())
        .unwrap_or_default();

    let envelope = Envelope::new(id, deployment, cognition);
    let tracker: Arc<dyn Tracker> = openpanel::build(&decision, envelope);
    handle.install(tracker);

    handle.track(Event::InstanceStarted {
        companies: state.registry().list().len() as u64,
        storage: state.storage_kind().as_str(),
        setup_complete: state.setup_complete() || !state.registry().is_empty(),
    });

    decision
}

/// Chooses the opaque id this host's events are attributed to.
///
/// Split out of [`install`] so the choice can be asserted directly. It was
/// inline, and the test that covered it recomputed the expected id itself and
/// compared the two — which passes whatever `install` actually does, and did:
/// a deliberate mutation making the keyless path fall back to a baked-in salt
/// went undetected. A decision this consequential has to be observable.
///
/// A tenant slug is identified by keyed digest, and **only** when the platform
/// configured a key. There is deliberately no unkeyed fallback: a plain hash of
/// a slug is not an opaque id, because the slug is usually the customer's brand
/// and a few thousand guesses invert it, and a salt compiled into a GPL-3.0
/// binary is one every reader of the source already has. Without a key the host
/// identifies *itself*, by the random instance id that names nobody's customer
/// — see [`OpaqueId`] for the full argument.
pub(crate) fn identify(state: &AppState, env: &dyn EnvSource) -> OpaqueId {
    match (
        state.config().tenant_namespace.as_deref(),
        crate::analytics::config::tenant_id_key(env),
    ) {
        (Some(tenant), Some(key)) => OpaqueId::tenant(crate::app::canonical_tenant(tenant), &key),
        _ => OpaqueId::instance(state.instance_id()),
    }
}

/// The one line a boot log carries about analytics.
///
/// Said out loud on purpose. Silence is the correct default, but a *silent*
/// default is how an operator spends an afternoon on a tenant that was never
/// going to report — and, in the other direction, a hosted tenant's operator is
/// entitled to see in their own logs that reporting is on.
///
/// It reports what the process will actually **do**, not what was configured,
/// and those differ in exactly one case: a build compiled without the
/// `analytics` feature resolves [`Decision::Report`] and then gets a
/// [`NullTracker`](crate::analytics::NullTracker) from
/// [`openpanel::build`](crate::analytics::openpanel::build), because there is no
/// transport in it to hand back. Saying "reporting to …" there is the exact
/// opposite of the truth, and the `openpanel::build` line that explains it is a
/// `tracing::info!` the CLI's default `EnvFilter` swallows — which is why every
/// other boot line here is a `println!`. So the build is named on this line
/// instead.
pub fn describe(decision: &Decision) -> String {
    match decision {
        Decision::Silent(reason) => {
            format!("analytics: off ({})", reason.as_str())
        }
        // The endpoint, never the credential — in either arm.
        Decision::Report { endpoint, .. }
            if crate::analytics::BuildFlags::of_this_build().analytics =>
        {
            format!("analytics: reporting to {}", loggable_endpoint(endpoint))
        }
        Decision::Report { endpoint, .. } => format!(
            "analytics: off (reporting to {} was configured, but this build was \
             compiled without the `analytics` feature)",
            loggable_endpoint(endpoint)
        ),
    }
}

/// The collector URL with anything credential-shaped removed.
///
/// `OPENCOMPANY_ANALYTICS_ENDPOINT` names the collector the operator self-hosts,
/// and such a collector is routinely reached through an authenticated proxy —
/// which carries its key in exactly the two places a URL can hold one: userinfo
/// (`https://user:pass@host/track`) and the query string
/// (`https://host/track?key=…`). Printing the raw value writes that secret
/// verbatim into container logs, which the [`ClientCredentials`] redaction does
/// nothing about — it guards two different strings.
///
/// A third place, which the two above do not reach: an opaque **path segment**,
/// as in `https://collector.example/ingest/<token>`, which is how a signed-URL
/// collector is usually configured. So the path is kept only as far as its first
/// segment — enough to name the route, which is what makes the line useful — and
/// the rest is elided rather than inspected, because "does this look like a
/// secret?" is not a question worth answering heuristically. The default
/// endpoint has a single segment (`/track`) and is unaffected.
///
/// So only the scheme, host and leading path segment are logged, which is all an
/// operator needs to answer "where is this going?". When something was removed
/// the line says so, because a silently shortened URL is its own hour of
/// confusion.
///
/// `pub(crate)` because the transport logs the same destination when a send
/// fails (`crate::analytics::openpanel`). One helper, deliberately: a second
/// redaction of the same string is a second thing to keep correct, and the two
/// diverge the first time only one of them learns about a new place a URL can
/// hold a secret.
///
/// [`ClientCredentials`]: crate::analytics::config::ClientCredentials
pub(crate) fn loggable_endpoint(raw: &str) -> String {
    // Query and fragment first: `?key=…` is at least as common as userinfo.
    let trimmed = raw.split(['?', '#']).next().unwrap_or(raw);

    // Then userinfo, and only within the authority — a path may legitimately
    // contain `@`, and truncating there would report the wrong destination.
    let cleaned = match trimmed.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, path) = match rest.split_once('/') {
                Some((authority, path)) => (authority, Some(path)),
                None => (rest, None),
            };
            let host = authority
                .rsplit_once('@')
                .map_or(authority, |(_userinfo, host)| host);
            match path {
                // Only the **first** path segment. A proxy can sign a request
                // with an opaque path segment too — `https://collector/ingest/<token>`
                // is how a signed-URL collector is usually configured — and that
                // segment is a credential in a place neither the userinfo nor
                // the query strip reaches. The first segment names the route,
                // which is what makes the line useful; anything after it is
                // elided rather than guessed at, because "does this look like a
                // secret?" is not a question worth answering heuristically.
                //
                // An OpenPanel collector's route is one segment (`/track`), so
                // the ordinary line is unchanged.
                Some(path) => {
                    let mut segments = path.split('/');
                    let first = segments.next().unwrap_or("");
                    if segments.any(|segment| !segment.is_empty()) {
                        format!("{scheme}://{host}/{first}/…")
                    } else {
                        format!("{scheme}://{host}/{path}")
                    }
                }
                None => format!("{scheme}://{host}"),
            }
        }
        None => trimmed.to_string(),
    };

    if cleaned == raw {
        cleaned
    } else {
        format!("{cleaned} (credentials redacted)")
    }
}

#[cfg(test)]
#[path = "boot_tests.rs"]
mod tests;
