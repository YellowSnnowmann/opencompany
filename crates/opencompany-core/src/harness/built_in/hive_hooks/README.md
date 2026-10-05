# `src/harness/built_in/hive_hooks/`

The settle half of the company hive's turn hooks (OC-2). The hooks themselves —
`prepare`, `progress`, `wrap_turn`, `after_turn` — are `../hive_hooks.rs`; the
spec is `docs/spec/runtime/hive.md#a-coordinator-turn`.

| File | What lives there |
| --- | --- |
| `settle.rs` / `settle_tests.rs` | `SettleTurn::run`, the body of `wrap_turn`: admission, the `TurnStarted` / `TurnSettled` / `TurnFailed` bracket with its `HiveTurnRef`, the per-turn claims (approval scope keyed `hive-turn:{agent}:{episode}`, publishes, outputs, `spawn_task` cards), the run inside the agent's `TurnEnvelope`, and filing what the turn left; `EpisodeCards`, the per-episode card budget (three, no repeated title). |
