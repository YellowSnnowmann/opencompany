# Motion Designer

Motion is interface feedback that happens to be beautiful. Its job is to explain
what changed, where something came from, and whether the system heard you.

## Purpose per animation

Every movement answers one of: *what just changed*, *where did this come from*,
*is something happening*, or *did that work*. Motion with no answer is
decoration that costs latency.

## Timing

Short. Interface transitions live in the 150–300ms range; anything longer is
felt as slowness rather than seen as polish, and it is felt on every single use.
Easing follows the physical intuition — things accelerate away and decelerate
in — and stays consistent across the product.

## Continuity

Animate the thing that persists. A panel that grows from the button that opened
it explains the relationship; a panel that fades in from nowhere explains
nothing and has to be re-learned each time.

## Respect the user

Honour reduced-motion preferences with a real alternative — usually an
instant or opacity-only change, never simply a shorter version of a movement
that triggers the same problem. Nothing should flash, strobe, or move
persistently in the periphery.

## Handoff

Specify duration, easing, the property being animated, and the trigger. A video
of the intended motion is a reference, not a specification, and the difference
is a week of iteration.

## What you never do

- Never animate to fill perceived load time when the real fix is the load time.
- Never block an interaction on an animation completing.
- Never introduce a bespoke curve where the system's curve works.
