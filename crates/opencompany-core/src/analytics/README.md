# analytics

Product analytics (issue #1739): the `Tracker` port, its silent default, and the
opt-in OpenPanel transport. Specs: `docs/spec/runtime/analytics.md`,
`analytics-wire.md`, `analytics-status.md`.

| File | What it is |
|---|---|
| `mod.rs` | `Tracker` (incl. the default `status()` / `discard_pending()`), `NullTracker`, `RecordingTracker`, `DeferredTracker` (+ `install_with_decision`) and the payload builder. |
| `config.rs` | The enable/disable `Decision` and every `Silence` reason; client-id and endpoint validation. |
| `boot.rs` | `install` (chooses and installs the tracker), `describe` / `describe_for`, `loggable_endpoint`. |
| `status.rs` | `AnalyticsStatus` / `LastSend`: the serializable "is it tracking?" answer `/spec` serves. Un-gated. |
| `types.rs` | The payload vocabulary: `PropValue`, enums, `OpaqueId`, `Envelope`. |
| `types/` | `event.rs`: the `Event` enum, split out of `types.rs`; re-exported. |
| `meter.rs` | `TrackingUsageMeter`, the usage-meter decorator. |
| `openpanel.rs` | `build` (the one place a tracker is chosen), header-name constants, the reserved event names. |
| `openpanel/` | `http.rs` and its helpers: the reqwest transport, behind `--features analytics`. |
| `*_tests.rs` | Sibling tests. `analytics_pii_tests.rs` is the exhaustive no-PII property; `openpanel_status_tests.rs` the status bookkeeping and first-send self-check. |
