# Phase 1a — Composio storage keys, part 2: tests, console, docs, gotchas

Continues [phase-1a-composio-keys.md](phase-1a-composio-keys.md) (goal, files,
current and target code, edit order, carry-over). Code read at
`upstream/main @ fcfb3e1bc` on 2026-09-14.

Fake values only: `th-not-a-real-key`, `ak-not-a-real-key` (and `-2` suffixes).
Existing fixtures such as `ak_live`, `backend-bearer`, `cmp_byo` are already
obviously fake and stay as they are.

## 6.1 `store_api_key` partial failures

Continues part 1 §6. Write numbers refer to the orders in part 1 §4.4 (set:
1 `BYOK_KEY_KEY` = key, 2 `LEGACY_API_KEY_KEY` = key, 3 `MODE_KEY` = `byok`;
clear: 1 `MODE_KEY` = `managed`, 2 `BYOK_KEY_KEY` = `""`,
3 `LEGACY_API_KEY_KEY` = `""`).

| Direction | Start | Failing write | Left behind | Effect |
|---|---|---|---|---|
| set | managed, no BYOK key | 1 | nothing changed | managed, inert |
| set | managed, legacy `ak-old` | 2 | new=`ak-new`, legacy=`ak-old`, mode `managed` | managed, BYOK keys unread → inert |
| set | managed | 3 | new=`ak-new`, legacy=`ak-new`, mode `managed` | managed, BYOK keys unread → inert |
| set | byok, both `ak-old` (rotation) | 1 | nothing changed | BYOK on `ak-old` |
| set | byok, both `ak-old` (rotation) | 2 | new=`ak-new`, legacy=`ak-old`, mode `byok` | this build presents `ak-new`; a pre-1a binary would still present `ak-old` until a retry |
| set | byok, both `ak-old` (rotation) | 3 | both `ak-new`, mode `byok` (unchanged) | rotated everywhere |
| clear | byok, key at new or legacy | 1 | nothing changed | BYOK intact with its key |
| clear | byok, both `ak-old` | 2 | mode `managed`, both `ak-old` | managed, inert |
| clear | byok, both `ak-old` | 3 | mode `managed`, new `""`, legacy `ak-old` | managed, inert |

In every row a failed call returns `Err`, which `set_api_key` turns into an
error response before the journal line and before `evict_catalog_cache`
(`ops/composio.rs:744-747`), exactly as today.

## 7. Tests

### 7.1 Existing Rust tests that change

Every test that names `TOKEN_KEY` or `API_KEY_KEY` on `fcfb3e1bc`. Each one
keeps its name, its intent and its assertions; only the constant changes,
unless noted.

**`src/company/composio.rs` (`mod tests`, `:618`)**

| Test | Line | Change |
|---|---|---|
| `a_clear_that_fails_on_the_mode_write_leaves_byok_intact` | `:871` | seed `BYOK_KEY_KEY` instead of `API_KEY_KEY` (`:883`). Assertions unchanged. |
| `a_clear_that_fails_on_the_key_write_still_lands_managed` | `:915` | `blocked_key: BYOK_KEY_KEY` (`:919`), seed `BYOK_KEY_KEY` (`:927`); doc comment `:912` names `BYOK_KEY_KEY`. |
| `byok_never_falls_back_to_a_managed_credential` | `:953` | seed `TINYHUMANS_KEY_KEY` (`:963`). Also seed `LEGACY_TOKEN_KEY` = `th-not-a-real-key` so the test proves neither managed address leaks into BYOK. |
| `the_byok_key_is_never_confused_with_the_backend_token` | `:980` | seed `TINYHUMANS_KEY_KEY` (`:986`). |
| `a_store_read_error_on_the_byo_token_propagates_rather_than_falling_back` | `:1109` | `blocked_key: TINYHUMANS_KEY_KEY` (`:1113`), seed `TINYHUMANS_KEY_KEY` (`:1120`). |

The helper stores `SecretsFailingToWrite` (`:844`) and `SecretsFailingToRead`
(`:1082`) are reused as they are.

