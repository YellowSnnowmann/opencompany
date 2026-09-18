# Slice 7 — rebuild-in-place for the wizard's key-save

This slice is mostly verification, not new code — it may turn out to be a
no-op once 4a lands, or it may surface a real race. Do not skip the
verification step to save time; that's the actual point of this slice.

## Architecture Impact

None, if the open question below resolves in the "no action needed"
direction. Potentially: a new `rebuild_if_pending`-equivalent call inserted
into the wizard's final submit path, if the verification finds a real gap.

## Files to Modify

Conditional on the verification's outcome:

- If a gap is found: `server/setup.rs`'s `apply_inner` (`:724-753,
  900-1035`), specifically wherever `seed_generated_company` →
  `register()` happens, may need a `rebuild_if_pending`-equivalent call
  after `register()` completes, to catch the case where slice 4a's key-save
  (step 1) happened before the runtime this submit boots even existed.
- If no gap is found: no code changes, this slice becomes a documented
  confirmation added to reuse-mapping.md §4 and open-questions.md (resolve
  the open question in place rather than leaving it open).

## New Files

None expected.

## Dependencies

Slice 4a (there's nothing to verify until Managed step 1 actually calls the
real fan-out).

## Implementation Steps

1. **Verify first, before writing anything.** Trace `register()`
   (`desktop.rs`, called from `seed_generated_company`, `:423`) — does it
   read the company's current inference configuration fresh at boot time
   (in which case a key saved in step 1, before this company existed, is
   correctly picked up the first time `register()` runs, and this slice is
   done), or does it depend on some cached/pre-resolved state that could be
   stale relative to a key saved earlier in the same wizard session?
2. If fresh-read-at-boot is confirmed: update open-questions.md's answer in
   place (this repo's docs are meant to be updated as questions resolve, not
   left stale) and close this slice with no code change.
3. If a staleness risk is found: add the `rebuild_if_pending`-equivalent
   call at the point `register()` completes within `apply_inner`, scoped
   narrowly to "just-created company whose inference was set during this
   same wizard session" — not a blanket rebuild-every-submit change.

## Testing Strategy

- A test that saves a key in Managed step 1, then completes the wizard, then
  immediately (same test, no artificial delay) asserts the resulting
  company's `GET …/inference` reports `cognition: harness` with no restart
  required — this is the actual race condition made concrete as a test, and
  it should exist regardless of which way step 1 resolves (it either passes
  trivially, confirming no gap, or fails, proving the gap real).

## Risks and Edge Cases

- **This is a genuine open question, not a formality** — open-questions.md
  is explicit that assuming `register()` is unaffected "just because it's a
  different code path" is exactly the mistake to avoid. Do not close this
  slice by inspection alone; the test in Testing Strategy is the actual
  proof, write it even if steps 1-2 above suggest no gap exists.

## Developer Handoff

Write the race-condition test from Testing Strategy *first*, before doing
the code trace — if it passes against today's `main` plus slice 4a's
changes with no further work, you have your answer empirically and the
trace in steps 1-2 becomes confirmation rather than discovery.
