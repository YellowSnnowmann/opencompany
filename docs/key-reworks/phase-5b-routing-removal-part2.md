# Phase 5b — routing removed, part 2: console, tests, auth matrix, docs

Continues [phase-5b-routing-removal.md](phase-5b-routing-removal.md). The same
revision and rules apply: lines are on `fcfb3e1bc`, and each site is found by
the quoted code.

## 1. Console

### 1.1 Files

| File | Site on `fcfb3e1bc` | Change |
|---|---|---|
| `frontend/src/inference/RoutingTab.tsx` | 473 lines | **delete** |
| `frontend/src/inference/WorkloadModelDialog.tsx` | 274 lines | **delete** |
| `frontend/src/inference/routing.ts` | 976 lines | **delete** after moving its survivors (§1.2) |
| `frontend/src/inference/removal.ts` | does not exist | **new**: removal impact and warnings, without routes |
| `frontend/src/inference/managed-copy.ts` | does not exist | **new**: managed sentences moved verbatim |
| `frontend/src/views/InferenceView.tsx` | import `:8`; tabs `:32-37`; `useHashTab` `:40-43`; `tabs=` `:55-63`; tabpanel `:76-92` | single page, no tabs |
| `frontend/src/inference/use-inference.ts` | imports `:24`, `:26`, `:42-43`; state `:53-57`, `:83`, `:101-103`; routes read `:112-144`; re-read `:178-207`; returns `:223-225`, `:262-268` | remove routes |
| `frontend/src/inference/ProviderList.tsx` | import `:18`; prop `:171`, `:206-210`, `:332`, `:373`, `:386-387`; `routingBadge` `:390`; badge `:417-430` | remove the routing badge |
| `frontend/src/inference/ProvidersTab.tsx` | imports `:21-31`; `routingMap` `:144-152`; `routingState=` `:334`; `removalImpact(…)` `:414-418`; managed dialog impact `:457-462` | new imports and signatures |
| `frontend/src/inference/RemoveProviderDialog.tsx` | imports `:11-12`; doc `:30-31`, `:38-40`, `:47-48`; comment `:84` | import from `./removal` |
| `frontend/src/inference/types.ts` | `Workload` `:98`, `ProviderRef` `:102-113`, `RoutingMap` `:115-116`, `RoutingMode` `:118-128` | delete each one `git grep` shows unused |
| `frontend/src/api/inference.ts` | `RoutingMode` import `:14`; `routes?` `:186-198`; `affectedTiers` `:437-438`; `RoutesResponse` `:478-486`; `getRoutes` / `putRoutes` `:699-720`; doc `:690` | delete; keep `routesNotCarried` (5a) |
| `frontend/src/inference/ProviderConnectDialog.tsx` | copy `:332-333` ("change that under Routing") | drop the Routing clause if 2c left it |

### 1.2 `frontend/src/inference/managed-copy.ts` (new)

Move these **verbatim**, with their doc comments, from `routing.ts`:
`MANAGED_NOT_SET_UP` (`:144-145`), `MANAGED_SWITCHED_OFF` (`:177-178`),
`managedFallbackNote` (`:163-170`), `nothingCanAnswer` (`:403-408`) and
`MANAGED_TARGET_LABEL` (`:696`). `nothingCanAnswer` needs
`import type { Provider } from "./types";`.

Do **not** move `MANAGED_NOT_SET_UP_ELSEWHERE` or
`MANAGED_SWITCHED_OFF_ELSEWHERE`. They name the "LLM Providers tab", no file in
`src` imports them, and they are deleted.

### 1.3 `frontend/src/inference/removal.ts` (new)

`primaryProvider` moves verbatim (`routing.ts:359-361`). The rest drops routes:

