# The loadable TinyMemory module as a memory driver

Issue #1524. The `module` driver binds the separately compiled TinyMemory
`cdylib` — over TinyBus, in-process — as a `MemoryProvider`, selected by:

```
OPENCOMPANY_MEMORY=embedded
OPENCOMPANY_MEMORY_DRIVER=module
```

Both variables, always. The driver knob alone does nothing under the `store`
default (`open_driver` is never reached), and a rollback that unsets only the
driver knob lands the tenant on the EngineCortex overlay — a *different* mode
than the `store` it likely ran before. **Record both prior values before a
flip; rollback restores both.**

## What is deliberately NOT done (read before proposing it)

- **No new `MemoryMode` or `MemoryBackend`.** The id routes inside the
  existing `Embedded` arm, beside `namespace`. An additive mode would have
  silently skipped the migrate exclusive lock (`_home_lock` matches
  `MemoryMode::Embedded`) and renegotiated `/spec`'s wire vocabulary.
- **No aliasing with `namespace`.** Different engine (tinycortex store vs
  `UnifiedMemory`), different directory (`memory-module` vs
  `memory-namespace`). Aliasing strands data silently.
- **No wide capability advertisement.** Exactly `Core | Recall | Portability`,
  every optional accessor `None`. The audit passes by construction, `/spec`
  stays at three names, and the advertisement is independent of the loaded
  artifact's version — there is no artifact-capability pin to go stale. The
  module's engine serves more families; this host deliberately does not reach
  them until a consumer exists.
- **No downloads at boot.** The artifact is baked into the image
  (root-owned, under `/app/modules`, beside its `modules.toml` digest
  allowlist) because the pod runs uid 10001 on a read-only root filesystem,
  the PVC is fsGroup-writable (tinybus refuses any group/other-writable
  ancestor), and rollback must never depend on network state.
- **No retry of a failed load.** tinybus never unloads; every terminal module
  state is terminal for the process. Failures are cached, boot loads eagerly
  and aborts with a named, scrubbed reason, and recovery is a restart.
- **No typed ChatHost request.** The callback takes `serde_json::Value` and
  refuses with a stable wire name — typing it would couple the seam to a
  `tinyagents` version for a callback whose answer is a refusal either way.
- **No defaulted taint.** `MemoryEntry.taint` serde-defaults to `Internal`
  (the trusted class), so a module answer omitting the field is refused
  before decode rather than silently upgrading external content.
- **No second module host.** openhuman's own host is gated on its `modules`
  feature, which this crate does not forward. If that dependency line ever
  grows it, the module's refusal of a second `claim_process_setup`
  (`SETUP_FAILED`) is the loud backstop.

## Failure modes and how to get out

| Symptom at boot | Meaning | Way out |
|---|---|---|
| abort naming `OPENCOMPANY_MEMORY_MODULE_PATH` | artifact absent / env unset | fix the image or the env; `opencompany modules check` shows the gates |
| abort: "refused" with a tinybus reason | admission failed (ABI, digest, directory modes) | `modules check` reproduces the directory and digest verdicts without loading |
| abort: module store directory refusal | store would land in `memory/` (incumbent engine's) | never point the module at the incumbent's directory; the builder refuses by name |
| first memory call: `Timeout` | module hung past the 30s interactive deadline | restart; if persistent, the artifact is wrong for this platform (glibc bucket) |
| migrate counts look wrong after a timeout | a bus deadline does not cancel remote work; the resumed run re-counts what the first silently completed | data is correct by `(namespace, key)` idempotence; trust the store, not the counts |
| `module→module` migrate refused | a loaded module is a process singleton serving one store | migrate via an intermediate engine, or move the data dir offline |

The runtime rollback is always available and never depends on a deletion
being undone: restore the recorded prior `OPENCOMPANY_MEMORY` +
`OPENCOMPANY_MEMORY_DRIVER` pair, restart. The module's store stays on disk
under `memory-module/`, untouched, for the next attempt.

## Testing

The module-backed conformance tests are `#[ignore]`d and run **one per
process** — a loaded module binds its broker tasks to the runtime that
created them, and two such tests in one process hang rather than fail. Gate:
`TINYMEMORY_TEST_MODULE` names the artifact (CI downloads the pinned release
archive; a developer without one gets skips). The differential test pins the
module driver and the `namespace` driver answering the same sequence
identically; a divergence is either a bug or gets enumerated here, in
writing.
