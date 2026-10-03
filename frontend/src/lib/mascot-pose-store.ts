// Settled mascot frames, kept across page reloads.
//
// Without this every hard reload shows the initials for a couple of seconds
// while hidden live instances re-settle each look (`lib/mascot-pose.ts`). The
// reason it was not done earlier is the reason for everything in this file: a
// *bad* frame that gets stored is worse than a slow one, because it comes back
// on every reload until something notices. So:
//
// - Every stored frame is keyed by what drew it — the `.riv`, the renderer, the
//   capture logic and the device pixel ratio. Change any of those and the old
//   frames are not merely ignored, they are deleted.
// - `lib/mascot-frame.ts` validates on the way in and again on the way out; the
//   caller discards and re-captures on any doubt.
// - It is bounded (count and total size, least recently used first) and every
//   storage call is guarded — private mode, a full quota or disabled storage
//   just means the in-memory behaviour, which is what it was before.
//
// `localStorage`, not IndexedDB: it is synchronous, so a stored frame can be on
// the very first render instead of a promise later, and the whole cache is a few
// hundred kilobytes.

import { hasExpectedSize } from "@/lib/mascot-frame";

/**
 * First 12 hex digits of the SHA-256 of `public/avatars/mascot-animated.riv`.
 * `test/unit/mascot-pose-store.test.ts` hashes the real file and fails when this
 * is stale, so editing the animation cannot ship with old frames still trusted.
 */
export const MASCOT_RIV_FINGERPRINT = "23caba3ccf79";

/**
 * The Rive packages that render the capture (`@rive-app/canvas` and
 * `@rive-app/react-canvas`). The same test compares this to the installed
 * versions, so a dependency bump has to come here on purpose.
 */
export const MASCOT_RENDERER_VERSION = "canvas-2.43.1+react-canvas-4.35.0";

/**
 * Bump when the settle/capture logic changes what a captured frame is (the
 * thresholds and timings in `mascot-avatar.tsx`, the capture size). Nothing
 * detects that automatically, so it is a manual one-line change in the same
 * commit.
 */
export const MASCOT_CAPTURE_LOGIC_VERSION = 1;

/** The most frames kept. */
export const MAX_STORED_POSES = 64;
/** The most characters of stored frames kept — about a third of a typical 5 MB quota. */
export const MAX_STORED_CHARS = 1_500_000;
/** Recency updates are written this long after the last one, in one go. */
export const TOUCH_DELAY_MS = 1500;

const PREFIX = "oc.mascot-pose.";
const BASE = `v${MASCOT_CAPTURE_LOGIC_VERSION}-${MASCOT_RIV_FINGERPRINT}-${MASCOT_RENDERER_VERSION}`;
const CURRENT = `${PREFIX}${BASE}@`;

interface StoredPose {
  /** The PNG data URL. */
  u: string;
  /** When it was last used, epoch ms — what the trim orders by. */
  t: number;
}

/**
 * `localStorage`, or `null` where it isn't usable — access itself can throw
 * (Safari private mode, "block all cookies"), same as `lib/last-channel.ts`.
 */
function storage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

function keyFor(poseKey: string, dpr: number): string {
  return `${CURRENT}d${dpr.toFixed(2)}:${poseKey}`;
}

function parseEntry(raw: string | null): StoredPose | null {
  if (!raw) return null;
  try {
    const value = JSON.parse(raw) as Partial<StoredPose> | null;
    if (value && typeof value.u === "string" && typeof value.t === "number") {
      return { u: value.u, t: value.t };
    }
  } catch {
    // Falls through: a truncated or hand-edited entry is simply not an entry.
  }
  return null;
}

let purged = false;

/** Deletes every frame stored under a different asset, renderer or capture version. Once per page. */
function purgeStale(store: Storage): void {
  if (purged) return;
  purged = true;
  try {
    const stale: string[] = [];
    for (let i = 0; i < store.length; i += 1) {
      const key = store.key(i);
      if (key && key.startsWith(PREFIX) && !key.startsWith(CURRENT)) stale.push(key);
    }
    stale.forEach((key) => store.removeItem(key));
  } catch {
    // Nothing to do: stale frames that cannot be deleted are still never read (wrong key).
  }
}