```ts
import type { Provider } from "./types";

/** The three things that can be done to a provider from its row. */
export type ProviderIntent = "disable" | "key" | "provider";

/** What removing a provider — or just its key — would cost. */
export interface RemovalImpact {
  /** Whether it is this company's default. */
  isDefault: boolean;
  /** Whether it is the last provider this company has switched on. */
  lastEnabled: boolean;
  /** The provider the default would move to, or `null`. */
  defaultMovesTo: string | null;
}

export function primaryProvider(providers: readonly Provider[]): Provider | undefined {
  return providers.find((p) => p.isDefault && p.enabled) ?? providers.find((p) => p.enabled);
}

export function removalImpact(
  provider: Pick<Provider, "slug" | "enabled"> & { isDefault?: boolean },
  providers: readonly Provider[],
): RemovalImpact {
  const remaining = providers.filter((p) => p.slug !== provider.slug);
  const isDefault = Boolean(provider.isDefault);
  return {
    isDefault,
    lastEnabled: provider.enabled && !remaining.some((p) => p.enabled),
    defaultMovesTo: isDefault ? (primaryProvider(remaining)?.label ?? null) : null,
  };
}
```

`removalWarnings(intent, label, impact, managed)` and `disableWarnings` move from
`routing.ts:569-676`, with exactly these changes and no others. The default
sentences are 2c's, so leave them as 2c left them.

- Key intent, first line: `` `${label} stays on this page, keeping its endpoint. It just has no credential, so it cannot answer until you add one.` ``
- Delete the whole `if (impact.routed.length > 0) { … }` block, in both functions.
- `disableWarnings` first line: `` `${label} keeps its endpoint and its key. Switching it back on restores both — nothing is deleted.` ``
- Delete `describeWorkloads` (`:685-690`) and the `WORKLOAD_COPY` use.

### 1.4 `frontend/src/views/InferenceView.tsx` (whole file)

```tsx
import type { OpenCompanyClient } from "@/api/client";
import { AdminOnlyNotice } from "@/components/admin-only-notice";
import { PageHeader } from "@/components/page-header";
import { useCanManage } from "@/hooks/use-can-manage";
import { ProvidersTab } from "@/inference/ProvidersTab";
import { useInference } from "@/inference/use-inference";

interface Props {
  client: OpenCompanyClient;
  company: string | null;
}

/**
 * What this company can reach a model through.
 *
 * It had a second tab, Routing, until the keys rework removed per-workload
 * routing (issue #2306). A company now has one default provider and model, and
 * an agent can pin its own. An old `?tab=routing` address is ignored and lands
 * here, which is the page the Routing tab's work moved to.
 */
export function InferenceView({ client, company }: Props) {
  // Changing the model or the key changes what every teammate's turn costs, so
  // it is an admin's.
  const canManage = useCanManage(client, company);
  const inference = useInference(client, company);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <PageHeader
        title="LLM"
        width="full"
        description="Configure AI providers, local models, and the agent chat tools."
      />
      <div className="min-h-0 w-full flex-1 space-y-6 overflow-y-auto px-4 py-6">
        {!canManage && (
          <AdminOnlyNotice
            testId="inference-read-only"
            title="Only an admin can change this company's model"
          >
            The model and its key decide what every agent&apos;s turn costs, so an admin sets
            them. You can see what is configured.
          </AdminOnlyNotice>
        )}
        <ProvidersTab state={inference} actions={inference} canManage={canManage} />
      </div>
    </div>
  );
}
```

If 2c or 3b added props to `ProvidersTab` here (for example `client` and
`company` for `DefaultModelDialog`), keep them.

### 1.5 `frontend/src/inference/use-inference.ts`

- Imports: delete `getRoutes`, `putRoutes`, the `./routing` import, and
  `RoutingMap` / `RoutingMode`. Keep `ApiError` and `Provider`.
- `InferenceState`: delete `routes`, `mode` and `orphaned`. `InferenceActions`:
  delete `saveRoutes`.
- State: delete `routes`, `mode` and `orphaned` (`:101-103`).
- `reload` becomes:

