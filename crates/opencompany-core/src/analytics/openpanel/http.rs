//! The reqwest-backed OpenPanel tracker, compiled only under
//! `--features analytics`.
//!
//! Moved out of `openpanel.rs` (which was past the 750-line cap); the parent
//! keeps the decision function [`super::build`], the header-name constants and
//! the reserved-name list, and re-exports [`HttpOpenPanelTracker`] so every
//! `crate::analytics::...` path resolves.

mod guards;
mod helpers;
mod stats;

use guards::CancelledDrain;
use helpers::{is_cleartext, is_collector_wide};
pub(super) use helpers::{loggable_send_error, request_headers};
use stats::SendStats;

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;

use crate::analytics::config::ClientCredentials;
use crate::analytics::status::{AnalyticsStatus, LastSend};
use crate::analytics::{Envelope, Event, Tracker, payload};

/// How often the background task drains the queue.
///
/// A threshold alone is not enough: a quiet instance would hold its events
/// until the next one arrived, which on a company that ran two turns and
/// stopped is forever.
const FLUSH_INTERVAL: Duration = Duration::from_secs(30);

/// The most events held before the oldest are dropped.
///
/// Analytics must never be able to grow without bound inside a tenant
/// container. If the collector is unreachable for long enough to fill this,
/// the right outcome is losing telemetry, not the process.
const MAX_QUEUED: usize = 500;

/// How long a send may take before it is abandoned. Short on purpose:
/// nothing waits on this, but a request that never completes is a task that
/// never ends.
const SEND_TIMEOUT: Duration = Duration::from_secs(5);

/// When the one-shot first drain runs, counted from construction.
///
/// The periodic loop waits [`FLUSH_INTERVAL`] before its first drain, so
/// without this a freshly booted host would have nothing to report on `/spec`
/// and no first-send self-check line for half a minute. One extra drain, run
/// once: it never blocks a turn (it is its own task), never retries, and an
/// empty queue makes it a no-op. Analytics still never delays a turn.
pub(super) const FIRST_DRAIN_DELAY: Duration = Duration::from_secs(5);

/// Queues events and POSTs them to OpenPanel, one request each.
pub struct HttpOpenPanelTracker {
    inner: Arc<Inner>,
}

