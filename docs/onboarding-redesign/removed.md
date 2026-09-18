# What this removes

Three things, each a deliberate deletion, not a deprecation or an opt-out.

## The standalone "Model" step as the first screen

Today, `PowerStep` (provider dropdown + raw API key field + Test connection)
is the very first thing a new operator sees, before they've said what kind of
setup they even want. In the target flow it still exists in substance — its
Test-connection mechanics carry into Managed step 1 (paste-a-key path) and
Self-managed step 1 (provider path) — but it no longer exists as a
freestanding first screen asking someone to make a low-level credential
decision before a high-level one.

## Company naming on the Review step

`ReviewStep` shows an editable company-name field today, alongside the
designed roster, after the roster's already been built. D-name-once
(README.md) moves this to step 2 and deletes it from Review — not hides it
conditionally, deletes it. A company only ever gets named once, before the
roster is designed around that name.

## The post-build checklist screen, in full

"Let's get your company running" — three items (Name your company / Connect
an integration / Run an automation), completable in any order, gone entirely
once all three are done, with its own "Skip setup" escape hatch.

This is a full removal, not a trim:

- **"Name your company"** is redundant on its own merits even before this
  redesign, since naming now happens in step 2 — there is nothing left for
  this checklist item to do.
- **"Connect an integration"** and **"Run an automation"** are not migrated
  anywhere else in the flow, not made optional-by-default, not turned into a
  dismissible banner in the console. They stop existing as an onboarding
  surface. If a discoverability need for these two actions still exists after
  this redesign ships, that is a separate product decision for a separate
  issue — not something this redesign should solve by keeping a smaller
  version of the screen it was asked to remove.

Confirmed with the operator directly, twice: once when first shown the
screen ("i want to drop this whole thing all together"), once when asked to
confirm scope while walking the self-managed branch ("rest all same" —
meaning the removal applies regardless of which setup-way branch was taken).