**`src/harness/built_in/composio.rs` (`mod tests`, `use super::*` at `:2824`)**

| Test | Line | Change |
|---|---|---|
| `resolve_prefers_the_stored_token_then_the_token_source_then_fails_closed` | `:2894` | `TOKEN_KEY` → `TINYHUMANS_KEY_KEY` at `:2933`, `:2955` (via the renamed re-export). |
| `the_company_key_credentials_composio_between_a_byo_token_and_the_instance` | `:2998` | same rename at `:3035`, `:3048`. |

**`src/server/ops/composio.rs`**

| Test | Line | Change |
|---|---|---|
| `a_failed_check_changes_absolutely_nothing` | `:2487` | `use crate::company::composio::{BYOK_KEY_KEY, LEGACY_API_KEY_KEY, MODE_KEY};` (`:2489`); `slots` returns a 3-tuple `(read(MODE_KEY), read(BYOK_KEY_KEY), read(LEGACY_API_KEY_KEY))` (`:2514`). The before/after equality assertion is unchanged. |

**No change needed** (verified 2026-09-14; they use routes or the public
`store_token` / `store_api_key` / `token_configured` functions, whose names and
signatures do not change):

- `src/server/ops/company_key/test.rs`: `a_pasted_composio_token_still_outranks_the_company_key`,
  `the_credential_plane_and_the_composio_plane_may_honestly_disagree`, and the
  test around `:241` (comment-only edit, see part 1 §5 step 8).
- `src/server/ops/write_test.rs`: `a_member_cannot_change_what_the_company_reaches_the_world_as`
  (`:6952`), `an_admin_is_refused_by_none_of_them` (`:7092`) — route paths only.
- `src/server/ops/capabilities.rs`: `the_composio_verdict_walks_every_credential_tier`
  (`:1289`), `the_dto_reports_exactly_what_the_resolver_resolves` (`:1350`),
  `both_response_paths_carry_the_credential_tier` (`:1407`).
- `src/harness/built_in/planning/test.rs`: `a_hosted_tenant_with_no_pasted_token_still_has_a_composio_credential`
  (`:1421`) and the `store_token` use at `:1477` — comment `:1414` only.
- `src/harness/built_in/composio.rs`: `byok_keeps_the_managed_credential_for_the_curated_catalog_only`
  (`:3370`), `byok_without_a_managed_tier_has_no_curated_catalog_credential`
  (`:3408`), `resolve_follows_the_stored_route` (`:3438`).
- `src/server/ops/composio.rs`: every route-level test that `PUT`s
  `/api/v1/company/composio/token` or `/composio/api-key`, and
  `the_managed_tier_reads_the_instance_identity_under_byok` (`:2184`).
- `src/company/composio_probe.rs`: no key reference (`:225` names a route).
- `tests/auth_matrix.rs:450-452`, `tests/snapshots/auth-matrix.txt`: routes
  unchanged, **must not move**.

### 7.2 New Rust unit tests — `src/company/composio.rs` `mod tests`

Put them after `a_store_read_error_on_the_byo_token_propagates_rather_than_falling_back`
under a `// ── Storage addresses and the legacy fallback (#2306) ──` banner.
All use `MemSecrets` (`:664`) unless a failing store is named. `use super::*`
already brings the four consts, `Arc` and `TinyhumansTokenSource`. Tests 2 and
6 also need `use crate::company::credentials::CredentialSource;`, which is
**not** imported at the top of `composio.rs`.

Helper to add once, beside `MemSecrets`:

```rust
/// Raw slot contents, blank-or-absent collapsed to `""`, so an assertion holds
/// on a backend that stores `""` and on one that treats it as absent.
async fn raw(secrets: &dyn SecretStore, company: &CompanyId, key: &str) -> String {
    secrets.get(company, key).await.unwrap().map(|SecretValue(v)| v).unwrap_or_default()
}
```

