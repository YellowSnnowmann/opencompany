# Phase 2a — TinyHumans on the proxy, part 2

Parts: [1](phase-2a-tinyhumans-on-proxy.md) · [2](phase-2a-tinyhumans-on-proxy-part2.md) · [3](phase-2a-tinyhumans-on-proxy-part3.md).

This continues part 1; section numbers carry on from it. `file:line` references
are on `upstream/main @ fcfb3e1bc` (2026-09-14).

**Fixtures.** Model ids in tests and examples are fake ids served by a mock
catalog (`acme/test-model`, `acme/other-model`). The key is `th-not-a-real-key`.

### 5.6 `providers.rs`: key and model required; restore a replaced key

**(a) Key required.** In `plan_add`'s cloud branch (:717), before
`return Ok(AddPlan {…})`:

```rust
if cloud.slug == crate::company::inference::MANAGED_SLUG && !has_key {
    return Err(invalid("TinyHumans needs an API key.".to_string()));
}
```

**(b) Model required,** independent of catalog content. In `add_provider`,
directly after `plan_add(…)?` (:333) and before any read or write:

```rust
if plan.slug == crate::company::inference::MANAGED_SLUG && asked_model.is_none() {
    return Err(ApiError(OpenCompanyError::InvalidRequest(
        "Choose a model for TinyHumans.".to_string())));
}
```

**(c) Duplicates:** no code change. `:343-348` already answers "TinyHumans is
already connected. Edit the existing row rather than adding a second one." for
an indexed `tinyhumans` row and for a managed entry zero, before any write.

**(d) Restore a replaced key.** `provider/tinyhumans/key` can hold the legacy
Managed row's key with no record behind it, and every rollback clears that slot
(`roll_back_add` → `store::delete_provider` :839; `clear_orphaned_key`
:857-863). So:

1. Read the old value after the duplicate check (:348):

```rust
let previous_key = if plan.slug == crate::company::inference::MANAGED_SLUG {
    secrets.get(runtime.id(), &store::provider_key_key(&plan.slug)).await.map_err(ApiError)?
        .map(|crate::ports::types::SecretValue(raw)| raw)
        .filter(|raw| !raw.trim().is_empty())
} else { None };
```

2. Add a helper beside `clear_orphaned_key`:

```rust
/// Puts back a key that an add replaced and then rolled back. `None` does nothing.
async fn restore_previous_key(runtime: &CompanyRuntime, slug: &str, previous: Option<&str>) {
    let Some(previous) = previous else { return };
    if let Err(err) = runtime.secrets().set(runtime.id(), &store::provider_key_key(slug),
        crate::ports::types::SecretValue(previous.to_string())).await {
        tracing::error!(company = %runtime.id(), provider = %slug, error = %err,
            "could not restore the key a rolled-back add had replaced");
    }
    crate::server::inference_models::evict_company_catalogs(runtime.id().as_ref());
}
```

3. Call `restore_previous_key(runtime, &plan.slug, previous_key.as_deref()).await;`
   immediately after each rollback: after `clear_orphaned_key` (:390), after
   `roll_back_add` (:443), and after `roll_back_add` (:481).

**(e) Health:** no code change. With (a), `worth_probing` is always true
(:409), so health is recorded as `"ok"` (:452) or as the failure class (:489).

### 5.7 `ops/inference.rs`: exactly one row on the wire

Add to `ManagedDto` (:366-390), after `configured`:

```rust
    /// Whether the console renders the separate legacy Managed row. False when
    /// `providers` already lists slug `tinyhumans` (an added row, or entry zero on a
    /// managed config): that row is the one TinyHumans row.
    legacy_row: bool,
```

In `managed_state` (:977), set `legacy_row: source.resolves(),`. In
`effective_status_with`, change `let managed` (:878) to `let mut managed` and
add:

```rust
managed.legacy_row = managed.configured
    && !providers.iter().any(|p| p.slug == inference::MANAGED_SLUG);
```

