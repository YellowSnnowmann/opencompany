# Phase 3b — agent pair editor: Provider + Model in Agent detail

Slice 3b of the keys rework (issue #2306). Written 2026-09-14 against
`upstream/main @ fcfb3e1bc`.

**Starts only after:** slice 3a is committed on `feat/key-reworks` (the route
accepts `provider`, the DTO returns it) and slice 2c's status fields
(`defaultChoice`, `providers[].model`, `providers[].modelAmbiguous`) exist.
PR #2305 was closed on 2026-09-14; nothing here depends on it. Slices 2a–2d may
have changed `ModelField.tsx` and `api/inference.ts`: re-find every line below
by its excerpt.

## 1. Goal

In Agent detail → "Harness & model", an admin picks a Provider and a Model for
an agent on a built-in harness, or returns it to "Company default · <provider> ·
<model>", saved in one `PATCH`.

- Dump items **13(c)**, **19**; decisions **Q5**, **F6**; README goal 3.
- Operator's examples: researcher → `anthropic` / `test-model-large`; web search →
  `anthropic` / `test-model-small` (a larger model for research, a smaller one
  for search, as the operator described, item 19); writer untouched →
  "Company default · …".

## 2. Files (with `fcfb3e1bc` lines)

| File | Where | Change |
|---|---|---|
| `frontend/src/api/types.ts` | `AgentDetailDto.harness` `:1386`, `.model` `:1394`; `EditAgentInput.model` `:1495`, `.harness` `:1497` | add `provider` |
| `frontend/src/lib/agent.ts` | `modelEdit` `:219-224`; `harnessEdit` `:232-236` | add `providerEdit`, `pairLabel`, `companyDefaultLabel` |
| `frontend/src/views/team/AgentDetailView.tsx` | `HARNESS_DEFAULT` `:92`; `modelDraft` `:297`; draft reset `:374`; status effect `:317-335`; open editor `:1054`; cancel `:1094`; props `:1099`; `saveHarnessAndModel` `:596-630`; `HarnessAndModel` `:1628-1859` (select `:1739`, ACP branch `:1753`, non-ACP text `:1824-1827`, view badges `:1839-1855`) | Provider select, ModelField, badges |
| `frontend/src/inference/ModelField.tsx` | props `:86-104` | reused; see §4.4 |
| `frontend/src/inference/types.ts` | `Provider` `:34-78` | read only |
| `frontend/src/api/inference.ts` | `InferenceStatus.providers` `:176`; `getInferenceStatus` `:326`; `listProviderModels` `:621-629` | read only |
| `frontend/test/unit/agent-detail.test.ts` | pure-function tests, `modelEdit` block `:182-201` | extend |
| `frontend/test/unit/agent-model-pair.test.ts` | **new** | component tests |
| `frontend/test/e2e/agent-detail.spec.ts` | tests `:92`, `:157`, `:170` | add one |

Backend reads used: `GET {scope}/inference` is `access=scoped` (member readable,
`tests/snapshots/auth-matrix.txt:1277`); `GET {scope}/inference/providers/{slug}/models`
is `ScopedCompany` (`src/server/ops/inference/providers.rs:88,103`).

## 3. Current code

`HarnessAndModel`, non-ACP branch (`AgentDetailView.tsx:1823-1828`):

```tsx
          ) : (
            <p className="text-xs text-muted-foreground">
              A model override only applies on an ACP harness — pick one above to set one.
            </p>
          )}
```

View mode (`:1844-1856`):

```tsx
          {agent.model ? (
            <Badge variant="secondary" className="gap-1 font-mono text-xs" data-testid="agent-model">
              <Cpu className="size-3" /> {agent.model}
            </Badge>
          ) : (
            <span className="text-sm text-muted-foreground" data-testid="agent-model-empty">
              {declaredKind === "acp"
                ? "No model override set — uses the harness's own default."
                : "No model override (this harness has no ACP transport to steer)."}
```

`saveHarnessAndModel` (`:596-606`):

```tsx
    const harness = harnessEdit(agent.harness, harnessDraft === HARNESS_DEFAULT ? "" : harnessDraft);
    const model = modelEdit(agent.model, modelDraft);
    if (harness === undefined && model === undefined) {
      setEditingHarness(false);
      return;
    }
    const edits: EditAgentInput = {};
    if (harness !== undefined) edits.harness = harness;
    if (model !== undefined) edits.model = model;
```

The status effect loads `getInferenceStatus` only while the persona editor is
open (`:317-335`), for `cognition`.

## 4. Target code

### 4.1 Types (`api/types.ts`)

`AgentDetailDto`, after `model`:

```ts
  /**
   * The provider half of this teammate's own `{provider, model}` pair (keys
   * rework, issue #2306). Set only together with `model`, and only on a
   * built-in harness; `undefined` means the teammate uses the company default.
   */
  provider?: string;
```

Rewrite the `model` doc: "On an ACP harness, the model hint forwarded to it; on
a built-in harness, the model half of the pair with `provider`."

`EditAgentInput`, after `model`:

```ts
  /**
   * The provider half of the pair. Same three states as `model`: absent leaves
   * it, `null` clears it (company default), a slug sets it. Always sent together
   * with `model`. Admin-only on the host (`403` for a member).
   */
  provider?: string | null;
```

### 4.2 Helpers (`lib/agent.ts`)

```ts
/**
 * The `PATCH` value for the provider half of an agent pair, or `undefined` when
 * the draft did not change. A select's value, so no trim — the sibling of
 * [`harnessEdit`].
 */
export function providerEdit(current: string | undefined, draft: string): string | null | undefined {
  const before = current ?? "";
  if (draft === before) return undefined;
  return draft === "" ? null : draft;
}

/** "Company default · <provider> · <model>", or the not-chosen form. */
export function companyDefaultLabel(
  choice: { provider: string; model?: string | null } | null | undefined,
  providers: Pick<Provider, "slug" | "label">[],
): string {
  if (!choice?.provider || !choice.model) return "Company default · none chosen";
  const label = providers.find((p) => p.slug === choice.provider)?.label ?? choice.provider;
  return `Company default · ${label} · ${choice.model}`;
}

/** "<provider label> · <model>" for a pinned agent. */
export function pairLabel(provider: string, model: string, providers: Pick<Provider, "slug" | "label">[]): string {
  return `${providers.find((p) => p.slug === provider)?.label ?? provider} · ${model}`;
}
```

Use the real `defaultChoice` type slice 2c exported instead of the inline shape
if it differs (keep the copy; adapt only the field reads). "none chosen" is this
brief's wording for a company with no full default; the operator gave only the
full form.

### 4.3 View state (`AgentDetailView.tsx`)

1. `const PROVIDER_COMPANY_DEFAULT = "__company_default__";` beside `:102`.
2. State beside `modelDraft` (`:297`):
   `const [providerDraft, setProviderDraft] = useState("");`
   `const [inference, setInference] = useState<InferenceStatus | null>(null);`
3. Reset `setProviderDraft("")` wherever `setModelDraft("")` resets (`:374`, `:1094`).
4. Opening the editor (`:1054`): `setProviderDraft(agent.provider ?? "")` next to
   `setModelDraft(agent.model ?? "")`.
5. A new effect, keyed on `[client, company, agentId]` (not on `editing`),
   loading `getInferenceStatus` once into `inference`; on error set `null`.
   Leave the `:317-335` cognition effect alone.
6. Harness change handler passed as `onHarnessChange`: when the drafted harness
   kind changes between `acp` and non-`acp`, also `setProviderDraft("")` and
   `setModelDraft("")` — unless the new kind equals the declared kind, in which
   case restore `agent.provider ?? ""` / `agent.model ?? ""`. An ACP model id is
   not a pair model, and a pair is refused on ACP (3a G9).
7. `saveHarnessAndModel`:

   ```tsx
    const harness = harnessEdit(agent.harness, harnessDraft === HARNESS_DEFAULT ? "" : harnessDraft);
    const draftKind = resolvedHarnessKind(harnesses, harnessDraft === HARNESS_DEFAULT ? undefined : harnessDraft);
    const edits: EditAgentInput = {};
    if (harness !== undefined) edits.harness = harness;
    if (draftKind === "acp") {
      const model = modelEdit(agent.model, modelDraft);
      if (model !== undefined) edits.model = model;
      if (agent.provider) edits.provider = null;
    } else {
      const provider = providerEdit(agent.provider, providerDraft);
      const model = modelEdit(agent.model, providerDraft === "" ? "" : modelDraft);
      if (provider !== undefined || model !== undefined) {
        // One PATCH carrying both halves, so the host checks the pair it will store.
        edits.provider = providerDraft === "" ? null : providerDraft;
        edits.model = providerDraft === "" ? null : modelDraft.trim();
      }
    }
    if (Object.keys(edits).length === 0) {
      setEditingHarness(false);
      return;
    }
   ```

   The rest (saving flag, stale-agent guard, `updateAgent`, toast on error)
   stays. A `400` message from the host is shown verbatim in the existing error
   toast.
8. Pass to `HarnessAndModel` (`:1099`): `client`, `company`, `providerDraft`,
   `onProviderChange={setProviderDraft}`,
   `providers={(inference?.providers ?? []).filter((p) => p.enabled)}`,
   `defaultChoice={inference?.defaultChoice ?? null}`.

### 4.4 `HarnessAndModel` JSX outline

New props: `client: OpenCompanyClient; company: string | null; providerDraft: string;
onProviderChange: (v: string) => void; providers: Provider[]; defaultChoice: DefaultChoiceDto | null;`.
`editable` becomes `["harness","model","provider"].some((f) => agent.editable.includes(f))`.
Subtitle: "Which engine this agent runs on, and which provider and model it uses."

Replace the non-ACP `<p>` (`:1824-1827`) with:

```tsx
            <div className="space-y-3" data-testid="agent-pair-editor">
              <div className="space-y-1.5">
                <Label htmlFor="agent-provider-select">Provider</Label>
                <Select
                  value={providerDraft === "" ? PROVIDER_COMPANY_DEFAULT : providerDraft}
                  onValueChange={(v) => {
                    const next = !v || v === PROVIDER_COMPANY_DEFAULT ? "" : v;
                    onProviderChange(next);
                    // Prefill with that row's own model; the operator may pick another.
                    onModelChange(next ? (providers.find((p) => p.slug === next)?.model ?? "") : "");
                  }}
                >
                  <SelectTrigger id="agent-provider-select" className="w-full" data-testid="agent-provider-select">
                    <SelectValue>{providerTriggerLabel}</SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value={PROVIDER_COMPANY_DEFAULT}>
                      {companyDefaultLabel(defaultChoice, providers)}
                    </SelectItem>
                    {providers.map((p) => (
                      <SelectItem key={p.slug} value={p.slug}>
                        {p.label}
                        {p.model && <span className="text-muted-foreground"> · {p.model}</span>}
                      </SelectItem>
                    ))}
                    {unlistedProvider && (
                      <SelectItem value={unlistedProvider}>
                        {unlistedProvider}
                        <span className="text-muted-foreground"> — not available</span>
                      </SelectItem>
                    )}
                  </SelectContent>
                </Select>
              </div>
              {providerDraft !== "" && (
                <div className="space-y-1.5" data-testid="agent-model-field">
                  <Label htmlFor="agent-model-field-input">Model</Label>
                  <ModelField
                    client={client}
                    company={company}
                    slug={providerDraft}
                    id="agent-model-field-input"
                    value={modelDraft}
                    disabled={saving}
                    onChange={onModelChange}
                  />
                  {modelDraft.trim() === "" && (
                    <p className="text-xs text-status-blocked-text" data-testid="agent-pair-model-required">
                      Choose a model for this provider.
                    </p>
                  )}
                </div>
              )}
              {(providerDraft !== "" || agent.provider) && (
                <Button variant="ghost" size="sm" onClick={() => { onProviderChange(""); onModelChange(""); }}
                  data-testid="agent-pair-clear">
                  Use company default
                </Button>
              )}
            </div>
```

- `providerTriggerLabel` is the render-function form (same reason as
  `harnessLabel`, `:1716-1722`): default sentinel → `companyDefaultLabel(…)`,
  a slug → that row's `label`, else the slug.
- `unlistedProvider = agent.provider && !providers.some((p) => p.slug === agent.provider) ? agent.provider : undefined`
  — a pair naming a deleted or switched-off provider stays visible, never
  silently dropped (F6).
- Options: enabled rows of `GET …/inference` `providers[]`. A row whose `model`
  is absent (none or ambiguous, 2c) is still listed; its model field starts blank.
- Save button (`agent-harness-save`, `:1832`): `disabled={saving || (draftKind !== "acp" && providerDraft !== "" && modelDraft.trim() === "")}`.
- ModelField: if, after 2c/2d, it still renders a blank "send the tier" choice
  (`TIER_DEFAULT_LABEL`, `ModelField.tsx:15`), add one optional prop
  `required?: boolean` (default `false`) that hides that choice, and pass
  `required`. Change nothing else in `ModelField`.
- ACP branch (`:1753-1822`) is unchanged: the ACP harness still shows its own
  picker (`agent-model-select` / `agent-model-input`).

View mode (`:1839-1856`): keep `agent-harness`. Then:

```tsx
          {declaredKind === "acp" ? (
            /* today's agent-model badge / agent-model-empty for ACP, unchanged */
          ) : agent.provider && agent.model ? (
            <Badge variant="secondary" className="gap-1 font-mono text-xs" data-testid="agent-pair-badge">
              <Cpu className="size-3" /> {pairLabel(agent.provider, agent.model, providers)}
            </Badge>
          ) : (
            <span className="text-sm text-muted-foreground" data-testid="agent-pair-default">
              {companyDefaultLabel(defaultChoice, providers)}
            </span>
          )}
```

For a member (no `provider` in `editable`), the view is the same; if the status
read failed, `providers` is empty and the labels fall back to slugs.

Design tokens only: `text-muted-foreground`, `text-status-blocked-text`, the
existing `Badge`/`Button`/`Select`/`Label` components. No raw hex (checked by
`scripts/ci/assert-design-tokens.sh`).

## 5. Ordered edit list

1. `api/types.ts`: `AgentDetailDto.provider`, `EditAgentInput.provider`, docs.
2. `lib/agent.ts`: `providerEdit`, `companyDefaultLabel`, `pairLabel` (import
   `Provider` type from `@/inference/types`).
3. `AgentDetailView.tsx`: constant, state, resets, status effect (§4.3 1–5).
4. Harness-change handler and `saveHarnessAndModel` (§4.3 6–7).
5. `HarnessAndModel` props, editor branch, view badges (§4.3 8, §4.4).
6. `ModelField.tsx` `required` prop only if needed (§4.4).
7. Unit tests, then e2e (§7). Run the three typecheck gates and the token script.
8. Commit, push, CI by head SHA, then browser verification (§10).

## 6. Data carry-over

None. The console only writes through the 3a route; an agent with no `provider`
renders "Company default · …". See 3a part 2 §6 for the stored shapes.

## 7. Tests

**`frontend/test/unit/agent-detail.test.ts`** (pure functions, beside `:182`)
- `describe("what a provider edit sends")`: `providerEdit(undefined, "")` →
  `undefined`; `providerEdit("anthropic", "anthropic")` → `undefined`;
  `providerEdit(undefined, "anthropic")` → `"anthropic"`; `providerEdit("anthropic", "")` → `null`.
- `companyDefaultLabel({provider:"openrouter",model:"acme/test-model"}, [{slug:"openrouter",label:"OpenRouter"}])`
  → `"Company default · OpenRouter · acme/test-model"`; `null` → `"Company default · none chosen"`;
  unknown slug → slug used.
- `pairLabel("anthropic","test-model-large",[{slug:"anthropic",label:"Anthropic"}])` → `"Anthropic · test-model-large"`.

**New `frontend/test/unit/agent-model-pair.test.ts`** — render `AgentDetailView`
the way `agent-detail-instructions.test.ts` does (`createRoot` + `act`, a stub
`OpenCompanyClient` whose `get` answers `…/team/{id}`, `…/harnesses`,
`…/inference` with `providers:[{slug:"anthropic",label:"Anthropic",enabled:true,model:"test-model-small",…},{slug:"groq",enabled:false,…}]`
and `defaultChoice:{provider:"openrouter",model:"acme/test-model"}`, and
`…/providers/anthropic/models` with a two-id catalog).
- `a writer with no pair shows the company default` — `agent-pair-default` text
  is `Company default · openrouter · acme/test-model`.
- `the researcher pinned to anthropic shows the pair badge` — agent
  `{provider:"anthropic",model:"test-model-large"}` → `agent-pair-badge` text
  `Anthropic · test-model-large`.
- `only enabled providers are offered` — open editor; `groq` absent.
- `saving a pair sends provider and model in one request` — pick `anthropic`,
  model `test-model-large`, click `agent-harness-save`; the stub's `patch` was
  called once with `{provider:"anthropic",model:"test-model-large"}`.
- `clearing sends null for both` — pinned agent, click `agent-pair-clear`, save
  → `{provider:null,model:null}`.
- `a provider with no model blocks save` — pick a provider, empty the model →
  `agent-pair-model-required` visible, save disabled.
- `the ACP harness still shows the ACP picker` — `ACP` harness drafted →
  `agent-provider-select` absent, `agent-model-select` or `agent-model-input` present.
- `a pair naming a gone provider stays visible` — agent `provider:"gone"` →
  the trigger shows `gone`.

**E2E `frontend/test/e2e/agent-detail.spec.ts`** — add
`test("an admin pins an agent to a provider and model, then clears it")`:
1. Setup via `page.request` (as `composio-account-choice.spec.ts:70` does):
   create an `openai_compatible` provider row with slug `e2e-pair`, base URL
   `http://127.0.0.1:9/v1`, key `sk-not-a-real-key`, model `e2e-model`, using the
   add body slice 2c defines. Nothing calls the endpoint in this test.
2. Add a teammate through the dialog as the `:170` test does; open "Harness & model".
3. Select `e2e-pair` in `agent-provider-select`; the catalog read fails, so
   `ModelField` shows text; fill `e2e-model-2`; save.
4. Expect `agent-pair-badge` to contain `e2e-model-2`; reload; still there.
5. Edit, click `agent-pair-clear`, save; expect `agent-pair-default` visible.
6. `finally`: delete the teammate and the `e2e-pair` provider.

## 8. Console / UI summary

| Element | `data-testid` | Copy |
|---|---|---|
| Provider select | `agent-provider-select` | label "Provider"; first item "Company default · <provider> · <model>" |
| Model field wrapper | `agent-model-field` | label "Model" |
| Missing model note | `agent-pair-model-required` | "Choose a model for this provider." |
| Clear | `agent-pair-clear` | "Use company default" |
| View: pinned | `agent-pair-badge` | "<provider label> · <model>" |
| View: default | `agent-pair-default` | "Company default · <provider> · <model>" |

Unchanged: `agent-harness-edit`, `agent-harness-select`, `agent-harness-save`,
`agent-harness`, `agent-model-select`, `agent-model-input`, `agent-model`,
`agent-model-empty` (ACP only now).

## 9. Must not touch

- The backend (3a owns it), the inference page, `ProvidersTab`, routing UI.
- The ACP picker branch and `ensureAcpModels` / `cachedAcpModels`.
- `ModelField` beyond the optional `required` prop.
- `[harness.inference]` behaviour; internal passes (Q13); routes.
- The persona-editor cognition effect (`:317-335`).

## 10. Done when

- `npm run typecheck`, `npm run typecheck:unit`, `npm run typecheck:e2e` each
  pass (name all three in the commit report); `scripts/ci/assert-design-tokens.sh` passes.
- Pushed; CI by head SHA (`gh api repos/tinyhumansai/opencompany/commits/<sha>/check-runs`):
  zero failures and zero pending, both Console E2E lanes finished.
- Seen in a real browser, light **and** dark, with screenshots of: Team roster
  with 15 agents; agent detail for a pinned agent (badge), an unpinned agent
  (company default line), the editor open on a provider whose catalog has at
  least `MODEL_LIMIT = 50` ids (`frontend/src/inference/model-filter.ts:26`,
  capped note visible), and the ACP harness editor. Claim a port with
  `local/wt ports --take` and verify with `--verify` during the run.

## 11. Gotchas

- **A pair needs both halves.** Never send `provider` without `model`; the host
  400s. Clearing sends `null` for both.
- **Kind switches** must reset the drafts (§4.3 6), or an ACP model id is sent as
  a pair model, or a pair is sent to an ACP harness (3a G9).
- **Typo slugs exist only from manifests** (the select cannot produce one); such
  an agent fails closed at its first turn. Show it as "not available", do not hide it.
- **No catalog assumptions.** The model picker lists exactly what the chosen
  provider's model list returns; never filter, prefer or reject an id by vendor
  or name.
- **Disabled or deleted provider** named by a pair: the badge still shows the
  pair; turns fail naming the provider; no fallback (F6).
- **`GET …/inference` may be slow**; the section renders with slugs until it
  lands, never blocks the agent page (same rule as `:317-320`).
- **Roster cap.** No 15-agent cap exists in code on `fcfb3e1bc` (setup's
  `MAX_AGENTS` is 6, `src/company/setup.rs:555`); 15 is the operator's stated
  cap for the screenshot. Seed 15 teammates through the add dialog or API.