```ts
  const reload = useCallback(async () => {
    try {
      setStatus(await getInferenceStatus(client, company));
      setLoad("ready");
    } catch (err) {
      // A host that does not serve this route at all is not an error worth a
      // banner — the page simply is not available on that build.
      setLoad(err instanceof ApiError && err.status === 404 ? "unavailable" : "error");
    }
  }, [client, company]);
```

- In `write`, delete the whole `try { const table = await getRoutes(…) } catch { … }`
  block (`:179-207`), leaving `setStatus(result.status); toast.success(result.note); return result;`.
- In the returned object, delete `routes`, `mode`, `orphaned` and `saveRoutes`.
- Header comment (`:1-11`): replace "Both tabs render from this… or the reverse."
  with "The LLM page renders from this.", and delete `routing.ts` from the list
  of decision modules.

### 1.6 `ProviderList.tsx`, `ProvidersTab.tsx`, `RemoveProviderDialog.tsx`

- **`ProviderList.tsx`:** delete `import { routingBadge } from "./routing";`, the
  `routingState` prop (declaration, doc and pass-through) on `ProviderList` and
  on the row, `const routing = routingBadge(routingState);`, and the
  `{routing && (<Badge … data-testid={`inference-provider-${provider.slug}-routing`}>…)}`
  block with its comment. Keep `cn` if another use remains. On `:192`, reword the
  managed-toggle doc to "Switch managed on or off — never its credential."
- **`ProvidersTab.tsx`:** replace the `./routing` import and
  `import type { RoutingMap }` with
  `import { MANAGED_TARGET_LABEL, managedFallbackNote, nothingCanAnswer } from "./managed-copy";`
  and `import { removalImpact } from "./removal";`. Delete the `routingMap`
  block and the `routingState={…}` prop. Replace the remove-dialog impact with
  `confirming ? removalImpact(confirming.provider, state.providers) : { isDefault: false, lastEnabled: false, defaultMovesTo: null }`,
  and the managed dialog impact with
  `{ isDefault: false, lastEnabled: false, defaultMovesTo: null }`. Delete
  `categoryOf` from the imports if nothing else uses it. Reword comments that
  mention the Routing tab or routes (`:305-310`, `:322-324`, `:346-351`,
  `:421-424`, `:452-454`) so they no longer name either.
- **`RemoveProviderDialog.tsx`:** import `removalWarnings`, `ProviderIntent` and
  `RemovalImpact` from `./removal`. In the doc, replace "deletes a record, its
  endpoint and every route that named it" with "deletes a record and its
  endpoint", "the workloads whose routes reset" with nothing, and "the credential
  and the routes" with "and the credential".
- **`types.ts`:** for each of `Workload`, `ProviderRef`, `RoutingMap` and
  `RoutingMode`, run `git grep -n "\b<Name>\b" frontend/src frontend/test` and
  delete the type when only `types.ts` matches.

## 2. Tests

### 2.1 `src/company/inference.rs`

**Delete:** `a_route_sends_its_workload_to_the_provider_and_model_it_names` (`:3444`),
`the_route_beats_the_default_providers_own_tier_map` (`:3475`),
`a_route_naming_managed_resolves_through_the_managed_chain` (`:3597`),
`a_route_naming_managed_fails_closed_once_managed_is_switched_off` (`:3936`),
`a_route_naming_a_provider_that_is_gone_fails_closed` (`:3987`),
`a_route_naming_a_switched_off_provider_fails_closed_too` (`:4012`),
`coding_reads_the_agentic_route_rather_than_one_of_its_own` (`:4036`),
`routing_to_managed_while_the_switch_is_off_resolves_to_nothing` (`:4139`),
`routing_to_managed_with_nothing_behind_it_still_resolves_to_nothing` (`:4197`),
`an_unset_route_is_not_a_managed_route` (`:4221`),
`the_managed_branch_runs_last_and_changes_no_company_that_already_resolved` (`:4246`),
`a_route_naming_entry_zero_reaches_the_legacy_config` (`:4269`),
`a_tier_nobody_routes_resolves_exactly_as_it_always_did` (`:4317`), the helper
`route` (`:3434-3441`), and `wire_model` (`:3430-3432`) if nothing else uses it.
2d may already have moved some of these; delete whichever remain.