| # | Test name | Setup | Call | Assert |
|---|---|---|---|---|
| 1 | `the_storage_addresses_are_pinned` | — | — | `TINYHUMANS_KEY_KEY == "composio/tinyhumans/key"`, `LEGACY_TOKEN_KEY == "composio/token"`, `BYOK_KEY_KEY == "composio/byok/key"`, `LEGACY_API_KEY_KEY == "composio/api_key"`, `MODE_KEY == "composio/mode"`, `DEFAULTS_KEY == "composio/defaults"` |
| 2 | `a_legacy_only_tinyhumans_key_is_still_presented` | `LEGACY_TOKEN_KEY` = `th-not-a-real-key` | `resolve_credential(&c, &s, None)`, `token_configured`, `load_tinyhumans_key` | credential `current()` = `Some("th-not-a-real-key")`, `source()` = `CredentialSource::Static`; `token_configured` = `true`; loader = `Some("th-not-a-real-key")` |
| 3 | `a_legacy_only_byok_key_is_still_presented` | `MODE_KEY` = `byok`; `LEGACY_API_KEY_KEY` = `ak-not-a-real-key` | `resolve_access(&c, &s, None)`, `load_byok_key` | mode `Byok`; `current()` = `Some("ak-not-a-real-key")`; loader same |
| 4 | `the_new_address_wins_over_the_legacy_one` | new = `th-not-a-real-key-2`, legacy token = `th-not-a-real-key`; `BYOK_KEY_KEY` = `ak-not-a-real-key-2`, legacy api = `ak-not-a-real-key` | both loaders | `Some("th-not-a-real-key-2")`, `Some("ak-not-a-real-key-2")` |
| 5 | `a_blank_new_address_falls_back_to_a_non_empty_legacy_one` | `TINYHUMANS_KEY_KEY` = `"   "`, `LEGACY_TOKEN_KEY` = `th-not-a-real-key`; `BYOK_KEY_KEY` = `""`, `LEGACY_API_KEY_KEY` = `ak-not-a-real-key` | both loaders | legacy values returned (documents the rule in part 1 §6) |
| 6 | `a_byok_value_is_never_presented_as_the_tinyhumans_bearer` | mode unset (managed); `LEGACY_API_KEY_KEY` = `ak-not-a-real-key`; `BYOK_KEY_KEY` = `ak-not-a-real-key-2`; no company key; no token source | `resolve_credential(&c, &s, None)`, `token_configured`, `load_tinyhumans_key` | `!credential.configured()`; `source()` = `CredentialSource::None`; `false`; `None` |
| 7 | `a_tinyhumans_value_is_never_presented_as_the_byok_key` | `MODE_KEY` = `byok`; `LEGACY_TOKEN_KEY` = `th-not-a-real-key`; `TINYHUMANS_KEY_KEY` = `th-not-a-real-key-2` | `resolve_access(&c, &s, None)`, `load_byok_key` | mode `Byok`; `!credential.configured()`; `None` |
| 8 | `a_write_mirrors_to_the_legacy_address_for_one_release` | `LEGACY_TOKEN_KEY` = `th-not-a-real-key`; `LEGACY_API_KEY_KEY` = `ak-not-a-real-key` | (a) `store_token(&c, &s, " th-not-a-real-key-2 ")`; (b) `store_api_key(&c, &s, "ak-not-a-real-key-2")` | after (a): `raw(TINYHUMANS_KEY_KEY)` = `raw(LEGACY_TOKEN_KEY)` = `th-not-a-real-key-2` (trimmed, same value); BYOK addresses still hold `""` / `ak-not-a-real-key` (untouched). After (b): returns `Byok`; `raw(BYOK_KEY_KEY)` = `raw(LEGACY_API_KEY_KEY)` = `ak-not-a-real-key-2`; `raw(MODE_KEY)` = `byok`; token addresses unchanged |
| 9 | `clearing_the_token_clears_both_addresses` | new = `th-not-a-real-key-2`; legacy = `th-not-a-real-key` | `store_token(&c, &s, "")` | both raw `""`; `token_configured` = `false`; `resolve_credential(&c,&s,None)` not configured |
| 10 | `a_failed_legacy_mirror_write_propagates` | `SecretsFailingToWrite { blocked_key: LEGACY_TOKEN_KEY }`; inner legacy = `th-not-a-real-key` | `store_token(&c, &s, "th-not-a-real-key-2")` | `Err(OpenCompanyError::Store(_))`; `raw(inner, TINYHUMANS_KEY_KEY)` = `th-not-a-real-key-2` (new written first); `raw(inner, LEGACY_TOKEN_KEY)` = `th-not-a-real-key`; `load_tinyhumans_key` = `Some("th-not-a-real-key-2")` |
| 11 | `a_failed_legacy_clear_keeps_the_old_value_readable` | same store; inner new = `th-not-a-real-key-2`, legacy = `th-not-a-real-key` | `store_token(&c, &s, "")` | `Err`; `load_tinyhumans_key` = `Some("th-not-a-real-key")` (part 1 §6 row 11) |
| 12 | `clearing_the_byok_key_clears_both_addresses` | `MODE_KEY` = `byok`; new = `ak-not-a-real-key-2`; legacy = `ak-not-a-real-key` | `store_api_key(&c, &s, "")` | returns `Managed`; both BYOK raw `""`; `raw(MODE_KEY)` = `managed` |
| 13 | `byok_with_blank_new_and_blank_legacy_keys_withholds_tools` | `MODE_KEY` = `byok`; `BYOK_KEY_KEY` = `""`; `LEGACY_API_KEY_KEY` = `"  "`; `TINYHUMANS_KEY_KEY` = `th-not-a-real-key` | `resolve_access(&c, &s, Some(Arc::new(TinyhumansTokenSource::static_key("platform-identity"))))` | mode `Byok`; `!credential.configured()` |
| 14 | `a_byok_set_whose_legacy_mirror_fails_leaves_a_managed_company_managed` | `SecretsFailingToWrite { blocked_key: LEGACY_API_KEY_KEY }`; inner: legacy = `ak-not-a-real-key`, no mode | `store_api_key(&c, &s, "ak-not-a-real-key-2")` | `Err`; `load_mode` = `Managed`; `resolve_access(..).mode` = `Managed`; `raw(inner, MODE_KEY)` = `""` (the mode write never ran); `raw(inner, BYOK_KEY_KEY)` = `ak-not-a-real-key-2` (inert) |
| 15 | `a_byok_clear_whose_legacy_clear_fails_still_lands_managed` | same store; inner: `MODE_KEY` = `byok`, new = `ak-not-a-real-key-2`, legacy = `ak-not-a-real-key` | `store_api_key(&c, &s, "")` | `Err`; `load_mode` = `Managed`; `resolve_access(..).mode` = `Managed`; `raw(inner, BYOK_KEY_KEY)` = `""`; `raw(inner, LEGACY_API_KEY_KEY)` = `ak-not-a-real-key` (inert) |
| 16 | `a_store_read_error_on_the_legacy_token_propagates` | `SecretsFailingToRead { blocked_key: LEGACY_TOKEN_KEY }`; nothing at the new address | `resolve_credential(&c, &s, None)` | `Err(OpenCompanyError::Store(_))` — never falls through to `company_key::resolve` |
| 17 | `a_non_empty_new_address_does_not_read_the_legacy_one` | `SecretsFailingToRead { blocked_key: LEGACY_API_KEY_KEY }`; inner `BYOK_KEY_KEY` = `ak-not-a-real-key`, `MODE_KEY` = `byok` | `resolve_access(&c, &s, None)` | `Ok`; `current()` = `Some("ak-not-a-real-key")` |
| 18 | `reading_never_writes_either_address` | `LEGACY_TOKEN_KEY` = `th-not-a-real-key`; `MODE_KEY` = `byok`; `LEGACY_API_KEY_KEY` = `ak-not-a-real-key` | `resolve_access`, `resolve_credential`, `token_configured`, both loaders | `s.get(TINYHUMANS_KEY_KEY)` and `s.get(BYOK_KEY_KEY)` are `None` (not `Some("")`); legacy values unchanged |

