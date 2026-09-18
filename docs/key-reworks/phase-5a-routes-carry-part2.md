# Phase 5a — routes carry-over, part 2: console code, edit list, data, tests

Continues [phase-5a-routes-carry.md](phase-5a-routes-carry.md), which holds §1–§4.3
(goal, files, current code, `routes_carry.rs`, the builder call, the status DTO).
The same revision and rules apply: lines are on `fcfb3e1bc`; find each site by
the quoted code.

### 4.4 TypeScript types (`frontend/src/api/inference.ts`)

After `routes?: Record<string, string>;` (`:198`) inside `InferenceStatus`:

```ts
  /**
   * Routing rows the default did not absorb (keys rework, phase 5a). Non-null
   * while `inference/routes` names anything and the default is not a full
   * provider + model. Optional because an older host does not send it.
   */
  routesNotCarried?: RouteNotCarried[] | null;
```

Export beside `InferenceStatus`:

```ts
/** One stored routing row, as the host lists it for the banner. */
export interface RouteNotCarried {
  /** The stored tier key, e.g. `chat-v1`. */
  tier: string;
  /** The stored route string, e.g. `acme:test-model` or `managed`. */
  route: string;
}
```

### 4.5 `frontend/src/inference/routes-not-carried.ts` (new)

Pure and self-contained. **It must not import `./routing`**, which 5b deletes.

```ts
import type { RouteNotCarried } from "@/api/inference";

const TIER_WORD: Record<string, string> = {
  "chat-v1": "chat",
  "reasoning-v1": "reasoning",
  "agentic-v1": "agentic",
  "vision-v1": "vision",
};

/** `acme:test-model` → `acme · test-model`; `managed` → `Managed`; `acme` → `acme`. */
export function describeRoute(route: string): string {
  const raw = route.trim();
  if (raw === "managed") return "Managed";
  const colon = raw.indexOf(":");
  if (colon < 0) return raw;
  const slug = raw.slice(0, colon).trim();
  const model = raw.slice(colon + 1).trim();
  return model ? `${slug} · ${model}` : slug;
}

/** `chat → acme · test-model, reasoning → Managed`. */
export function describeRows(rows: readonly RouteNotCarried[]): string {
  return rows
    .map((row) => `${TIER_WORD[row.tier] ?? row.tier} → ${describeRoute(row.route)}`)
    .join(", ");
}

/** The banner sentence, or `null` when the host sent nothing to say. */
export function routesNotCarriedCopy(
  rows: readonly RouteNotCarried[] | null | undefined,
): string | null {
  if (!rows || rows.length === 0) return null;
  return (
    `Routing is going away. Each workload used: ${describeRows(rows)}. ` +
    "Choose one default provider and model, and pin agents that need something else."
  );
}
```

### 4.6 `frontend/src/inference/RoutesNotCarriedBanner.tsx` (new)

Design tokens only: `Alert` (`@/components/ui/alert`, exports `Alert`,
`AlertTitle`, `AlertDescription`, `AlertAction`; variants `default`, `warning`,
`destructive`) and `Button`. No raw colours.

```tsx
import { Info } from "lucide-react";

import type { RouteNotCarried } from "@/api/inference";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { routesNotCarriedCopy } from "./routes-not-carried";

export function RoutesNotCarriedBanner({
  rows,
  canManage,
  onChooseDefault,
}: {
  rows: readonly RouteNotCarried[] | null | undefined;
  canManage: boolean;
  /** Opens 2c's set-default step. Absent: no button. */
  onChooseDefault?: () => void;
}) {
  const copy = routesNotCarriedCopy(rows);
  if (!copy) return null;
  return (
    <Alert variant="warning" data-testid="inference-routes-not-carried-banner">
      <Info className="size-4" />
      <AlertDescription>
        <p>{copy}</p>
        {canManage && onChooseDefault && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="mt-2"
            data-testid="inference-routes-not-carried-choose"
            onClick={onChooseDefault}
          >
            Choose a default
          </Button>
        )}
      </AlertDescription>
    </Alert>
  );
}
```

If `Button` has no `size="sm"`, drop the prop.

## 5. Ordered edit list

1. Confirm 2b/2c names and signatures (top of this file). Grep:
   `git grep -n "fn load_default\b\|fn set_default_choice\|pub struct ModelChoice\|pub enum DefaultChoice\|fn check_model_id" src`.
