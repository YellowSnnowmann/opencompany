# Hive Module

`src/hive/` hosts the **company hive** (OC-2): one TinyHiveMind
`Coordinator` per company over the `HiveStore` port, every agent registered on
a `tinyhivemind-openhuman` `OpenHumanHost` with one continuing session, one
hive per desk plus `#general`, and a projector that journals what the hive did.
The normative account — where an operator line goes, starters, episodes, the
`hivemind_*` tools, reach, approvals, retention, what lands in the journal — is
[`docs/spec/runtime/hive.md`](../../spec/runtime/hive.md). This page is the
module's own shape and the reasoning behind its boundaries. The file-by-file
table is `crates/opencompany-core/src/hive/README.md`.

## Layout

| Seam | Where |
| --- | --- |
| the Coordinator, the host, the run loop, desk sync | `hive/runtime.rs` (`CompanyHive`, `for_company`) |
| storage | `hive/storage.rs` (`PortStorage` over `ports::hive::HiveStore`; backends in `store/hive/`) |
| transcript → journal | `hive/projector.rs` (`Projector`, `HiveRoster`, `TurnMetaBoard`) |
| who starts an operator line | `hive/route.rs` (+ `jev.rs`) |
| who may message whom | `hive/policy.rs` (`ReachPolicy`, the `SendAuthorizer`) |
| pacing and retention | `hive/routing.rs` (`coordinator_options`, `turn_timeout`) |
| console chat ↔ hive id | `hive/mod.rs` (`hive_id_for_chat`, `chat_for_hive`) |
| the turn hooks | `harness/built_in/hive_hooks.rs` + `hive_hooks/settle.rs` |
| the operator path | `harness/built_in/brain/hive_chat.rs` (`send_to_hive`) |
| approval release | `company/runtime/hive_seat.rs`, `runtime/hive_resume.rs` |
| measuring | `hive/measure.rs`, `scripts/lib/coordination-metrics.mjs` |

`runtime.rs`, the authorizer half of `policy.rs` and the Jev half of
`route.rs` are gated on `openhuman`; storage, projector, routing and measure
compile in every build so the store, the console's DTOs and
`opencompany measure` do not need the runtime.

## The seams, and why they are where they are

**The Coordinator schedules; the host runs and records.** OpenCompany never
decides who runs next or when an episode ends — that is the Coordinator's
durable state, committed behind a revision CAS. The host supplies the agents,
the storage, the turn hooks and the authorizer, and reads back what happened
through `read_transcript`. Nothing in the console reads the Coordinator
directly: the projector is the one bridge, so `chat/history`, `/events`, and
`opencompany measure` all read the same journal rows.

**One Coordinator per company, keyed by store.** The process-wide registry is
keyed by company and by the identity of the `HiveStore` instance, because a
company with several harness pools has one hive and one transcript. A second
process on the same store fences the first; the fenced run loop stops rather
than fighting for the writer epoch.

**One session per agent, so pool turns are isolated.** An agent's
conversational history lives in the one OpenHuman session the Coordinator
continues. A pooled turn (a card, a workflow node, a copilot thread) therefore
runs in a fresh session: two writers on one session would interleave turns the
Coordinator thinks it serializes.

**Reach is a host rule behind a library seam.** The Coordinator enforces hive
membership; OpenCompany's `delegates_to` reach rule is a manifest fact it
cannot see, so it lives behind `SendAuthorizer` and a refusal is a tool error
the model reads, not a failed turn.

**Turn hooks are a harness turn.** A coordinator turn is metered, gated,
bracketed and settled exactly like a pooled turn — the shared `TurnEnvelope`
holds the turn lock, the tool executor and the stop hooks — which is why the
hooks live under `harness/built_in/` rather than here.

## Two contracts that are easy to break

**Every hive id goes through the mapping.** Core reserves `general` / `main`;
a hive created as `general` is accepted but fails every episode and stalls the
run loop. Any new code that turns a console chat into a hive id, or back, must
use `hive_id_for_chat` / `chat_for_hive`.

**`hivemind_*` agent ids are Coordinator ids.** `{company}--{agent}`, not the
manifest id `spawn_task` takes. The team brief names both; the reach policy
answers a bare manifest id with the id to use.

## Running the tests

```bash
cargo test -p opencompany-core --features openhuman,mcp,media --lib hive::
cargo test -p opencompany-core --features openhuman,mcp,media --test hive_e2e
cargo test -p opencompany-core --features openhuman,mcp,media --test one_card_per_message
```

`hive_e2e` boots a real company per test with a unique company id and a
scripted model that reads each coordinator turn's `TurnRequest` off its
prompt (`tests/support/room.rs`); set `ROOM_TURNS=1` to print every turn the
model saw, `ROOM_DUMP=1` to dump the journal on a timeout, and
`ROOM_LOG=info` to see the host's tracing.
