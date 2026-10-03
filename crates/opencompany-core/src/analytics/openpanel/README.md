# analytics/openpanel

The reqwest-backed OpenPanel transport, compiled only under
`--features analytics`. `../openpanel.rs` is the module root and re-exports
`HttpOpenPanelTracker`, so `crate::analytics::...` paths are unchanged.

| File | What it is |
|---|---|
| `http.rs` | `HttpOpenPanelTracker`, its queue, the drain loop, the one-shot first drain (`FIRST_DRAIN_DELAY`) and `Inner::drain` with the first-send self-check. |
| `http/guards.rs` | `CancelledDrain`, the `Drop` guard that reports events lost to a cancelled drain. |
| `http/helpers.rs` | Request headers, the cleartext and collector-wide predicates, and `loggable_send_error`. |
| `http/stats.rs` | `SendStats`: the counters and last outcome behind `AnalyticsStatus`. |
