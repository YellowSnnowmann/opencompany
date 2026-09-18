# Phase 2 — TinyHumans on the proxy, provider + model, no tiers on the wire

Part of [the keys rework](README.md). The first inference phase. Nothing blocks
it (PR #2305 is closed and is not a dependency). Code references are on
`upstream/main @ fcfb3e1bc` (2026-09-14).

| Slice | File | Dump items | One line |
|---|---|---|---|
| 2a | [phase-2a-tinyhumans-on-proxy.md](phase-2a-tinyhumans-on-proxy.md) | 13b, 14, 15; Q3 | `tinyhumans` is an ordinary catalogue row; its model list and chat use `/agent-integrations/openrouter`; exactly one TinyHumans row |
| 2b | [phase-2b-default-shape.md](phase-2b-default-shape.md) | 1, 16; Q1 | store and resolver understand `{provider, model}`; inert until a writer exists |
| 2c | [phase-2c-model-required.md](phase-2c-model-required.md) (+ [part 2](phase-2c-model-required-part2.md)) | 1, 4, 14; Q2 | set-default and add always take a model; console model step; "Provider · model" |
| 2d | [phase-2d-no-tier-on-the-wire.md](phase-2d-no-tier-on-the-wire.md) | 16; F3; D-legacy | a tier name is never sent as a model; legacy path sends a real id or fails closed |

## The three things the operator asked for first

1. **Remove tiers as model ids.** `chat-v1`, `reasoning-v1`, `agentic-v1`,
   `vision-v1` are never sent as a model (2d; the source of the model is 2b/2c).
2. **Managed's model list URL** moves from `/openai/v1/models` to
   `/agent-integrations/openrouter/models`, which answers
   `{success, data:{data,total,limit,offset}}`, paged (`limit` up to 500, follow
   `total`), unknown fields ignored (2a).
3. **Managed's chat URL** moves from `/openai/v1/chat/completions` to
   `/agent-integrations/openrouter/chat/completions` — OpenAI-compatible, with
   extra `openhuman`, `service_tier` and `usage.cost`; a streamed final frame
   before `[DONE]` carries only `openhuman` (2a).

## Do not do this (what went wrong in #2305)

- **No new secret-store keys.** #2305 invented `inference/managed/models`; it was
  rejected.
- **No Managed-specific machinery:** no Managed model dialog, no draft-probe
  route, no `proxied_model` special rule, no URL-origin derivation module.
  TinyHumans follows D-set: a row in `inference/providers`, key at
  `provider/tinyhumans/key`, model in that row's `models`, and the company
  default `{provider, model}` like every provider.
- **Never two rows.** #2305 showed a `tinyhumans` row and a legacy "Managed" row
  reading the same `provider/tinyhumans/key`. 2a specifies the one-row rule and
  its test "fresh company, add TinyHumans → exactly one row".
- **Never skip the model step or leave health `unchecked`.** The TinyHumans
  catalog never lists tier names, so the model step always appears and the probe
  result is recorded.
- **No special model slot.** Until a model is chosen, a TinyHumans chat fails
  with a clear "choose a model" error.
- **No guessed ids, no assumptions about catalog contents.** Any id the model
  list returns is valid; code never hardcodes, filters, prefers or rejects ids
  by vendor or name. Tests and examples use fake ids such as `acme/test-model`.

## What changes

- A `tinyhumans` catalogue row, a per-kind `CatalogShape`, a paged-catalog
  parser; `PLATFORM_BASE_URL` and `DEFAULT_TINYHUMANS_INFERENCE_URL` point at the
  proxy.
- `inference/default` holds `{"provider","model"}`; a bare slug still reads.
- Every provider row carries one model (stored under the four tier keys as a
  storage encoding, read through `Provider::model()`).
- Adding any provider ends with choosing a model; "Set as default" asks for one.
- The wire model is: agent pair / default model → `OPENCOMPANY_INFERENCE_MODEL`
  → a configured real id from a legacy tier map → fail closed. Never a tier name.

## Where

| Area | Files |
|---|---|
| Catalogue / catalog reads | `src/company/inference/catalogue.rs`, `frontend/src/inference/catalogue.ts`, `src/server/inference_models.rs`, `src/company/inference/probe.rs`, `src/server/setup.rs` |
| URL constants | `src/company/inference.rs:152` (`PLATFORM_BASE_URL`), `src/harness/built_in/provider.rs:55` (`DEFAULT_TINYHUMANS_INFERENCE_URL`) |
| Managed row | `src/server/ops/inference.rs` (`managed_state` ~:951-990), `frontend/src/inference/{ProviderList.tsx,connect.ts,ProvidersTab.tsx}` |
| Store | `src/company/inference/store.rs` (`DEFAULT_PROVIDER_KEY` :814, `load_default_slug` :826, `StoredProvider` :223-236) |
| Resolver | `src/company/inference.rs` (`InferenceDecl` :488, `model_for_tier` :360, `resolve_effective_scoped` :1213, `resolve_effective_for_tier` :1618), `src/harness/built_in/provider.rs` (`request_plan`, `TenantProvider::resolve` :2096) |
| Routes / DTOs | `src/server/ops/inference/providers.rs` (`add_provider` :309, `set_default` :1294, `edit_provider` :907), `src/server/ops/inference.rs` (`ProviderDto` :408, `InferenceStatusDto` :233) |
| Tier callers | `src/harness/built_in/build.rs:188-204` and the internal passes listed in 2d; the setup brain `src/harness/roster_build.rs:127-212` |
| E2E fixtures | `frontend/playwright.config.ts:214-226` (gain `OPENCOMPANY_INFERENCE_MODEL` in 2d) |

## Use cases

1. **Operator adds TinyHumans.** Pastes `th-not-a-real-key`; the host reads the
   paged catalog (every page, up to `total`); the model step lists them; operator picks
   `acme/test-model`, ticks "Make this the default"; Save. One row
   `tinyhumans`, health `ok`, default
   `{"provider":"tinyhumans","model":"acme/test-model"}`. Turns go to
   `https://api.tinyhumans.ai/agent-integrations/openrouter/chat/completions`
   with that id.
2. **Operator adds Anthropic** with `sk-not-a-real-key`, picks `test-model-small`,
   sets it as default. Every agent's next turn sends `test-model-small`.
3. **Ollama as default.** `http://localhost:11434` (the host's localhost), no
   key, picks `local-test-model`, default set.
4. **Operator switches the default** from Anthropic to OpenRouter: "Set as
   default" opens the model step prefilled with that row's model; one JSON
   write replaces the default.
5. **A company whose account key is set but has no TinyHumans row** still shows
   the legacy Managed row (one row), resolved through the fallback chain.
6. **Company with a bare-slug default from before** (`"openrouter"`): the page
   shows "Your default provider has no model. Choose one."; turns send the
   row's single model if it has one, else fail closed with "choose a model".
7. **Hosted tenant with no default.** Resolves through the env default. After
   2d it sends `OPENCOMPANY_INFERENCE_MODEL` (the manager injects it, with the
   proxy URL); if the manager has not, turns fail with "No model is chosen for
   this company. Choose a default provider and model in Settings → Inference."
8. **Internal passes** (title, triage, planning) send the default model, never
   `chat-v1`.

## Handled

Items 1, 4, 13b, 14, 15 and item 16's "no tiers"; Q1, Q2, Q3, Q13.

## Not handled (and why)

- Agent pairs — phase 3. Account-key flow — phase 4. Routing removal — phase 5.
- The managed fallback chain, `inference/managed/enabled` —
  [not-handled.md](not-handled.md).
- Rewriting an injected `/openai/v1` `OPENCOMPANY_INFERENCE_URL` in this repo —
  D-proxy: the manager injects the proxy URL. (`OPENCOMPANY_INFERENCE_URL`
  itself is removed later, in slice 6a — [phase-6a-remove-env-vars.md](phase-6a-remove-env-vars.md).
  This slice (2a) still reads it as today.)
- `TINYHUMANS_TOKEN_FILE` (item 18) is removed in slice 6a, not here.

## Data carry-over

None automatic. A bare-slug default stays a bare slug until an admin sets a
model. A row whose four tier keys hold different ids is `Ambiguous` and shows
"Needs a model"; nothing picks one. A company holding `provider/tinyhumans/key`
from the legacy Managed row keeps that row until it adds a `tinyhumans` row.

```json
// inference/default before (legacy)
"openrouter"
// after "Set as default" with a model
{"provider":"openrouter","model":"acme/test-model"}
```

## Commit order

1. 2a — catalogue row + `CatalogShape` + paged parser + URL constants + tests.
2. 2a — one-row rule (status + console) + tests + browser check.
3. 2b — store types + resolver + tests (no route or console change).
4. 2c backend — routes, DTOs, route tests.
5. 2c console — model step, set-default step, badges, unit + e2e, browser check.
6. 2d — no tier on the wire, `legacy_tiers` quarantine, setup brain, E2E fixture
   `OPENCOMPANY_INFERENCE_MODEL`, tests.

## Rollback

- A binary before 2b reads a JSON default as a slug that matches no row, so
  `resolve::primary` falls back to the first enabled provider (inferred from
  `resolve.rs:339-346`): degraded, not crashed. State this in the PR body.
- Rows keep the four-tier encoding, so an older binary sends the same model.
- Rolling back 2a returns TinyHumans calls to `/openai/v1`; a `tinyhumans` row
  written by the new binary is then an unknown kind to the old catalogue —
  record the observed behaviour in the 2a PR notes.