The existing `a_clear_that_fails_on_the_mode_write_leaves_byok_intact` and
`a_clear_that_fails_on_the_key_write_still_lands_managed` (7.1), plus rows 14
and 15, cover the failing writes of both `store_api_key` directions that change
what is left behind (§6.1).

### 7.3 New Rust tests elsewhere

**`src/harness/built_in/composio.rs` `mod tests`** (real `FsSecretStore`, so the
nested address is exercised on a real backend):

- `a_legacy_only_token_still_wires_managed_composio` — `FsSecretStore` in a
  tempdir (copy the setup of `:2894`); `secrets.set(&company,
  crate::company::composio::LEGACY_TOKEN_KEY, SecretValue("th-not-a-real-key".into()))`;
  `TenantComposio::resolve(&company, &secrets, Vec::new(), None, None, None)` is
  `Some`; `token_of(&resolved)` = `Some("th-not-a-real-key")`;
  `resolved.credential().source()` = `CredentialSource::Static`.
- `a_legacy_only_byok_key_still_resolves_the_byok_route` — tempdir
  `FsSecretStore`; set `MODE_KEY` = `byok` and
  `crate::company::composio::LEGACY_API_KEY_KEY` = `ak-not-a-real-key`;
  `TenantComposio::resolve(…, None)` is `Some`, `config.mode()` = `Byok`,
  `config.current_token()` = `Some("ak-not-a-real-key")`. Then
  `store_api_key(&company, &secrets, "ak-not-a-real-key-2")` and assert both
  `BYOK_KEY_KEY` and `LEGACY_API_KEY_KEY` read back `ak-not-a-real-key-2`.

