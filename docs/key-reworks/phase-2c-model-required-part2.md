# Phase 2c — a model is required (part 2: console, tests, done-when)

Part 2 of 2 covers §7 tests, §8 console, §9 must not touch, §10 done when and §12 console gotchas. [Part 1](phase-2c-model-required.md) has the goal, backend, JSON shapes and carry-over (§1–§6), the TinyHumans flow (§4.9) and backend gotchas (§11).

Written 2026-09-14. Code read at `upstream/main @ fcfb3e1bc`. This slice runs after slices 2a and 2b, so re-check every line on your branch. Frontend paths are relative to `frontend/`.

---

## 7. Tests

### 7.1 Rust tests to add

**`src/company/inference/store.rs`, `mod tests` (`:1025`)**: `check_model_id`.

| Test | Asserts |
|---|---|
| `a_model_id_is_trimmed` | `"  acme/test-model \n"` → `Ok("acme/test-model")` |
| `an_empty_model_id_is_refused` | `""`, `"   "` → `Err` containing `Choose a model` |
| `a_model_id_with_a_control_character_is_refused` | `"test\u{0007}model"` → `Err` containing `control characters` |
| `a_model_id_with_inner_whitespace_is_refused` | `"test model"` → `Err` containing `spaces` |
| `a_model_id_is_bounded_in_chars_not_bytes` | `"é".repeat(256)` → `Ok`; `"é".repeat(257)` → `Err` containing `256` |
| `every_tier_name_is_refused_as_a_model_id` | each `INFERENCE_TIERS` entry and `" chat-v1 "` → `Err` containing `workload name` |
| `model_ids_of_every_shape_pass` | `acme/test-model`, `acme/test-model:free`, `test-model:8b`, `test-model`, `test.deployment-1` → `Ok` |

**`providers.rs`, `mod tests` (`:2110`):** `a_tinyhumans_add_is_probed_so_its_health_is_recorded` (pure). It asserts `plan_add("tinyhumans", None, None, true).unwrap().probes` and `catalogue::auth_style_for("tinyhumans") == AuthStyle::Bearer`. Together these make `worth_probing` true whenever a key is sent (part 1 §4.9).

**`src/server/ops/inference.rs`, `mod tests` (`:1390`).** Helpers: `home()` `:1407`, `state_with_company` `:1476`, `send` `:1812`, `UNREACHABLE`, `default_slug` `:3676`. "Add X" means `POST …/inference/providers {"kind":"custom","label":X,"baseUrl":UNREACHABLE,"key":"sk-not-a-real-key","model":…}`. The row is kept with amber health, so no mock is needed.

| Test | Setup → call → assert |
|---|---|
| `setting_a_default_requires_a_model` | add `Acme` `acme-1` → `POST …/providers/acme/default` body `{}` → **400**, raw contains `Choose a model`; status `defaultChoice` is `null` |
| `a_default_model_may_not_be_a_tier_name_or_contain_spaces` | add `Acme` → `{"model":"chat-v1"}`, `{"model":"test model"}`, `{"model":"ab"}`, `{"model":"x"×257}` each → **400** and `defaultChoice` stays `null`; `{"model":"x"×256}` → **200** |
| `setting_a_default_writes_one_json_value_and_the_rows_model` | add `Acme` `acme-old` → `POST …/acme/default {"model":"acme-new"}` → **200**, note contains `Acme · acme-new`; `defaultChoice == {"provider":"acme","model":"acme-new"}`; `providers[acme].model == "acme-new"`, all four `models` values `acme-new`. If an existing test in the module reaches the company `SecretStore`, also assert the raw `inference/default` equals `{"provider":"acme","model":"acme-new"}` |
| `an_add_without_a_model_is_refused_before_anything_is_written` | add `Acme` **without** `model` → **400** containing `Choose a model`, raw lacks `sk-not-a-real-key`; no `acme` row; the same add **with** a model → **200** |
| `an_add_with_make_default_writes_the_default_last` | add `Acme` `acme-1` + `makeDefault:true` → `defaultChoice == {acme, acme-1}`; add `Beta` `beta-1` (no flag) → unchanged; add custom `Groq` (reserved slug) + `makeDefault:true` → **400**, `defaultChoice` unchanged |
| `editing_the_default_rows_model_moves_the_default_with_it` | add `Acme` `acme-1` + `makeDefault`, add `Beta` `beta-1` → `PUT …/acme {"model":"acme-2"}` → default and row model `acme-2`; `PUT …/beta {"model":"beta-2"}` → default unchanged; `PUT …/acme {"model":"chat-v1"}` → **400**, nothing moves |
| `an_add_records_health_from_its_probe` | loopback axum server on `127.0.0.1:0` (pattern: the `axum::serve(listener, app)` test in `src/server/inference_models.rs:790-800`; `probe::default_policy()` allows loopback, `probe.rs:673-677`) serving `GET /v1/models` → `{"data":[{"id":"acme/test-model"}]}` → add custom `Mock`, `baseUrl` `http://<addr>/v1`, model `acme/test-model` → **200**, `probe.ok == true`, `probe.models` contains the id, `providers[mock].health.state == "ok"` |
| `status_reports_the_default_choice_and_each_rows_model` | fresh → `defaultChoice: null`; add `Acme` `acme-1` → `model: "acme-1"`, `modelAmbiguous: false` |
| `the_status_maps_a_bare_slug_default_to_a_null_model` | pure: `default_choice_dto(ProviderOnly("acme"))` → `{acme, None}`; `Unset` → `None`; `Full` → both set |
| `an_ambiguous_row_reports_no_model_and_the_flag` | pure: `model_on_row_dto(Ambiguous(["a","b"]))` → `(None, true)`; `None` → `(None, false)`; `One("a")` → `(Some("a"), false)` |