struct Inner {
    /// Carries the client-id header as a **default header**, set once at
    /// construction and marked sensitive.
    ///
    /// This is the whole of the credential handling, and it is the part of
    /// the change worth reading twice. Mixpanel wanted its token stamped
    /// into every event's property bag, so the transport had to reach into
    /// a rendered payload and mutate it — which meant a captured body, a
    /// recorded event or a test fixture could carry the credential, and the
    /// only thing stopping it was that nothing did. OpenPanel authenticates
    /// with request headers, so the credential is set once, here, and never
    /// touches the body builder at all. There is no longer a code path that
    /// could put it in a payload.
    ///
    /// `HeaderValue::set_sensitive` on it, which keeps it out of
    /// `HeaderValue`'s own `Debug` and out of HPACK's shared table on
    /// HTTP/2.
    client: reqwest::Client,
    endpoint: String,
    /// Behind a lock because its cognition labels are re-read after boot
    /// — see [`Envelope::set_cognition`]. Only ever held to render one
    /// payload or to relabel, never across an await.
    envelope: std::sync::RwLock<Envelope>,
    queue: Mutex<Vec<serde_json::Value>>,
    /// Held for the whole of one `drain`, take **and** requests.
    ///
    /// Without it, the shutdown flush and the 30-second drain could
    /// overlap: the drain takes the entire queue and awaits its POSTs, the
    /// flush finds an empty queue, returns at once, and process exit
    /// cancels the requests still in flight. That loses events exactly when
    /// the collector is slow — the one case the graceful flush exists for.
    /// An **async** mutex because it is held across an await; the `queue`
    /// lock below stays a `std::sync` one and is never held across one.
    sending: tokio::sync::Mutex<()>,
    stop: tokio::sync::Notify,
    /// Whether the collector has already told us the credential is no good.
    ///
    /// A `401` is not a failure like the others. Every other thing that can
    /// go wrong here is transient — a collector restarting, a network
    /// blip — and deserves the `debug!` that #1739 settled on, because it
    /// resolves itself and a `warn!` per event would be a log flood for a
    /// problem nobody needs to act on. A refused credential resolves itself
    /// never: every event for the rest of the process's life is dropped, the
    /// boot line said "reporting to …", and the only trace is a `debug!` no
    /// operator has enabled. That is the exact failure this module exists to
    /// make impossible, arriving one layer below where the boot line can see
    /// it.
    ///
    /// So it is a `warn!`, and it is said **once**: the condition is
    /// permanent, so repeating it adds nothing and would drown the log of a
    /// busy tenant.
    credential_refused: std::sync::atomic::AtomicBool,
    /// Whether the collector has already answered with a redirect.
    ///
    /// The client follows none of them — see
    /// [`HttpOpenPanelTracker::new`] — which closes the credential leak and
    /// opens a diagnostic hole in its place: a `3xx` arrives here as an
    /// ordinary non-success response, so a misconfigured endpoint would
    /// look exactly like a collector rejecting every event, behind a
    /// `debug!` nobody has enabled, forever. That is the failure shape this
    /// module exists to refuse.
    ///
    /// So a redirect gets the [`Self::credential_refused`] treatment: it is
    /// a verdict on the *endpoint* rather than on one event, every event
    /// behind it gets the same one, and it is a `warn!` said exactly once.
    endpoint_redirects: std::sync::atomic::AtomicBool,
    /// How many events have been lost to a **cancelled** drain.
    ///
    /// Every other way a drain ends states its own count in its own log
    /// line. Cancellation cannot: the future is dropped, so there is no
    /// branch to log from. [`CancelledDrain`] reports it on `Drop` and
    /// records the total here, which makes the loss assertable — a log line
    /// alone is not something a test can hold to account.
    lost_to_cancellation: std::sync::atomic::AtomicUsize,
    /// What the transport has done so far; see [`SendStats`].
    stats: SendStats,
    /// The deployment slug the status reports, from the envelope at
    /// construction. Deployment never changes after boot.
    deployment: &'static str,
}