**`src/server/ops/composio.rs` `mod tests`** (helpers `home` `:1511`,
`state_with_manifest_id` `:1554`, `send_for` `:1611`, `runtime_of` `:1713`,
`GRANTED` `:2992`, `probe_override::set`):

- `a_token_put_mirrors_to_the_legacy_slot` — company `legacytoken`; seed
  `LEGACY_TOKEN_KEY` = `th-not-a-real-key` through
  `runtime.secrets().set(…)`. `GET /api/v1/company/composio` →
  `credentialSource` = `"static"`. `PUT /api/v1/company/composio/token
  {"token":"th-not-a-real-key-2"}` → 200. Raw store: `TINYHUMANS_KEY_KEY` and
  `LEGACY_TOKEN_KEY` both `th-not-a-real-key-2`. Neither response body
  contains `th-not-a-real-key`. Then `PUT … {"token":""}` → 200; both slots
  blank.
- `the_api_key_test_route_reads_a_legacy_byok_key` — company `legacybyok`;
  seed `MODE_KEY` = `byok`, `LEGACY_API_KEY_KEY` = `ak-not-a-real-key`;
  `probe_override::set("legacybyok", Ok(()))`; `POST
  /api/v1/company/composio/api-key/test` → 200 with `ok: true` (proves
  `stored_api_key` falls back). Then `PUT /api/v1/company/composio/api-key
  {"apiKey":"ak-not-a-real-key-2"}` → 200; raw `BYOK_KEY_KEY` and
  `LEGACY_API_KEY_KEY` both `ak-not-a-real-key-2`, `MODE_KEY` = `byok`.

**`src/server/ops/capabilities.rs` `mod tests`** (helpers `home` `:613`,
`state_with_manifest` `:620`, `GRANTS_COMPOSIO` `:1245`):

- `a_legacy_only_token_still_reads_as_configured` — seed `LEGACY_TOKEN_KEY` =
  `th-not-a-real-key`; `composio::token_configured(…)` = `true`;
  `super::composio_credential_source(runtime.as_ref(), None)` =
  `Some(CredentialSource::Static)`.

### 7.4 Frontend tests