/**
 * The stored frame for a pose at this device pixel ratio, or `null`. Anything
 * that does not parse, or is not a PNG of the right size, is deleted on the
 * spot. The pixels are *not* checked here — that is asynchronous
 * (`frameIsGood`) — so the caller must not show this until they have been.
 */
export function readStoredPose(poseKey: string, dpr: number): string | null {
  const store = storage();
  if (!store) return null;
  try {
    purgeStale(store);
    const key = keyFor(poseKey, dpr);
    const raw = store.getItem(key);
    if (raw === null) return null;
    const entry = parseEntry(raw);
    if (entry && hasExpectedSize(entry.u, dpr)) return entry.u;
    store.removeItem(key);
  } catch {
    // An unreadable store behaves like an empty one.
  }
  return null;
}

/** Drops one stored frame (it failed validation, or the look changed). */
export function removeStoredPose(poseKey: string, dpr: number): void {
  try {
    storage()?.removeItem(keyFor(poseKey, dpr));
  } catch {
    // Nothing to do.
  }
}

/** Removes the oldest frames until the store is within `maxCount` and `maxChars`. */
export function trimStoredPoses(store: Storage, maxCount = MAX_STORED_POSES, maxChars = MAX_STORED_CHARS): void {
  const entries: { key: string; t: number; chars: number }[] = [];
  for (let i = 0; i < store.length; i += 1) {
    const key = store.key(i);
    if (!key || !key.startsWith(PREFIX)) continue;
    const raw = store.getItem(key);
    const entry = parseEntry(raw);
    // An unparseable entry sorts first: it is worth nothing and costs space.
    entries.push({ key, t: entry ? entry.t : 0, chars: raw ? raw.length : 0 });
  }
  entries.sort((a, b) => a.t - b.t);
  let count = entries.length;
  let chars = entries.reduce((sum, e) => sum + e.chars, 0);
  for (const entry of entries) {
    if (count <= maxCount && chars <= maxChars) break;
    store.removeItem(entry.key);
    count -= 1;
    chars -= entry.chars;
  }
}

/**
 * Stores a frame the caller has already validated. Trims to the bounds after,
 * and if the browser refuses the write (quota) drops the older half of what is
 * stored and tries once more; after that it gives up, silently.
 */
export function writeStoredPose(poseKey: string, dpr: number, url: string): void {
  const store = storage();
  if (!store) return;
  const value = JSON.stringify({ u: url, t: Date.now() } satisfies StoredPose);
  try {
    purgeStale(store);
    try {
      store.setItem(keyFor(poseKey, dpr), value);
    } catch {
      trimStoredPoses(store, Math.floor(MAX_STORED_POSES / 2), Math.floor(MAX_STORED_CHARS / 2));
      store.setItem(keyFor(poseKey, dpr), value);
    }
    trimStoredPoses(store);
  } catch {
    // Persisting is an optimisation; the in-memory frame is unaffected.
  }
}

const pendingTouches = new Map<string, number>();
let touchTimer: ReturnType<typeof setTimeout> | undefined;

/**
 * Marks a stored frame as just used, so the trim keeps what is actually on
 * screen. Batched: many tiles restoring at once cost one round of writes, after
 * the page has settled, not synchronous I/O on the render path.
 */
export function touchStoredPose(poseKey: string, dpr: number): void {
  pendingTouches.set(keyFor(poseKey, dpr), Date.now());
  if (touchTimer !== undefined) return;
  touchTimer = setTimeout(() => {
    touchTimer = undefined;
    const store = storage();
    const touches = [...pendingTouches];
    pendingTouches.clear();
    if (!store) return;
    for (const [key, at] of touches) {
      try {
        const entry = parseEntry(store.getItem(key));
        if (entry) store.setItem(key, JSON.stringify({ u: entry.u, t: at } satisfies StoredPose));
      } catch {
        // Recency is best effort.
      }
    }
  }, TOUCH_DELAY_MS);
}

/** Test seam: forget the once-per-page purge flag and any queued recency writes. */
export function resetMascotPoseStore(): void {
  purged = false;
  pendingTouches.clear();
  if (touchTimer !== undefined) clearTimeout(touchTimer);
  touchTimer = undefined;
}
