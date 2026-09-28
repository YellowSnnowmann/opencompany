# Rendering strategy: where the live canvas goes

## The call-site audit

`TeammateAvatar` (`frontend/src/components/teammate-avatar.tsx`) renders in
roughly 30 places across the console. They split cleanly into two groups.

**Mass-render surfaces** — many instances mounted at once, mostly small
(`size-4` to `size-9`): the avatar-picker's own flavour grid, episode
conversation chips, team member cards, round-band seat rows, the DM channel
list, the `@mention` picker, transcript step sub-lists, thread reply rows,
channel members panes, the channel-create member picker, the main chat
gutter (one per message row), reply facepiles, the comms graph (one node per
agent), an agent's session/log list, and — the highest-count case — the org
chart, where every seat, every "on the roster" dropdown row, and every
desk-membership chip renders one. Mounting an independent WASM/canvas
instance at every one of these, simultaneously, on a busy screen would be a
real performance problem: dozens of Rive runtimes initializing at once for
tiles too small to show the animation meaningfully anyway (most are under
36px).

**Single/hero surfaces** — at most one or two on screen: the agent profile
sheet header (`size-12`), the current user's own avatar button, the
avatar-picker's "current selection" preview (`size-14`, separate from the
flavour grid), a DM channel header face (`size-6` — small despite being a
hero spot), the "live" chat receipt row, the transcript's one-time intro
mark, the "still working" indicator row, and the agent detail page's hero
avatar (`size-14`).

## The decision, as shipped

The plan first written here resolved `mascot:animated` to `null`, so every
mass-render surface above kept its tone tile, and mounted a live canvas at two
hero spots. **That was reversed after the operator tried it**: a teammate whose
mascot looks different in the sidebar than on their own profile sheet reads as
broken, and the cached-snapshot variant of the plan captured blank frames (see
"Why not a snapshot for everyone").

One component, one path. `TeammateAvatar` mounts `MascotAvatar` for every
`mascot:animated` teammate at every call site, with that teammate's own costume,
colors and mode — carried on the roster read (`GET …/team`:
`mascotCostume`, `mascotSkinColor`, `mascotHandColor`, `mascotMode`), the same
values `GET …/team/{id}` returns, so a face is the same in a chat gutter as on
its profile sheet. `markOnly` tiles (too small to read) keep the tone tile. The
hero surfaces (profile sheet, agent detail header, avatar picker) mount
`MascotAvatar` directly, because they also drive `state` from hover.

### What makes live-everywhere affordable

1. **One parsed file.** `useRive({ src })` fetches and parses the ~1.7 MB `.riv`
   and decodes its embedded raster assets once *per instance*. `MascotAvatar`
   builds a single module-level `RiveFile` and hands it to every instance. The
   runtime counts references (`getInstance` +1, `cleanup` −1, release at zero),
   so the module takes one permanent reference: without it the file is destroyed
   the moment the last mascot on a page unmounts and the next page's mascots
   fail with "Problem loading file; may be corrupt!" — invisible to any test
   that reloads between pages.
2. **A fresh ViewModel instance per mascot** (`useViewModelInstance(vm, { useNew })`).
   With a shared file `useDefault` hands every canvas the same instance, so one
   teammate's skin color would repaint all the others.
3. **Viewport gating.** A tile only holds a live canvas while within 300 px of
   the viewport (`IntersectionObserver`, released 1.5 s after leaving so a
   wobble at the edge does not rebuild it). A transcript has a tile per message;
   what matters is how many are near the screen, not how many exist.
4. **Static and reduced-motion release the instance.** They play just long
   enough to reach a settled pose, keep that exact frame as an `<img>`, and
   unmount Rive — so a screenful of static teammates costs no live canvases.

### Why not a snapshot for everyone

The mascot is not a set of stills. Every costume plays a rise-in on load — a peek
to ~24% coverage that holds ~0.25 s, a pop to ~53%, an overshoot that settles by
~1.3 s — and then an idle loop that ducks it out of frame and back about every
six seconds (all but two costumes; headphones and glass1 instead bob
continuously and never duck out). There is no "the resting frame" to capture at
an arbitrary moment: the earlier `MascotWarmer` grabbed `canvas.toDataURL()` two
frames after the ViewModel write and cached it per look, which produced blank
and half-risen tiles that varied between reloads, and left small tiles static
while big ones moved.