**Rewrite onto `resolve_for_turn(&company, &manifest, env, &secrets, &HarnessScope::default(), None)`**
(it returns `Result<InferenceDecl>`, so `.expect(…)` replaces `.unwrap().expect(…)`):
- `a_workload_with_no_route_still_falls_through_to_the_primary` (`:3566`) →
  rename `a_turn_with_no_default_falls_through_to_the_primary`. Delete the
  `route(…)` line and the `wire_model` assertion; keep the base URL and bearer
  assertions.
- `switching_managed_off_never_makes_the_status_unreadable` (`:3837`): swap the call.
- `an_unset_workload_stops_falling_back_to_managed_once_it_is_switched_off` (`:3876`)
  → rename `a_turn_stops_falling_back_to_managed_once_it_is_switched_off`, and
  swap all three calls.

**Replace** `a_company_routed_to_managed_resolves_rather_than_landing_on_echo` (`:4091`)
with `a_company_routed_only_to_managed_is_told_to_choose_a_model_never_silently_echoed`:
- Setup, as before: `add_indexed(&secrets, "anthropic", "sk-not-a-real-key-anthropic")`,
  `store::set_enabled(…, "anthropic", false)`, `managed_key(&secrets, "sk-not-a-real-key-managed")`,
  and a raw write of `{"agentic-v1":"managed","chat-v1":"managed","reasoning-v1":"managed","vision-v1":"managed"}`
  to `store::ROUTES_KEY`.
- Assert `resolve_effective(&company, &Inference::default(), None, &secrets)` is
  `Ok(None)`. Boot then selects echo, the same as a company with nothing.
- Assert `resolve_for_turn(…, None)` is `Err`, and its message contains
  `NO_MODEL_CHOSEN`.
- Assert `routes_carry::routes_not_carried(&company, &secrets)` is `Some` with
  four rows whose `route == "managed"`. That is the banner, so this is not silent.
- Assert `routes_carry::carry_routes_into_default` gives
  `NotEligible(NotAProviderModel { tier: "chat-v1", route: "managed" })`, and
  `store::load_default` stays `Unset`.

**Add** `a_stored_routes_blob_changes_nothing_about_a_turn`:
- Setup: `add_indexed` for `first` (`sk-not-a-real-key-1`) and `second`
  (`sk-not-a-real-key-2`), and `managed_key(…)`.
- Resolve once with no routes: `resolve_for_turn(…, None)` and
  `resolve_effective(…)`.
- Then write raw routes `{"chat-v1":"second:acme/other-model","reasoning-v1":"managed","agentic-v1":"ghost:x","vision-v1":"local:local-vision-model"}`
  and resolve again.
- Assert equal `base_url`, `bearer(&decl)`, `is_proxied()` and `models`
  between the two runs, with `base_url == "https://first.example/v1"`.

### 2.2 `src/server/ops/inference.rs`

- `deleting_a_provider_clears_its_credential_and_scrubs_its_routes` (`:3313`) →
  rename `deleting_a_provider_clears_its_credential`. Delete the `PUT …/routes`
  call, the `affectedTiers` assertion and the `GET …/routes` block. Keep the
  re-add `keyConfigured == false` half.
- Delete `disabling_keeps_the_route_and_names_the_tiers_it_parks` (`:3377`),
  `a_route_naming_a_provider_nobody_holds_is_refused` (`:3686`),
  `a_tier_this_runtime_does_not_have_is_refused` (`:3709`),
  `the_routing_mode_is_inferred_from_the_routes` (`:3728`),
  `the_only_provider_a_company_can_use_is_routed_to` (`:3795`) and
  `a_provider_added_beside_managed_is_not_routed_to` (`:3828`).
