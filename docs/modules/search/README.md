# Search providers

`Connections → API Keys → Search`: which index this company's teammates search
the web through.

This surface is being brought to the shape the LLM/inference surface reached in
`docs/modules/inference/` (landing in PR #2262, so those cross-references
resolve only once that branch merges) — a list of connected providers, an add-provider modal,
one credential per provider, one marked default, and per-provider controls. The
goal is that an operator who has used the LLM page finds this one obvious.

## The files

| File | What it answers |
|---|---|
| [`current-state.md`](current-state.md) | what exists today, and the one-key-slot bug that makes a list worth building |
| [`data-model.md`](data-model.md) | the record, where the credential lives, entry-zero convergence, what "default" means here |
| [`connect-flow.md`](connect-flow.md) | the page, the modal, the two dialogs, and the classified probe |
| [`catalogue.md`](catalogue.md) | the four providers — endpoints, auth headers, failure shapes |
| [`architecture.md`](architecture.md) | the module seams and how each is tested |
| [`known-defects.md`](known-defects.md) | what is deliberately **not** inherited from the inference design |

**Keys rework (2026-09-14, issue #2306).** The per-provider endpoint moves into
the `search/providers` record, with `search/provider/<slug>/endpoint` kept as a
read fallback and still written for one release. See
[`docs/key-reworks/phase-1b-search-endpoint.md`](../../key-reworks/phase-1b-search-endpoint.md).

## The two rules that outrank the redesign

Both are already in the code and neither is negotiable.

**Credentials are company-scoped before the deployment fallback.** BYO provider
keys remain isolated per company and per provider. Managed search first reads
`search/managed/key`, a copy of that company's own TinyHumans account key, then
falls through to the deployment identity. The company tier is billed to the
same company whose agents present it, so it does not create the ambient-
credential problem the original environment-only boundary prevented.

**The configuration surface is not feature-gated; the harness is.**
`src/harness/built_in/search_byo.rs` is behind `openhuman`;
`src/company/search` and `src/server/ops/search.rs` are always compiled, so a
build with no agent harness renders "this build has no search tools" rather than
a 404.

## The shortest statement of the change

One credential slot became many, keyed by the provider it authenticates.

Today a company has one `search/api_key` and a separate `search/provider` field
that selects which API it is presented to. Changing the provider without
re-pasting the key leaves the old key authenticating against the new provider,
and every layer — the status route, the console badge, the harness — agrees the
company is correctly configured until an agent's first search returns a 401 that
nothing on the page can explain.

Everything else here follows from fixing that honestly.