### 7.2 Rust tests to update or remove

**Give every add fixture in `ops/inference.rs` a model.** On `fcfb3e1bc` these tests
post to `…/inference/providers` (line of the test, then line of the POST):

`an_endpoint_carrying_a_credential_is_refused_before_anything_is_written` (2225/2242), `a_provider_name_past_the_bound_is_refused_rather_than_breaking_the_store` (2387/2400, 2436), `key_never_leaks_across_any_response` (3003/3058), `a_cloud_provider_takes_its_endpoint_from_the_catalogue` (3118/3126), `a_rename_that_posts_back_the_served_endpoint_keeps_the_stored_one` (3141/3154), `a_second_provider_holds_its_own_credential` (3206/3221), `an_unreachable_endpoint_keeps_the_key_and_creates_the_row` (3240/3251), `a_slug_that_shadows_a_builtin_is_refused_before_anything_is_written` (3274/3282), `a_custom_provider_with_no_name_has_no_slug_to_write_under` (3297/3305), `deleting_a_provider_clears_its_credential_and_scrubs_its_routes` (3313/3321, 3359), `disabling_keeps_the_route_and_names_the_tiers_it_parks` (3377/3390), `the_default_is_explicit_and_survives_a_delete_that_is_not_it` (3532/3541), `disabling_or_deleting_the_default_never_leaves_it_marked` (3580/3589), `a_provider_that_is_switched_off_cannot_be_made_the_default` (3646/3653), `the_routing_mode_is_inferred_from_the_routes` (3728/3763), `the_only_provider_a_company_can_use_is_routed_to` (3795/3809), `a_provider_added_beside_managed_is_not_routed_to` (3828/3848).

- If 2a added a test that posts a `tinyhumans` add, give it `"model":"acme/test-model"`. An `already connected` refusal still wins, because the collision check runs first. Confirm nothing was missed with `grep -n '"/api/v1/company/inference/providers"' src/server/ops/inference.rs`.
- Every `…/default` POST that sends `None` in the tests at `:3532`, `:3580` and `:3646` sends `Some(json!({"model":"acme-model"}))` instead. The switched-off test also asserts that raw contains `switched off`.

**`providers.rs` tests:** remove `a_direct_vendor_catalog_needs_a_model_named` (`:2262`), `a_local_runtime_catalog_needs_a_model_named` (`:2274`), `a_tier_native_catalog_needs_nothing_named` (`:2285`) and `a_catalog_of_shipped_ids_needs_nothing_named` (`:2297`); all four call the deleted `needs_an_explicit_model`. Keep `fn ids` (`:2252`), which `:2423` still uses. Change `a_named_model_covers_every_tier` (`:2308`) to call `tier_overrides("acme/test-model")` and drop its `tier_overrides(None)` assert.