No frontend test **asserts** a storage address. The three e2e specs call the
unchanged **routes** and do not move:
`frontend/test/e2e/composio-account-choice.spec.ts` (`:70`, `:136`),
`composio-catalog-deadline.spec.ts` (`:107`, `:120`, `:325`),
`composio-provider-detail.spec.ts` (`:104`, `:166`).

Comment-only edits (storage address, not route):

- `frontend/test/unit/composio-managed-token-card.test.ts:21` —
  "`composio/api_key` or `composio/mode`" → "`composio/byok/key` or
  `composio/mode`".
- `frontend/test/unit/composio-managed-token-card.test.ts:95-96` —
  "`composio/token` is a bearer … `composio/api-key` is a key" →
  "`composio/tinyhumans/key` is a bearer … `composio/byok/key` is a key".
- `frontend/test/unit/composio-rows.test.ts:490-491` — "Writing
  `composio/token`" → "Writing `composio/tinyhumans/key`".
- `:422`, `:467` name `POST …/composio/api-key/test` (a route): unchanged.

## 8. Console/UI

**No behaviour, copy, component or API-client change.** No rendered string
changes, so no browser verification is owed for this slice; say so in the PR.

- `frontend/src/api/composio.ts`: `setComposioToken` (`:358`),
  `setComposioApiKey` (`:388`) and `testComposioApiKey` (`:430`) unchanged.
  Comments at `:11`, `:208` name routes: unchanged.
- `frontend/src/views/connections/ComposioSection.tsx` labels ("Composio
  token", "Composio API key"): unchanged.
- Comment-only edits (storage address):
  - `frontend/src/api/types.ts:1861` — "stored under `composio/token`" →
    "stored under `composio/tinyhumans/key` (or its legacy address
    `composio/token`)". The field `composioTokenConfigured` keeps its name.
  - `frontend/src/composio/rows.ts:191` — "the `composio/token` override" →
    "the `composio/tinyhumans/key` override".
  - `frontend/src/composio/rows.ts:242` — "Writing `composio/token`" →
    "Writing `composio/tinyhumans/key`".
  - `frontend/src/composio/rows.ts:256`, `:294` and
    `frontend/src/composio/classify.ts:78` name a route: unchanged.

### 8.1 Docs to update

From `git grep -n 'composio/token\|composio/api_key' -- docs` on `fcfb3e1bc`.
Only lines naming a **storage address** change; route mentions stay.

| File:line | Change |
|---|---|
| `docs/modules/composio/data-model.md:14` | row key → `composio/tinyhumans/key`; add "(read fallback: `composio/token`)" to "Read by" |
| `docs/modules/composio/data-model.md:15` | row key → `composio/byok/key`; "(read fallback: `composio/api_key`)" |
| `docs/modules/composio/data-model.md` after `:16` | add a short "Legacy addresses (#2306)" paragraph: read new-then-legacy with the fixed mapping; for one release every write stores the same value at both addresses and a clear clears both; no boot migration; a later release stops the legacy write |
| `docs/modules/composio/data-model.md:20` | `composio/token` → `composio/tinyhumans/key`; `composio/api_key` → `composio/byok/key` |
| `docs/modules/composio/data-model.md:91` | `composio/token` → `composio/tinyhumans/key` |
| `docs/modules/composio/README.md:96-97` | both names → new addresses |
| `docs/modules/composio/architecture.md:44` | "BYOK reading only `composio/api_key`" → "BYOK reading only `composio/byok/key` (then `composio/api_key`)"; add "the legacy fallback never crossed" to the must-exist list |
| `docs/modules/composio/connect-flow.md:108` | `composio/token` → `composio/tinyhumans/key` (`:265` is a route: unchanged) |
| `docs/modules/composio/resolution.md:26` | tier 1 → `composio/tinyhumans/key` (then `composio/token`) |
| `docs/modules/composio/resolution.md:46`, `:57`, `:74` | `composio/api_key` → `composio/byok/key`; `:57` `composio/token` → `composio/tinyhumans/key` |
| `docs/modules/composio/resolution.md:83` | "paste a token into `composio/token`" → "into `composio/tinyhumans/key`" |
| `docs/modules/inference/credentials.md:16` | slot `composio/token` → `composio/tinyhumans/key` (the "Set by" route stays `PUT {scope}/composio/token`) |
| `docs/modules/inference/credentials.md:78`, `:142` | diagram label → `composio/tinyhumans/key` (keep column alignment) |
| `docs/modules/server/connections.md:99` | "the BYO override `composio/token`" → "`composio/tinyhumans/key`" (`:188` is a route: unchanged) |
| `docs/spec/runtime/credentials.md:215-216` | both `composio/token` → `composio/tinyhumans/key` |
| `docs/spec/runtime/credentials.md:239` | `composio/api_key` → `composio/byok/key` (`:245`, `:365` are routes: unchanged) |
| `docs/spec/runtime/desktop.md:116` | `composio/token` → `composio/tinyhumans/key` |
| `docs/modules/server/authority.md:26` | route only: **unchanged** |

