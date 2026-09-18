# Slice 4a — Managed step 1: reuse the real fan-out

**Scope note:** an earlier draft of this plan also included a net-new
"Login with TinyHumans" one-click grant button (calling `link/start`,
catching the redirect). The operator descoped that explicitly — the grant
machinery (`link/start`, `useRedeemKeyGrant`, `pending-key-link.ts`) is real
and working today, but nothing in the console calls it, and this
implementation is **not** adding a new entry point for it. It stays exactly
as reachable as it is today. If that work resumes later, the
grant-landing-relocation notes that used to live in this file and in
`open-questions.md` are the starting point.

With that cut, this slice is one thing: reuse.

## Architecture Impact

The wizard's paste-a-key path stops calling `company::inference::store_key`
and instead calls `setCompanyCredential`/`setCompanyCredentialModel` — the
exact functions `ApiKeyView.tsx`'s `write()`/`writeModel()` already call.
Managed step 1 is today's real "Connect to TinyHumans" dialog (API-key
input, an external "Get an API key ↗" link to the TinyHumans dashboard, the
"saving also adds this key to the LLM page… and connects it for Composio"
copy, Cancel/Save key), mounted inside the wizard as-is. This is a straight
swap of which endpoint a form submit hits; no new UI shape.

## Files to Modify

- `frontend/src/views/setup/SetupWizard.tsx` — the Managed branch's step-1
  component (new, replacing `PowerStep`'s TinyHumans-specific path on this
  branch): paste-a-key submit calls `setCompanyCredential`/
  `setCompanyCredentialModel` (`@/api/credential`) instead of whatever
  currently posts to the wizard's own inference-key endpoint.
- Backend: confirm `PUT …/credential` and `PUT …/credential/model` are
  reachable pre-company-creation. They're scoped to an existing company
  today (`ApiKeyView` always has a `company` prop from a mounted Connections
  page); the wizard runs *before* a company exists. **This is the real open
  risk in this slice and is not addressed anywhere else in the docs**:
  either these endpoints already support an unscoped/pending-company call
  shape (if `client.scopeFor(null)` — referenced in `credential.ts`'s
  `setCompanyCredentialModel` — resolves to something valid pre-creation,
  check what), or the wizard needs to stage the key/model choice locally and
  defer the actual `setCompanyCredential` call to just after
  `seed_generated_company` succeeds in submit. Check `client.scopeFor`'s
  behavior with `company: null` before assuming either path.

## New Files

- The Managed step-1 component itself (new file or new case in
  `SetupWizard.tsx`, matching whatever convention slice 3 established).

## Dependencies

- Slice 3 (the branch point needs to exist to mount this inside it).
- Slice 2a (`Slot::Search`) for the fan-out to actually be complete — but
  this slice does not need to *wait* for 2a to land; it correctly calls the
  real fan-out either way, and search simply won't be filled until #2342
  ships. Do not block this slice on 2a; do note in the PR description that
  search coverage is pending.

## Implementation Steps

1. Resolve the pre-company-existence scoping question above — this
   determines whether steps 2-5 below happen "live" during the wizard or are
   staged and flushed at submit time.
2. Swap the paste-a-key submit handler to `setCompanyCredential`.
3. Wire the `needsModel` response into a model-picker step, matching
   `ApiKeyView.tsx`'s existing two-step UI shape (reuse-mapping.md §1).
4. On model selection, call `setCompanyCredentialModel` (or
   `setCompanyCredential` with both key+model if there's no staged
   intermediate state — match whichever of `write()`/`writeModel()`'s shape
   applies).
5. Delete the now-dead `company::inference::store_key` call site in the
   wizard's submit path (`server/setup.rs:962-965`-adjacent) — confirm
   nothing else still depends on it before removing (grep for other
   callers).

## Testing Strategy

- An e2e spec parallel to `tinyhumans-account-key.spec.ts` but starting from
  the wizard instead of Connections → Account, asserting the same end state
  (fan-out filled, `cognition: harness`, no restart banner).
- A unit test on whichever scoping resolution was chosen in step 1 — pending-
  company vs. staged-then-flushed each need their own failure-mode coverage
  (e.g.: operator closes the wizard mid-step-1 after a key was already sent
  live — is the key now stranded attached to nothing? Does staged-then-flush
  risk losing the tested key if submit fails for an unrelated reason later?).

## Risks and Edge Cases

- **The pre-company scoping question is the real risk in this whole slice.**
  Everything else is mechanical once it's answered.
- **`ReuseAccountKeyBanner`** (`frontend/src/inference/ReuseAccountKeyBanner.tsx`)
  is an existing, shipped pattern for "you have a TinyHumans key, use it
  here too?" on the Connections pages. It is not reused directly by this
  slice — the wizard's cascade fills slots outright rather than asking — but
  its existence means there's already a team-established tone/UX pattern for
  this kind of prompt. Worth a look before designing the wizard's own copy
  from scratch, purely for consistency.

## Developer Handoff

Do not start writing UI for this slice until the pre-company scoping
question has a written answer from whoever owns the credential endpoints —
that answer changes the shape of every subsequent step. This slice alone is
already a complete, shippable improvement: a company set up in onboarding
today doesn't even get a Provider row from its tested key. This fixes that,
independent of anything grant-related.