**Unchanged:** the `src/company/inference.rs` tests that call `store::set_default_slug` (`:3344`, `:3534`, `:4290`), which slice 2b owns, and `tests/auth_matrix.rs`.

### 7.3 Frontend unit tests (vitest, node environment)

**`test/unit/inference-connect.test.ts`**: add `describe("the model step")`.

- `checkModelId` returns `null` for the §7.1 accepted ids and for `"é".repeat(256)`, and `"empty"`, `"control"`, `"whitespace"`, `"tooLong"` (`"é".repeat(257)`) or `"tier"` for the refusals. `modelIdErrorCopy` returns the §8.2 strings.
- `makeDefaultInitially`: `{defaultChoice: null}` → `true`; `{defaultChoice: {provider:"a", model:null}}` → `false`; `null` → `false`.
- **`probeEndpoint("tinyhumans")`** → `"https://api.tinyhumans.ai/agent-integrations/openrouter"`
  (slice 2a).
- **`modelAskFromProbe`**:
  - TinyHumans url with `{ok:true, models: <a mock catalogue of 500 fake ids>}` → the same 500 ids, in order,
    `freeTextOnly: false`, no `error`. This case is the one proving **the TinyHumans
    model step always has a list**.
  - `{ok:false, message}` → `models: []`, and `error` starts with
    `Could not read this provider's models`.
  - `url === null` → `{models: [], freeTextOnly: false}`.
  - an `https://x.openai.azure.com/…` url → `freeTextOnly: true`.
- **Fixture:** add `model: null, modelAmbiguous: false` to `provider()` (`:30-41`).

**`test/unit/inference-model-field.test.ts`**, at the real cap.

Fixture:
`cap = Array.from({length: 500}, (_, i) => \`vendor/model-${String(i).padStart(3, "0")}\`)`.

- `showsCatalogSelect({catalog: {models: cap, freeTextOnly: false}, typed: false, value})` is `true` both for `value: ""` and for `value: "vendor/model-499"`.
- `filterModels(cap, "")` gives `shown.length === 50` and `total === 500`, and `cappedNote` → `"Showing the 50 closest of 500. Keep typing to narrow it."`.
- `modelRows(cap, "", {allowBlank: false})` → 50 rows, none with `value === ""`. With `allowBlank: true`, row 0 is `""`.

**New `test/unit/inference-default-model.test.ts`** (§8.6 helpers):

- `defaultNeedsModel` is true only for `{provider:"a", model:null}`.
- `isFullDefault` is true only when the slug matches **and** the model is set.
- `defaultBadgeLabel` → `"Default · acme/test-model"` for a full default on the row,
  else `"Default"`.
- `rowNeedsModel` is true for `modelAmbiguous`, or for the row named by a bare default.
- `defaultModelPrefill` → the row's `model`, else the choice's model when the choice
  names the row, else `""`.
- A resolved-but-unmarked default still gets "Set as default":
  `providerMenu({...row, isDefault: isFullDefault(row, null)})` includes `"default"`.

### 7.4 E2E: `test/e2e/inference.spec.ts`

Add this helper after `addCustom` (`:86-91`):

```ts
/** Step 2 of the connect dialog: type a model, set "make default", submit. */
async function pickModel(page: Page, model: string, makeDefault = false) {
  await expect(page.getByTestId("inference-connect-model-step")).toBeVisible({ timeout: 30_000 });
  const box = page.getByTestId("inference-make-default");
  if ((await box.isChecked()) !== makeDefault) await box.click();
  await page.locator("#inference-connect-model").fill(model);
  await page.getByTestId("inference-connect-submit").click();
}
```

Against `UNREACHABLE` the draft probe fails, so the field is a text input. Update each
existing add flow to call the helper after its first submit:

| Test (line on `fcfb3e1bc`) | Change |
|---|---|
| `a provider behind an unreachable endpoint…` (`:127`) | after `:140`: `pickModel(page, "e2e-model")` |
| `a second provider holds a credential of its own` (`:159`) | inside the loop, after `:170` |
| `the add dialog stops offering a provider once it is connected` (`:180`) | after `:186`: `pickModel(page, "test-model")`. **Superseded by round-2 review, decision P1-8, KR-L1-03** — see below; the row that follows described a since-changed behaviour. |
| `disabling a provider keeps its credential` (`:226`) | after `:236` |
| `deleting a provider removes its row…` (`:253`) | after `:262` |
| `:96` (Managed row), `:206` (reserved name) | unchanged |