- `configuring_only_managed_after_boot_reports_restart_required` (`:4083`, gated
  `#[cfg(feature = "openhuman")]`) → rename
  `a_company_routed_only_to_managed_boots_echo_and_names_its_routes`. Replace
  the `PUT …/routes` call with a raw write of the four `managed` rows through
  `state.registry().get(&id).unwrap().secrets().set(&id, store::ROUTES_KEY, SecretValue(…))`.
  Assert `dto["cognition"] == "echo"`, `dto["restartRequired"] == false`, and
  that `dto["routesNotCarried"]` has four rows, each `"route":"managed"`. Delete
  the `managed.configured`, `managed.source` and workflow-run assertions, and
  update the doc comment to say what is asserted now.
- **Add** `the_routes_api_is_gone`: on `state_with_company`, send
  `GET /api/v1/company/inference/routes` and
  `PUT /api/v1/company/inference/routes` with body `{"routes":{}}`. Assert
  `StatusCode::NOT_FOUND` for the GET, and `NOT_FOUND` or `METHOD_NOT_ALLOWED`
  for the PUT (print `raw`).

### 2.3 Rust tests elsewhere

`routes_carry.rs` tests (5a) pass unchanged. The builder tests from 5a pass
unchanged. Then run `git grep -n "inference/routes" src tests` and expect only
`routes_carry.rs`, `store.rs`'s constant and the new tests.

### 2.4 Frontend unit tests (`frontend/test/unit/`)

- **Delete** `inference-routing.test.ts`.
- **New** `inference-removal.test.ts`: move the cases from
  `inference-routing.test.ts` `describe("what a removal costs")` (`:462-539`)
  and `describe("where the default goes when a provider is removed")`
  (`:561-587`), importing from `@/inference/removal`. Call
  `removalImpact(provider, providers)` and drop every `routed:` field. Delete
  "names the workloads whose routes would reset" and "counts the routed
  workloads…". Add `it("never mentions routes")`: for each intent in `disable`,
  `key` and `provider`, and each of `isDefault` / `lastEnabled` true and false,
  every line from `removalWarnings` fails `toMatch(/rout/i)`.
- **Move** "says nothing can answer only when nothing actually can" (from
  `describe("the false Managed floor")`) into `inference-managed-row.test.ts`,
  importing `nothingCanAnswer` from `@/inference/managed-copy`. Delete the
  `primaryLabel` cases; `primaryLabel` is gone.
- `inference-managed-row.test.ts` (`:14-19`): import from `@/inference/managed-copy`.
  Delete the expectations that name `MANAGED_NOT_SET_UP_ELSEWHERE`.
- `inference-card-honesty.test.ts` (`:79-80`): delete the `/inference/routes` stub branch.
- `inference-hub-account-links.test.ts` (`:74`): delete `import("@/inference/RoutingTab?raw"),`.
- `page-section-heading-level.test.ts` (`:71-77`): set `sections: ["../inference/ProvidersTab"]`
  and change the comment to "One section: the Routing tab was removed (issue #2306)."
- 5a's `inference-routes-banner.test.ts` stays unchanged.
- Run `git grep -n "routes: {}\|mode: \"managed\"\|orphaned\|saveRoutes" frontend/test`
  and remove the fields from any `InferenceState` fixture.

### 2.5 E2E (`frontend/test/e2e/`)

- `inference.spec.ts`:
  - Delete `the routing mode is inferred from the routes and round-trips` (`:297-319`),
    `coding is shown as an alias and cannot be routed separately` (`:321-333`),
    and 5a's `the routes-not-carried banner names a route until its provider is removed`.
    Its setup needs the Routing tab; the banner stays covered by the unit test.
  - `deleting a provider removes its row and resets the routes that named it` (`:253`)
    → rename `deleting a provider removes its row`. Delete the routing steps
    (`:264-275`), the "LLM Providers" tab click (`:277-278`) and the final
    Routing assertions (`:291-294`).
  - In the header doc (`:17-18`), replace "and a delete that has to move a route
    in a different subsystem" with "and a delete that has to clear a credential".
  - Give `openInference` an optional `hash = "/#/settings/inference"` parameter
    and use it in `page.goto(hash)`.
  - **Add** at the end:

