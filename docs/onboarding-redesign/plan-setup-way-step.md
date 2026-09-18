# Slice 3 — the setup-way choice (new step 0)

## Architecture Impact

A new step joins `SetupWizard.tsx`'s `STEPS` array (`:104-111`) ahead of
`power`. It renders no form fields of its own — it's a single choice that
decides which of two step-1 components mounts next. This means the wizard's
step model changes from a flat array to one with a branch, which
`visibleSteps` (`:665-674`) and `problem()` (`:1044-1075`) both need new
logic for, not just a new entry.

## Files to Modify

- `frontend/src/views/setup/SetupWizard.tsx`:
  - `STEPS` array — insert the new step before `"power"`.
  - `visibleSteps` (`:665-674`) — today's filter is a flat predicate list; it
    needs a branch-aware rule: once `setup-way` is answered, only the
    matching step-1 (`managed-login` or `self-managed-connect`) is visible,
    the other is filtered out entirely (not disabled, not shown-then-skipped).
  - `problem()` (`:1044-1075`) — new `current.id === "setup-way"` branch
    (blocks Next until an option is picked), and the existing `current.id ===
    "power"` branch's logic (today: `tested.kind !== "ok"/"skipped"/"hosted"`)
    needs to be re-homed onto whichever of the two new step-1 components ends
    up owning a `tested`-shaped state — see plan-managed-step1.md and
    plan-self-managed-step1.md for what each one's own gate looks like; they
    are not identical to today's single `power` gate.
  - The "hosted" case — `tested.kind === "hosted"` today hides `power`
    entirely when the host already supplies inference. With `power` replaced
    by branch + two step-1s, this needs its own explicit check: if the host
    is hosted, skip `setup-way` and both step-1s outright, landing directly
    on step 2. This is new logic, not a port — see open-questions.md's last
    section, this is where it gets resolved.

## New Files

- A `SetupWayStep` component (new file under `frontend/src/views/setup/`, or
  inline in `SetupWizard.tsx` alongside the other step components if that
  matches the file's existing convention — check whether `PowerStep`/
  `BusinessStep`/etc. are separate files or all in `SetupWizard.tsx` before
  deciding; reuse-mapping.md's citations suggest they're all in the one file
  today).

## Dependencies

None. This can be built and merged before 4a/4b exist, rendering a
placeholder or the still-current `PowerStep` behind both options, as long as
the branch/skip logic in `visibleSteps` is correct — 4a and 4b then replace
the placeholders in place.

## Implementation Steps

1. Add the `setup-way` step id and a minimal `SetupWayStep` (two buttons, no
   network call, writes a local `setupWay: "managed" | "self-managed" | null`
   piece of state).
2. Update `visibleSteps` for the branch: filter both step-1 ids out until
   `setupWay` is set, then show only the matching one.
3. Add the hosted-host skip: if `tested.kind === "hosted"` (or whatever the
   equivalent check becomes once 4a/4b own their own state — coordinate this
   with those slices rather than assuming today's shape survives unchanged),
   skip `setup-way` and both step-1s.
4. Add the `problem()` gate for `setup-way` itself (must have a selection).
5. Leave the `power`-branch gate logic in place but dead (unreachable once
   `power` is removed from `STEPS`) until 4a/4b land, or remove it in the
   same PR if 4a/4b are landing immediately after — coordinate to avoid a
   window where `STEPS` has no step matching the still-present gate logic.

## Testing Strategy

- A test asserting the hosted-host path skips straight to step 2 (previously:
  skips `power`).
- A test asserting picking either setup-way option shows only that branch's
  step-1, and switching the choice (if that's allowed via Back) correctly
  swaps which step-1 is visible without leaving stale state from the other
  branch.
- Existing wizard step-count/step-visibility tests will need their expected
  step counts updated (today's "step 1 of 4" screenshot case becomes "step 1
  of 5": setup-way, self-managed-connect or managed-login, business, signin,
  review — recount per the actual visibility rules once account/advanced
  conditionality is factored in).

## Risks and Edge Cases

- **Back-navigation across the branch.** If an operator picks "Managed,"
  starts step 1, then hits Back twice to change their mind at step 0, does
  the self-managed step's local state (if any was entered) need clearing? Not
  specified in the docs — decide explicitly, don't leave it implicit.
- **The hosted-host skip logic is new, not ported** (open-questions.md's
  final section) — do not assume porting `tested.kind === "hosted" ⇒ hide
  power` to `⇒ hide setup-way and both step-1s` is a mechanical rename.
  Confirm at implementation time whether `tested` (or its replacement) is
  even known before `setup-way` renders — if the hosted probe hasn't
  resolved yet, step 0 may need to render provisionally and skip forward
  once it does, same as today's step 0 already handles the in-flight probe
  (`SetupWizard.tsx`'s comment about "the step stays while it is in flight").

## Developer Handoff

This slice is the one place a wrong call here breaks both branches at once —
get `visibleSteps`' branch logic and the hosted-skip case reviewed carefully
before building on top of it. Land it with the *existing* `PowerStep`
temporarily wired to both branches (functionally identical to today, just
reachable via the new step 0) if 4a/4b aren't ready yet, so this slice is
independently testable.