2. Create `routes_carry.rs` (§4.1) and its tests (§7.1).
3. Add `pub mod routes_carry;` after `pub mod resolve;` in `src/company/inference.rs`.
4. Add the builder call (§4.2) and the builder tests (§7.2).
5. Add `RouteNotCarriedDto`, the field and its fill (§4.3); add the route tests (§7.3).
6. `cargo fmt --all -- --check`. Commit: `Carry a unanimous routing table into an empty default`.
7. Add the TS type (§4.4), `routes-not-carried.ts` (§4.5), the banner (§4.6).
8. In `ProvidersTab.tsx`, render
   `<RoutesNotCarriedBanner rows={state.status?.routesNotCarried} canManage={canManage} onChooseDefault={…} />`
   as the **first child** of the tab's root element, above the header card.
   `onChooseDefault` opens 2c's set-default dialog (`DefaultModelDialog`, state
   `settingDefault` / `setSettingDefault`, `phase-2c-model-required-part2.md`):
   take the first row whose route has a colon, split off its slug, and find it
   with `state.providers.find((p) => p.slug === slug && p.enabled)`. If found,
   call `setSettingDefault(provider)`; otherwise call `setAdding(true)`.
9. Unit test (§7.4), e2e test (§7.5). Run all three typecheck gates and
   `scripts/ci/assert-design-tokens.sh`. Commit: `Show the routes-not-carried banner`.
10. Push, and verify by head SHA (§10). **5b does not start before this is green.**

## 6. Data carry-over, before and after

In every case `inference/routes` is left byte-identical, and no row in
`inference/providers` changes.

| Case | `inference/routes` before | `inference/default` before | Default after boot | Outcome | Banner |
|---|---|---|---|---|---|
| four identical | `{"agentic-v1":"acme:test-model","chat-v1":"acme:test-model","reasoning-v1":"acme:test-model","vision-v1":"acme:test-model"}`, `acme` enabled | absent or `""` | `{"provider":"acme","model":"test-model"}` | `Copied` | no (`null`) |
| single row (F7) | `{"reasoning-v1":"acme:test-model"}` | absent | absent | `MissingTiers([chat-v1, agentic-v1, vision-v1])` | `reasoning → acme · test-model` |
| three of four | three rows `acme:test-model`, no `vision-v1` | absent | absent | `MissingTiers([vision-v1])` | three rows |
| mixed | `{"agentic-v1":"acme:test-model","chat-v1":"groq:test-model-b","reasoning-v1":"acme:test-model","vision-v1":"acme:test-model"}` | absent | absent | `Disagree` | four rows |
| all managed | four rows `"managed"` | absent | absent | `NotAProviderModel{tier:"chat-v1",route:"managed"}` | four rows, each `→ Managed` |
| bare slug rows | four rows `"acme"` | absent | absent | `NotAProviderModel{tier:"chat-v1",route:"acme"}` | four rows |
| provider disabled | four rows `acme:test-model`, `acme` switched off | absent | absent | `ProviderDisabled("acme")` | four rows |
| provider missing | four rows `ghost:test-model`, no `ghost` row | absent | absent | `ProviderMissing("ghost")` | four rows |
| bare-slug default | four rows `acme:test-model` | `acme` | `acme` (unchanged) | `DefaultAlreadySet` | four rows (default not full) |
| full default | four rows `acme:test-model` | `{"provider":"groq","model":"test-model-b"}` | unchanged | `DefaultAlreadySet` | no |
| unreadable routes | `{oops` | absent | absent | `NotEligible(Unreadable)` | no (`null`, warn logged) |
| no routes | absent, `""`, or `{}` | any | unchanged | `Nothing` | no |

A route naming **entry zero's** slug (`list_providers` puts entry zero first,
and `get_provider` finds it) is eligible. Entry zero is always enabled
(`store.rs:452-458`), and 2b's `decl_for_choice(EntryZero)` resolves it through
the legacy chain.

## 7. Tests

### 7.1 `src/company/inference/routes_carry.rs` — `#[cfg(test)] mod tests`

