# Memory

This page covers what a company remembers, where it lives, and the Operator's
rights over it. For the mechanics, see
[runtime/memory-engine.md](../runtime/memory-engine.md).

## What is remembered

Company memory is OpenHuman's memory engine. It is scoped to the company's
root, `team:<company>`, and each teammate has its own node under that root.

| Kind | Where | Written by |
| --- | --- | --- |
| Logged turns (conversations) | `team:<company>/agent:<teammate>` | the runtime, after every teammate turn |
| Learnings: facts, preferences, procedures, corrections | `team:<company>` | an operator on the Brain page; agents through the `memory` tool's `learn`; the brain device tools' `context_put` |
| Beliefs | the node they were built from | the engine's background consolidation |
| Brain documents | `team:<company>/source:<kind>` | file and link drops on the Brain page |

Before every turn, the runtime recalls a token-budgeted pack for the teammate.
That pack draws on:
- the company's learnings;
- the brain;
- the teammate's own history;
- a little of its teammates' turns.

A teammate's turns live on its own node. A company never reads another
company's memory.

**Not memory**: compressed cycle traces and task results (`TraceStore`). They
are a bounded inspection window of what each cycle did, written every cycle and
never recalled (issue #1175).

## Dropping documents and links in (the Brain drop zone)

An operator can put a file, a whole folder, or a link into memory by dropping
it on the Brain page. The host extracts its text (`src/ingest`) and files it as
one brain document under its source kind: `pdf`, `markdown`, `web` (links and
HTML), or `document` (everything else). Every teammate's recall then reaches
it (`src/server/ops/memory_ingest.rs`).

Four properties are normative, because each one is a way this could quietly
lie:

- **The text is stored, the file is not.** Memory keeps what the document
  *said*. Files an operator wants back live in the workspace tree.
- **Extraction never guesses.** A format the build cannot read is reported as
  unsupported for that file. It is never stored as decoded noise. A scanned
  PDF with no text layer is *empty*, which is a different answer from
  *failed*.
- **Every file gets its own row in the answer.** One unreadable file never
  fails a folder drop, and it is never skipped silently.
- **A drop can be taken back.** `DELETE …/memory/document/{source}` forgets
  every brain document of that source.

Links are fetched **by the host**, so the URL path is guarded server-side. Only
`http`/`https` is allowed, and never a loopback, link-local or private address.
The guard is TinyMemory's pinned fetcher, which re-checks every redirect hop.

## Operator rights (normative)

- **Inspect**: `GET …/memory` lists the company's memory by teammate and kind.
  `GET …/memory/agents` names the teammates with memory, and `GET
  …/memory/brain` lists the brain's sources. `GET …/memory/traces` shows the
  retained cycle-trace window.
- **Delete**: the Operator MAY forget any item (`DELETE …/memory/{id}`), a
  whole teammate's turns (`DELETE …/memory/agents/{id}`), or a brain source.
  Deletion reaches the engine and is journaled to the `EventLog` as
  `MemoryFactDeleted`, so the fact that a deletion happened stays auditable
  while the content is gone.
- **Redact**: every write passes OpenHuman's scrubbing guard, which removes
  secrets and PII before anything is stored. Required for the privacy stance in
  [feedback-loop/privacy.md](../feedback-loop/privacy.md).
- **Export**: memory is held by the memory engine, not by the storage backend,
  so it does **not** travel in the bundle
  ([runtime/lifecycle.md](../runtime/lifecycle.md), export). The engine keeps
  it under the company's root, and a restored company with the same id reads
  it back.
