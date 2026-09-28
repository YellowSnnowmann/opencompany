# Animated mascot avatar — plan

The plan, and its implementation, in one PR. A tracking issue follows once this
is reviewed. The documents below began as the pre-code deep-dive; where the
shipped design differs from what they first proposed, each says so in place.

## What this is

An operator supplied a Rive file (`mascotprofile.riv`) exported from Figma's
Rive plugin: an animated character ("the mascot") whose hands and skin can be
recolored and which plays one of nine costume animations on demand. The ask is
to offer it as an alternate, editable face for a teammate — alongside the
eleven static `tiny:` mascots and an uploaded image — that reacts to hover and
to the agent actively replying.

This directory is the deep-dive that preceded any code: what the `.riv` file
actually contains, how it plugs into the console's closed avatar-reference
grammar, which of the ~30 places an avatar renders should get a live canvas
versus keep the existing static tile, where the animation states should be
driven from, and what still needs the real Rive runtime to answer before
implementation starts.

## Documents

- [`rive-parameters.md`](rive-parameters.md) — the `.riv` file's structure
  (artboard, state machine, ViewModel) and the nine costumes that superseded
  the original "4-state budget".
- [`avatar-grammar.md`](avatar-grammar.md) — the host + console changes to
  the closed `tiny:`/`blob:` avatar-reference grammar, and the test mirrors
  a third form needs.
- [`rendering-strategy.md`](rendering-strategy.md) — the call-site audit
  (which of the ~30 avatar render sites get a live canvas) and the
  bundle/code-splitting plan.
- [`state-mapping.md`](state-mapping.md) — where idle/hover/replying signals
  come from, how the colors are chosen, and what "replying" looks like.
- [`open-questions.md`](open-questions.md) — the empirical unknowns that
  need the actual Rive runtime, as an implementation checklist.

## Approach, as shipped, in one paragraph

Add a third closed avatar-reference form, `mascot:animated`, and draw it the
same way everywhere: `TeammateAvatar` mounts one `MascotAvatar` at every one of
the ~30 places it renders, with the teammate's own costume, colors and mode
(carried on the roster read, not just the detail read). An animated mascot is a
live Rive canvas — sharing one parsed `.riv` between all of them and only held
while its tile is near the viewport; a static (or reduced-motion) one plays just
long enough to settle, is kept as an image, and releases its instance. The
first draft of this plan kept every mass-render surface on the tone tile and
went live at two hero spots only; that was reversed once the operator saw a
teammate look different in the sidebar than on their own profile
(`rendering-strategy.md` has the reasoning and the measurements). Load the Rive
runtime and the ~1.8&nbsp;MB `.riv` asset lazily, the same way the console
already isolates `recharts` and `@xyflow/react` from the main bundle.
