# Slice 5 — move company naming to step 2

The smallest slice in this plan. Low risk, no backend change.

## Architecture Impact

A single field moves from one step's render to another's. `SetupInput.name`
(the wire field `submit()` sends) is unaffected — only which step's local
state populates it changes.

## Files to Modify

- `frontend/src/views/setup/SetupWizard.tsx`:
  - `BusinessStep` (`:2362` area, current-flow.md) — gains a company-name
    text field alongside the template/industry choice.
  - `ReviewStep` (`:1989` area) — loses its own editable company-name field
    entirely (D-name-once, README.md: deleted, not hidden).
  - Whatever local state currently backs Review's name field needs to move
    up to `BusinessStep`'s state, and `submit()`'s `SetupInput.name` read
    needs to point at the new location.

## New Files

None.

## Dependencies

None.

## Implementation Steps

1. Add the name field to `BusinessStep`'s render and local state.
2. Remove the name field from `ReviewStep`'s render.
3. Repoint `submit()`'s `name` source at `BusinessStep`'s state.
4. Add a gate: `problem()`'s `business` branch (current-flow.md: "template
   chosen, or industry text filled") gains a third condition — name must be
   non-empty too. Decide the exact validation (matches
   `clampToCompanyNameLimit`, referenced in `frontend/src/onboarding/OnboardingGate.tsx:54`
   — reuse that same limit/validation function rather than writing a new
   one, since it already exists for exactly this kind of field).

## Testing Strategy

- A test asserting `business` step blocks Next with no name entered, same
  shape as the existing template/industry gate tests.
- A test asserting `ReviewStep` no longer renders a name field or accepts
  name edits.
- A test asserting `submit()`'s payload carries the name from step 2's
  state, unchanged wire shape.

## Risks and Edge Cases

- **Reuse `clampToCompanyNameLimit`** rather than re-deriving name-length
  validation — it already exists (`OnboardingGate.tsx:54`, ironically in the
  file slice 6 deletes; move this specific function somewhere shared before
  deleting the rest of that file, don't let it get deleted along with the
  checklist it currently lives beside).
- **Design's roster proposal** (`design()` → `POST /api/v1/setup/roster`)
  may currently use or reference the name in its request/response shape
  (current-flow.md's Review-step description doesn't rule this out) — check
  whether `propose_roster` (`server/setup.rs:1525`) reads the name at all
  before assuming this move is purely frontend.

## Developer Handoff

Trivial to implement once `clampToCompanyNameLimit`'s relocation is settled
(coordinate with whoever does slice 6, since that's the file it currently
lives in). Land this slice independently of everything else in this plan —
it has no real dependency on the branch/reuse work.
