# analytics

Product analytics (issue #1739): the `Tracker` port, its silent default, and the
opt-in OpenPanel transport. Specs: `docs/spec/runtime/analytics.md`,
`analytics-wire.md`, `analytics-status.md`, `analytics-desktop.md`.

| File | What it is |
|---|---|
| `mod.rs` | `Tracker` (incl. the default `status()` / `discard_pending()`), `NullTracker`, `RecordingTracker`, `DeferredTracker` (+ `install_with_decision`) and the payload builder. |
| `config.rs` | The enable/disable `Decision` and every `Silence` reason; client-id and endpoint validation. |
| `boot.rs` | `install` / `install_for_shell` (choose and install the tracker; a shell stamps its version), `describe` / `describe_for`, `loggable_endpoint`. |
| `selftest.rs` | `analytics-test`: resolves like boot, sends one `analytics_self_test` under a throwaway `s_` id, maps the outcome to an exit code. |
| `status.rs` | `AnalyticsStatus` / `LastSend`: the serializable "is it tracking?" answer `/spec` serves. Un-gated. |
| `types.rs` | The payload vocabulary: `PropValue`, enums, `OpaqueId`, `Envelope`. |
| `types/` | `event.rs`: the `Event` enum, split out of `types.rs`; re-exported. |
| `meter.rs` | `TrackingUsageMeter`, the usage-meter decorator. |
| `openpanel.rs` | `build` (the one place a tracker is chosen), header-name constants, the reserved event names. |
| `openpanel/` | `http.rs` and its helpers: the reqwest transport, behind `--features analytics`. |
| `*_tests.rs` | Sibling tests. `analytics_pii_tests.rs` is the exhaustive no-PII property; `openpanel_status_tests.rs` the status bookkeeping and first-send self-check. |
