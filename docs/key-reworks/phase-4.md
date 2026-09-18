# Phase 4 — one account key sets up TinyHumans for LLM and Composio

Part of [the keys rework](README.md). After phases 1a and 2 (the renamed
Composio slot, the `tinyhumans` row from slice 2a, `{provider, model}` defaults
from 2b and the model step from 2c). Code references are on `upstream/main @ fcfb3e1bc`
(2026-09-14).

| Slice | File | Dump items | One line |
|---|---|---|---|
| 4a | [phase-4a-account-key-fanout.md](phase-4a-account-key-fanout.md) | 6, 11; Q6, Q7, Q10 | server-side fan-out behind `PUT …/credential` and `finish_link` |
| 4b | [phase-4b-account-dialog.md](phase-4b-account-dialog.md) | 5, 6; Q9 | the dialog's conditional line, model step and after-save note |
| 4c | [phase-4c-reuse-banner.md](phase-4c-reuse-banner.md) | 12; Q8 | "use the account key?" banner, host-side copy routes, remove warning |

## What changes

- Saving the account key (`tinyhumans/key`) also fills, in order and under one
  per-company lock: `composio/tinyhumans/key`, the TinyHumans LLM provider (row
  + `provider/tinyhumans/key`), and — only when the company has no default — the
  default `{tinyhumans, model}`, then runs a health probe. A slot is written only
  when it is empty or still equals the old account key (Q7).
- A rejected key (health class `auth`) rolls back what this request wrote on
  the inference side, and keeps the Composio copy (Q6).
- `finish_link` (the key-grant return) runs the same plan and stops writing
  `inference/config` (Q10).
- The dialog names only the slots a save will fill, asks for a model when
  needed, and shows what was actually filled.
- Removing the TinyHumans key while it is the default warns and keeps the
  default (Q8); a banner offers to reuse the account key, copied on the host.

## Where

| Area | Files |
|---|---|
| Account routes | `src/server/ops/company_key.rs` (`CONSEQUENCE` :83, `get_status` :232, `set_key` :243, `start_link` :308, `finish_link` :446) |
| Account store | `src/company/company_key.rs` (`KEY_KEY` :64, `resolve` :123) |
| Slots written | `src/company/composio.rs` (after 1a), `src/company/inference/store.rs` (row, provider key, default, health) |
| Console | `frontend/src/views/connections/AccountKeyDialog.tsx`, `ApiKeyView.tsx`, `frontend/src/api/credential.ts`, `frontend/src/inference/ProvidersTab.tsx`, `RemoveProviderDialog.tsx`, `frontend/src/views/connections/ComposioSection.tsx` |

## Use cases

1. **Fresh company, admin pastes the account key and picks a model.** Nothing
   else set. Save → Composio slot filled, TinyHumans row + key added, no
   default so the dialog shows the catalog; admin picks `acme/test-model`;
   default written; health `ok`. Note: "Saved. Also connected TinyHumans for
   LLM (default: acme/test-model) and Composio."
2. **Company already on Anthropic as default.** Save fills Composio and the
   TinyHumans row, leaves the default alone, asks for no model.
3. **Admin had pasted a separate TinyHumans key on the LLM page.** Save leaves
   it (custom value), fills Composio if empty, and says what was kept.
4. **Rotation.** The account key changes from `th-not-a-real-key` to
   `th-not-a-real-key-2`. Copies still equal to the old key move to the new
   one; a custom LLM key stays.
5. **Revoked key.** Health says `auth`: the inference copy, the row this save
   created and the default this save wrote are rolled back; the account key and
   Composio copy stay; the dialog says why.
6. **Hand-minted key.** Composio will refuse it (no `connections` scope); the
   note says so (or the copy is skipped — decided in 4a with evidence).
7. **Admin removes the TinyHumans LLM key while it is the default.** The dialog
   warns; the default stays; the LLM page shows "Your TinyHumans account is
   connected. Use the same key for LLM?"; Yes copies `tinyhumans/key` into
   `provider/tinyhumans/key` on the host.
8. **Hosted tenant with nothing pasted.** Nothing changes: no account key, so no
   fan-out; the legacy chain keeps resolving through the projected token.

## Handled

Items 5, 6, 11, 12; Q6–Q10.

## Not handled (and why)

- Removing the fallback chains (item 10): hosted tenants have no stored key —
  see [not-handled.md](not-handled.md). Until item 10 lands, a company with no
  default still resolves Managed through `tinyhumans/key`; the banner matters
  for companies whose default or an agent pair names `tinyhumans`.
- `TINYHUMANS_TOKEN_FILE` (item 18) is **not** handled here — it is removed in
  slice 6a, after 5b, alongside three other environment variables. Do not touch
  it in 4a. See [phase-6a-remove-env-vars.md](phase-6a-remove-env-vars.md).
- A new sign-in button for the key-grant flow — #2304's operator request
  removed it; Q10 keeps only the backend.
- Copying into `composio/byok/key` or changing `composio/mode` — never.

## Data carry-over

No boot migration. Existing companies are filled on the next account-key save.

```text
before  tinyhumans/key="th-not-a-real-key"  provider/tinyhumans/key=""  composio/tinyhumans/key=""  inference/default=""
save    key="th-not-a-real-key-2", model="acme/test-model"
after   tinyhumans/key="th-not-a-real-key-2"
        composio/tinyhumans/key="th-not-a-real-key-2"
        inference/providers += {"slug":"tinyhumans",…}   provider/tinyhumans/key="th-not-a-real-key-2"
        inference/default={"provider":"tinyhumans","model":"acme/test-model"}
        inference/health.tinyhumans={"state":"ok",…}
```

## Commit order

1. 4a — `fan_out` + lock + tests (store-level).
2. 4a — `set_key` / `finish_link` wiring, DTO, route tests.
3. 4b — dialog, client, unit/e2e tests, browser check.
4. 4c — status field, copy routes (+ auth matrix rows), banners, remove
   warning, tests, browser check.

## Rollback

An older binary reads the Composio copy through 1a's legacy mirror and keeps
reading `tinyhumans/key`. A
TinyHumans row and JSON default written by 4a behave per phase 2's rollback
note. Nothing is cleared that the older binary needs.