Holding a pose therefore waits for the animation's own clock (Rive's `advance`
events — the wall clock lies when many canvases start at once and the main
thread stalls) to pass the rise-in, and for the mascot to be clearly visible
(≥38% coverage; the peek is ~24%, a settled pose 48–61% for all nine costumes).
Between 1.8 s and 3.5 s of animation time — after the rise-in, before the first
duck-out — any visible frame is a valid pose. After it, only a frame that has
stopped moving is trusted. Pausing the runtime on that frame does **not** work:
a paused instance redraws differently from the playing one (every costume with a
duck-out loop came back scaled and cropped), so the pixels are copied out
instead.

### Measured

A 30-message transcript (60 rows in the DOM), fresh browser each run, headless
Chrome for Testing at DPR 1, scroll of the whole transcript over 3 s.

| build | JS heap | live canvases at rest | scroll avg / p95 / frames >33 ms |
|---|---|---|---|
| snapshot cache (previous) | 13.0 MB | 0 | 16.8 / 17.7 ms / 2 of 179 |
| live, gated — 2 wearers | 13.2 MB | 4 | 17.0 / 18.2 ms / 5 of 176 |
| live, gated — 9 wearers | 14.0 MB | 11 (13 mid-scroll) | 17.0 / 17.9 ms / 5 of 177 |
| shipped — 8 animated + 1 static | 13.9 MB | 10 (12 mid-scroll) | 16.9 / 18.2 ms / 2 of 178 |

Threshold, fixed before the live numbers were seen: p95 ≤ 20 ms, ≤ 5% of frames
over 33 ms, and live canvases bounded by what is near the viewport rather than by
message count — all met. **Renderer RSS was also in the threshold (≤ baseline +
150 MB) and is not a usable gate**: fresh-browser readings were 153 MB (snapshot
build), 313 MB (4 canvases), 328 MB (11), and 425 MB with *zero* live canvases
(all tiles settling at once at load inflates the high-water mark). That ±100 MB
spread is larger than the effect being measured. What the numbers do support is
a fixed cost for holding the decoded asset (the pinned file is never released)
and roughly 2 MB per additional live canvas.

## Test impact

Checked every test file that touches an avatar: nothing in `frontend/test`
references the avatar-picker's `avatar-flavours` / `avatar-flavour-<x>` /
`avatar-preview` testids outside `avatar-picker.tsx` itself, and no spec
imports `AvatarPicker` at all. The tests that do assert on avatar `<img>`
markup — `chat-thread-avatar` (unit + e2e), `chat-channel-intro`,
`company-cards.spec.ts`'s `agent-avatar` check — all target other
components' testids on paths this work does not change. **This is additive:
nothing existing needed changing to keep passing.** New coverage:
`mascot-avatar-settle.test.ts` (the keep-the-frame rules, the swap to an image,
the shared and pinned file, a fresh ViewModel instance per mascot) and
`teammate-avatar-mascot-tile.test.ts` (viewport gating and forwarding the look).

## Bundle and load strategy

No Rive dependency exists in `frontend/package.json` today — `@rive-app/react-canvas`
is net new, and so is the ~1.8&nbsp;MB `.riv` asset. The console already has
an established pattern for isolating exactly this kind of weight:
`lazy(() => import("@/path").then((m) => ({ default: m.X })))`, used eleven
times today (`StandaloneStyleguide` for `recharts`, `WorkflowsView` for
`@xyflow/react`, `Joyride` for `react-joyride`, plus `ObservatoryView`,
`WorkspaceView`, `MemoryView`, `FinanceSection`, `PagesView`, `UsageView`,
`KnowledgeGraph`), each wrapped in `<Suspense>` with the existing
`route-loading.tsx` fallback (or a tile-shaped inline fallback, so the two
hero slots above don't jump layout while the chunk loads).

`vite.config.ts` has no manual Rollup chunk-splitting config
(`build.rollupOptions.output.manualChunks` is absent entirely) — every
existing split comes from these `lazy()` boundaries, not build config, so
`MascotAvatar` should follow the identical pattern rather than introduce a
new splitting mechanism. Concretely: `MascotAvatar` itself is the
`lazy()`-loaded module, pulling in `@rive-app/react-canvas` and the `.riv`
asset URL as its own dependencies — the two call sites above import
`MascotAvatar` lazily, not `@rive-app/react-canvas` directly.

## Accessibility

`prefers-reduced-motion: reduce` holds a pose exactly as `mode="static"` does —
a settled frame kept as an image — rather than not drawing: `autoplay: false`
does not mean "paint one frame and stop", it means Rive never starts its render
loop, so a canvas that never plays paints nothing at all (an earlier pass
rendered reduced-motion users a blank tile). The same accessibility carve-out an
animated GIF avatar already has to consider per `docs/spec/runtime/avatars.md`
("a moving one is more recognisable, not less" assumes the viewer can tolerate
motion, which reduced-motion says they can't).