```ts
test("the inference page has no routing tab", async ({ page }) => {
  // An old `?tab=routing` link still lands on the providers.
  await openInference(page, "/#/settings/inference?tab=routing");
  await expect(page.getByRole("tab", { name: "Routing" })).toHaveCount(0);
  await expect(page.getByTestId("inference-mode-own")).toHaveCount(0);
  await expect(page.getByTestId("inference-add-open")).toBeVisible();
});
```

- `connections-authority.spec.ts` (`:216-221`, `:306-309`): comment edits only.
  Delete the sentences about the Routing tab and `inference-own-save`.

## 3. Auth matrix

The matrix is `tests/auth_matrix.rs`, compiled as a test target of
`crates/opencompany-core` (`crates/opencompany-core/Cargo.toml:50`,
`path = "../../tests/auth_matrix.rs"`, so `CARGO_MANIFEST_DIR/../../` is the repo
root). `committed_snapshot_pins_every_expected_cell` (`:1818-1828`) compares
`render_snapshot()` with `include_str!("snapshots/auth-matrix.txt")`. With
`BLESS_AUTH_MATRIX` set, it writes the snapshot instead.
`source_path_set_equals_the_ops_matrix_path_set` (`:1862`) scans
`src/server/ops` for `scoped("…")` literals, so the router line and the table row
must go together.

1. Delete `r!(Get, "/inference/routes", Admin, Ordinary, ""),` and
   `r!(Put, "/inference/routes", Admin, Authority, ""),` (`:595-596`).
2. Update the counts relative to their values **at that time** (4c may have
   changed them). Each route row renders two concrete patterns, `company` and
   `companies/{id}`.
   - `:1743` concrete route-method rows: **minus 4** (467 → 463 on `fcfb3e1bc`).
   - `:1751` concrete paths: **minus 2** (370 → 368). GET and PUT share each path.
   - `:1754` `render_snapshot().lines().count()`: **minus 28** (3_269 → 3_241):
     2 routes × 2 patterns × 7 principals.
   - `:1773` admin rows: **minus 2** (77 → 75). In the message at `:1774`,
     subtract 2 from the signature-admin number (62 → 60); both handlers take
     `AdminScopedCompany`.
3. Snapshot: delete the 28 rows. Run
   `perl -ni -e 'print unless m{ /api/v1/compan(y|ies/\{id\})/inference/routes }' tests/snapshots/auth-matrix.txt`,
   then check that `grep -c "inference/routes" tests/snapshots/auth-matrix.txt`
   prints `0`, and that `git diff --stat` shows exactly 28 deletions there. Do
   not run cargo locally to bless it; CI runs the matrix. If CI reports a diff,
   the lane's log names the exact lines.

## 4. Docs

- Delete `docs/modules/inference/routing.md` and `docs/modules/inference/routing-states.md`.
- `docs/modules/inference/README.md`: delete the `routing.md` row (`:16`), redraw
  the diagram header at `:75` without the Routing view, and rewrite point 4 at
  `:96` as history ("Routing existed until issue #2306").
- `architecture.md` (`:70`, `:109`, `:123`), `connect-flow.md` (`:23`, `:93-95`),
  `data-model.md` (`:177`), `staging.md` (`:107`), `known-defects.md` (`:68`) and
  `current-state.md`: remove or rewrite every sentence about routes, the Routing
  tab or modes.