impl HttpOpenPanelTracker {
    /// Builds a tracker and starts its drain loop.
    ///
    /// The credential is validated for header-safety in
    /// [`crate::analytics::config::resolve`], which is why the
    /// `from_str` call here can fall back rather than fail: by the time a
    /// [`Decision::Report`](crate::analytics::config::Decision::Report)
    /// exists, the client id is printable ASCII with no space, which is a
    /// strict subset of what `HeaderValue` takes. The fallback is an empty
    /// header value, which the collector refuses with a 401 — a loud,
    /// bounded outcome rather than a panic at boot, for a branch that is
    /// unreachable given the check upstream.
    ///
    /// # Fallible, because there is no acceptable degraded client
    ///
    /// The client built here is the only place the credential header and
    /// [`SEND_TIMEOUT`] are set, so a client built without them is not a
    /// weaker version of this one — it is one that authenticates against
    /// nothing and can hang a shutdown. [`super::build`] turns the error
    /// into a `NullTracker` and one `warn!`; see the note there for why
    /// `reqwest::Client::default()` is not the fallback it looks like.
    ///
    /// # Redirects are never followed
    ///
    /// `reqwest`'s default policy follows up to ten hops, and its
    /// cross-origin sanitization removes only `Authorization`, `Cookie`,
    /// `cookie2`, `Proxy-Authorization` and `WWW-Authenticate`
    /// (`redirect.rs::remove_sensitive_headers`, reqwest 0.12.28, read
    /// rather than assumed). `openpanel-client-id` is none of those, so a `302` from the configured endpoint to any other
    /// authority — a reverse proxy sending unauthenticated callers to an
    /// SSO host is the ordinary way one arrives — would have handed this
    /// instance's write credential to a host the operator never named.
    /// `HeaderValue::set_sensitive` does not help: it governs `Debug` and
    /// HPACK indexing, not redirect handling.
    ///
    /// That sanitization also compares only **host and port**, never the
    /// scheme, so an `https` endpoint that redirected to `http://` on the
    /// same host would have carried the credential across in cleartext — the
    /// `Silence::InsecureEndpoint` rule in
    /// [`crate::analytics::config`] bypassed by a response the operator
    /// does not control.
    ///
    /// So: [`reqwest::redirect::Policy::none`], with no same-origin
    /// exception. A same-origin policy would also be safe, but it is a
    /// predicate to keep correct rather than an invariant to state, and all
    /// it buys is a collector that 301s `/track` to `/api/track` — an
    /// endpoint the operator can type correctly once, after reading the
    /// warning [`Inner::report_redirected_endpoint`] emits. Following none
    /// of them makes "the credential only ever goes to the configured
    /// endpoint" a property of this client rather than a claim about a
    /// comparison.
    ///
    /// # A cleartext endpoint never goes through a proxy
    ///
    /// The same hole as the redirect one, by a different route, and it
    /// invalidates the loopback exception rather than merely widening it.
    /// `resolve` permits plain `http` only for a loopback host, and the
    /// entire justification is that such a request **does not leave the
    /// host** — so there is no wire between machines for the credential to
    /// be read off. A proxy makes that false. `reqwest`'s builder defaults
    /// to `auto_sys_proxy: true` (`async_impl/client.rs:309`), which pushes
    /// `ProxyMatcher::system()`, and that reads `HTTP_PROXY`/`ALL_PROXY`
    /// with exclusions taken **only** from `NO_PROXY` — hyper-util 0.1.20's
    /// matcher has no implicit carve-out for `localhost` or `127.0.0.0/8`,
    /// checked rather than assumed. So on a host with `HTTP_PROXY` set and
    /// no matching `NO_PROXY`, `http://localhost:3000/track` was sent to the
    /// proxy instead, in cleartext, with the credential header on it.
    ///
    /// So the cleartext case builds with
    /// [`reqwest::ClientBuilder::no_proxy`], which makes "it does not leave
    /// the host" true by construction instead of by assumption about the
    /// operator's environment. That is the same move as
    /// `redirect::Policy::none()`: a security property should be a fact
    /// about this client, not a prediction about its surroundings.
    ///
    /// **`https` keeps its proxy support, deliberately.** A proxied `https`
    /// request is a `CONNECT` tunnel: the proxy learns the host and port and
    /// never sees a header, so the credential is not exposed to it, and
    /// egress-restricted networks genuinely need it to reach a collector at
    /// all. Disabling proxies outright would break those deployments to fix
    /// a leak they do not have.
    ///
    /// The scheme is the whole test, because by the time a
    /// [`Decision::Report`](crate::analytics::config::Decision::Report)
    /// exists, `http` **implies** loopback — `config::is_secure_endpoint`
    /// has already refused every other `http` endpoint.
    ///
    /// # Crate-private, because that implication is the invariant
    ///
    /// The sentence above is only true of endpoints that came through
    /// [`resolve`](crate::analytics::config::resolve). While this
    /// constructor was `pub` it was also a way around it: the type is
    /// re-exported from a `pub mod`, so an `analytics`-enabled caller could
    /// hand it `http://collector.internal/track` directly and get a tracker
    /// that posts the client id across a network in cleartext, with
    /// [`is_cleartext`] dutifully turning off the proxy on the way. A
    /// safety property enforced only by the route callers happen to take is
    /// the thing this module keeps arguing against, so the route is now the
    /// only one there is: [`super::build`] takes a `&Decision`, and a
    /// `Decision::Report` is what `resolve` produces.
    ///
    /// The `debug_assert!` is defence in depth against the same mistake
    /// arriving from *inside* the crate later. It calls
    /// `config::is_secure_endpoint` rather than restating the rule, because
    /// a second reader of a security predicate is a bypass waiting to be
    /// found — the same reason `is_usable_endpoint` refuses to hand-roll the
    /// URL grammar `reqwest` already parses.
    pub(crate) fn new(
        endpoint: &str,
        credentials: &ClientCredentials,
        envelope: Envelope,
    ) -> Result<Self, reqwest::Error> {
        debug_assert!(
            crate::analytics::config::is_secure_endpoint(endpoint),
            "a tracker was built for an endpoint the credential cannot safely cross; \
             every endpoint must come through config::resolve"
        );
        let mut builder = reqwest::Client::builder()
            .timeout(SEND_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .default_headers(request_headers(credentials));
        if is_cleartext(endpoint) {
            builder = builder.no_proxy();
        }
        let deployment = envelope.deployment.as_str();
        let inner = Arc::new(Inner {
            client: builder.build()?,
            endpoint: endpoint.to_string(),
            envelope: std::sync::RwLock::new(envelope),
            queue: Mutex::new(Vec::new()),
            sending: tokio::sync::Mutex::new(()),
            stop: tokio::sync::Notify::new(),
            credential_refused: std::sync::atomic::AtomicBool::new(false),
            endpoint_redirects: std::sync::atomic::AtomicBool::new(false),
            lost_to_cancellation: std::sync::atomic::AtomicUsize::new(0),
            stats: SendStats::default(),
            deployment,
        });

        // A `Weak` so the loop cannot keep the tracker alive, and
        // `try_current` so constructing one outside a runtime is a
        // flush-only tracker rather than a panic. Neither is theoretical:
        // the drop path is how a rebuilt runtime retires its tracker, and a
        // synchronous test constructs one with no reactor.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let weak = Arc::downgrade(&inner);
            handle.spawn(async move { drain_loop(weak).await });
            let weak = Arc::downgrade(&inner);
            handle.spawn(async move { first_drain(weak).await });
        }

        Ok(Self { inner })
    }

