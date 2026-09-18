# Phase 1 — key names and dead code

Part of [the keys rework](README.md). Phase 1 does not touch inference, so it
lands first. Code references are on `upstream/main @ fcfb3e1bc` (2026-09-14).

| Slice | File | Dump items | One line |
|---|---|---|---|
| 1a | [phase-1a-composio-keys.md](phase-1a-composio-keys.md) (+ [part 2](phase-1a-composio-keys-part2.md)) | 7, 8; Q11, Q12 | `composio/token` → `composio/tinyhumans/key`, `composio/api_key` → `composio/byok/key`; new-first read, legacy mirror for one release |
| 1b | [phase-1b-search-endpoint.md](phase-1b-search-endpoint.md) | 9 | the SearXNG-style endpoint moves into the `search/providers` record |
| 1c | [phase-1c-dead-code.md](phase-1c-dead-code.md) | 17 (dead code only); Q10, Q16 | delete verified unwired code; keep every `OPENHUMAN_*` name |

## What changes

- **Storage addresses only** for Composio (1a). Routes, request bodies, the
  console API client and the auth matrix stay as they are.
- **One field** on the search index row (1b), with the per-slug key still
  written for one release.
- **Deletions** of files nothing imports (1c), and reworded comments that named
  them.

## Where

| Area | Files |
|---|---|
| Composio storage | `src/company/composio.rs` (`TOKEN_KEY` :26, `MODE_KEY` :172, `API_KEY_KEY` :181, `store_token` :66, `resolve_credential` :98, `token_configured` :140, `store_api_key` :320, `resolve_access` :393) |
| Composio readers | `src/server/ops/composio.rs` (`stored_api_key` :949), `src/harness/built_in/composio.rs` (imports :104-105), `src/server/ops/capabilities.rs` (doc comments) |
| Search store | `src/company/search/store.rs` (`IndexEntry` :150-155, `list_providers` :199, `save_index` :264, `put_provider_locked` :406) |
| Dead code | `frontend/src/views/connections/CompanyCredentialCard.tsx`, `ConnectTinyHumansButton.tsx`, `HubAccountLinks.tsx` (orphaned by the card), their unit tests, `src/harness/built_in/provider.rs:169` (doc link) |

## Use cases

1. **Self-hosted company on its own Composio account upgrades.** It stored
   `composio/api_key = "ak-not-a-real-key"` and `composio/mode = "byok"` last
   month. After deploy, agents still get Composio tools: `resolve_access` reads
   `composio/byok/key` (empty), falls back to `composio/api_key`, presents it to
   `backend.composio.dev`. Nothing is rewritten at boot.
2. **The same admin re-saves the key on the Composio page.** The host writes
   `composio/byok/key`, then mirrors the same value to `composio/api_key` (one
   release, so a rolled-back binary keeps working). The next read finds the new
   address. A later clear clears both.
3. **A company that pasted a TinyHumans bearer for Composio** (`composio/token`)
   keeps working through the same fallback, and writes the new address on its
   next save.
4. **An admin toggles SearXNG off and on.** The address stays: it is carried in
   the index row and merged on update, and the per-slug key is still written.
5. **A developer searches for the "Sign in with TinyHumans" button.** It is
   gone, as it has been unreachable since #2304. A grant that returns to the
   Account page is still redeemed (its tests move to
   `use-redeem-key-grant.test.ts`).

## Handled

- Items 7 and 8 (Composio rename first, so phase 4 writes the new name).
- Item 9 (endpoint into `search/providers`, not the legacy `search/provider`).
- The dead-code half of item 17.

## Not handled (and why)

- Route renames for Composio: no requirement asks for them, and keeping them
  leaves the auth matrix and three e2e specs untouched.
- `composio/mode` removal: Q11 keeps it.
- Dropping the legacy fallbacks and mirrors: they must live at least one release.
- `OPENHUMAN_*` names, the key-grant backend routes, CLI login rows (F8),
  `RunnerDispatch` (a feature-gated, tested transport documented as not yet
  wired — see 1c's evidence).

## Data carry-over

Mirror on write, new-first on read. Examples:

```text
before  composio/token = "th-not-a-real-key"     composio/tinyhumans/key = (absent)
read    → "th-not-a-real-key" (legacy fallback)
save    composio/tinyhumans/key = "th-not-a-real-key-2"; composio/token = "th-not-a-real-key-2" (mirror)
clear   composio/tinyhumans/key = ""; composio/token = ""
```

```json
// search/providers before
[{"slug":"searxng","enabled":true}]
// after the next update of that row (search/provider/searxng/endpoint still written too)
[{"slug":"searxng","enabled":true,"endpoint":"https://search.example"}]
```

## Commit order

1. `docs: key reworks plan` (this folder).
2. 1a — one commit: constants, helpers, readers, writers, tests, docs.
3. 1b — one commit: `IndexEntry.endpoint`, read/merge/write, tests, docs.
4. 1c — one commit: deletions, moved redeem tests, comment rewording.

Push after each. Verify each by head SHA before starting the next.

## Rollback

- **1a:** safe — every write mirrors to the legacy address for one release.
- **1b:** safe — the per-slug key is still written for one release.
- **1c:** revert the commit; nothing stored depends on it.
