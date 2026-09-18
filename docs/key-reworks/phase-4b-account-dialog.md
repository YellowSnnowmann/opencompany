# Phase 4b — the account-key dialog

- **Goal:** the Account page's key dialog says, in one conditional line, which
  TinyHumans slots saving will fill, asks for a model in a second step when the
  host answers `needsModel`, and shows the host's note after saving.
- **Dump items:** 5, 6 (UI). **Decision:** Q9. Depends on 4a (response shape) and
  2c (`ModelField` list mode).
- **Code read at:** `upstream/main @ fcfb3e1bc` (2026-09-14). Re-verify lines.
- **Not handled here:** item 10. Before it lands the legacy chain still
  resolves `tinyhumans/key`, so a company with no default thinks and connects
  apps on the account key whether or not the copies exist. The line describes
  what saving **stores**, which is true either way.

## 1. Files

| File | What changes |
|---|---|
| `src/server/ops/company_key.rs` | `CredentialStatusDto` :121-150 gains three booleans; `effective_status` :197-228 fills them |
| `src/company/company_key/fan_out.rs` (from 4a) | new `pub async fn slot_facts` |
| `src/server/ops/company_key/test.rs` | DTO tests |
| `frontend/src/api/credential.ts` | `CompanyCredentialStatus` :37-78, `CompanyCredentialMutation` :88-91, `setCompanyCredential` :106-112 |
| `frontend/src/views/connections/account-fill.ts` (new) | pure copy functions |
| `frontend/src/views/connections/AccountKeyDialog.tsx` | new props, fill line, model step |
| `frontend/src/views/connections/ApiKeyView.tsx` | `write` :221-251, `<AccountKeyDialog …>` :491-498, header sentence :283-287 |
| `frontend/src/inference/ModelField.tsx` | no change (uses 2c's list mode) |
| `frontend/test/unit/account-key-fill-line.test.ts` (new) | copy per combination |
| `frontend/test/unit/api-key-view.test.ts` | two-step flow |
| `frontend/test/e2e/account-key-fanout.spec.ts` (new) | browser flow |

## 2. Current code

The dialog is deliberately minimal (`AccountKeyDialog.tsx:46-49`): "a heading,
the field, the 'Get an API key' link, Save and Cancel, and an error only when a
save fails. No explanatory paragraph." Its props are
`{ open, onOpenChange, replacing, busy, error, onSubmit: (key) => void }`, and it
clears the key on every open change (`:61-63`).

`ApiKeyView.write` (`:221-251`) calls `setCompanyCredential(client, company, key)`,
toasts `"Key saved."` with `description: result.note`, and closes the dialog.

The header card claims precedence (`ApiKeyView.tsx:283-287`): "Connecting points
both at this company's account — unless the LLM page already holds a TinyHumans
key of its own, which keeps precedence."

`ModelField` reads its catalog by provider slug
(`listProviderModels(client, company, slug)`, `ModelField.tsx:113`), which is
`GET …/inference/providers/{slug}/models` and 404s before the `tinyhumans` row
exists. That is why the second step needs the catalog 4a returns in `models`.

## 3. Target code

### 3.1 Host DTO

```rust
// CredentialStatusDto, after `hub_link`
/// The LLM TinyHumans key slot holds a key that is not the account key.
/// Saving leaves it alone (Q7). Never the key.
inference_has_own_key: bool,
/// The same for `composio/tinyhumans/key` (with 1a's legacy read).
composio_has_own_key: bool,
/// `inference/default` is set (ProviderOnly or Full).
default_set: bool,
```

```rust
// fan_out.rs
pub struct SlotFacts { pub inference_has_own_key: bool, pub composio_has_own_key: bool, pub default_set: bool }
/// Read-only. Same reads as fan_out step 3.2; no lock (a read races nothing it could corrupt).
pub async fn slot_facts(company: &CompanyId, secrets: &dyn SecretStore) -> Result<SlotFacts>;
```

`*_has_own_key` = trimmed slot value is non-empty **and** differs from the
trimmed `tinyhumans/key`. This is deliberately not "is set": a copy equal to
the account key is filled again on rotation, so saving does fill it. The field
names say what they hold, which is the point of this rework. It reveals whether
two stored values are equal, never either value; `GET …/credential` is
`Scoped` and already reveals `configured`.

`GET …/credential` example (company from 4a's M6):

```json
{ "configured": true, "source": "company", "notice": "…", "hubLink": false,
  "inferenceHasOwnKey": true, "composioHasOwnKey": true, "defaultSet": false }
```

### 3.2 TS types and client

```ts
// credential.ts — CompanyCredentialStatus gains (optional: an older host omits them)
inferenceHasOwnKey?: boolean;
composioHasOwnKey?: boolean;
defaultSet?: boolean;

export type FanOutSlot = "composio" | "inference" | "provider" | "default" | "health";
export type FanOutOutcome =
  | "filled" | "rotated" | "cleared" | "rolledBack" | "kept" | "skipped" | "failed" | "ok";
export interface FanOutSlotReport { slot: FanOutSlot; outcome: FanOutOutcome; detail?: string }

export interface CompanyCredentialMutation {
  status: CompanyCredentialStatus;
  note: string;
  slots?: FanOutSlotReport[];
  needsModel?: boolean;
  setsDefault?: boolean;
  models?: string[];
}

export function setCompanyCredential(
  client: OpenCompanyClient, company: string | null, key: string, model?: string,
): Promise<CompanyCredentialMutation> {
  return client.put<CompanyCredentialMutation>(
    `${client.scopeFor(company)}/credential`, model ? { key, model } : { key });
}
```

### 3.3 Pure copy (`account-fill.ts`)

```ts
export const LLM_PAGE_HREF = "#/connections/inference";      // id at src/views/connection-pages.ts:106
export const COMPOSIO_PAGE_HREF = "#/connections/composio";  // id at :127

export interface AccountFills { llm: boolean; composio: boolean }
/** null when the host did not say (older host): render no line. */
export function accountFills(status: CompanyCredentialStatus | null): AccountFills | null;
export function accountFillLine(fills: AccountFills | null): string | null;
export function modelStepTitle(setsDefault: boolean): string;
```

`accountFills` returns `null` unless both `inferenceHasOwnKey` and
`composioHasOwnKey` are booleans; else `{ llm: !inferenceHasOwnKey, composio: !composioHasOwnKey }`.

| `llm` | `composio` | `accountFillLine` |
|---|---|---|
| true | true | `Saving also connects TinyHumans for LLM and Composio.` |
| true | false | `Saving also connects TinyHumans for LLM.` |
| false | true | `Saving also connects TinyHumans for Composio.` |
| false | false | `null` (no line) |
| — | — (`fills` null) | `null` |

`modelStepTitle(true)` = `Choose the model new work uses`;
`modelStepTitle(false)` = `Choose the model TinyHumans uses`.

## 4. Ordered edits

1. **Host.** Add `slot_facts` (reuse 4a's private readers; do not duplicate the legacy-read logic). Add the three fields to `CredentialStatusDto` and fill them in `effective_status`. A read error propagates as `ApiError`, the way `key_configured` does today.
2. **Client.** Types and `setCompanyCredential(…, model?)` as §3.2.
3. **Copy.** Create `account-fill.ts` as §3.3.
4. **ModelField.** No change in 4b. 2c adds list mode (`models?: readonly string[]`, `catalogError?`, `allowBlank?`, `label?`; with `models` given nothing is fetched and `slug` may be `null`). Use exactly that.
5. **Dialog props** (`AccountKeyDialog.tsx`):
   ```ts
   fills: AccountFills | null;
   /** Set when the host answered needsModel; the dialog shows step two. */
   modelStep: { models: string[]; setsDefault: boolean; note: string } | null;
   onSubmitModel: (model: string) => void;
   ```
   Keep every existing prop.
6. **Dialog, step one.** Under the "Don't have an API key?" paragraph, when `accountFillLine(fills)` is non-null:
   ```tsx
   <p className="text-xs text-muted-foreground" data-testid="account-key-fill-line">
     {line}{" "}
     <a href={LLM_PAGE_HREF} data-testid="account-key-llm-link" className="font-medium text-foreground underline underline-offset-4">LLM page</a>
     {" · "}
     <a href={COMPOSIO_PAGE_HREF} data-testid="account-key-composio-link" className="…same…">Composio page</a>
   </p>
   ```
   Render only the link(s) for the slot(s) named in the line.
7. **Dialog, step two.** When `modelStep` is non-null, render instead of the key form (same `DialogContent`):
   - `DialogTitle`: `modelStepTitle(modelStep.setsDefault)`.
   - `<p data-testid="account-key-note" className="text-xs text-muted-foreground">{modelStep.note}</p>`.
   - `<div data-testid="account-key-model-step"><ModelField client={client} company={company} slug={null} id="account-key-model" label="Model" allowBlank={false} models={modelStep.models} catalogError={modelStep.models.length === 0 ? "TinyHumans did not list its models, so type an id." : undefined} value={model} onChange={setModel} /></div>`. The dialog therefore also takes `client` and `company` props.
   - Footer: `Cancel` (closes; the key and Composio copy are already saved, so no rollback) and `<Button type="submit" data-testid="account-key-model-save" disabled={busy || !model.trim()}>Save model</Button>` calling `onSubmitModel(model.trim())`.
   - Clear `model` state on open change, like `key`.
8. **ApiKeyView.** Add `const pendingKey = useRef<string | null>(null)` and `const [modelStep, setModelStep] = useState<…|null>(null)`. In `write(key, "save")`:
   - `result = await setCompanyCredential(client, company, key)`.
   - `result.needsModel === true` → `pendingKey.current = key`; `setModelStep({ models: result.models ?? [], setsDefault: result.setsDefault ?? false, note: result.note })`; keep the dialog open; `setGeneration(n => n + 1)` so the page reflects the saved key.
   - else → today's toast (`description: result.note`) and close.
   - New `writeModel(model)`: `setCompanyCredential(client, company, pendingKey.current ?? "", model)`; on success toast `"Key saved."` with `description: result.note`, clear `pendingKey`, `setModelStep(null)`, close. On error, show it in `error` inside step two. Guard: if `pendingKey.current` is empty, close the dialog and do not send.
   - Any close (`onOpenChange(false)`) sets `pendingKey.current = null` and `setModelStep(null)`.
   - Pass `fills={accountFills(status)}`, `modelStep`, `onSubmitModel={(m) => void writeModel(m)}`.
9. **Header sentence** (`ApiKeyView.tsx:283-287`): replace with `One key for the apps your agents act through and the models they think with. Saving copies it to the LLM and Composio pages wherever they hold no key of their own.` and update the JSX comment above it to cite Q7 instead of the old precedence chain.
10. Update `AccountKeyDialog`'s doc comment ("What it writes") to describe 4a's fan-out and Q9's single conditional line.

## 5. Data carry-over

None. 4b writes nothing new; every write is 4a's route. The three DTO fields
are derived per read, never stored.

## 6. Tests

**Host — `src/server/ops/company_key/test.rs`** (prober override from 4a set):

| Test | Setup → call → assert |
|---|---|
| `status_reports_no_own_keys_on_an_empty_company` | fresh → `GET …/credential` → both `HasOwnKey` false, `defaultSet` false |
| `a_copy_equal_to_the_account_key_is_not_an_own_key` | `PUT {key:"th-not-a-real-key"}` → GET → both false |
| `a_key_set_on_the_llm_page_is_an_own_key` | PUT account key; `PUT …/inference/managed/key {key:"th-not-a-real-key-custom"}` → GET → `inferenceHasOwnKey` true |
| `a_legacy_composio_token_counts_as_the_composio_slot` | raw `composio/token = "th-not-a-real-key-custom"` → GET → `composioHasOwnKey` true |
| `default_set_is_true_for_a_bare_slug` | raw `inference/default = "openrouter"` → `defaultSet` true |
| `status_never_carries_a_key` | all of the above: raw body contains neither fake key |

**Unit — `frontend/test/unit/account-key-fill-line.test.ts`** (vitest, pure):
`accountFillLine` for each of the five rows of §3.3 (exact strings);
`accountFills` returns `null` when either field is `undefined`;
`modelStepTitle` both values.

**Unit — `frontend/test/unit/api-key-view.test.ts`**, extending `adminClient`
(:46-60) so `put` answers from a queue:

- `the_fill_line_names_only_the_slots_saving_fills` — status `{inferenceHasOwnKey:true, composioHasOwnKey:false}` → open dialog → `account-key-fill-line` text is `Saving also connects TinyHumans for Composio.`; `account-key-llm-link` absent; `account-key-composio-link` present.
- `no_fill_line_when_both_pages_hold_their_own_keys` — both true → no `account-key-fill-line`.
- `needs_model_opens_step_two_then_reposts_key_and_model` — first `put` answers `{needsModel:true, setsDefault:true, models:["acme/test-model"], note:"Key saved. Choose a model…"}` → type `th-not-a-real-key`, Save → `account-key-model-step` visible, title `Choose the model new work uses`, `account-key-note` shows the note → pick `acme/test-model`, `account-key-model-save` → `writes` equals `[{key:"th-not-a-real-key"}, {key:"th-not-a-real-key", model:"acme/test-model"}]` → dialog closed.
- `closing_step_two_forgets_the_pending_key` — reach step two, Cancel, reopen → step one, key field empty; no second PUT.
- `a_response_without_needs_model_closes_at_once` — `put` answers `{note:"Key saved."}` → one write, dialog closed.

**E2E — `frontend/test/e2e/account-key-fanout.spec.ts`.** The host's real save
would dial `api.tinyhumans.ai`, so stub with `page.route` (the pattern in
`approval-timeout-honesty.spec.ts:152-160`): `GET **/credential` →
`{configured:false, source:"none", notice:"n", hubLink:false, inferenceHasOwnKey:false, composioHasOwnKey:false, defaultSet:false}`;
the first `PUT **/credential` → the M1 body from 4a §3.3; the second → M11 with
`note: "Key saved. TinyHumans is set up for LLM with acme/test-model. It is now the default for new work."`.

- `saving_the_account_key_asks_for_a_model_when_the_host_needs_one` — Account page → `account-add-key` → line `Saving also connects TinyHumans for LLM and Composio.` → fill `th-not-a-real-key` → Save → step two → choose model → Save model → toast contains `TinyHumans is set up for LLM`.
- Assert the second request's JSON body is `{key:"th-not-a-real-key", model:"acme/test-model"}` via `route.request().postDataJSON()`.

Run all three typecheck gates (`npm run typecheck`, `typecheck:unit`,
`typecheck:e2e`) and `scripts/ci/assert-design-tokens.sh`.

## 7. Console / UI

| Element | `data-testid` | Copy |
|---|---|---|
| fill line | `account-key-fill-line` | §3.3 table |
| LLM link | `account-key-llm-link` | `LLM page` → `#/connections/inference` |
| Composio link | `account-key-composio-link` | `Composio page` → `#/connections/composio` |
| step two wrapper | `account-key-model-step` | title from `modelStepTitle` |
| host note in step two | `account-key-note` | `modelStep.note`, verbatim |
| model save | `account-key-model-save` | `Save model` |
| existing | `account-key-input`, `account-key-get-link`, `account-key-error`, `account-key-save` | unchanged |

Browser verification (required): the dialog at step one with each of the three
lines, and step two with a 450-id catalog (the real cap is the host's 500), in
light **and** dark, screenshots named `4b-<state>-<theme>.png` in your session
scratch directory. Tokens only; no raw hex.

## 8. Must not touch

The route (`PUT …/credential` path and `Admin` authority); `useRedeemKeyGrant`
and `ConnectTinyHumansButton` (Q10: no new UI entry to the grant); the Remove-key
`AlertDialog` copy (`REMOVAL_CONSEQUENCE`, `REMOVAL_AND_THINKING`); billing rows;
any explanatory paragraph beyond the one conditional line (Q9).

## 9. Done when

- The host tests, both unit files and the e2e spec pass on CI by head SHA (zero failures, zero pending; the two Console E2E lanes included).
- Screenshots exist for every state in §7, light and dark.
- `git grep -n "keeps precedence" frontend/src/views/connections` prints nothing.

## 10. Gotchas

- **The key must survive step one but not the dialog.** Hold it in a ref, never in React state that outlives the dialog, and never in `localStorage`.
- `needsModel` can arrive with `models: []` (catalog read failed, non-auth). `ModelField` then falls back to free text; do not block Save model on an empty list.
- An auth failure answers `needsModel: false` and a note saying the key was rejected for LLM. The dialog closes with that note as the toast — it is not a save error, because the account key did save.
- A host predating 4a answers no `slots`/`needsModel`; everything must degrade to today's single-step dialog.
- `replacing` still comes from `canRemoveKey(status)`; the fill line must not depend on it.
- The second PUT re-sends the same key. 4a reports `kept`/`alreadyCurrent` for the copies; that is expected, not a regression.
