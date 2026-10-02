// Where a mascot avatar animates, and the cache of settled poses that lets a
// small tile rest without a live canvas.
//
// Nothing here imports the Rive runtime, so `TeammateAvatar` (which is in the
// main bundle) can use the vocabulary and the cache while the runtime itself
// stays behind the lazy `mascot-avatar` chunk.
//
// Why a pose cache exists at all. The `.riv` is not a set of stills: every
// costume plays a rise-in (peek, pop, overshoot, settle) and most then duck out
// of frame about every six seconds, so an animated tile is blank a third of the
// time, and a freshly mounted live instance always starts from below the frame.
// A small tile therefore rests on a *settled* frame instead — captured once per
// look (costume + colors) by a hidden live instance that waits for the pose to
// settle (`holdPoseOnceSettled`), then shared by every tile with that look.
//
// Frames are also kept across reloads (`lib/mascot-pose-store.ts`). A key's
// stored frame is restored — and its pixels checked, `lib/mascot-frame.ts` —
// before any capture for it is allowed to start, so a returning visit shows the
// mascots at once instead of re-settling every look, and a stored frame that
// fails its checks is deleted and re-captured rather than trusted.

import {
  mascotCostumeNumber,
  mascotHandColorHex,
  mascotSkinColorHex,
  type MascotCostume,
} from "@/lib/avatar";
import { domFrameProbe, frameIsGood, type FrameProbe } from "@/lib/mascot-frame";
import { readStoredPose, removeStoredPose, touchStoredPose, writeStoredPose } from "@/lib/mascot-pose-store";

/**
 * When a mascot avatar animates. The one place this vocabulary is defined.
 *
 * - `"loop"` — a live instance that keeps playing its idle loop (the mascot
 *   ducks out and back about every six seconds). The hero surfaces — the profile
 *   sheet, the agent page header, the picker preview — are this, by mounting
 *   `MascotAvatar` directly; they also drive hover/replying themselves.
 * - `"hover"` — rests on a settled frame and plays once when the pointer (or
 *   keyboard focus) enters the enclosing row. What every other tile does.
 * - `"none"` — the settled frame only.
 *
 * A teammate whose own `mascotMode` is `"static"` never animates, whatever a
 * surface asks for, and neither does anyone under `prefers-reduced-motion`
 * ({@link effectiveMascotTrigger}).
 */
export const MASCOT_TRIGGERS = ["loop", "hover", "none"] as const;
export type MascotTrigger = (typeof MASCOT_TRIGGERS)[number];

/** What a `TeammateAvatar` does when its caller does not say — the mass-render surfaces. */
export const DEFAULT_MASCOT_TRIGGER: MascotTrigger = "hover";

/**
 * The trigger a surface actually gets: a static teammate and reduced motion both
 * mean "settled frame only", regardless of what the surface asked for.
 */
export function effectiveMascotTrigger(
  requested: MascotTrigger,
  mode: string | undefined,
  reducedMotion: boolean,
): MascotTrigger {
  if (mode === "static" || reducedMotion) return "none";
  return requested;
}

/**
 * The costume a `"hover"` tile swaps to. It is the costume behind the fixed
 * hover number the hero surfaces already write (`REACTIVE_NUMBERS.hover` in
 * `mascot-avatar.tsx`, which a unit test keeps in step), so hovering a tile and
 * hovering the profile-sheet hero show the same reaction.
 */
export const MASCOT_HOVER_COSTUME: MascotCostume = "headphones";

/**
 * The costume a tile wears while its teammate is replying — the one behind the
 * fixed replying number (`REACTIVE_NUMBERS.replying` in `mascot-avatar.tsx`,
 * kept in step by a unit test). See `PoseMascot` for how it is shown.
 */
export const MASCOT_REPLYING_COSTUME: MascotCostume = "headband";

/**
 * The identity of a settled pose: the costume's `mascotAnimationNumber` and both
 * resolved colors. Anything that resolves to the same three draws the same frame,
 * so an unrecognised costume id and the default share a key, and two teammates
 * with the same look share one capture.
 */
export function mascotPoseKey(
  costume: string | undefined,
  skinColor: string | undefined,
  handColor: string | undefined,
): string {
  return `${mascotCostumeNumber(costume)}|${mascotSkinColorHex(skinColor)}|${mascotHandColorHex(handColor)}`;
}

// ---------------------------------------------------------------------------
// The cache
// ---------------------------------------------------------------------------

const poses = new Map<string, string>();
const poseListeners = new Map<string, Set<() => void>>();

