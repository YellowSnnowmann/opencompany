# `src/memory/`

A company's memory: OpenHuman's memory v2 engine, scoped to the company. The
host owns no engine, store or index; it binds each teammate to a per-company
root and reaches memory through the `CompanyMemory` handle. The spec is
`docs/spec/runtime/memory-engine.md`; this folder is its implementation.

| File | What lives there |
| --- | --- |
| `mod.rs` | `memory_root` (the `team:<company>` root, with TinyMemory's sanitising rule), the `CompanyMemory` handle and its off-build stand-in (`status` reports off, every other call is `NotInBuild`). Re-exports `types` and the `context_op` tags. Default build. |
| `memory_tests.rs` | The root rule: plain ids map to themselves, ids outside `[A-Za-z0-9_-]{1,128}` fold with a hash suffix. |
| `types.rs` | The host's shapes, independent of the engine types: `MemoryItem` and `MemoryItemKind`, `LearningKind`, `MemoryQuery` / `MemoryPage`, `MemoryStatus`, `MemoryAgent(s)`, `RecallAnswer` / `RecallCitation`, `BrainSource(s)` / `BrainFiled`. Default build. |
| `engine.rs` / `engine_tests.rs` | `CompanyMemory` over `openhuman_embed::Runtime::memory(root)`: status, list, search, get, forget, learn, per-teammate agents, recall, and the brain (sources, file, forget). Every read carries `Reach::subtree(root)`, so one company never sees another's items. Tests run on the in-memory reference engine with a unique company id each. Gated on `openhuman`. |
| `context_op.rs` / `context_op_tests.rs` | The brain device tools' `context_put` / `context_list` / `context_peek` / `context_search` wire (`CycleHost::context_op`) on top of learnings, tagged `CONTEXT_TAG` and `label:<label>` (plus `INBOUND_TAG` for cycles triggered by outside content, issue #1113). |