- `git grep -n "inference/routes\|Routing tab\|RoutingTab\|routing table\|WorkloadModelDialog" docs README.md -- ':!docs/key-reworks'`
  must print nothing afterwards, except the `ROUTES_KEY` mention you add to
  `docs/modules/inference/data-model.md` ("`inference/routes` is no longer read
  except by the 5a banner").
- Leave `docs/key-reworks/*` alone. Keep every file at 500 lines or fewer.

## 5. Must not touch

- `inference/managed/enabled`, `POST …/inference/managed/enabled`, its store
  functions and the Managed row's switch (dump item 3 is not handled). Only the
  parked-tier note goes.
- The managed credential chain and its fallbacks, `load_managed_key` and
  `managed_identity` (item 10).
- Entry zero (`inference/config`, `inference/key`) and `resolve_legacy_scoped`'s
  three steps.
- The legacy env default on `/openai/v1` (`OPENCOMPANY_INFERENCE_URL`).
- 5a's work: `routes_carry.rs`, the builder carry call, `routes_not_carried` in
  the status DTO, `routes-not-carried.ts`, `RoutesNotCarriedBanner.tsx`, and
  `store::ROUTES_KEY`.
- `resolve::primary`, 2c's default writes, `clear_default_if_marked`, and
  `frontend/src/inference/proxy-compat.ts`.
- The stored `inference/routes` value: never written, cleared or rewritten.

## 6. Done when

- The grep in part 1 §7 step 1 prints only `routes_carry.rs` and `ROUTES_KEY`,
  and the frontend grep in step 8 prints nothing.
- `cargo fmt --all -- --check` is clean. `npm run typecheck`,
  `npm run typecheck:unit` and `npm run typecheck:e2e` pass (name each), and
  `scripts/ci/assert-design-tokens.sh` passes.
- CI is verified by head SHA: `gh api repos/tinyhumansai/opencompany/commits/<sha>/check-runs`
  shows **zero failures and zero pending**, including both Console E2E lanes and
  the `Rust (openhuman, tinycortex)` lane that runs `auth_matrix`.
- In a real browser, light and dark, with screenshots: the LLM page shows no
  tab bar; `#/settings/inference?tab=routing` shows the providers; a provider
  row has no In use or Parked badge; a company with stored `managed` rows shows
  the 5a banner; the remove dialog names no routes.

## 7. Gotchas

1. **5a first.** Removing the readers without the carry and the banner strands
   companies whose only choice is in routes (the step-4 branch). Check that 5a is
   green by SHA before the first deletion.
2. **Managed-only companies go to echo.** That is intended: §5, row 1. The proof
   that it is not silent is the banner (`routesNotCarried`) plus `NO_MODEL_CHOSEN`
   on the turn path. Do not add a managed fallback to "fix" it; item 10 is
   removing fallbacks, not adding them.
3. **`managed` rows carry no model**, so nothing can be derived from them. Never
   write a `tinyhumans` default or row on a company's behalf.
4. **No writes on read paths.** Deleting `routing_table` must not become a
   "clean up the stale key" write. The store has no delete, and the banner needs
   the value.
5. **Spend moves.** Rows 2–7 of the §5 table change which account pays for some
   turns, on companies that already see the banner. This is decision Q14, not a
   regression; say it in the PR body with the table.
6. **Per-workload cost control is gone** and Q13 decided against a background
   model. Do not add one.
7. **`affected_tiers` is on the wire** (`affectedTiers`). Delete it on both sides
   in the same push.
8. **Members' fallback** (`use-inference.ts:130-143`, reading `status.routes` on
   a 403) goes with the routes state. Members now read only the status, which
   they could already.
9. **The auth matrix counts are relative.** Subtract from what the file says when
   you edit it, not from the `fcfb3e1bc` numbers.
10. **Unused parameters and imports fail clippy.** Grep for every symbol whose
    last caller you deleted (`legacy_hint`, `LEGACY_MANAGED`, `is_primary`,
    `categoryOf`, `resolve`, `cn`) before pushing, because clippy only runs on CI.