// Frames kept from an earlier visit (`lib/mascot-pose-store.ts`). A key is
// restored at most once per page, and a restored frame is shown only after its
// pixels have been checked — so `restoring` is what keeps a tile from starting
// a hidden capture for a look whose stored frame is a few milliseconds away.
const restoring = new Set<string>();
const restoreAttempted = new Set<string>();
let frameProbe: FrameProbe = domFrameProbe;

/** Test seam: how a frame's pixels are read for validation (the browser's canvas by default). */
export function setMascotFrameProbe(probe: FrameProbe | null): void {
  frameProbe = probe ?? domFrameProbe;
}

function devicePixelRatio(): number {
  return typeof window !== "undefined" && window.devicePixelRatio > 0 ? window.devicePixelRatio : 1;
}

/**
 * Starts restoring `key`'s frame from storage, if there is one. Synchronous
 * checks (the entry parses, the PNG is the right size) decide whether there is
 * anything to validate; the pixel check is asynchronous. Whichever way it goes,
 * a tile waiting on this key hears about it: a good frame is published like any
 * capture, a bad one is deleted and the queued capture is let through.
 */
function beginRestore(key: string): void {
  if (poses.has(key) || restoring.has(key) || restoreAttempted.has(key)) return;
  restoreAttempted.add(key);
  const dpr = devicePixelRatio();
  const stored = readStoredPose(key, dpr);
  if (stored === null) return;
  restoring.add(key);
  void frameIsGood(stored, dpr, frameProbe).then((good) => {
    restoring.delete(key);
    if (good && !poses.has(key)) {
      setPose(key, stored);
      touchStoredPose(key, dpr);
    } else {
      if (!good) removeStoredPose(key, dpr);
      pump();
    }
  });
}

/** The settled frame (a PNG data URL) for a pose key, once it has been captured or restored. */
export function getMascotPose(key: string): string | undefined {
  return poses.get(key);
}

/** Subscribes to one key's frame arriving. Returns the unsubscribe. */
export function subscribeMascotPose(key: string, notify: () => void): () => void {
  beginRestore(key);
  let set = poseListeners.get(key);
  if (!set) {
    set = new Set();
    poseListeners.set(key, set);
  }
  set.add(notify);
  return () => {
    set.delete(notify);
    if (set.size === 0) poseListeners.delete(key);
  };
}

/** Makes a frame the one for `key`, wakes everything waiting on it and frees its capture slot. */
function setPose(key: string, url: string): void {
  poses.set(key, url);
  failed.delete(key);
  for (const request of [...active]) {
    if (request.key === key) finish(request);
  }
  poseListeners.get(key)?.forEach((notify) => notify());
  pump();
}

/**
 * Records a freshly captured frame. It is also stored for the next visit — but
 * only once its pixels check out, so nothing that would not be trusted on the
 * way back out is ever written.
 */
export function publishMascotPose(key: string, url: string): void {
  setPose(key, url);
  const dpr = devicePixelRatio();
  void frameIsGood(url, dpr, frameProbe).then((good) => {
    if (good) writeStoredPose(key, dpr, url);
  });
}

// ---------------------------------------------------------------------------
// The capture scheduler
// ---------------------------------------------------------------------------

/**
 * How many hidden live instances may be capturing at once. A first visit to a
 * roster of distinct looks captures them all; this is what keeps that from being
 * a dozen simultaneous render loops (the earlier live-everywhere build ran ~10 at
 * rest and was fine, so this is a ceiling, not a tight budget).
 */
export const MAX_ACTIVE_CAPTURES = 6;
/** Hover-costume frames are a nicety, captured in the background: never more than this many at once. */
export const MAX_ACTIVE_HOVER_CAPTURES = 2;
/**
 * A capture that has not produced a frame by now is abandoned. `holdPoseOnceSettled`
 * gives up silently after 12 s; without this the slot would be held forever and
 * the tile would retry forever. The look is then remembered as failed, and its
 * tiles keep showing the initials underneath — never a blank.
 */
export const CAPTURE_TIMEOUT_MS = 15_000;

export type CapturePriority = "rest" | "hover";

interface CaptureRequest {
  key: string;
  priority: CapturePriority;
  grant: () => void;
  revoke: () => void;
  timer?: ReturnType<typeof setTimeout>;
}

const queue: CaptureRequest[] = [];
const active = new Set<CaptureRequest>();
const failed = new Set<string>();

function finish(request: CaptureRequest) {
  clearTimeout(request.timer);
  active.delete(request);
}

function keyIsActive(key: string): boolean {
  for (const request of active) if (request.key === key) return true;
  return false;
}

