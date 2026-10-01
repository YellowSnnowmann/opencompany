# Product analytics: the status surface and the first-send self-check

Until this existed, the only evidence that analytics reached a collector was the
collector. A refused credential (`401`) was a said-once `warn!`, but `serve`'s
default log filter was `error,…`, which swallowed it; every other failure was a
`debug!`. The boot line said "analytics: reporting to …" and an operator — or the
desktop shell, which has no collector to look at — could not tell a working
instance from one whose every event was being dropped. This page covers the two
things that close that gap. Neither changes **what is sent**.

## `/spec` → `analytics`

`GET /spec` (unauthenticated, like the rest of the handshake) carries an
`analytics` object, produced by `AnalyticsStatus` (`src/analytics/status.rs`):

| Field | Meaning |
|---|---|
| `decision` | `"reporting"` or `"off"`: what this process will actually do. A build without the `analytics` feature is `off` even when configured to report. |
| `reason` | Why — the `Silence` text when off, a fixed phrase when reporting, `"not wired"` for a host nobody wired a tracker into. |
| `deployment` | `desktop`, `self-hosted` or `hosted-tenant`. |
| `endpoint` | The collector, **always** through `boot::loggable_endpoint` (userinfo, query and deep path segments removed). `null` when there is none. |
| `in_build` | Whether the network transport was compiled in. |
| `consent` | `true`/`false` where a deployment asks the user, `null` where no question applies. The desktop's gate ([analytics-desktop.md](analytics-desktop.md)); `null` elsewhere. |
| `last_send` | `never`, `accepted`, `refused-credential`, `redirect`, `collector-busy`, `unreachable` or `rejected-event`. |
| `last_status` | The HTTP status of that send; `null` for `never` and `unreachable`. |
| `last_at` | RFC-3339 UTC when it ended, `null` until one has. |
| `accepted` | Events the collector accepted since boot. |
| `dropped` | Events lost since boot: refused, abandoned with a drain, shed from a full queue, or cut off by a cancelled drain. |

**The client id is never in it.** `/spec` is unauthenticated, so the endpoint is
redacted by the one helper the boot line and the send path already share, and the
id has no field at all. `analytics_spec_carries_the_status_and_never_the_client_id`
searches the whole serialized body for it.

The data comes from `Tracker::status()`, a default method returning `None`. The
HTTP transport answers from `SendStats` (atomics for the counters, one mutex for
the last outcome), updated in **every** arm of `drain`. `DeferredTracker` — the
handle the host actually holds — stores the boot `Decision` through
`install_with_decision` and overlays the transport's counters onto it, because
the transport knows what it sent but not why a process is silent. A host with no
tracker wired reports `{decision: "off", reason: "not wired"}`.

A sibling default method, `Tracker::discard_pending()`, clears whatever is queued
and unsent (the transport's queue and the deferred handle's pre-install buffer),
so events recorded before a consent withdrawal are not delivered after it.

## The first-send self-check

The first drain that ends in an outcome says so once:

- **accepted**: `info!` "the collector accepted the first event (HTTP 200)";
- **`401`, `3xx`**: their existing said-once `warn!` *is* the self-check line, so
  there is exactly one `WARN`, not two;
- **a per-event `4xx`**: the first rejection is a `warn!`; every later one stays
  `debug!`, so a persistent `400` cannot flood a log;
- **`429`/`5xx`, or no answer at all**: one `warn!` naming the outcome class and
  the redacted endpoint; later failures of the same kind stay `debug!`.

No line ever contains the client id. Because the first drain used to wait the
full 30-second interval, the transport also runs **one** extra drain
`FIRST_DRAIN_DELAY` (5s) after construction, so `/spec` and the self-check
populate soon after boot. It is its own task, never retries, and an empty queue
makes it a no-op: analytics still never delays a turn.

## The default log directive

`serve`'s `DEFAULT_LOG_FILTER` now includes `opencompany::analytics=warn`,
following the `tinyagents::observability=warn` precedent: those warnings are the
only account of a collector that will never take this instance's events, each is
said once and bounded, and `RUST_LOG` still replaces the whole string. The
`info!` acceptance line is below that threshold on purpose; `/spec` is where a
positive answer lives by default.

## Tests

All under the `analytics` filter, so `scripts/ci/run-scoped-suite.sh "analytics"
analytics analytics` selects them: `openpanel_status_tests.rs` (every drain arm,
level capture, exactly-once logging, the first drain), `analytics_status_tests.rs`
(the summary and the deferred merge), `analytics_pii_tests.rs` (an exhaustive
no-PII property over every `Event` variant crossed with every enum value) and
`server/routes_analytics_tests.rs` (`/spec`).