Every file above stays under 500 lines (largest: `docs/spec/runtime/credentials.md`,
483 lines on `fcfb3e1bc` — add at most a clause there, no new paragraph).

## 9. Must not touch

- Route paths, methods and request/response bodies in
  `src/server/ops/composio.rs:281-283`; `SetToken`, `SetApiKey`,
  `MutationResponse`, `ApiKeyTestDto`, `ComposioStatusDto`.
- `tests/auth_matrix.rs`, `tests/snapshots/auth-matrix.txt`.
- Journal kinds `credential_set`, `credential_cleared`, `composio_byok_set`,
  `composio_byok_cleared`; the notes `CLEAR_NOTE`, `SWITCH_NOTE`,
  `INACTIVE_TOKEN_NOTE`, `BYOK_NOTE`, `MANAGED_NOTE`.
- `MODE_KEY`, `DEFAULTS_KEY`, `ComposioMode::parse`, `load_mode`,
  `load_defaults` / `set_default` / `clear_default` / `forget_connection`.
- `company_key::resolve` and the managed precedence (BYO key → company key →
  instance identity → none). Item 10 (dropping fallbacks) is a later slice.
- `DTO` / capability field names `composioTokenConfigured`,
  `managedCredentialSource`, `credentialSource`.
- `frontend/src/api/composio.ts` functions; any console component or copy.
- `src/company/inference/**`, `src/company/search/**`,
  `src/server/ops/company_key.rs` code (comment `:191` only).
- The four `SecretStore` backends (`src/store/fs.rs`, `sqlite.rs`,
  `mongodb.rs`): they already store any key string; nested addresses need no
  change (`src/store/paths.rs:592` encodes the whole key into one filename).
- `vendor/`.
- No boot-time migration, no copy-on-read, no new secret-store key beyond the
  two in the naming contract.
- **Do not stop the legacy write in this PR**, and do not remove the legacy
  read. Both are the follow-up in §11.

## 10. Done when

1. `git grep -nw -e TOKEN_KEY -e API_KEY_KEY -- src` prints nothing.
2. `git grep -n '"composio/token"\|"composio/api_key"' -- src` prints only the
   `LEGACY_TOKEN_KEY` / `LEGACY_API_KEY_KEY` definitions and test 1
   (`the_storage_addresses_are_pinned`).
3. `git grep -n 'composio/token\|composio/api_key' -- docs frontend src` shows
   only route mentions (`…/composio/token`, `…/composio/api-key`), the two
   legacy const definitions, test 1, and explicit "legacy"/"fallback" mentions.
4. `git diff --stat upstream/main -- tests/ frontend/src/api/` is empty.
5. `cargo fmt --all -- --check` passes locally; `npm run typecheck`,
   `npm run typecheck:unit`, `npm run typecheck:e2e` pass.