    /// How many events a **cancelled** drain has lost so far.
    ///
    /// Exists so the shutdown-budget loss is assertable rather than merely
    /// logged: a `warn!` is what an operator sees, and a counter is what a
    /// test can hold to account. Every other way a drain ends already names
    /// its own count in its own line.
    #[cfg(test)]
    pub(super) fn lost_to_cancellation(&self) -> usize {
        self.inner
            .lost_to_cancellation
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Drop for HttpOpenPanelTracker {
    fn drop(&mut self) {
        self.inner.stop.notify_waiters();
    }
}

impl std::fmt::Debug for HttpOpenPanelTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // No endpoint and certainly no credential: this type holds one, and
        // a `{:?}` in a log line is exactly how one escapes.
        f.write_str("HttpOpenPanelTracker")
    }
}

/// The one-shot drain [`FIRST_DRAIN_DELAY`] describes. Holds only a `Weak`,
/// like the loop, so a tracker dropped inside the delay costs nothing.
async fn first_drain(weak: Weak<Inner>) {
    tokio::time::sleep(FIRST_DRAIN_DELAY).await;
    if let Some(inner) = weak.upgrade() {
        inner.drain().await;
    }
}

async fn drain_loop(weak: Weak<Inner>) {
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let stopped = {
            let stop = &inner.stop;
            tokio::select! {
                _ = stop.notified() => true,
                _ = tokio::time::sleep(FLUSH_INTERVAL) => false,
            }
        };
        inner.drain().await;
        if stopped {
            return;
        }
    }
}

