# Slice 6 — remove the post-build checklist, in full

## Architecture Impact

Deletes a component tree and its gate, not a config flag. After this slice,
`AppShell` has one fewer conditional render path; `submit()`'s success
handoff goes straight from "Open the console" to a plain `AppShell` mount
with nothing in between.

## Files to Modify

- `frontend/src/components/app-shell.tsx`:
  - Remove the `OnboardingGate` import (`:43`).
  - Remove the `shouldShowOnboardingGate` import (`:62`) and its call site
    (`:3653`).
  - Remove the conditional render block (`:3670` on).
  - Read the surrounding comments first (`:601`, `:993`, `:1080`,
    `:1172`, `:3532`, `:3572`, `:3600`) — several describe *other* logic's
    behavior in terms of "until `OnboardingGate` completes" or "when
    `OnboardingGate` would mount." Each of those needs its own re-check, not
    just the gate's own removal — this component's presence is load-bearing
    for other conditionals nearby, per those comments' own admission.

## New Files

**Important — do not delete blind:** `frontend/src/onboarding/OnboardingGate.tsx`
also defines `clampToCompanyNameLimit` (`:54`), which
[plan-naming-move.md](plan-naming-move.md) needs relocated to a shared
location (e.g. alongside wherever company-name validation already lives, or
a new small `frontend/src/lib/company-name.ts`) **before** this file is
deleted. Sequence this: extract the function first (its own small commit),
then delete the rest of the file.

## Files to Delete

- `frontend/src/onboarding/OnboardingGate.tsx` (after extracting
  `clampToCompanyNameLimit`).
- `frontend/src/onboarding/IntegrationStep.tsx` (backs the "Connect an
  integration" checklist item — confirm it has no other importer before
  deleting; it's named generically enough that it's worth a real grep, not
  an assumption).
- Any other file under `frontend/src/onboarding/` that only exists to back
  this screen — audit the directory's full contents, not just the two files
  found so far, before calling this slice done.

## Dependencies

Should land after (or in the same PR as) plan-naming-move.md's extraction of
`clampToCompanyNameLimit`, to avoid a window where that function is deleted
before its new home exists.

## Implementation Steps

1. Extract `clampToCompanyNameLimit` out of `OnboardingGate.tsx` into a
   shared location; repoint `BusinessStep`'s new name field (slice 5) at it.
2. Grep the whole frontend for every import of `OnboardingGate`,
   `shouldShowOnboardingGate`, and `IntegrationStep` — confirm the only
   importers are `app-shell.tsx` (and `IntegrationStep.tsx`'s own importer,
   `OnboardingGate.tsx` itself) before deleting anything.
3. Remove the render block and its imports from `app-shell.tsx`.
4. Re-read every comment listed above that references `OnboardingGate`'s
   presence/absence as a condition for other logic; update or remove each
   one's reasoning to match the new reality (some may describe behavior that
   still needs to exist in a different form — do not assume every mention
   is safe to delete verbatim).
5. Delete `OnboardingGate.tsx` and `IntegrationStep.tsx` (and anything else
   found in step 2's audit).
6. Search `frontend/test/` for any spec exercising this screen
   (`"Let's get your company running"`, `"Skip setup"`, `"Run an
   automation"` as search strings) and remove those tests, not just leave
   them to fail.

## Testing Strategy

- An e2e assertion that a fresh company's post-submit landing goes straight
  to the normal console view, with no checklist screen ever rendering —
  add this if no existing spec already covers the "straight to console"
  path without the checklist in between.
- Confirm no orphaned route (`#/...` fragment or similar) still points at
  the deleted screen anywhere (grep for its route string, not just its
  component import).

## Risks and Edge Cases

- **The comments in `app-shell.tsx` that reference `OnboardingGate` as a
  condition for other behavior are the real risk here**, not the deletion
  itself. Read every one before removing; at least one (`:1172`,
  "completed cannot matter once `isActivated` is true") describes logic
  that may need to survive in some form even without the gate component
  itself.
- **`clampToCompanyNameLimit` orphaned mid-refactor** if this slice and
  slice 5 land out of order without coordination — sequence explicitly, per
  Dependencies above.

## Developer Handoff

This reads like a simple deletion and mostly is, except for the
`app-shell.tsx` comments describing other logic in terms of this
component's presence — budget time to actually understand and re-resolve
those, not just delete the import and see what breaks.
