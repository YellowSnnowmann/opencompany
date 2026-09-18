# Phase 6 — four environment variables removed

Part of [the keys rework](README.md). Last, because it removes the escape
hatches every earlier slice still relied on. Code references are on
`upstream/main @ fcfb3e1bc` (2026-09-14).

- **Requested by:** the operator, 2026-09-15, after the rest of this folder was
  drafted. Not one of the original 20 dump items; added directly.
- **Runs after:** 2a (the `tinyhumans` catalogue row and paged-catalog
  machinery an E2E fixture now stands in for), 2d (no tier name reaches the
  wire — the proxy 400s on one), and 5b (the routing removal, so the legacy
  resolution arms 6a touches are in their final shape). See
  [phase-6a-remove-env-vars.md](phase-6a-remove-env-vars.md) §0 for why the
  order is load-bearing, not a preference.

| Slice | File | One line |
|---|---|---|
| 6a | [phase-6a-remove-env-vars.md](phase-6a-remove-env-vars.md) (+ [part 2](phase-6a-remove-env-vars-part2.md)) | delete the reads of `OPENCOMPANY_INFERENCE_KEY`, `OPENCOMPANY_INFERENCE_URL`, `OPENCOMPANY_COMPOSIO_BACKEND_URL`, `TINYHUMANS_TOKEN_FILE` |

## What changes

Four environment variables stop being read anywhere in `src/`. Nothing else
prefixed `OPENCOMPANY_` or `TINYHUMANS_` is touched — in particular every
`OPENHUMAN_*` name stays (Q16, unchanged by this phase), and so do
`TINYHUMANS_API_KEY`, `TINYHUMANS_API_URL` and `OPENCOMPANY_INFERENCE_MODEL`.

| Variable | Was | After 6a |
|---|---|---|
| `OPENCOMPANY_INFERENCE_KEY` | Per-tenant override, checked before the platform token source | gone; the platform token source (`TINYHUMANS_API_KEY` only, once `TINYHUMANS_TOKEN_FILE` is also gone) is the only instance-identity credential |
| `OPENCOMPANY_INFERENCE_URL` | Overrode the managed/env-default base URL, and was itself an independent legacy resolution arm | gone; the hardcoded constant (the proxy, after 2a) is the only base URL for that arm |
| `OPENCOMPANY_COMPOSIO_BACKEND_URL` | Overrode the Composio managed backend URL | gone; `TINYHUMANS_API_URL` (unchanged), then the hardcoded prod default |
| `TINYHUMANS_TOKEN_FILE` | The hosted-tenant projected-token credential tier | gone; `TinyhumansTokenSource` becomes static-key-only |

## Why this is safe only in this order

`OPENCOMPANY_INFERENCE_URL` is not only a URL override: current-state.md §3.1
step 4 and §8 describe it as one of the sources `resolve_legacy_scoped` tries
in its own right (the "env default" arm), independent of whether a company's
`inference/config` names `managed`. Phase 2a's own §0 gate exists because
flipping the platform-URL constants to the proxy while an injected
`OPENCOMPANY_INFERENCE_URL` could still point traffic at the old
`/openai/v1` surface would leave two live behaviours. Once 6a removes the
variable, that fork disappears by construction: the constant is the **only**
base URL left for that arm, so it must already be the proxy address (2a) and
the wire model it sends must already never be a tier name (2d), or every
company that resolves through that arm starts getting 400s the moment 6a
ships. Running 6a before 2a or 2d would reintroduce exactly the outage 2a's
gate was written to prevent, with no variable left to unblock it. See
[phase-6a-remove-env-vars.md](phase-6a-remove-env-vars.md) §0 for the full
argument and the E2E fixture consequence.

## Handled

The operator's 2026-09-15 request, all four variables.

## Not handled (and why)

- **Item 10** (dropping the managed fallback chains entirely) and **item 3**
  (`inference/managed/enabled`) are unchanged by this phase — see
  [not-handled.md](not-handled.md). 6a removes four *environment variables*;
  it does not remove `tinyhumans/key`, `provider/tinyhumans/key`, or any step
  of either managed chain that does not depend on the four variables.
- Any other `OPENCOMPANY_*` or `TINYHUMANS_*` name. A blanket removal was not
  asked for and is explicitly out of scope, the same way Q16 scoped
  `OPENHUMAN_*` in phase 1c.

## Data carry-over

None. No secret-store key is read, written, or migrated by 6a — it removes
process-environment reads only.

## Commit order

See [phase-6a-remove-env-vars.md](phase-6a-remove-env-vars.md) §14 (added at
handover — the two-part slice file did not otherwise spell out a numbered
commit sequence the way phase-1.md/phase-3.md/phase-5.md do; treat §14 as a
first draft, not independently re-verified).

## Rollback

An older binary re-reads all four variables exactly as it did before 6a. The
only thing 6a's own commits could strand is a manager or `docker compose`
deployment that had come to depend on `OPENCOMPANY_INFERENCE_URL` or
`OPENCOMPANY_COMPOSIO_BACKEND_URL` pointing somewhere non-default — see
phase-6a §9 for what the operator/manager must provide instead, and do so
**before** this phase ships to hosted tenants, not after.