Define a local `MemSecrets` (a `Mutex<HashMap<String,String>>` store, the same
shape as `store.rs:1032-1051`) and a local `FailsWriting { inner, failing_key }`
(the same shape as `store.rs:1053-1070`). Those are private to `store.rs`'s own
tests, so they cannot be imported. Helpers:
- `add(secrets, slug, enabled)`: `store::put_provider` with a `ProviderDraft`
  (`slug`, `label`, `kind: "openai_compatible"`, `base_url: https://<slug>.example/v1`,
  `models: BTreeMap::new()`, `enabled: true`), then
  `store::set_enabled(…, false)` when `enabled` is false.
- `routes(secrets, json)`: write the raw JSON to `ROUTES_KEY`.
- `raw(secrets, key)`: read it back.

Use `CompanyId::new("acme")` and fake values only.

| Test | Setup | Call | Assert |
|---|---|---|---|
| `the_route_grammar_is_read_the_way_the_routing_table_wrote_it` | none | `parse_route` over `""`, `default`, `managed`, `acme`, `acme:`, `acme:test-model`, `ollama:local-model:8b`, `local:local-vision-model`, `claude-code:test-model` | `Unset, Unset, Managed, ProviderOnly, ProviderOnly, ProviderModel(acme,test-model), ProviderModel(ollama,local-model:8b), Category(local), Category(claude-code)` |
| `four_identical_rows_are_copied` | `add acme`; four rows `acme:test-model` | carry | `Copied(acme,test-model)`; `load_default` is `Full` with those values; the routes raw string is unchanged |
| `a_single_row_is_not_copied` | `add acme`; `{"reasoning-v1":"acme:test-model"}` | carry | `NotEligible(MissingTiers(["chat-v1","agentic-v1","vision-v1"]))` in `INFERENCE_TIERS` order; default raw absent |
| `three_of_four_rows_are_not_copied` | no `vision-v1` row | carry | `MissingTiers(["vision-v1"])` |
| `mixed_rows_are_not_copied` | `add acme`, `add groq`; the mixed row from §6 | carry | `Disagree`; no default written |
| `managed_rows_carry_no_model_and_are_not_copied` | four `managed` rows | carry | `NotAProviderModel{tier:"chat-v1",route:"managed"}`; no default written |
| `rows_without_a_model_are_not_copied` | four `acme` rows, then four `local:local-vision-model` rows | carry, twice | `NotAProviderModel` both times |
| `a_full_or_bare_slug_default_is_never_overwritten` | four `acme:test-model`; (a) `set_default_slug("groq")`; (b) `set_default_choice(groq, test-model-b)` | carry | both `DefaultAlreadySet`; the default raw value is byte-identical before and after |
| `a_disabled_or_missing_provider_is_not_copied` | (a) `add acme` disabled; (b) no row, routes `ghost:test-model` | carry | `ProviderDisabled("acme")`, `ProviderMissing("ghost")`; no default |
| `a_route_naming_entry_zero_is_copied` | `super::save_runtime_config` with `provider: "openai_compatible"`, `base_url: Some("https://legacy.example/v1")`; four rows `<entry-zero slug>:test-model`, slug from `store::list_providers(..)[0].slug` | carry | `Copied` with that slug |
| `an_unreadable_routes_blob_is_not_eligible_and_writes_nothing` | routes `{oops` | carry | `NotEligible(Unreadable)`; default absent |
| `no_routes_is_nothing` | none; then `""`; then `{}`; then `{"chat-v1":""}` | carry | `Nothing` each time |
| `a_second_run_writes_nothing` | four `acme:test-model`, `add acme` | carry twice, wrapping the store in a counting double (`sets: AtomicUsize` incremented in `set`) | first `Copied`, second `NotEligible(DefaultAlreadySet)`; set count is 1 after the first run and still 1 after the second |
| `a_failing_write_leaves_boot_running` | `FailsWriting { failing_key: store::DEFAULT_PROVIDER_KEY }`; `add acme`; four `acme:test-model` | carry | returns `Err` and does not panic; routes raw unchanged; `load_default` still `Unset` |
| `routes_not_carried_lists_set_rows_in_tier_order` | `{"vision-v1":"managed","chat-v1":"acme:test-model","reasoning-v1":""}` | `routes_not_carried` | `Some([chat-v1 acme:test-model, vision-v1 managed])` |
| `routes_not_carried_is_none_without_routes_or_with_a_full_default` | none; then four rows + `set_default_choice` | helper | `None`, `None` |
| `routes_not_carried_is_some_for_a_bare_slug_default` | four rows + `set_default_slug("acme")` | helper | `Some` with four rows |