impl Inner {
    /// Takes everything queued and posts it, **one request per event**.
    /// Every failure is swallowed after one debug line: a dead collector is
    /// a no-op, per #1739's constraints.
    ///
    /// Serialized: a caller entering while another drain is in flight waits
    /// for it and then takes whatever has arrived since. That is what makes
    /// [`Tracker::flush`] a real guarantee rather than a queue inspection —
    /// see [`Inner::sending`].
    ///
    /// # Why the queue survived the loss of batching
    ///
    /// The obvious reading of "OpenPanel has no batch endpoint" is to drop
    /// the queue and fire a request from `track` itself. That is the wrong
    /// trade twice over. `track` is called from the cycle bracket and the
    /// usage meter, both on a turn's hot path, and it is **synchronous and
    /// infallible** by contract — it cannot await, so firing from it means
    /// spawning a task per event, which is unbounded concurrency against a
    /// collector the process does not control, with no back-pressure and no
    /// ceiling on memory. The queue is what bounds both: at most
    /// [`MAX_QUEUED`] events exist at once, and at most one drain runs at a
    /// time.
    ///
    /// # The unreachable collector, and why the drain gives up early
    ///
    /// Each request has its own [`SEND_TIMEOUT`]. Sequentially, a full
    /// queue against a black-holing collector would be
    /// `500 × 5s` — over forty minutes of a task doing nothing but proving
    /// the same thing five hundred times, and forty minutes in which the
    /// shutdown flush would block behind [`Inner::sending`] until its own
    /// budget cut it off. So a **transport** error abandons the rest of the
    /// drain: the collector is down, the remaining events are going
    /// nowhere, and the next interval will try again with whatever has
    /// accumulated since. They are dropped rather than requeued, because
    /// requeuing an unbounded backlog is how a bounded queue stops being
    /// bounded.
    ///
    /// An **HTTP status** failure does not abandon it. That is a per-event
    /// answer — a rejected event name, a payload the collector will not
    /// accept — and the events behind it may well be fine. Treating the two
    /// alike would let one malformed event silence a whole drain.
    ///
    /// **A `401` is the exception, because it is not a per-event answer at
    /// all.** It is the collector's verdict on this process's credential,
    /// so every event behind it in the queue will get the same one. Carrying
    /// on would fire up to [`MAX_QUEUED`] requests, every
    /// [`FLUSH_INTERVAL`], for the life of a misconfigured tenant — a
    /// thousand pointless requests a minute at the operator's own
    /// collector, to learn something already known. So it abandons the drain
    /// like a transport failure, and says so once.
    ///
    /// **A `3xx` is the same shape of exception, for the same reason.**
    /// This client follows no redirect at all — see
    /// [`HttpOpenPanelTracker::new`] for why the alternative hands the
    /// write credential to a host nobody configured — so a redirecting endpoint
    /// arrives here as a plain non-success response that will never
    /// resolve. It is a verdict on the endpoint, not on the event, so it
    /// abandons the drain and warns once rather than logging a `debug!` per
    /// event for the life of the process.
    ///
    /// # The tail a cancelled drain loses, and why it is said out loud
    ///
    /// Every path above ends the drain *deliberately* and says how many
    /// events it dropped. There is one that does not: the shutdown flush is
    /// wrapped in a [`tokio::time::timeout`] at its call site
    /// (`src/bin/opencompany.rs`, bounded by
    /// `server::shutdown::flush_budget`, at most **2s**), so when the budget
    /// runs out this future is simply **dropped mid-drain**. The events it
    /// had already taken out of the queue are gone, and nothing in this
    /// module ever said so — the only trace was a `debug!` at the call site
    /// that names no count.
    ///
    /// That gap is new with OpenPanel, and it is a direct consequence of
    /// there being no batch endpoint. Mixpanel's whole queue left in **one**
    /// request, so 2s was never the binding constraint; one request per
    /// event means a queue of `n` costs `n` round trips, and at a very
    /// ordinary 25 ms each the budget is spent after about eighty. A busy
    /// tenant restarting therefore loses the tail of its telemetry, quietly,
    /// on every rollout.
    ///
    /// [`CancelledDrain`] makes that loud instead. It is armed with the
    /// number of events still unsent, disarmed by every deliberate exit
    /// above (each of which logs its own line), and on `Drop` — which is
    /// what cancellation *is* — reports the count that never left.
    ///
    /// **The loss itself is not fixed here, on purpose.** The obvious
    /// remedy is to send with bounded concurrency, which would fit roughly
    /// `concurrency ×` more events into the same budget. It is declined
    /// because it is paid for out of the guarantee directly above: a drain
    /// that issues eight requests at once against a black-holing collector
    /// opens eight connections rather than one, and
    /// `an_unreachable_collector_costs_one_timeout_for_the_whole_drain`
    /// asserts exactly one. Trading a bounded shutdown for a multiplied
    /// hammering of a collector that is already unreachable is the wrong
    /// direction, and #1739 is explicit that telemetry loss beats a
    /// shutdown overrun — the budget exists because an overrun buys a
    /// `SIGKILL` mid-turn. The real fix is a batch endpoint on the
    /// collector, which OpenPanel does not have.
    async fn drain(&self) {
        let _sending = self.sending.lock().await;
        let events = {
            let mut queue = self.queue.lock().expect("analytics queue");
            if queue.is_empty() {
                return;
            }
            std::mem::take(&mut *queue)
        };

        let total = events.len();
        // Armed for the whole loop. Every `return` below disarms it first,
        // because those paths log their own count; what is left for the
        // guard is the one exit that cannot log for itself — being dropped.
        let mut cancelled = CancelledDrain {
            remaining: total,
            endpoint: crate::analytics::boot::loggable_endpoint(&self.endpoint),
            lost: &self.lost_to_cancellation,
        };
        for (sent, event) in events.into_iter().enumerate() {
            cancelled.remaining = total - sent;
            match self.client.post(&self.endpoint).json(&event).send().await {
                Ok(response) if response.status().is_success() => {
                    self.stats
                        .record(LastSend::Accepted, Some(response.status().as_u16()));
                    self.say_first_outcome_accepted(response.status());
                }
                // Not a per-event answer: the credential is wrong for every
                // event behind this one too.
                Ok(response) if response.status() == reqwest::StatusCode::UNAUTHORIZED => {
                    cancelled.disarm();
                    self.stats.record(LastSend::RefusedCredential, Some(401));
                    self.stats.drop_events(total - sent);
                    // Its own said-once warning is the self-check line.
                    self.stats.claim_first_outcome();
                    self.report_refused_credential(total - sent);
                    return;
                }
                // Also not a per-event answer, and — because this client
                // follows no redirects — not one that resolves itself.
                Ok(response) if response.status().is_redirection() => {
                    cancelled.disarm();
                    self.stats
                        .record(LastSend::Redirect, Some(response.status().as_u16()));
                    self.stats.drop_events(total - sent);
                    self.stats.claim_first_outcome();
                    self.report_redirected_endpoint(response.status(), total - sent);
                    return;
                }
                // Not a per-event answer either — but unlike the two above,
                // this one resolves itself, so it gets the transient
                // treatment rather than a `warn!` per event.
                Ok(response) if is_collector_wide(response.status()) => {
                    cancelled.disarm();
                    self.stats
                        .record(LastSend::CollectorBusy, Some(response.status().as_u16()));
                    self.stats.drop_events(total - sent);
                    if self.stats.claim_first_outcome() {
                        self.warn_first_failure(
                            LastSend::CollectorBusy,
                            Some(response.status()),
                            total - sent,
                        );
                    } else {
                        tracing::debug!(
                            endpoint = %crate::analytics::boot::loggable_endpoint(&self.endpoint),
                            status = %response.status(),
                            dropped = total - sent,
                            "[analytics] the collector cannot take traffic right now; \
                             dropping the rest of this drain"
                        );
                    }
                    return;
                }
                Ok(response) => {
                    self.stats
                        .record(LastSend::RejectedEvent, Some(response.status().as_u16()));
                    self.stats.drop_events(1);
                    self.stats.claim_first_outcome();
                    // The first per-event rejection is a `warn!`: a
                    // persistent `400` is a collector that will never take
                    // this client's events, and it used to be invisible.
                    // Later ones stay `debug!` so it cannot flood a log.
                    if self.stats.claim_first_rejection() {
                        tracing::warn!(
                            status = %response.status(),
                            "[analytics] the collector refused an event and dropped it. \
                             Further refusals are logged at debug level only; see \
                             `analytics` on /spec for the running counts."
                        );
                    } else {
                        tracing::debug!(
                            status = %response.status(),
                            "[analytics] the collector refused an event; dropping it"
                        );
                    }
                }
                Err(error) => {
                    cancelled.disarm();
                    self.stats.record(LastSend::Unreachable, None);
                    self.stats.drop_events(total - sent);
                    if self.stats.claim_first_outcome() {
                        self.warn_first_failure(LastSend::Unreachable, None, total - sent);
                    } else {
                        tracing::debug!(
                            endpoint = %crate::analytics::boot::loggable_endpoint(&self.endpoint),
                            error = %loggable_send_error(error),
                            dropped = total - sent,
                            "[analytics] could not reach the collector; dropping the rest \
                             of this drain"
                        );
                    }
                    return;
                }
            }
        }
        cancelled.disarm();
    }

