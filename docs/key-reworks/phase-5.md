# Phase 5 — routing carried over, then removed

Part of [the keys rework](README.md). After phases 2 and 3, so that every
per-workload choice has a replacement (the default, or an agent pair). Code
references are on `upstream/main @ fcfb3e1bc` (2026-09-14).

| Slice | File | Dump items | One line |
|---|---|---|---|
| 5a | [phase-5a-routes-carry.md](phase-5a-routes-carry.md) | 2; Q14; F7 | copy a unanimous four-row table into an empty default at build; banner otherwise |
| 5b | [phase-5b-routing-removal.md](phase-5b-routing-removal.md) | 2, 13a | delete the routes API, resolver arms, auto-route/scrub/park, Routing tab — backend and console together |

## What changes

- 5a, while routes are still read: at each company build, if `inference/default`
  is unset and all four tier rows name the same `slug:model` on an existing,
  enabled provider, write that pair as the default. Otherwise write nothing and
  report the table on the status so the LLM page can show a banner.
- 5b: nothing reads `inference/routes` except the carry-over. The Routing tab,
  `WorkloadModelDialog`, `routing.ts`, the routes API and every route side
  effect go.

## Where

| Area | Files |
|---|---|
| Carry-over | new `src/company/inference/routes_carry.rs`; `src/runtime/builder.rs` (before `resolve_effective`) |
| Resolver | `src/company/inference.rs` (step-4 routes branch ~:1262-1315, `resolve_effective_for_tier` :1618), `src/company/inference/resolve.rs` (routing helpers and tests) |
| Store | `src/company/inference/store.rs` (`ROUTES_KEY` :871, `load_routes`/`save_routes`) |
| Routes | `src/server/ops/inference/providers.rs` (auto-route, scrub, park, `get_routes`/`put_routes`), `src/server/ops/inference.rs` (`routing_table`, status `routes`) |
| Auth matrix | `tests/auth_matrix.rs`, `tests/snapshots/auth-matrix.txt` (routes rows) |
| Console | `frontend/src/inference/{RoutingTab,WorkloadModelDialog}.tsx`, `routing.ts`, `frontend/src/views/InferenceView.tsx`, `use-inference.ts`, `ProviderList.tsx`, `ProvidersTab.tsx`, `api/inference.ts`, `RemoveProviderDialog.tsx` |

## Use cases

1. **"Own" mode company.** Routes: all four tiers `acme:test-model`, no default.
   Next boot writes `inference/default = {"provider":"acme","model":"test-model"}`.
   Turns are unchanged. After 5b, the routes blob is still stored and unread.
2. **"Advanced" mode company.** `chat-v1 → groq:test-model-b`,
   `agentic-v1 → anthropic:test-model-small`, others unset. Nothing is written.
   The LLM page shows "Routing is going away. Each workload used: chat → groq ·
   test-model-b, agentic → anthropic · test-model-small. Choose one
   default provider and model, and pin agents that need something else." The
   admin sets a default and pins the heavy agents (phase 3).
3. **Company routed only to Managed.** All rows `managed` — no model, not
   copied. Banner shown before 5b. After 5b, if still no default, its turns
   resolve through the legacy chain (entry zero / manifest / env default); if
   that chain resolves nothing, they fail with "No model is chosen for this
   company. Choose a default provider and model in Settings → Inference." —
   never silently echo.
4. **Company with a default already** — routes are never consulted for the copy.
5. **A company that never used routing** sees no banner and no change.

## Handled

Items 2 and 13(a); Q14 and F7.

## Not handled (and why)

- `inference/managed/enabled` and the Managed switch (item 3) — see
  [not-handled.md](not-handled.md).
- The managed fallback chain (item 10) and entry zero — unchanged.
- Per-workload cost control — gone by design; no background model (Q13).
- Deleting the stored `inference/routes` value — the store cannot delete.

## Data carry-over

```json
// inference/routes (left stored)
{"chat-v1":"acme:test-model","reasoning-v1":"acme:test-model","agentic-v1":"acme:test-model","vision-v1":"acme:test-model"}
// inference/default before
""
// inference/default after the build
{"provider":"acme","model":"test-model"}
```

A table with any tier missing, any row differing, any `managed` row, or a
provider that is missing or disabled writes nothing.

## Commit order

1. 5a — `routes_carry.rs` + builder call + status field + tests.
2. 5a — banner + unit/e2e tests + browser check. Push and let CI finish before 5b.
3. 5b — backend deletions + auth matrix snapshot + tests.
4. 5b — console deletions + tests + browser check. (3 and 4 may be one commit;
   they must be pushed together.)
5. Docs: `docs/modules/inference/routing.md`, `routing-states.md` rewritten or
   marked superseded; `docs/spec/runtime/providers.md` default section.

## Rollback

- 5a writes only into an empty default, which an older binary reads per phase
  2's rollback note.
- 5b leaves `inference/routes` stored, so an older binary resumes routing
  exactly as before.