### 7.2 `src/runtime/builder.rs` tests (`mod test`, `:4638`)

Model the build on `:9034-9047` (`RuntimeBuilder::new(home, manifest).with_id(id).with_harness(Arc::new(HarnessPool::new()))`)
and inject secrets with `.with_secrets(Arc<dyn SecretStore>)` (`:1414`). Use
the in-memory secret store existing builder tests pass to `with_secrets`
(`git grep -n "with_secrets(" src/runtime/builder.rs`). Gate both tests with
`#[cfg(feature = "openhuman")]`, like the branch they exercise.

- `a_unanimous_routing_table_becomes_the_default_at_boot`: seed provider `acme`
  and four `acme:test-model` rows in the store, then build. Assert
  `store::load_default` is `Full(acme, test-model)`.
- `a_failing_route_carry_write_does_not_fail_the_build`: wrap the store so writes
  to `inference/default` return `Err`, seed the same data, then build. Assert
  `build()` is `Ok`, and the default is still `Unset`.

### 7.3 `src/server/ops/inference.rs` tests

Helpers: `home()` `:1407`, `state_with_company` `:1476`, `send` `:1812`.

- `status_names_routes_that_were_not_carried`: add provider `acme` through
  `POST /api/v1/company/inference/providers` (the body 2c requires, with fake key
  `sk-not-a-real-key`). Then `PUT /api/v1/company/inference/routes` with
  `{"routes":{"chat-v1":"acme:test-model","reasoning-v1":"managed"}}`, then
  `GET /api/v1/company/inference`. Assert
  `dto["routesNotCarried"] == json!([{"tier":"chat-v1","route":"acme:test-model"},{"tier":"reasoning-v1","route":"managed"}])`.
- `status_routes_not_carried_is_null_without_routes`: on a fresh company, assert
  `dto["routesNotCarried"].is_null()`.
- `status_routes_not_carried_is_null_once_the_default_is_full`: set the same
  routes, then write the default with `store::set_default_choice` on
  `state.registry().get(&CompanyId::new("acme")).unwrap().secrets()`. Assert
  `null`.

### 7.4 `frontend/test/unit/inference-routes-banner.test.ts`

Vitest. Render with `createElement` + `renderToStaticMarkup` from
`react-dom/server` (the pattern at `inference-hub-account-links.test.ts:26,34`).

- `describeRoute` handles `acme:test-model`, `managed`, `acme`, `acme:`, and `ollama:local-model:8b` (→ `ollama · local-model:8b`).
- `routesNotCarriedCopy` returns `null` for `null`, `undefined` and `[]`.
- `routesNotCarriedCopy([{tier:"chat-v1",route:"acme:test-model"},{tier:"vision-v1",route:"managed"}])`
  equals exactly `Routing is going away. Each workload used: chat → acme · test-model, vision → Managed. Choose one default provider and model, and pin agents that need something else.`
- The banner markup contains `data-testid="inference-routes-not-carried-banner"`
  and the choose button when `canManage` and `onChooseDefault` are given. Without
  `canManage` there is no button. With `rows: null` the markup is `""`.

### 7.5 `frontend/test/e2e/inference.spec.ts`

Add after the test at `:253`:
`test("the routes-not-carried banner names a route until its provider is removed", …)`.

1. `openInference(page)`. Read `await (await page.request.get("/api/v1/company/inference")).json()`
   (same-origin reads are used at `mcp.spec.ts:162`). If its `defaultChoice` is
   non-null with a non-null `model` (2c's wire shape: `{"provider","model"}`,
   `model: null` for a bare slug, `null` when unset), fail with
   `expect(…, "the E2E company already has a full default, so the banner cannot show; move this test above the one that sets it").toBe(false)`.
   Never `test.skip`.
2. `addCustom`, then label `E2E Banner ${Date.now()}`, URL `UNREACHABLE`, a fake
   key, and the model field 2c added (`e2e-banner-model`). Submit. Read the row's slug from its
   `data-testid`.
3. Route one workload to it exactly as `:265-275` does (Routing tab → Advanced →
   reasoning → provider → apply).
4. Open the LLM Providers tab. Expect `inference-routes-not-carried-banner` to
   contain `reasoning → <slug>`.