6. CI on the pushed head SHA: zero failures **and** zero pending, including
   the `Rust (openhuman, tinymemory)` lane (`.github/workflows/ci.yml:1060`;
   the harness tests need `openhuman`) and both Console E2E lanes.
7. Every test in §7.1 still passes under its existing name; every test in §7.2
   and §7.3 exists under the name given and passes.
8. Pushed as two commits: (a) "Move Composio keys to new addresses, mirroring
   the legacy ones" — part 1 §5 steps 1-8 with tests; (b) "Name the renamed
   Composio keys in docs and console comments" — steps 9-10.
9. The PR description records the follow-up (§11) and the re-upgrade caveat.

## 11. Gotchas

- **Error handling follows search, not inference.**
  `inference/store.rs:697-721` logs a failed legacy write and returns `Ok(())`;
  `search/store.rs:569-591` logs and **returns the error**. This slice
  propagates.
- **Do not trim in `read_with_legacy`.** It returns the stored bytes so
  `resolve_credential` presents exactly what it presents today; the writers
  already trim, and `stored_api_key` trims on its own.
- **A read error on the new address must propagate, not fall to legacy.**
  `secrets.get(new).await?` comes first; only a successful blank read moves on.
  `a_store_read_error_on_the_byo_token_propagates_rather_than_falling_back`
  guards it.
- **The legacy write is unconditional.** Do not read the legacy slot first to
  decide; that adds a read-then-write window and a second error path.
- **The mode flip is the commit point** in `store_api_key`: last when
  selecting BYOK, first when clearing. Putting the legacy write after the mode
  write in the set direction would turn a failed legacy write into "BYOK live
  but the route answered 500".
- **Rollback is safe for one release.** A pre-1a binary reads only
  `composio/token` / `composio/api_key`. Every save on this build writes the
  same value there, so after a rollback the managed token and the BYOK key keep
  working with no action (part 1 §6 row 12). The one exception is a save whose
  legacy write failed (§6.1, part 1 §6 row 10): the route answered 500 and the
  legacy slot still holds the previous value until a retry.
- **Re-upgrading after changes made on a rolled-back binary.** The old binary
  writes only the legacy address, but the new address still holds the value
  this build saved, and it wins on read. A key **rotated** on the old binary
  therefore reads as the older key after re-upgrading, and a key **cleared**
  on the old binary **comes back**. After re-upgrading, re-save or re-clear the
  Composio keys. Put this in the PR description.
- **Follow-up, not in scope.** A later release changes `write_both` so the
  legacy address is cleared (`""`) rather than mirrored. A release after that
  removes the legacy read, `read_with_legacy`'s fallback and the `LEGACY_*`
  consts. Record it as a follow-up in the PR description; do not do it here.
- **A hand-edited `composio/mode = byok`** after a failed BYOK clear (§6.1, last
  row) can resurface the legacy key. Only `store_api_key` writes the mode, and
  it writes the key first, so this needs a hand edit.
- **An error response after a partial write** skips `evict_catalog_cache` and
  the journal (`ops/composio.rs:658-675`, `:744-756`), exactly as today. Do not
  reorder the handlers to "fix" that here.
- **`FsSecretStore` has its own, unrelated "legacy"** (slugged filenames,
  `src/store/fs.rs:2921-2960`). It is not this slice's legacy address; do not
  touch it.
- **Harness re-export rename breaks a doc link, not the build**
  (`harness/built_in/mod.rs:538`). CI runs no `cargo doc`, so nothing flags it;
  fix it anyway (part 1 §5 step 8).
- **Feature gates.** The whole `harness` module — so the tests in
  `src/harness/built_in/composio.rs` and `planning/test.rs` — compiles only
  with `openhuman` (`src/lib.rs:41-42`). `probe_transport` in
  `ops/composio.rs` differs under `composio`. The ops test
  `the_api_key_test_route_reads_a_legacy_byok_key` uses `probe_override`, so it
  is independent of the `composio` feature.
- **Never log a value.** The only new log line names `key` and `legacy_key`
  constants and the store error; do not add `?value`, `%token` or a length.
