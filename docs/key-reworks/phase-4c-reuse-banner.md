# Phase 4c — reuse the account key after a TinyHumans key is removed

- **Goal:** when the TinyHumans LLM key or Composio key is gone but the account
  key exists, an admin sees one banner offering to copy the account key back —
  on the host — and removing that key while TinyHumans is the default (or an
  agent's provider) warns and keeps the default.
- **Dump item:** 12. **Decision:** Q8. Depends on 4a (`slot_guard`, fan-out
  internals), 2a (the `tinyhumans` row), 2b (`store::load_default`), 2c
  (`store::check_model_id`, `ModelField` list mode, `defaultChoice`), 3a (agent
  pairs), 1a (Composio key names).
- **Code read at:** `upstream/main @ fcfb3e1bc` (2026-09-14). Re-verify lines.
- **Not handled here:** item 10. Until it lands, the legacy chain still resolves
  `tinyhumans/key` for a company with **no** default and for Composio
  (`src/company/composio.rs:98-115`), so removing a copy changes nothing for
  those companies and the banner would be noise there. Its value is for
  companies whose default or an agent pair names `tinyhumans`: those fail
  closed (F6) once the row has no key. The banner conditions below are scoped
  to exactly that.

## 1. Files

| File | What changes |
|---|---|
| `src/company/company_key/fan_out.rs` (4a) | `copy_account_key_to_inference`, `copy_account_key_to_composio` |
| `src/server/ops/inference.rs` | `InferenceStatusDto` :233-360 gains `account_key_available`, `tinyhumans_referenced`; `effective_status_with` :840 fills them |
| `src/server/ops/inference/providers.rs` | new route + handler `copy_account_key` (router :69-120) |
| `src/server/ops/composio.rs` | new route + handler `copy_account_key` (router :279-295) |
| `tests/auth_matrix.rs` | two `r!` rows |
| `tests/snapshots/auth-matrix.txt` | 28 lines (§4 step 7) |
| `frontend/src/api/inference.ts` | `InferenceStatus` :66 fields; `copyAccountKeyToInference` |
| `frontend/src/api/composio.ts` | `copyAccountKeyToComposio` |
| `frontend/src/inference/reuse-banner.ts` (new) | pure visibility + dismissal |
| `frontend/src/inference/ReuseAccountKeyBanner.tsx` (new) | banner component (both pages) |
| `frontend/src/inference/ProvidersTab.tsx` | render banner; model-step dialog |
| `frontend/src/views/connections/ComposioSection.tsx` | render banner |
| `frontend/src/inference/routing.ts` | `RemovalImpact` :514, `removalImpact` :536, `removalWarnings` :569 |
| tests | §6 |

## 2. Current code

- LLM page banners already use this shape (`ProvidersTab.tsx:368-383`):
  `<p className="text-xs text-muted-foreground" data-testid="inference-managed-fallback">`.
- Remove-key confirmation copy for a default (`routing.ts:594-598`):
  `It is this company's default, so every unrouted workload goes through it — and would start failing.`
- Remove key on a row is `PUT …/inference/providers/{slug} {key:""}`; `edit_provider`
  clears the key and health and never touches the default
  (`providers.rs:1011-1070`). The managed row's Remove key is
  `PUT …/inference/managed/key {key:""}` (`providers.rs:1697-1777`), also
  default-neutral. **So Q8's "keep the default" is already true on the host;
  4c adds no host change to removal.**
- A literal segment next to a `{slug}` capture is avoided on purpose
  (`providers.rs:83-86`: "a routing ambiguity waiting to be resolved the wrong
  way"). So the route is **not** `/inference/providers/tinyhumans/key/from-account`.
- `ComposioStatusDto` must not gain a boolean about a secret slot (issue #886,
  `composio.rs:320-336`: "Do not reintroduce one under any name"). The Composio
  banner therefore reads `GET …/credential`'s existing `configured` instead of a
  new Composio field.
- Existing per-viewer storage pattern with try/catch: `frontend/src/lib/last-channel.ts:28-40`.

## 3. Target code

### 3.1 Host functions (`fan_out.rs`)

```rust
/// Copies `tinyhumans/key` into the LLM TinyHumans slot and, with a model,
/// adds the row and fills an unset default. Under `slot_guard`. Reuses fan_out
/// steps 3.2, 3.5 (with old_account = new = account key), 3.7-3.11.
pub async fn copy_account_key_to_inference(company: &CompanyId, secrets: &dyn SecretStore,
    model: Option<&str>, prober: &dyn InferenceProber) -> Result<FanOutReport>;

/// Copies `tinyhumans/key` into `composio/tinyhumans/key` (1a write, legacy cleared).
/// Under `slot_guard`. Never reads or writes composio/mode or composio/byok/key.
pub async fn copy_account_key_to_composio(company: &CompanyId, secrets: &dyn SecretStore)
    -> Result<FanOutReport>;
```

Refusals, all `OpenCompanyError::InvalidRequest` (400) and all before any write:

| Case | Message |
|---|---|
| `tinyhumans/key` empty | `There is no account key to reuse. Add one on the Account page.` |
| target slot holds a different non-empty key | `TinyHumans already has its own key here. Remove it first to reuse the account key.` |
| inference: entry zero is managed | `This company's TinyHumans setup predates the provider list; replace its key from its row.` |
| model with invalid id | 2c's `store::check_model_id` message |

A target slot already equal to the account key is not a refusal: it answers
`kept`/`alreadyCurrent` and still proceeds to row/default (so Yes after a
`needsModel` answer completes the setup).

### 3.2 Routes and DTOs

| Method, path (both scope forms) | Handler | Extractor | Body | Response |
|---|---|---|---|---|
| `POST …/inference/tinyhumans/key/from-account` | `providers::copy_account_key` | `AdminScopedCompany` | `{ "model"?: string }` | `ProviderMutation` + `slots`, `needsModel`, `setsDefault`, `models` |
| `POST …/composio/tinyhumans/key/from-account` | `composio::copy_account_key` | `AdminScopedCompany` | none | `MutationResponse` (existing Composio shape) + `slots` |

```rust
#[derive(Debug, Default, Deserialize)] #[serde(rename_all = "camelCase")]
struct CopyAccountKey { #[serde(default)] model: Option<String> }

// providers.rs: extend ProviderMutation (additive, all skip when empty/false)
#[serde(skip_serializing_if = "Vec::is_empty")] slots: Vec<SlotReportDto>,
#[serde(skip_serializing_if = "std::ops::Not::not")] needs_model: bool,
#[serde(skip_serializing_if = "std::ops::Not::not")] sets_default: bool,
#[serde(skip_serializing_if = "Vec::is_empty")] models: Vec<String>,
```

Move `SlotReportDto` from `ops/company_key.rs` to `src/server/ops/slot_report.rs`
(`pub(crate)`) so all three routes serialise one shape.

Response example, Yes with no default:

```json
{ "status": { "…": "…" }, "note": "LLM now uses your account key. Choose a model to finish setting up TinyHumans for LLM.",
  "slots": [ {"slot":"inference","outcome":"filled"}, {"slot":"provider","outcome":"skipped","detail":"needsModel"},
             {"slot":"default","outcome":"skipped","detail":"needsModel"}, {"slot":"health","outcome":"ok"} ],
  "needsModel": true, "setsDefault": true, "models": ["acme/test-model"] }
```

Notes: inference `Filled` → `LLM now uses your account key.`; composio `Filled`
→ `Composio now uses your account key. A key you created by hand may lack the connections permission Composio needs.`;
then 4a's §3.4 sentences for provider/default/health/needsModel.

Journal: inference route `company_key_inference_filled` (+ `company_key_provider_filled`,
`company_key_default_filled`, `company_key_inference_rolled_back` as 4a);
Composio route `company_key_composio_filled`. No `credential_set` (that word
means a pasted Composio token).

### 3.3 Status DTO fields (`InferenceStatusDto`, after `managed`)

```rust
/// A non-empty `tinyhumans/key` exists. Not a secret: `GET …/credential`
/// already tells members `configured`. Only admins act on it.
account_key_available: bool,
/// `inference/default` names `tinyhumans` (ProviderOnly or Full), or any agent
/// pair (3a) names `tinyhumans`.
tinyhumans_referenced: bool,
```

Fill in `effective_status_with` with `company_key::key_configured` and 2b's
`store::load_default` plus 3a's agent-pair reader (whatever 3a names it). A read
error propagates like every other field there.

### 3.4 TS

```ts
// api/inference.ts — InferenceStatus gains (optional for older hosts)
accountKeyAvailable?: boolean;
tinyhumansReferenced?: boolean;
// ProviderMutation gains slots?, needsModel?, setsDefault?, models? (types from api/credential.ts)
export function copyAccountKeyToInference(client: OpenCompanyClient, company: string | null, model?: string): Promise<ProviderMutation> {
  return client.post<ProviderMutation>(`${client.scopeFor(company)}/inference/tinyhumans/key/from-account`, model ? { model } : {});
}
// api/composio.ts
export function copyAccountKeyToComposio(client: OpenCompanyClient, company: string | null): Promise<ComposioMutation> {
  return client.post<ComposioMutation>(`${client.scopeFor(company)}/composio/tinyhumans/key/from-account`, {});
}
```

(`ComposioMutation` = the existing mutation type in `api/composio.ts`; use its real name.)

```ts
// inference/reuse-banner.ts
export const TINYHUMANS_SLUG = "tinyhumans";
export function showsInferenceReuseBanner(a: {
  canManage: boolean; status: InferenceStatus | null; providers: readonly Provider[]; dismissed: boolean;
}): boolean;
export function showsComposioReuseBanner(a: {
  canManage: boolean; accountConfigured: boolean; mode: ComposioMode | undefined;
  managedCredentialSource: string | undefined; dismissed: boolean;
}): boolean;
export function reuseDismissKey(page: "inference" | "composio", company: string | null): string;
export function readDismissed(key: string): boolean;   // try/catch → false
export function writeDismissed(key: string): void;     // try/catch → no-op
```

- `showsInferenceReuseBanner` = `canManage && status?.accountKeyAvailable === true && !dismissed && !usable && (status.tinyhumansReferenced === true || rowWithoutKey)`, where `usable = providers.some(p => p.slug === TINYHUMANS_SLUG && p.enabled && p.keyConfigured)` and `rowWithoutKey = providers.some(p => p.slug === TINYHUMANS_SLUG && !p.keyConfigured)`.
- `showsComposioReuseBanner` = `canManage && accountConfigured && mode === "managed" && managedCredentialSource !== "static" && !dismissed`. (`static` is the stored-token tier today; if 1a renames it, use 1a's name.)
- `reuseDismissKey` = `` `oc.reuse-account-key.${page}.${company ?? "_"}` ``.

## 4. Ordered edits

1. Host functions in `fan_out.rs` (§3.1), each taking `slot_guard` first.
2. `src/server/ops/slot_report.rs`; switch 4a's `company_key.rs` to it.
3. Inference route: add `.merge(scoped("/inference/tinyhumans/key/from-account", post(copy_account_key)))` to `providers::router`; handler parses `Option<Json<CopyAccountKey>>`, calls `copy_account_key_to_inference(…, &prober_for(runtime))` (4a's override seam, moved to a shared `pub(crate)` spot), evicts `inference_models::evict_company_catalogs`, journals, answers `ProviderMutation { status: effective_status(…), note, probe: None, affected_tiers: vec![], slots, needs_model, sets_default, models }`.
4. Composio route: `.merge(scoped("/composio/tinyhumans/key/from-account", post(copy_account_key)))`; handler calls `copy_account_key_to_composio`, `evict_catalog_cache(runtime)`, journals `company_key_composio_filled` via the module's `journal(&company, …, None)`, answers the existing `MutationResponse { status, note, advisory: None, probe_class: None }` plus `slots`.
5. Status fields (§3.3).
6. `tests/auth_matrix.rs`: after `r!(Put, "/inference/managed/key", Admin, Credential, "")` (:597) add `r!(Post, "/inference/tinyhumans/key/from-account", Admin, Credential, ""),`; after `r!(Post, "/composio/api-key/test", Admin, Credential, "")` (:452) add `r!(Post, "/composio/tinyhumans/key/from-account", Admin, Credential, ""),`. Classification: **Admin** (it writes a credential slot and can move billing), **Credential** blast (same as `PUT /inference/managed/key` and `PUT /composio/token`).
7. Snapshot: run `BLESS_AUTH_MATRIX=1 scripts/ci/assert-auth-matrix.sh` (the bless switch is `tests/auth_matrix.rs:1823`) on CI or locally, then commit. Expect exactly 28 new lines — for each of the two paths, both scope forms (`/api/v1/companies/{id}/…` and `/api/v1/company/…`), seven principals each:
   ```
   POST /api/v1/company/inference/tinyhumans/key/from-account admin permitted source=ops access=admin features=openhuman blast=credential note="" wait=-
   POST /api/v1/company/inference/tinyhumans/key/from-account anonymous refused:401:unauthorized source=ops access=admin features=openhuman blast=credential note="" wait=-
   POST /api/v1/company/inference/tinyhumans/key/from-account member refused:403:forbidden source=ops access=admin features=openhuman blast=credential note="" wait=-
   POST /api/v1/company/inference/tinyhumans/key/from-account must-change-password-admin refused:403:password_change_required source=ops access=admin features=openhuman blast=credential note="" wait=-
   POST /api/v1/company/inference/tinyhumans/key/from-account platform permitted source=ops access=admin features=openhuman blast=credential note="" wait=-
   POST /api/v1/company/inference/tinyhumans/key/from-account tenant-non-owner refused:403:forbidden source=ops access=admin features=openhuman blast=credential note="" wait=-
   POST /api/v1/company/inference/tinyhumans/key/from-account tenant-owner permitted source=ops access=admin features=openhuman blast=credential note="" wait=-
   ```
   and the same seven for `/api/v1/companies/{id}/inference/…`, and fourteen for `composio/tinyhumans/key/from-account` (the existing Composio rows show the same `features=openhuman` column, `auth-matrix.txt:2339,3046`). If the blessed diff shows anything other than these 28 lines, stop and report.
8. Client functions and types (§3.4).
9. `reuse-banner.ts` (§3.4).
10. `ReuseAccountKeyBanner.tsx`:
    ```ts
    export function ReuseAccountKeyBanner({ testId, text, busy, onYes, onNotNow }: {
      testId: string; text: string; busy: boolean; onYes: () => void; onNotNow: () => void; })
    ```
    A bordered `div role="status"` with `data-testid={testId}`, the text, `Button` `Yes` (`data-testid={`${testId}-yes`}`, disabled when `busy`) and `Button variant="ghost"` `Not now` (`data-testid={`${testId}-not-now`}`). Tokens only.
11. `ProvidersTab.tsx`: above the providers `Card`, render the banner when `showsInferenceReuseBanner(…)`. `dismissed` is state seeded from `readDismissed(reuseDismissKey("inference", company))`. Yes → `copyAccountKeyToInference(client, company)`; on `needsModel` open a model dialog (a `Dialog` holding 2c's `ModelField` in list mode — `models`, `allowBlank={false}` — exactly as 4b step 7, title `modelStepTitle(setsDefault)` from `account-fill.ts`, `data-testid="inference-reuse-model-step"`, save `data-testid="inference-reuse-model-save"` → `copyAccountKeyToInference(client, company, model)`); else toast `note` and refresh. Not now → `writeDismissed` + hide.
12. `ComposioSection.tsx`: inside the ready branch, before `GrantNamespace`, render the banner when `showsComposioReuseBanner(…)`. `accountConfigured` comes from one `getCompanyCredential(client, company)` read in the section's `refresh` (ignore its errors → `false`). Yes → `copyAccountKeyToComposio` → toast `note`, `refresh()`.
13. `routing.ts`: `RemovalImpact` gains `tinyhumansDefault: boolean` and `tinyhumansAgentPair: boolean`; `removalImpact(provider, providers, routing, categoryOf, referenced?: { isDefault: boolean; agentPair: boolean })` sets both only when `provider.slug === "tinyhumans"`. In `removalWarnings`, for `intent === "key"`: if `impact.tinyhumansDefault`, push the Q8 default line **instead of** the generic `It is this company's default…` line; else if `impact.tinyhumansAgentPair`, push the agent line. `intent === "provider"` and `"disable"` are unchanged.
14. `ProvidersTab.tsx`: pass `referenced` for both the row dialog (:420-450) and the managed-row dialog (:456-470, whose provider is `tinyhumans` by `MANAGED_SLUG`): `isDefault` from the status's default provider (2c's `defaultChoice?.provider === "tinyhumans"`), `agentPair` = `status.tinyhumansReferenced && !isDefault`.

## 5. Data carry-over

| Before | Action | After |
|---|---|---|
| `tinyhumans/key=th-not-a-real-key`, `provider/tinyhumans/key=""`, `row(m)`, default `{tinyhumans,m}` | inference Yes | `provider/tinyhumans/key=th-not-a-real-key`; row, default unchanged; health recorded |
| same, no row, default unset | inference Yes, then model `m` | key copied; then `row(m)` and default `{tinyhumans,m}` |
| same, prober `auth` | inference Yes | key restored to `""` (rolledBack); health `auth`; banner stays |
| `provider/tinyhumans/key=th-not-a-real-key-custom` | inference Yes | 400, nothing written |
| `tinyhumans/key=""` | either Yes | 400, nothing written |
| `composio/tinyhumans/key=""`, `composio/mode=byok` | Composio Yes | not offered (banner requires managed); a direct POST fills the slot, mode unchanged |
| `row(m)` default `{tinyhumans,m}`, key present | Remove key | key `""`, default **unchanged** (Q8), banner shows |

No boot-time migration; nothing clears or overwrites a non-empty slot.

## 6. Tests

**Host — `src/server/ops/inference.rs` `mod tests` (:1390)**, prober override set:
- `copying_the_account_key_fills_the_llm_slot_and_asks_for_a_model` — account key set, no row → POST → 200, `needsModel`, `provider/tinyhumans/key` equals the account key (raw store read in the test).
- `copying_with_a_model_adds_the_row_and_default` — POST `{model:"acme/test-model"}` → `GET …/inference` has `tinyhumans` row, default `{tinyhumans, acme/test-model}`.
- `copying_is_refused_without_an_account_key` / `…_over_a_custom_key` / `…_over_a_legacy_managed_config` — 400, store unchanged.
- `copying_rolls_back_on_auth` — override `Err(Auth)` → `inference` `rolledBack`, key `""`.
- `a_member_cannot_copy_the_account_key` — member cookie → 403, store unchanged.
- `status_reports_account_key_available_and_tinyhumans_referenced` — four cases: none; account key only; default `{tinyhumans,m}`; agent pair `tinyhumans` (3a store).
- `removing_the_tinyhumans_key_keeps_the_default` — row + default → `PUT …/inference/providers/tinyhumans {key:""}` → default unchanged. (Pins Q8 on the host.)
- `no_copy_response_contains_a_key` — raw bodies lack the fake key.

**Host — `src/server/ops/composio.rs` `mod tests` (:1484):** `copying_the_account_key_fills_the_composio_tinyhumans_key`; `…_clears_the_legacy_token` (legacy `composio/token` custom → refused; legacy equal to account → converged); `…_never_touches_mode_or_byok`; `a_member_cannot_copy_…`.

**Auth matrix:** `scripts/ci/assert-auth-matrix.sh` green with the 28 blessed lines.

**Unit — `frontend/test/unit/reuse-banner.test.ts`:** `showsInferenceReuseBanner` table — non-admin false; no account key false; usable row false; row without key true; referenced with no row true; not referenced and no row false (the pre-item-10 noise case); dismissed false. `showsComposioReuseBanner` table — byok false; `static` false; `company` true; `none` true; non-admin false. `readDismissed` returns false when `localStorage.getItem` throws (stub `Object.defineProperty(window, "localStorage", { get() { throw new Error("blocked"); } })`); `writeDismissed` does not throw then.

**Unit — `frontend/test/unit/removal-warnings-tinyhumans.test.ts`:** `removalWarnings("key", "TinyHumans", impact{tinyhumansDefault:true}, …)` contains the Q8 default line and not `every unrouted workload`; agent-pair line; a non-`tinyhumans` default still gets the generic line; `intent "provider"` unchanged.

**E2E — `frontend/test/e2e/inference-reuse-account-key.spec.ts`** with `page.route` stubs for `GET **/inference` (row `tinyhumans` with `keyConfigured:false`, `accountKeyAvailable:true`, `tinyhumansReferenced:true`) and `POST **/inference/tinyhumans/key/from-account` (first `needsModel`, then success): banner visible → Yes → model step → Save → banner gone after the refreshed status stub; Not now hides it and it stays hidden after reload. A second test on the Composio page with its banner.

All three typecheck gates, `assert-design-tokens.sh`.

## 7. Console / UI

| Element | `data-testid` | Copy |
|---|---|---|
| LLM banner | `inference-reuse-account-key-banner` | `Your TinyHumans account is connected. Use the same key for LLM?` |
| Composio banner | `composio-reuse-account-key-banner` | `Your TinyHumans account is connected. Use the same key for Composio?` |
| buttons | `…-banner-yes`, `…-banner-not-now` | `Yes`, `Not now` |
| model step | `inference-reuse-model-step`, `inference-reuse-model-save` | title from `modelStepTitle`; `Save model` |
| Q8 default line | inside `inference-remove-impact` | `TinyHumans is your default. New work will fail until you choose another default or add a key again.` |
| Q8 agent line | inside `inference-remove-impact` | `An agent uses TinyHumans. Its work will fail until you choose another model for it or add a key again.` |

Browser verification: both banners, the model step, and the Remove-key dialog
with the Q8 line, light and dark, with a real `tinyhumans` row list at its
realistic size (several providers), screenshots `4c-<state>-<theme>.png` in your
session scratch directory.

## 8. Must not touch

`clear_default_if_marked` and `delete_provider`'s default handling (Remove
**provider** keeps 2b/2c's rule for every slug — no TinyHumans special case);
`ComposioStatusDto` (no new slot boolean, #886); `composio/mode`,
`composio/byok/key`; `company_key::resolve`; `inference/managed/enabled`;
`TINYHUMANS_TOKEN_FILE` (6a's job, after this slice, not this one's); the
Account page. Never send `tinyhumans/key` to the browser to prefill anything.

## 9. Done when

- All §6 tests pass on CI by head SHA, zero failures and zero pending.
- The auth-matrix snapshot diff is exactly the 28 lines of §4 step 7.
- Screenshots for every §7 state, light and dark.
- `git grep -n "from-account" frontend/src` shows only the two client functions.

## 10. Gotchas

- The route path deliberately differs from the rundown's `…/inference/managed/key/from-account` and the brief's `…/inference/providers/tinyhumans/key/from-account`: `managed/` names the legacy chain, and `providers/tinyhumans/…` sits next to the `{slug}` capture. `/inference/tinyhumans/…` has no capture beside it.
- A banner Yes is a single-slot copy. Never wire it to `PUT …/credential` (that re-runs the whole fan-out and rewrites the account key).
- Members must never see the banner; the host refuses them anyway.
- `localStorage` can throw (private mode, blocked storage); the banner must render correctly with dismissal unavailable.
- Before item 10, a company with no default and no reference already resolves the account key; the "not referenced and no row" false case is what keeps the banner off those pages.
- The managed row's Remove key and the `tinyhumans` row's Remove key clear the same `provider/tinyhumans/key`; both dialogs need the Q8 line.