Leave `makeDefault = false` in every existing flow. The spec shares one company, and a
full default pointing at `UNREACHABLE` would move every later test off the legacy path.

> **Superseded (round-2 review, decision P1-8; found live, KR-L1-03, build
> `f9fe35988`).** The row above still describes the original plan: a probe
> rejected on the `auth` class (a bad key) was supposed to open the model
> step in free text with "Could not read this provider's models: … rejected
> the credential. Type a model id.", offering **Add anyway** there. That is
> no longer what happens, deliberately: a rejected key now **stays on the
> details step** with the host's own refusal message (X9 wording) and never
> reaches the model step at all — `ProvidersTab.tsx`'s `submitConnect`, the
> comment at the `probe.class === "auth"` branch. There is no "Add anyway"
> for a credential that was actually rejected; that escape hatch is reserved
> for a probe that failed for some other reason (`endpoint`, `timeout`,
> `unknown` — the network could not be reached to say either way), where the
> operator may still know the endpoint is fine.
>
> Note the distinction this doc's original wording collapsed: the **probe**
> (step 1's own check) and the **add** (step 2's write) are two different
> requests that can reject a credential separately. Only the probe's `auth`
> class is affected by this change — `test/e2e/inference.spec.ts`'s "the add
> dialog stops offering a provider once it is connected" (Groq) is a
> **final-add** rejection reached after a probe that succeeded (a real
> vendor's `/models` route commonly needs no key to list), so it is unchanged
> and still reaches the model step and "Add anyway" exactly as written. A
> test exercising a **probe-time** `auth` rejection is the one that needs the
> updated expectation — stays on the details step, never opens the model
> step; see `test/unit/inference-default-model.test.ts`'s `modelAskFromProbe`
> suite for the non-auth probe failures that still open it in free text.

**New test: `add TinyHumans: key, model list from the paged catalog, one row, health ok`.**

1. `const key = process.env.OPENCOMPANY_E2E_TINYHUMANS_KEY;` then
   `test.skip(!key, "needs a real TinyHumans key in the environment")`.
   - No workflow provides that secret today:
     `grep -rn TINYHUMANS .github/workflows` finds none on 2026-09-14. **Report this to
     the operator.**
   - Run it locally with the key exported only; never write it to a file.
2. `choose(page, "cloud", "TinyHumans")`, fill `#inference-connect-key` with `key`, then
   click `inference-connect-submit` (it reads "Continue").
3. `inference-connect-model-step` is visible, and the field is a combobox (the catalogue returned ids). Click `#inference-connect-model`, take `const option = page.getByRole("option").first()` (with `allowBlank={false}` there is no blank row), save `const picked = (await option.textContent())!.trim()`, then click the option. **Never name a specific id: any id the endpoint returns is valid.**
4. Leave `inference-make-default` unchecked, then submit.
5. `getByTestId("inference-provider-tinyhumans")` has count **1**.
   `getByTestId("inference-provider-tinyhumans-health")` contains `ok`
   (`healthLabel("ok")`, `classify.ts:112-113`) and not `unchecked`.
6. After `page.reload()` and `openInference(page)`, the health still reads `ok`.

**New test, placed last because it changes the shared default:
`setting a default asks for a model and the row shows provider · model`.**

1. `addCustom` with name `E2E Default`, `UNREACHABLE`, key `pw-e2e-${Date.now()}`;
   submit; `pickModel(page, "e2e-default-1")`.
2. Open `inference-provider-e2e-default-menu`, then click
   `getByRole("menuitem", { name: "Set as default" })`. Use the role, because the menu
   item's test id is the same as the badge's (`ProviderList.tsx:412`, `:479`).
3. `inference-default-model-step` is visible, and `#inference-default-model` has the
   value `e2e-default-1`.
4. Fill `e2e-default-2` and click `inference-default-model-submit`. The dialog closes.
5. `getByTestId("inference-provider-e2e-default").getByText("Default · e2e-default-2")`
   is visible, and still visible after a reload.

## 8. Console

### 8.1 Types and API client

**`src/inference/types.ts`**

- `Provider` (`:34-78`) gains `model?: string | null;` and `modelAmbiguous?: boolean;`.
- Add `export interface DefaultChoice { provider: string; model: string | null }`.
- **Move `ModelAsk` here** from `ProviderConnectDialog.tsx` (`:48-50`), as
  `{ models: string[]; freeTextOnly: boolean; error?: string }`. `connect.ts` needs it,
  and the dialog already imports `connect.ts`.

**`src/api/inference.ts`**

- `InferenceStatus`: add `defaultChoice?: DefaultChoice | null;` after `providers` (`:176`).
- `ProbeResult`: delete `needsModel` (`:428`).
- `AddProviderInput` (`:442-468`): `model: string;` becomes **required** ("The one model
  this provider serves."). Add `makeDefault?: boolean;`.
- `EditProviderInput` (`:471-476`): `models?: Record<…>` becomes `model?: string;`.
- `setDefaultProvider` (`:661-670`):
  ```ts
  export function setDefaultProvider(client: OpenCompanyClient, company: string | null,
    slug: string, model: string): Promise<ProviderMutation> {
    return client.post<ProviderMutation>(
      `${client.scopeFor(company)}/inference/providers/${encodeURIComponent(slug)}/default`, { model });
  }
  ```

**`src/inference/use-inference.ts`**

- `:69` becomes `makeDefault: (slug: string, model: string) => Promise<ProviderMutation>;`.
- `:233` becomes
  `makeDefault: (slug, model) => write(slug, () => setDefaultProvider(client, company, slug, model)),`.

### 8.2 Pure rules (`src/inference/connect.ts`)

```ts
export const MAX_MODEL_ID_CHARS = 256;
export type ModelIdError = "empty" | "control" | "whitespace" | "tooLong" | "tier";
/** The console half of the host's `check_model_id`: same order, counts code points. */
export function checkModelId(raw: string): ModelIdError | null; // trim; /\p{Cc}/u; /\s/; [...id].length > 256; tier list
export function modelIdErrorCopy(error: ModelIdError): string;
export function makeDefaultInitially(status: { defaultChoice?: DefaultChoice | null } | null): boolean; // status !== null && status.defaultChoice === null
/** What step 2 is fed. A failed or absent probe still opens it, in free text. */
export function modelAskFromProbe(url: string | null, probe: ProbeResult | null): ModelAsk;
```

`modelAskFromProbe`: `url === null` → `{models: [], freeTextOnly: false}`. Otherwise `models` = `probe?.ok ? probe.models ?? [] : []` and `freeTextOnly` = `isAzureEndpoint(url)` (`catalogue.ts:424`). Only when the probe is not ok, `error` = `` `Could not read this provider's models: ${probe.message ?? "no answer"}. Type a model id.` ``.

**Error copy**

| Error | Copy |
|---|---|
| `empty` | "Choose a model." |
| `control` | "A model id cannot contain control characters." |
| `whitespace` | "A model id cannot contain spaces." |
| `tooLong` | "A model id can be at most 256 characters." |
| `tier` | "That is a workload name, not a model. Choose a model id." |

`providerMenu` (`:86-122`) is unchanged.

### 8.3 `ModelField.tsx`: list mode and required mode

New **optional** props (`:86-103`). The Routing tab call site does not change.

```ts
models?: readonly string[];   // a list already in hand: nothing is fetched and `slug` may be null
freeTextOnly?: boolean;       // list mode: Azure deployment names
catalogError?: string;        // list mode: why the list is empty
allowBlank?: boolean;         // default true ("Send the tier"); every model step passes false
label?: string;               // default "Model id"
```

- **Effect `:109-124`.** When `models !== undefined`, skip `listProviderModels`. Set
  `catalog = {baseUrl: "", models: [...models], freeTextOnly: !!freeTextOnly, error: catalogError}`
  and `loading = false`. Add the three props to the deps.
- **Input `:140-141`.**
  - `disabled={disabled || (!slug && models === undefined)}`;
  - the placeholder is `"Type a model id"` when `allowBlank === false`.
- **`:151`.** `chosen={Boolean(slug) || models !== undefined}`.
- **Rows `:283-289`.** Extract `export function modelRows(models, term, { allowBlank })`.
  It adds the blank row only when `allowBlank`. The trigger text (`:318`) is
  `value || (allowBlank ? TIER_DEFAULT_LABEL : "Choose a model")`.
- **Empty list in list mode.** Show `catalogError ?? "This provider publishes no model
  list, so type an id."` (testid `inference-model-no-catalog`).

### 8.4 `ProviderConnectDialog.tsx`: two steps

- **Props and state.** `ConnectDraft` (`:29-37`) gains `makeDefault?: boolean`. New props: `makeDefaultInitially: boolean`, `onBack: () => void`, `client` and `company`. Add `const [makeDefault, setMakeDefault] = useState(() => makeDefaultInitially);`. In edit mode, `model` seeds from `editing?.model ?? ""` (`:164`).
- **Steps.** `const step = editing ? "edit" : modelAsk ? "model" : "details";`
  - `"details"`: today's fields, without the model block.
  - `"model"`: `{ask.title}` and the model block. The step-1 fields are hidden but keep their state, and `submit` still sends them.
  - `"edit"`: today's fields plus `ModelField` (`slug={editing.slug}`, `id="inference-edit-model"`, `allowBlank={false}`).

**Model block** (replaces `:298-336`):

```tsx
<div className="grid gap-1.5" data-testid="inference-connect-model-step">
  <ModelField client={client} company={company} slug={null} id="inference-connect-model" label="Model"
    models={modelAsk.models} freeTextOnly={modelAsk.freeTextOnly} catalogError={modelAsk.error}
    allowBlank={false} value={model} onChange={setModel} />
  {model.trim() && checkModelId(model) && (
    <p className="text-xs text-status-blocked-text" data-testid="inference-model-id-error">
      {modelIdErrorCopy(checkModelId(model)!)}</p>)}
  <label className="flex items-center gap-2 text-sm">
    <input type="checkbox" className="size-4 accent-primary" data-testid="inference-make-default"
      checked={makeDefault} onChange={(e) => setMakeDefault(e.target.checked)} />
    Make this the default
  </label>
</div>
```

- **Checkbox.** Native, as at `views/finance/SendInvoiceDialog.tsx:323` (there is no `ui/checkbox`). Tokens only, no raw hex.
- **Ready** (`:178-184`):
  - `"details"`: today's rule, without `modelOk`.
  - `"model"`: `checkModelId(model) === null`.
  - `"edit"`: today's rule, plus `checkModelId` when `model.trim()` is not empty.
- **Submit** (`:186-207`): add `makeDefault: step === "model" ? makeDefault : undefined`. In edit mode, send `model` only when it changed.
- **Footer** (`:383-412`):
  - `"details"`: label `busy ? "Reading models…" : "Continue"`.
  - `"model"`: today's labels, plus `Back` (`variant="outline"`, `data-testid="inference-connect-back"`, calls `onBack`).
  - `inference-connect-submit` is used in both steps. `offerAddAnyway` shows only in `"model"`, and the account-link block (`:345-361`) only in `"details"`.

### 8.5 `ProvidersTab.tsx`: probe → model step → one POST

**New props: `client` and `company`.** The only call site is
`src/views/InferenceView.tsx:82`, which already has both in scope (`:39`). Pass them.

**`submitConnect`.** Keep the edit branch, and whatever managed-key branch slice 2a left
at `:219`. Replace the final `else` (`:223-249`):

```ts
} else if (!modelAsk) {
  const url = probeEndpoint(draft.kind, draft.baseUrl);
  const probe = url
    ? await actions.probeDraftEndpoint({ baseUrl: url, key: draft.key, kind: draft.kind })
    : null;
  setModelAsk(modelAskFromProbe(url, probe));   // the model step ALWAYS opens
  return;
} else {
  await actions.add({ ...draft, model: draft.model ?? "", makeDefault: draft.makeDefault });
}
```

**Connect-dialog props.** Pass
`makeDefaultInitially={makeDefaultInitially(state.status)}`,
`onBack={() => { setModelAsk(null); setError(null); setProbeFailure(null); }}`, `client`
and `company`.

**Set as default.** Add state `const [settingDefault, setSettingDefault] = useState<Provider | null>(null);`. `:333` becomes `onMakeDefault={(p) => setSettingDefault(p)}`. Render `<DefaultModelDialog …>` (§8.6). Its `onSubmit(model)` sets busy and awaits `actions.makeDefault(settingDefault.slug, model)`. On success the dialog closes; on error it stays open showing `stripEnvelopePrefix(message)`.

**Banner** (§8.6). Render it between the restart notice (`:269-274`) and the first
`Card` (`:276`).

### 8.6 New display pieces

**New `src/inference/DefaultModelDialog.tsx`**: a `Dialog` containing:

- `data-testid="inference-default-model-step"` and the title
  `Set {provider.label} as the default`;
- a `ModelField` with `slug={provider.slug}`, `id="inference-default-model"`,
  `allowBlank={false}` and `label="Model"`:
  - the value is seeded once from `defaultModelPrefill(provider, defaultChoice)`;
  - the catalogue is read host-side via `GET …/providers/{slug}/models`;
- the `inference-model-id-error` line and a Cancel button;
- a submit button, `inference-default-model-submit`:
  - label `busy ? "Saving…" : "Make default"`;
  - disabled unless `checkModelId(model) === null`.

Key the dialog on `provider?.slug`, as `ProvidersTab.tsx:401` keys the connect dialog.

**Helpers** (`connect.ts`):

```ts
export function defaultNeedsModel(c: DefaultChoice | null | undefined): boolean;   // c != null && c.model == null
export function isFullDefault(p: Pick<Provider, "slug">, c: DefaultChoice | null | undefined): boolean;
export function defaultBadgeLabel(p: Pick<Provider, "slug">, c: DefaultChoice | null | undefined): string;
export function rowNeedsModel(p: Pick<Provider, "slug" | "modelAmbiguous">, c: DefaultChoice | null | undefined): boolean;
export function defaultModelPrefill(p: Pick<Provider, "slug" | "model">, c: DefaultChoice | null | undefined): string;
```

**`ProviderList.tsx`**

- **Prop.** Add `defaultChoice?: DefaultChoice | null` to `ProviderList` (`:154-200`)
  and `ProviderRow` (`:362-386`). `ProvidersTab` passes `state.status?.defaultChoice`.
- **Menu (`:396`).** Use
  `providerMenu({ ...provider, isDefault: isFullDefault(provider, defaultChoice) })`.
  Without this, the resolved default of a company that has no marker could never be
  given a model.
- **Badge (`:411-415`).** Keep the condition and the testid. The text becomes
  `{defaultBadgeLabel(provider, defaultChoice)}`, with `className="max-w-48 truncate"`
  for 400px screens.
- **Chip.** After the badge:
  ```tsx
  {rowNeedsModel(provider, defaultChoice) && (
    <Badge variant="outline" className="border-status-blocked text-status-blocked-text"
      data-testid={`inference-provider-${provider.slug}-needs-model`}>Needs a model</Badge>)}
  ```
  The testid is per row rather than `inference-needs-model-chip`, because one fixed id
  would match several rows.

**Banner** (`ProvidersTab`; `defaultRow` is the provider whose slug equals
`state.status?.defaultChoice?.provider`):

```tsx
{defaultNeedsModel(state.status?.defaultChoice) && (
  <Card data-testid="inference-default-needs-model-banner">
    <CardContent className="flex flex-wrap items-center justify-between gap-3">
      <p className="text-sm">Your default provider has no model. Choose one.</p>
      {canManage && defaultRow && (
        <Button type="button" variant="outline" data-testid="inference-default-needs-model-choose"
          onClick={() => setSettingDefault(defaultRow)}>Choose a model</Button>)}
    </CardContent>
  </Card>
)}
```

### 8.7 Copy (exact)

| Where | String |
|---|---|
| checkbox | `Make this the default` |
| badge | `Default · <model>` (U+00B7 with spaces), or `Default` |
| chip | `Needs a model` |
| banner, button | `Your default provider has no model. Choose one.`, `Choose a model` |
| step 1 submit | `Continue`, busy `Reading models…` |
| default dialog | title `Set <label> as the default`, submit `Make default` |
| host note | `New work now goes through <label> · <model>.` |

## 9. Must not touch

- **Routing:** the routes API, `auto_route_sole_provider`, `RoutingTab.tsx`, `routing.ts`. Touch `WorkloadModelDialog.tsx` only if it stops compiling; it should not, because every new `ModelField` prop is optional.
- **Legacy config:** `PUT …/inference` (`ops/inference.rs:1010`), `inference/config`, `inference/key`.
- **Managed:** the managed switch, `PUT …/inference/managed/key`, `…/managed/test`, and the managed row's Test and Remove key.
- **Other surfaces:** the Composio and Account pages, `proxy-compat.ts`, and the search default store.
- **Nothing new:** no Managed-specific model dialog, route, `proxied_model` rule or secret-store key.

## 10. Done when

1. **Rust (local):** `cargo fmt --all -- --check` passes. Clippy and tests run on CI.
2. **Frontend (local):**
   - `npm run typecheck`, `npm run typecheck:unit` and `npm run typecheck:e2e` all
     pass. Name each one in the PR.
   - `npx vitest run test/unit/inference-connect.test.ts test/unit/inference-model-field.test.ts test/unit/inference-default-model.test.ts`
     passes.
   - `scripts/ci/assert-design-tokens.sh` passes.
3. **CI, checked by head SHA:**
   `gh api repos/tinyhumansai/opencompany/commits/<sha>/check-runs` shows zero failures
   and zero pending. That includes both Console E2E lanes and
   `Rust (openhuman, tinycortex)`. Never use `gh pr checks`.
4. **Auth matrix:** `tests/snapshots/auth-matrix.txt` is unchanged.
5. **Browser, light and dark, with a screenshot of each.** Use a port claimed and
   verified with `local/wt ports --verify`, and keys from env only. Capture:
   - the custom provider's model step (free text, with the probe-failure line);
   - the model step at its real cap: a custom provider pointed at a local mock `/v1/models` serving a large fake catalogue (for example 500 ids), showing the cap note;
   - TinyHumans' model step listing whatever its paged catalogue returned, one id picked, then the row with health `ok`;
   - `Default · <picked id>` at 400px width, with a long id truncated;
   - the chip and the banner on a scratch company whose `inference/default` was written
     as the bare slug `tinyhumans`;
   - the Set as default dialog, prefilled.
6. **Stored keys** after the run:
   - `inference/default` = `{"provider":"tinyhumans","model":"<picked id>"}`;
   - the `tinyhumans` element of `inference/providers` has that id under all four tier
     keys and `base_url` `https://api.tinyhumans.ai/agent-integrations/openrouter`;
   - `provider/tinyhumans/key` is set;
   - `inference/health.tinyhumans.state` is `ok`;
   - no key exists that is not listed in [current-state.md](current-state.md).
7. **TinyHumans Test:** Test on the TinyHumans row answers ok.
8. **TinyHumans e2e:** the test ran with a real key, locally or in a lane given the
   secret. The PR says which. A skipped run is not evidence.

## 12. Gotchas (console)

- **Three typecheck gates.** Every test that builds a `Provider` or an
  `AddProviderInput` needs the new fields. Find them with
  `grep -rln "keyConfigured: \|AddProviderInput" test`.
- **`MODEL_LIMIT` is 50 rendered rows** (`model-filter.ts:26`). Validate the popover
  (`max-h-64`) and the cap note with 500 ids, not with 3.
- **Azure deployment names are never in `/models`.** `isAzureEndpoint` switches the
  step to free text. Never block on catalogue membership.
- **Assume nothing about what a catalogue contains.** Any id the endpoint returns is valid. Never hardcode, filter, prefer or reject ids by vendor or name. Tests use fake ids from a mock catalogue (`acme/test-model`), and real-cap checks use a large mock catalogue (for example 500 ids).
- **The proxy returns 400 for tier names.** `checkModelId` refuses them before any
  request is sent.
- **Keys never reach the browser.** Catalogues come only from `POST …/inference/probe`
  and `GET …/providers/{slug}/models`.
- **The badge and a menu item share a testid** (`inference-provider-<slug>-default`).
  Click menu items by role.
- **The E2E company is shared.** `makeDefault` stays unchecked except in the last test.
  Check that the live-brain lane's later specs do not depend on this company's default.