    /// The first-send self-check, success half: one `info!` the first time
    /// the collector takes an event, so "is it tracking?" has a positive
    /// answer in the log and not only an absence of warnings.
    fn say_first_outcome_accepted(&self, status: reqwest::StatusCode) {
        if self.stats.claim_first_outcome() {
            tracing::info!(
                "[analytics] the collector accepted the first event (HTTP {})",
                status.as_u16()
            );
        }
    }

    /// The first-send self-check, failure half: one `warn!` naming the
    /// outcome class when the very first drain ends in a transient failure.
    /// Names the destination through the one redaction helper and never the
    /// client id.
    fn warn_first_failure(
        &self,
        class: LastSend,
        status: Option<reqwest::StatusCode>,
        dropped: usize,
    ) {
        tracing::warn!(
            endpoint = %crate::analytics::boot::loggable_endpoint(&self.endpoint),
            outcome = class.as_str(),
            status = status.map(|s| s.as_u16()),
            dropped,
            "[analytics] the first send to the collector failed ({}); these events \
             are dropped and the next drain will try again. Later failures of this \
             kind are logged at debug level only; see `analytics` on /spec.",
            class.as_str()
        );
    }

    /// Says once, out loud, that the configured endpoint redirects and that
    /// nothing is being sent as a result.
    ///
    /// **Never prints the `Location` header.** It is a URL the collector
    /// chose, and a URL is the one place this module already knows a
    /// credential hides — an authenticated proxy's key lives in the
    /// userinfo or the query string, which is the whole reason
    /// [`loggable_send_error`] exists. A redirect target is *less* trusted
    /// than the configured endpoint, not more: the operator did not write
    /// it, and printing it verbatim would hand a hostile or merely careless
    /// collector a way to write arbitrary text into a tenant's logs. The
    /// status code alone is enough to act on, and the fix is in the
    /// operator's own environment file either way.
    fn report_redirected_endpoint(&self, status: reqwest::StatusCode, dropped: usize) {
        use std::sync::atomic::Ordering;
        if self.endpoint_redirects.swap(true, Ordering::Relaxed) {
            return;
        }
        tracing::warn!(
            endpoint = %crate::analytics::boot::loggable_endpoint(&self.endpoint),
            status = %status,
            dropped,
            "[analytics] the collector answered with a redirect, which this client \
             never follows: the credential header would otherwise travel to a host \
             OPENCOMPANY_ANALYTICS_ENDPOINT does not name. Every event will be \
             dropped until that variable points at the collector directly. For a \
             self-hosted OpenPanel behind its bundled Caddy that is \
             https://<your-domain>/api/track."
        );
    }