`managed_resolves` (:936) and `configured` do not change; routing still asks
whether the chain resolves.

### 5.8 Managed Test after the change

`test_managed` keeps its credential chain (:1515-1542) and its health slot
`inference/health["tinyhumans"]`. Only the shape argument (§5.5) is new. It
probes:

- **before 3b:** `…/openai/v1/models`;
- **after 3b:** `…/agent-integrations/openrouter/models?limit=500&offset=0`,
  paged;
- **with `OPENCOMPANY_INFERENCE_URL` set:** that URL, in the shape its path
  implies.

### 5.9 Step 3b (GATED, §0): the two constants

| `file:line` | Change |
|---|---|
| `src/company/inference.rs:152` | `pub const PLATFORM_BASE_URL: &str = "https://api.tinyhumans.ai/agent-integrations/openrouter";`; doc (:146-151): "The TinyHumans OpenRouter proxy — the legacy managed chain's endpoint." |
| `src/harness/built_in/provider.rs:55` | `pub const DEFAULT_TINYHUMANS_INFERENCE_URL: &str = "https://api.tinyhumans.ai/agent-integrations/openrouter";` |
| `docs/spec/runtime/config.md:122`, `docs/modules/openhuman/README.md:81` | default URL column |

**No edit needed:**

- **Uses that follow the constant:** `inference.rs:661`, `:682`;
  `provider.rs:193`; `roster_build.rs:194`; `ops/inference.rs:857`, `:862`,
  `:983`; `providers.rs:1547`.
- **Tests that name the constant:** `inference.rs:2808`, `:2991`, `:3665`;
  `provider.rs:2432`, `:4437`; `ops/inference.rs:2499`, `:2569`, `:2939`.
- **Literals that stay:**
  - staging and fixtures: `provider.rs:2443/2449`, `ops/inference.rs:2462`,
    `setup-wizard-finish-gate.test.ts:323/431/465/525`;
  - backend error text: `provider.rs:1749/1757/5368-5535`;
  - non-OpenRouter host tests: `catalogue.rs:1992/2016`;
  - comments: `cost.rs:87/229`, `metering/inference.rs:65`,
    `ports/types.rs:2801`, `run_trace.rs:259`, `provider.rs:623/1205`.

**Never rewrite the injected URL.** `OPENCOMPANY_INFERENCE_URL` stays
`env.get(…)` verbatim (`provider.rs:190-192`, `roster_build.rs:192-194`). Hosted
tenants run on whatever the manager injects, so that is a manager-side
prerequisite (part 3 §12.1).

**Response parser.** `model_response_from_payload` (`provider.rs:953`) reads
`/choices/0/…` and `usage` through JSON pointers, so extra top-level keys
(`openhuman`, `service_tier`) and extra `usage` keys (`cost`, `is_byok`) are
ignored. The turn path does not stream: `send_plan` (`provider.rs:1843`) reads
one JSON body, and `provider.rs` has no `"stream"` request field. A final SSE
frame without `choices` never reaches this parser; say so in the PR. The parser
test is in part 3 §8.1.

## 5.10 Target code (console)

### `frontend/src/inference/catalogue.ts`

Append as the **last** `CLOUD_PROVIDERS` entry, after `modelscope` (:229-235).
Follow the rules at :11-23: fields in order, double quotes, no comments inside
an entry.

```ts
  {
    slug: "tinyhumans",
    label: "TinyHumans",
    endpoint: "https://api.tinyhumans.ai/agent-integrations/openrouter",
    auth: "bearer",
    keyPlaceholder: "th-...",
  },
```

Comments:

- `:53`: "26" becomes "27".
- `:448-456`: add "`tinyhumans` is also a cloud row now; it stays here so
  reservation does not depend on the table."

`INTERNAL_SLUGS` (:457) is unchanged.

### `frontend/src/inference/connect.ts`

