# `src/store/hive/`

Backends for the `HiveStore` port (`src/ports/hive.rs`): the hive
coordinator's per-company compare-and-swap state document and append-only
message log, with opaque JSON bodies. The contract is
`docs/spec/runtime/ports-state.md#hivestore`; how each backend keeps a commit
whole is `docs/spec/runtime/storage.md#hive-store`.

| File | What lives there |
| --- | --- |
| `mod.rs` | Index only. |
| `memory.rs` / `memory_tests.rs` | `MemoryHiveStore`: one mutex over a document and an ordered row map per company. For tests and adapter work without a backend. |
| `fs.rs` / `fs_tests.rs` | `FsHiveStore`: `<bundle>/hive/state.json` (with `messagesLen`, the committed byte length of the log) and `hive/messages.jsonl`. Truncate to the committed length, append, `sync_data`, then atomic durable rename of the state. The fs tests plant whole and torn orphan lines past the committed length. The default when no backend is selected. |
| `sqlite.rs` | `impl HiveStore for SqliteStore` and `HIVE_MIGRATIONS` (`hive_state`, `hive_messages`), one `IMMEDIATE` transaction per commit. Feature `sqlite`. Tests: `store/sqlite_hive_tests.rs`. |
| `mongodb.rs` | `impl HiveStore for MongoStore`: `hive_state` keyed `_id = company_id` with the commit's rows carried as `pending` inside the swap, and `hive_messages` keyed `_id = {c, s}`. Feature `mongodb`. Tests (env-gated on a live server): `store/mongodb_hive_tests.rs`. |
| `conformance.rs` | `assert_hive_store`, `assert_hive_commit_race`, and the backend-neutral half of crash recovery (`seed_committed`, `orphan_row`, `assert_orphans_ignored`). Test-only. |

The sqlite and mongodb tests live beside their stores' other tests rather than
here because they reuse those stores' test helpers, and because the CI lanes
select them by the `store::sqlite` / `store::mongodb` path filters.