5. Remove the provider as `:278-289` does. Expect the banner either to have
   count 0 or not to contain `<slug>`. Other specs may have left rows, so do not
   assert that the banner is gone.

**Why there is no "becomes the default on restart" e2e** (plan D1 listed one):
the Console E2E lanes share one host and one company (`playwright.config.ts:401-402`,
`workers: 1`). A carried default on that company would point every later spec's
turns at the discard port, and the store has no way to put the default back to
`Unset`. The carry is proven by §7.1–§7.2 instead.

## 8. Console / UI

- **Copy (exact):** `Routing is going away. Each workload used: chat → acme · test-model, reasoning → Managed. Choose one default provider and model, and pin agents that need something else.`
  Workload words are lower case; routes render as `slug · model`, `Managed`, or
  the bare slug.
- **Test ids:** `inference-routes-not-carried-banner`, `inference-routes-not-carried-choose`.
- **Placement:** top of the LLM Providers tab. The Routing tab is unchanged in 5a.
- **Who sees it:** everyone who can read the status. Only an admin gets the button.
- **Tokens:** `Alert variant="warning"` and `Button variant="outline"`. No hex,
  no inline colour. `scripts/ci/assert-design-tokens.sh` must pass.
- **Browser:** check light and dark at 390 px and desktop width. The four-row
  case (the real cap: four tiers) must wrap without overflow.

## 9. Must not touch

- Any route reader: `resolve_effective_for_tier`, the step-4 branch, `resolve.rs`,
  `store::load_routes` / `save_routes`, the routes API, the Routing tab. They stay
  live until 5b.
- `inference/managed/enabled` and its switch; the managed chain and its fallbacks
  (dump item 10); entry zero `inference/config`; the env default on `/openai/v1`.
- `inference/routes` itself: never cleared, rewritten or re-serialized.
- `inference/providers`: no row is written.
- Read paths: `routes_not_carried` and the status route never write.

## 10. Done when

- `cargo fmt --all -- --check` is clean locally.
- Frontend: `npm run typecheck`, `npm run typecheck:unit` and `npm run typecheck:e2e`
  all pass (name each gate you ran), and `scripts/ci/assert-design-tokens.sh` passes.
- CI is verified by head SHA, not `gh pr checks`:
  `gh api repos/tinyhumansai/opencompany/commits/<sha>/check-runs`, with **zero
  failures and zero pending**, including both Console E2E lanes (they start late).
- The banner has been seen in a real browser, light and dark, with screenshots:
  a company with a mixed table shows it; after a unanimous table and a restart,
  the banner is gone and the default row shows `acme · test-model`.
- The §7 tests exist and pass on CI.

## 11. Gotchas

1. **F7.** The store drops unset rows, so `{"reasoning-v1":"acme:test-model"}` is a
   complete stored table. "Every stored row agrees" would copy it and move chat,
   agentic and vision spend to `acme`. Only all four `INFERENCE_TIERS` present
   and equal qualifies.
2. **Managed only in routes.** A company whose only choice is four `managed` rows
   (the step-4 branch, `inference.rs:1262-1315`) is not carried: `managed`
   carries no model, and no row may be written. It must see the banner. After 5b
   it gets the "choose a model" error, never a silent echo.
3. **Order.** Removing route readers before this slice ships strands exactly the
   companies in gotcha 2 on the echo brain. 5a is pushed and green first.
4. **No writes on read paths** (`store.rs:83-90` states the rule). The carry runs
   only at build. The status helper only reads.
5. **Never fatal.** Boot logs and continues on `Err`. A default that will not
   parse is `Err`, never `Unset`, so it is never overwritten.
6. **Races.** Two builds of one company may both write. They write the same
   value into the same slot, so the result is the same.
7. **Hosts without the harness** (no `openhuman` feature, or no pool) never run
   the carry, because the call sits in the harness branch. Their turns never
   read routes either, so the banner there is informational.
8. **A bare-slug default blocks the carry** but still shows the banner. The
   admin chose a provider and no model, and the routes may name another provider.
9. **Per-workload cost control disappears.** A company that routed cheap chat
   and expensive agentic work gets one default model. Q13 decided against a
   background model, so do not add one. The banner is the only notice.
10. **Hosted tenants** carry exactly like any other company. The env default is
    untouched and no credential moves.