    /// Says once, out loud, that the collector will not accept this
    /// process's credential. Never quotes it.
    fn report_refused_credential(&self, dropped: usize) {
        use std::sync::atomic::Ordering;
        if self.credential_refused.swap(true, Ordering::Relaxed) {
            return;
        }
        tracing::warn!(
            endpoint = %crate::analytics::boot::loggable_endpoint(&self.endpoint),
            dropped,
            "[analytics] the collector refused this instance's credential (401). \
             Every event will be dropped until OPENCOMPANY_ANALYTICS_CLIENT_ID names \
             a write client on that collector whose secret check is off (\"ignore \
             CORS and secret\"). Note that OpenPanel requires the client id to be a \
             UUIDv4."
        );
    }
}

#[async_trait]
impl Tracker for HttpOpenPanelTracker {
    fn track(&self, event: Event) {
        let body = {
            let envelope = self.inner.envelope.read().expect("analytics envelope");
            payload(&envelope, &event)
        };
        let mut queue = self.inner.queue.lock().expect("analytics queue");
        if queue.len() >= MAX_QUEUED {
            queue.remove(0);
            self.inner.stats.drop_events(1);
        }
        queue.push(body);
    }

    async fn flush(&self) {
        // Waits on any in-flight periodic drain before taking what is left,
        // so a shutdown overlapping the 30-second loop does not return while
        // the previous drain is still on the wire.
        self.inner.drain().await;
    }

    fn status(&self) -> Option<AnalyticsStatus> {
        Some(
            self.inner.stats.to_status(
                crate::analytics::boot::loggable_endpoint(&self.inner.endpoint),
                self.inner.deployment,
                self.inner
                    .lost_to_cancellation
                    .load(std::sync::atomic::Ordering::Relaxed) as u64,
            ),
        )
    }

    fn discard_pending(&self) {
        self.inner.queue.lock().expect("analytics queue").clear();
    }

    fn observe_cognition(&self, cognition: crate::ports::brain::Cognition) {
        self.inner
            .envelope
            .write()
            .expect("analytics envelope")
            .set_cognition(cognition);
    }
}