1. **Delete** `offersManaged` (:127-154).
2. **Delete** the `managedEntry` block in `addOptions` (:172-182), and change the
   signature to `addOptions(providers: readonly Provider[]): AddOptions`.
   `cloud` becomes `CLOUD_PROVIDERS.filter((p) => !isConnected(providers, p.slug)).map(…)`
   with the same mapping; `local` and `cli` are unchanged. Add to the doc:
   "TinyHumans is offered once, as its catalogue row, and hidden once any row
   has slug `tinyhumans` (an added row or entry zero)."
3. **Delete** `if (optionSlug === MANAGED_OPTION_SLUG) return null;` (:254).
4. **Delete** the managed branch of `credentialAsk` (:295-305). Items 3 and 4
   can no longer be reached: `cloudProvider("tinyhumans")` matches first, giving
   title "Connect TinyHumans", `needsKey: true`, `needsEndpoint: false`,
   `keyPlaceholder: "th-..."`.
5. **Keep** `MANAGED_OPTION_SLUG = "tinyhumans"` (:125). Its doc becomes: "The
   TinyHumans catalogue slug; also the legacy Managed row's slug." Remove the
   `ManagedState` import (:24) if it is now unused.
6. **Add:**

```ts
/**
 * Whether the connect dialog asks for a model before adding. TinyHumans always
 * asks, whatever its catalog contains and whether or not the probe succeeded (a
 * failed probe leaves a free-text field). Other kinds ask when the probe says
 * `needsModel`. Slice 2c makes this unconditional.
 */
export function asksForModel(kind: string, probe: { ok: boolean; needsModel?: boolean }): boolean {
  return kind === MANAGED_OPTION_SLUG || (probe.ok && probe.needsModel === true);
}
```

### `frontend/src/inference/AddProviderDialog.tsx`

Remove the `managed` prop (:62, :68-69) and its import (:24). Call
`addOptions(providers)` (:73).

### `frontend/src/api/inference.ts`

Add to `ManagedState` (:210-236), after `configured`:

```ts
  /**
   * Whether the separate legacy Managed row renders. False when `providers`
   * already lists slug `tinyhumans`. Optional: an older host does not send it,
   * and absent reads as `configured`.
   */
  legacyRow?: boolean;
```

### `frontend/src/inference/ProviderList.tsx`

Add below `MANAGED_SLUG` (:68). Then replace `{managed?.configured && (` (:253)
with `{managed && showsLegacyManagedRow(managed) && (`. The empty-state gate
(:217) stays as it is.

```ts
/** Whether the legacy Managed row renders: its chain resolves and no row is listed for it. */
export function showsLegacyManagedRow(managed: ManagedState | undefined): boolean {
  return managed?.configured === true && managed.legacyRow !== false;
}
```

### `frontend/src/inference/ProvidersTab.tsx`

1. **Delete** the
   `else if (draft.kind === MANAGED_OPTION_SLUG) { await actions.saveManagedKey(…) }`
   branch (:219-222).
2. **Replace** the probe block (:229-242), importing `asksForModel` from
   `./connect`:

```tsx
if (!draft.model && !modelAsk) {
  const url = probeEndpoint(draft.kind, draft.baseUrl);
  if (url) {
    const probe = await actions.probeDraftEndpoint({ baseUrl: url, key: draft.key, kind: draft.kind });
    if (asksForModel(draft.kind, probe)) {
      setModelAsk({ models: probe.ok ? (probe.models ?? []) : [] });
      return;
    }
  }
}
```

3. **Remove** `managed={state.status?.managed}` from `<AddProviderDialog>`
   (:389).
4. **Pass** this to `<ProviderConnectDialog>` (:400-411):
   `replacesKey={connecting === MANAGED_OPTION_SLUG && editing === null && state.status?.managed?.source === "provider_key"}`.

These stay:

- `onManagedReplaceKey` (:342-345) still opens `MANAGED_OPTION_SLUG`. That is
  now the TinyHumans catalogue add, so "Add a key" on the legacy row turns it
  into a normal row.
