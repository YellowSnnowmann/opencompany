# Company memory: OpenHuman memory v2

OpenCompany has no memory engine of its own. Company memory is
**OpenHuman's memory**: TinyMemory's agent lifecycle, run by the embedded
OpenHuman runtime around every agent turn, and scoped so each company and
each teammate gets its own part of one namespace tree. The cutover is
#2568 and its follow-up. There was no migration: memory held by the
pre-cutover engine (`ContextStore`, `FactStore`, `OPENCOMPANY_MEMORY*`) is not
carried over.

The OpenHuman side is documented upstream in
`vendor/openhuman/docs/specs/memory-v2.md`. TinyMemory's layout and lifecycle
are in `vendor/openhuman/vendor/tinymemory`.

## Layout: one root per company, one node per teammate

```text
team:<company>                    learnings: operator facts, what agents learned
├── source:<kind>                 the brain: dropped files, remembered links
└── agent:<teammate id>           one teammate's logged turns
```

- **Root.** The root is `team:<company id>`, built by `crate::memory::memory_root`.
  An id outside `[A-Za-z0-9_-]{1,128}` is folded to `-` and given a hash
  suffix, which is TinyMemory's own `Segment::sanitized` rule. Shared-DB
  tenant ids already carry `<tenant>--`, so tenants stay apart too.
- **Learnings** sit at the root. Every teammate's recall reaches them. They
  include facts an operator types on the Brain page and what agents `learn`.
- **The brain** is `source:<kind>` nodes under the root, holding documents
  every teammate shares.
- **Conversations** are kept per teammate, at `agent:<manifest id>`. A turn is
  one item.

## How an agent is bound

`harness::built_in::build::agent_spec_for` binds every OpenHuman agent it
registers:

```rust
AgentSpec::new(runtime_id)
    .memory(MemoryBinding::new(teammate_id).root(memory_root(company)))
```

- The binding is the agent's identity for memory. OpenHuman never takes it
  from model arguments, and it covers sub-agents too.
- From the binding, the runtime runs the lifecycle on every turn:
  - a **pre-turn pack**, injected as an ephemeral `<memory-context>`;
  - the **post-turn log** to the teammate's node;
  - **compaction recall**;
  - the background belief builds.
- The agent's tool scope names OpenHuman's single `memory` tool
  (`recall | fetch | learn | forget`). The tool is built from the agent's own
  config, so its reach is the company's subtree.

**Every agent is bound.** An unbound agent would log at the engine's default
root, which every company in the process shares. Confined (workflow-copilot)
turns are therefore bound too, but **inactive**
(`AgentMemory::inactive`): no `memory` tool, recall and conversation logging
off. The runtime registers the memory domain (`domains.memory = true`,
`harness::openhuman_runtime::host_domains`).

## The engine

- The engine is OpenHuman's `[memory]` engine:
  - `tinyhumans`, which is CortexDB behind the TinyHumans backend, authenticated
    with the runtime's TinyHumans credential (`OPENCOMPANY_INFERENCE_KEY` /
    `TINYHUMANS_API_KEY`);
  - or `cortexdb` with a stored key.
- **With neither, memory is off.** Turns run without a pack and without
  logging, and the memory routes answer `409 not_configured`. There is no
  local engine.
- Every write passes OpenHuman's scrubbing guard (`ScrubbingEngine`), which
  removes secrets and PII.
- Unit tests install TinyMemory's in-memory `ReferenceEngine` on the shared
  runtime (`harness::openhuman_runtime::build` under `cfg(test)`).
  Integration targets install one with
  `openhuman_embed::memory::install_host_engine`. Tests isolate themselves by
  giving each a unique company id, which is the same isolation production
  relies on.

## The host's handle: `crate::memory::CompanyMemory`

`CompanyRuntime::memory()` returns a `CompanyMemory`. It wraps
`openhuman_embed::Runtime::memory(root)`, the per-tenant facade added in
tinyhumansai/openhuman#6994. Every call it makes stays inside the company's
subtree:
- reads carry `Reach::subtree(root)`;
- `forget` only removes ids found there, so another company's id is a no-op;
- `learn` writes at the root whatever metadata it is given.

| Call | What |
| --- | --- |
| `status` | on/off, engine, endpoint, reason |
| `list(MemoryQuery)` | a page, newest first: the whole root or one teammate's node, by kind or tag |
| `search(query, kind, limit)` | ranked matches |
| `get(ids)` / `forget(ids)` | read or forget items the company holds |
| `learn(text, kind, tags)` | a learning at the root |
| `agents` / `forget_agent(id)` | teammates with logged turns; forget one teammate's turns |
| `recall(question, agent)` | a synthesised answer, company-wide or for one teammate plus the learnings |
| `brain_sources` / `brain_file` / `brain_forget(source)` | the brain |
| `context_op(op, external)` | the brain device tools' `context_*` wire (below) |

Without the `openhuman` feature there is no runtime: `status` reports off and
every other call is `NotInBuild`.

### Who calls it

- **The console**: `server/ops/memory.rs` and `server/ops/memory_ingest.rs`
  (routes below), and the GraphQL `Company.memory` field.
- **The orchestrator's `query_company`**: its `## Facts` section lists the
  company's learnings, or searches them when given a `query`.
- **The workflow judge's recovery ladder** (`workflows/judge.rs::ask_around`):
  it searches learnings first, then all memory, then asks one peer.
- **The brain device tools** (`CycleHost::context_op`). The wire is unchanged
  for the hosted brain and the sidecar:
  - `context_put { label, body }` stores a learning tagged `context` and
    `label:<label>`, plus `inbound` when outside content triggered the cycle
    (issue #1113);
  - `context_list { prefix }` filters those by label;
  - `context_peek { addr }` reads by item id;
  - `context_search` ranks.

## Console routes

All routes are under `/api/v1/companies/{id}` and `/api/v1/company`:

| Route | What |
| --- | --- |
| `GET /memory?kind=&agent=&cursor=&limit=` | a page (`{items, nextCursor}`) |
| `GET /memory?query=` | ranked matches |
| `POST /memory {text, kind?}` | an operator fact, as a learning tagged `operator` |
| `DELETE /memory/{id}` | forget one item |
| `GET /memory/status` | never an error; off says why |
| `GET /memory/agents` / `DELETE /memory/agents/{agent_id}` | per-teammate memory |
| `POST /memory/recall {question, agent?}` | a synthesised answer |
| `GET /memory/brain` | brain sources and sizes |
| `POST /memory/ingest`, `POST /memory/ingest/links` | file and link drops (`crate::ingest` extracts the text; it is filed as one brain document per source) |
| `DELETE /memory/document/{source}` | forget every brain document of one source |
| `GET /memory/traces` | the cycle-trace window: **not** memory (`TraceStore`) |

A forget journals `CompanyEvent::MemoryFactDeleted` with the item id,
`agent:<id>` or `source:<kind>`.

## What is not memory

`TraceStore` (formerly `MemoryStore`) holds compressed cycle traces and task
results on the storage backend. It is a bounded inspection window
(`runtime::maintenance`), it is exported in bundles, and it is never recalled.

## Not carried in a bundle

The memory engine holds memory, not the storage backend. Export and import
carry the company, events and traces. A restored company starts with whatever
the engine already holds for its root.

## Known gaps

- Learnings are shared across a company: there are no per-teammate private
  learnings.
- An agent's `memory` tool can recall, and forget, a teammate's items within
  the company. That is OpenHuman's reach (`Reach::subtree(root)`), and it is
  bounded to the company.
- There is no memory export. Export is paged `list` only.
