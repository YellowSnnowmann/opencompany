# Driving the animation: idle, hover, replying

## Colors: v1 ships one fixed colorway

`handColor`/`skinColor` are the mascot's literal skin and hand color, not an
abstract UI accent — they're a different kind of thing from `TEAM_TONES`
(`frontend/src/lib/team.ts`), which tints a tile's background/initials.
Reusing a teammate's hashed `tone` hue directly as their mascot's *skin
color* risks looking wrong in a way no amount of engineering analysis can
settle — e.g. a `rose`-toned teammate getting a pink-skinned mascot — and
that's a call for whoever actually looks at it rendered, not something to
bake into v1 code sight unseen.

**v1 ships the `.riv` file's own defaults** (`handColor=#B4900B`,
`skinColor=#F7D145`) for every teammate that picks `mascot:animated`. No
palette-derivation logic, no new mapping table. Per-tone (or per-teammate)
color variation is a real, clearly-scoped v1.5+ follow-up once there's
something to look at.

## Hover: no new plumbing needed

Trivial everywhere it applies — a plain `onMouseEnter`/`onMouseLeave` on the
wrapping element, setting `MascotAvatar`'s `state` prop to `"hover"`. Applies
identically at both v1 call sites (the profile-sheet header, the
avatar-picker preview/tile).

## "Replying": shipped on the rows that already hold the signal

The only signal that honestly means "this agent is producing output right now"
is a turn being open for it. Three rows in the chat pane exist exactly as long as
one is, and each draws that teammate's avatar: `ChatLiveReceipt` (the receipt for
a message the operator just sent), `LiveTurnRow` (a turn with live steps) and
`TypingRow` (the "Replying…" line), the last two in `MessageTimeline.tsx`. They
pass `replying` to `TeammateAvatar`, which is a per-surface prop like `animate`
(`lib/mascot-pose.ts`, `components/mascot-pose.tsx`).

**Not every open turn is a replying one.** A *queued* turn is waiting on the
per-company lock, not progressing — the row's own status dot stills for the same
reason — and a *stalled* receipt (no live frame for 30 s) has gone quiet. Neither
tells the mascot to reply (`replying={!queued && !stalled}` on the receipt,
`!queued` on the typing row; the working row only renders when not queued). The
profile sheet is not one of these surfaces: it has no subscription to any turn,
and a per-agent "is a turn open" subscription would be new plumbing for a signal
it was never designed to carry.

**What it looks like.** The tile bobs gently (a 1.4 s breath, ~4% lift and 5%
scale, looping) and crossfades to the **headband** costume — a dark bandana
headband with a knot at the side, the costume behind `mascotAnimationNumber = 3`
(`MASCOT_REPLYING_COSTUME`, kept equal to `REACTIVE_NUMBERS.replying` by a unit
test) — in the teammate's own skin and hand colors. It reads as "putting on the
headband to get to work" at 36 px and at 96 px, in both themes. Candidates
watched, in a scratch page holding tiles in the state: the headband swap with the
bob (chosen); the bob alone (too faint at 36 px to say anything); and the live
`state="replying"` instance, which is the same headband but caught mid-rise or
ducked out at random moments — which is exactly why tiles use settled frames.
When the reply ends the bob is cancelled and the headband crossfades back out; a
teammate already wearing the headband just bobs.

**How it is built, and why it cannot stick.** No live canvas at rest and none
while replying: the replying frame is a settled pose like the hover one, captured
**on demand** the first time someone replies (about 2 s cold, 48 ms once stored
across reloads — `lib/mascot-pose-store.ts`), and until it exists the tile bobs on
its resting frame, never blank. The bob is a Web Animation started by an effect
whose cleanup cancels it, so it lives exactly as long as the `replying` prop: the
row that passes it unmounting, or the prop dropping, is what stops it. It composes
with the rest of the trigger vocabulary: a static teammate, `animate="none"` and
reduced motion never reply, and a hover reaction plays over it and hands back.

**What was observed in the real app.** In the demo host (no model configured) a
turn spends its whole ~4 s in the *queued* state, so the replying state itself is
visible for only tens of milliseconds there; the flag was seen to reach the
receipt's tile (`data-mascot-replying`) at the moments the turn was not queued
(75 ms and 4093 ms after sending), and after a completed turn, a send that failed
at the network, and a turn abandoned by navigating away, no tile is replying and no
looping animation is left behind. The look itself, and its on-demand capture, were
therefore judged in a scratch page that holds tiles in the state, not in a chat.

## Ruled out: presence

`frontend/src/hooks/use-presence.ts` is confirmed human-viewer-only —
keyed by `userId`, tracking who's looking at the console right now via
document-input idleness, with no concept of an agent or a running turn. Not
a candidate signal source for any mascot state.

## The state budget, mapped to the Number-input slots

Idle is the file's own default (`mascotAnimationNumber = 1`, confirmed from
the editor and, later, live in the runtime — it renders the mascot's cap).
The "4 slots, `glass1`-`glass4`" framing this section originally used was a
static-analysis guess later found wrong: the artboard actually has nine
costume animations (`open-questions.md` §1), of which `glass1`-`glass4` are
only one family. **`mascotAnimationNumber = 2` is confirmed live to render
headphones, not a `glass2` variant** — `STATE_NUMBERS` in
`mascot-avatar.tsx` maps `hover` to `2` on that confirmation, not a guess.
`3` (`replying`) is the headband costume — watched, and used for the replying
look above; slots beyond that
are unused by v1. See `open-questions.md` §1 and §4 for what's actually
been watched play versus what's still assumed.