- `onManagedRemoveKey`, `onManagedToggle`, `onManagedTest` and the managed
  `RemoveProviderDialog` (:455-474) still call the legacy routes.

### `frontend/src/inference/ProviderConnectDialog.tsx`

Add the prop `replacesKey?: boolean` (props ~:100-133, default `false`).
Directly under the key `<Input>` in the `ask.needsKey` block (:281-296), render:

```tsx
{replacesKey && (
  <p className="text-xs text-muted-foreground" data-testid="inference-connect-replaces-key">
    A key is already saved for Managed. Connecting TinyHumans replaces it with this one.
  </p>
)}
```

Keep the "Or connect your TinyHumans account" block (:345-361). Keep the model
field (:305-336) as it is: it lists exactly `modelAsk.models` and accepts any
typed id.

## 6. Ordered edit list

One commit per row, in order. Run `cargo fmt --all -- --check` before each Rust
commit and the three frontend typecheck gates before each console commit. Push
after each commit.

| # | Commit subject | Sections | Tests (part 3 §8) |
|---|---|---|---|
| 1 | `Add a paged catalog parser for the TinyHumans proxy` | 5.2 | 8.1 `paged_catalog.rs` |
| 2 | `Read model catalogs in the shape each provider publishes` | 5.1 (enum, const, fn), 5.3, 5.4, 5.5 | 8.1 shape, probe, `inference_models` |
| 3 | `Add TinyHumans to the provider catalogue` | 5.1 row and docs, 5.6, `catalogue.ts` row, `docs/modules/inference/catalogue.md` | 8.1 catalogue, add refusals, restore, resolution, parser; 8.2 catalogue |
| 4 | `Show exactly one TinyHumans row on the LLM page` | 5.7, 5.10 | 8.1 status; 8.2 managed-row, connect, one-row; 8.3 |
| 5 | **GATED:** `Point the managed endpoint constants at the OpenRouter proxy` | 5.9 | 8.1 3b check |

For `docs/modules/inference/catalogue.md` in commit 3:

- `:19`: "26" becomes "27".
- Add row 27 to the table at `:50`: `tinyhumans`, TinyHumans, bearer, `th-...`,
  the proxy URL.
- `:52-56`: TinyHumans is a row with a paged catalog; `openhuman` is still not
  ported.

After commit 4, raise the PR as a draft, unless part 3 §12.1 has been answered.

## 7. Data carry-over

- **Stored keys: none.** Nothing is copied, renamed or cleared at boot, and no
  key is created.
- **A legacy Managed key at `provider/tinyhumans/key`** keeps its row and its key
  until TinyHumans is added.
- **Adding TinyHumans needs the key typed in the dialog, like any provider.** It
  overwrites `provider/tinyhumans/key` (`providers.rs:352-362`), and the dialog
  says so first (`inference-connect-replaces-key`). On success the record is
  written, `legacyRow` becomes false, and one row remains. On rollback,
  §5.6 (d) restores the old key.
- **Why the stored key is not reused.**
  - The host never returns a credential to the console. Reuse would need a
    host-side copy route, which item 12 owns.
  - Item 14 asks for "add the key, pick the model".
  - The slot is one address, so after the overwrite the legacy chain (explicit
    `managed` routes) and the row present the same key.
- **Entry zero `inference/config = {provider: managed}`** is untouched: the add
  is refused, and entry zero stays the one TinyHumans row.
- **Health:** `inference/health["tinyhumans"]` is one slot, written by both the
  legacy Managed Test and the row. Only one of those rows renders.

---

Parts: [1](phase-2a-tinyhumans-on-proxy.md) · [2](phase-2a-tinyhumans-on-proxy-part2.md) · [3](phase-2a-tinyhumans-on-proxy-part3.md). Next: [part 3](phase-2a-tinyhumans-on-proxy-part3.md), from §8.
