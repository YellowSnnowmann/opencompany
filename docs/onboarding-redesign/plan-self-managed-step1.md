# Slice 4b — Self-managed step 1: mount Provider + Composio verbatim

## Architecture Impact

Two existing, independent connect flows get mounted a second place (inside
the wizard) in addition to their existing home (Connections pages). Neither
needs new backend work — both already work standalone. The work is entirely
in making them mountable outside their current single mount point cleanly.

## Files to Modify

- **Provider.** `frontend/src/inference/ProvidersTab.tsx` takes `client`,
  `company`, `state: InferenceState`, `actions: InferenceActions`,
  `canManage` as props — it does **not** own its own data fetching. Whatever
  today calls `useInference(client, company)` (find the hook's call site —
  likely the Connections → LLM page component) to produce `state`/`actions`
  needs an equivalent call inside the wizard's self-managed step 1. Do not
  assume `ProvidersTab` is a drop-in component; it is a controlled view over
  state its parent owns.
- **CAUTION — naming collision:** import `AddProviderDialog` and
  `ProviderConnectDialog` from `frontend/src/inference/`, **not**
  `frontend/src/search-providers/`. Both directories have files with these
  exact names; the search-providers ones are Search's own provider-add flow
  (unrelated to this slice). Get this wrong and it type-checks fine while
  building the wrong feature.
- **Composio.** `frontend/src/views/connections/ComposioSection.tsx`'s inline
  `Dialog` (`:811` on) plus `use-composio-credential.ts`'s
  `useComposioCredential` hook (`:90`). `ComposioSection` itself takes
  `client`, `company`, `canManage`, `onChanged` as props (confirmed — no
  hidden route/context dependency), which makes it more directly mountable
  than the Provider side. Consider mounting `ComposioSection` (or extracting
  just its dialog + hook if the full section renders too much
  Connections-page chrome for a wizard step) rather than reimplementing its
  submit logic.
- `frontend/src/views/setup/SetupWizard.tsx` — the Self-managed branch's
  step-1 component, hosting both mounted flows side by side with independent
  "set this up later" skip state for each.

## New Files

- The Self-managed step-1 wrapper component. Likely needs a thin
  `useInference`-instantiating wrapper around `ProvidersTab` if that hook
  isn't already easily reusable outside its current call site — check the
  hook's own dependencies (does it assume a route-derived `company`, or is
  it already parameterized cleanly?) before deciding whether this is a
  wrapper or a direct call.

## Dependencies

Slice 3 (branch point must exist).

## Implementation Steps

1. Find and read `useInference`'s definition and its current call site, to
   confirm it's safely callable a second time (a second mount, different
   `company` — though during onboarding `company` may be `null`/pending, see
   plan-managed-step1.md's scoping question, which applies here too: does
   `ProvidersTab`'s `POST …/inference/providers` call work pre-company-
   creation, same open question as 4a's `setCompanyCredential`).
2. Mount `ProvidersTab` + its dialogs inside the wizard's self-managed step
   1, wired to a locally-instantiated `useInference`.
3. Mount `ComposioSection`'s credential dialog (or the extracted
   dialog+hook) alongside it.
4. Add independent "set this up later" skip affordances for each — confirm
   against open-questions.md's Composio section whether Composio's own UI
   already has a clean "deferred" resting state or whether this needs new
   UI to represent "explicitly skipped" vs. "never tried."
5. Wire both into the step's `problem()` gate — likely no blocking gate at
   all (both are skippable), matching the design intent that this step
   never blocks Next.

## Testing Strategy

- A test confirming each flow's real endpoint fires when used inside the
  wizard (`POST …/inference/providers`, `setComposioApiKey`/
  `setComposioToken`) — not a new mock, the same assertions the existing
  Connections-page tests already make, just triggered from the wizard.
- A test confirming skipping one doesn't affect the other (independent
  state).
- A test confirming the pre-company-creation scoping question's resolution
  (same class of test as plan-managed-step1.md's, applied to Provider
  specifically).

## Risks and Edge Cases

- **The naming collision is the single easiest mistake in this entire
  plan.** Add an explicit code-review checklist item for it, or better,
  grep for the wrong import path in CI (`grep -r "search-providers/AddProviderDialog" frontend/src/views/setup/`
  returning non-empty should fail a lint step) rather than relying on review
  alone.
- **`ProvidersTab` not being a drop-in component** is easy to discover only
  after starting to wire it — confirmed here specifically so this doesn't
  cost the implementer a false start.
- **Composio's skip-for-later state** — open-questions.md flags this as
  unconfirmed; resolve it by reading the real Connections → Composio page's
  current empty/disconnected state before assuming the wizard can just
  reuse it unchanged.

## Developer Handoff

Read `useInference`'s definition first, before touching any wizard code —
it decides whether this slice is "mount two components" or "mount two
components plus a state-management wrapper." The Composio side is likely
the easier of the two given `ComposioSection`'s cleaner prop surface;
consider building and landing it first as a smaller, lower-risk PR before
tackling the Provider side's `useInference` wiring.