function pump() {
  for (let i = 0; i < queue.length; ) {
    // Already captured, or given up on: nothing left to do for this request.
    if (poses.has(queue[i].key) || failed.has(queue[i].key)) queue.splice(i, 1);
    else i += 1;
  }
  const ordered = [
    ...queue.filter((r) => r.priority === "rest"),
    ...queue.filter((r) => r.priority === "hover"),
  ];
  for (const request of ordered) {
    if (active.size >= MAX_ACTIVE_CAPTURES) return;
    // One capture per look at a time; another tile with the same look waits for it.
    if (keyIsActive(request.key)) continue;
    // A stored frame for this look is being checked: if it is good no capture is
    // needed, and if not `beginRestore` calls `pump` again.
    if (restoring.has(request.key)) continue;
    if (request.priority === "hover") {
      // Background work yields to any tile still waiting for its resting frame.
      const restWaiting = queue.some((r) => r.priority === "rest" && !keyIsActive(r.key));
      const hoverActive = [...active].filter((r) => r.priority === "hover").length;
      if (restWaiting || hoverActive >= MAX_ACTIVE_HOVER_CAPTURES) continue;
    }
    queue.splice(queue.indexOf(request), 1);
    active.add(request);
    request.timer = setTimeout(() => {
      finish(request);
      failed.add(request.key);
      request.revoke();
      pump();
    }, CAPTURE_TIMEOUT_MS);
    request.grant();
  }
}

/**
 * Asks to capture the settled frame for `key`. `grant` fires when this requester
 * may mount its hidden capture instance; `revoke` fires if the capture is
 * abandoned after the timeout. Returns the cancel function — call it on unmount
 * and when the frame turns up some other way; it hands the slot on.
 *
 * Only one requester captures a given key at a time. If it goes away first, the
 * next tile waiting on the same look takes over.
 */
export function requestMascotPoseCapture(
  key: string,
  priority: CapturePriority,
  grant: () => void,
  revoke: () => void = () => {},
): () => void {
  beginRestore(key);
  if (poses.has(key) || failed.has(key)) return () => {};
  const request: CaptureRequest = { key, priority, grant, revoke };
  queue.push(request);
  pump();
  return () => {
    const queued = queue.indexOf(request);
    if (queued >= 0) queue.splice(queued, 1);
    if (active.has(request)) finish(request);
    pump();
  };
}

/**
 * Test seam: forget every in-memory frame, queued request, failure and restore
 * attempt — what a page reload does. Stored frames are left alone.
 */
export function resetMascotPoses(): void {
  for (const request of active) clearTimeout(request.timer);
  active.clear();
  queue.length = 0;
  failed.clear();
  poses.clear();
  poseListeners.clear();
  restoring.clear();
  restoreAttempted.clear();
}

// ---------------------------------------------------------------------------
// The hover target
// ---------------------------------------------------------------------------

/** Mark an element to make it the hover target for every mascot inside it, whatever else encloses them. */
export const HOVER_SCOPE_ATTRIBUTE = "data-avatar-hover-scope";

const INTERACTIVE_ANCESTOR = "button, a, [role='button'], [role='option'], [role='menuitem'], [role='tab'], li";

/**
 * The element whose hover plays a tile's animation: an explicit
 * {@link HOVER_SCOPE_ATTRIBUTE} ancestor, else the nearest interactive ancestor
 * (the DM row, the member row), else the tile itself. In the sidebar the pointer
 * is on the row, not on a 24 px face, so the face alone is the wrong target.
 */
export function hoverScopeFor(el: Element): Element {
  return el.closest(`[${HOVER_SCOPE_ATTRIBUTE}]`) ?? el.closest(INTERACTIVE_ANCESTOR) ?? el;
}

/**
 * Calls `onTrigger` once each time the pointer enters the tile's hover scope (or
 * keyboard focus lands in it), and not again until it has left — so holding the
 * pointer still, or moving between children of the row, never retriggers.
 * Returns the cleanup.
 */
export function watchHoverScope(el: Element, onTrigger: () => void): () => void {
  const scope = hoverScopeFor(el);
  let armed = true;
  const fire = () => {
    if (!armed) return;
    armed = false;
    onTrigger();
  };
  const rearm = () => {
    armed = true;
  };
  const onFocusIn = (event: Event) => {
    const target = event.target as Element | null;
    // Only keyboard focus: clicking a row also focuses it, and that is not a hover.
    if (target?.matches?.(":focus-visible")) fire();
  };
  scope.addEventListener("pointerenter", fire);
  scope.addEventListener("pointerleave", rearm);
  scope.addEventListener("focusin", onFocusIn);
  scope.addEventListener("focusout", rearm);
  return () => {
    scope.removeEventListener("pointerenter", fire);
    scope.removeEventListener("pointerleave", rearm);
    scope.removeEventListener("focusin", onFocusIn);
    scope.removeEventListener("focusout", rearm);
  };
}
