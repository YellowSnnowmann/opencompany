// @vitest-environment jsdom
//
// Settled mascot frames kept across reloads: the checks a frame must pass to be
// stored or shown (`lib/mascot-frame.ts`), the bounded, versioned storage
// (`lib/mascot-pose-store.ts`), and how the pose cache uses it
// (`lib/mascot-pose.ts`) — restore before capture, and never trust a bad frame.
// The Rive runtime is not involved; pixel reads go through an injected probe.

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  PERSISTED_MIN_SHARE,
  POSE_CAPTURE_PX,
  contentShare,
  expectedPosePx,
  frameIsGood,
  hasExpectedSize,
  pngSize,
} from "@/lib/mascot-frame";
import {
  MASCOT_CAPTURE_LOGIC_VERSION,
  MASCOT_RENDERER_VERSION,
  MASCOT_RIV_FINGERPRINT,
  MAX_STORED_CHARS,
  MAX_STORED_POSES,
  TOUCH_DELAY_MS,
  readStoredPose,
  removeStoredPose,
  resetMascotPoseStore,
  touchStoredPose,
  trimStoredPoses,
  writeStoredPose,
} from "@/lib/mascot-pose-store";
import {
  getMascotPose,
  mascotPoseKey,
  publishMascotPose,
  requestMascotPoseCapture,
  resetMascotPoses,
  setMascotFrameProbe,
  subscribeMascotPose,
} from "@/lib/mascot-pose";

const here = dirname(fileURLToPath(import.meta.url));
const frontend = resolve(here, "../..");

/** A PNG data URL whose header declares `width`×`height` (the rest is not a real image; nothing here decodes it). */
function pngUrl(width: number, height = width, salt = 0): string {
  const be = (n: number) => [(n >>> 24) & 255, (n >>> 16) & 255, (n >>> 8) & 255, n & 255];
  const bytes = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
    0, 0, 0, 13,
    0x49, 0x48, 0x44, 0x52,
    ...be(width),
    ...be(height),
    8, 6, 0, 0, 0,
    0, 0, 0, 0,
    salt & 255,
  ];
  return `data:image/png;base64,${btoa(String.fromCharCode(...bytes))}`;
}

const SIDE = expectedPosePx(1); // jsdom's devicePixelRatio is 1
const px = (n: number, fill: (i: number) => number[]) => {
  const out = new Uint8ClampedArray(n * 4);
  for (let i = 0; i < n; i += 1) out.set([...fill(i), 255], i * 4);
  return out;
};
/** Half the pixels differ from the corner: a mascot on a background. */
const MASCOT = px(256, (i) => (i % 2 ? [240, 240, 240] : [200, 100, 50]));
/** Nothing but background: a blank or ducked-out tile. */
const BLANK = px(256, () => [240, 240, 240]);
/** The rise-in's peek: ~25% of the tile (pixel 0, the reference background, stays background). */
const PEEK = px(256, (i) => (i % 4 === 1 ? [200, 100, 50] : [240, 240, 240]));

const flush = () => new Promise<void>((done) => setTimeout(done, 0));
const storedKeys = () => Object.keys(localStorage).filter((k) => k.startsWith("oc.mascot-pose."));

beforeEach(() => {
  localStorage.clear();
  resetMascotPoseStore();
  resetMascotPoses();
  setMascotFrameProbe(async () => MASCOT);
});

afterEach(() => {
  setMascotFrameProbe(null);
  resetMascotPoseStore();
  resetMascotPoses();
  localStorage.clear();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("what a PNG data URL declares", () => {
  it("reads the size from the header", () => {
    expect(pngSize(pngUrl(96))).toEqual({ width: 96, height: 96 });
    expect(pngSize(pngUrl(192, 96))).toEqual({ width: 192, height: 96 });
  });

  it("refuses anything that is not a PNG data URL", () => {
    expect(pngSize(undefined)).toBeNull();
    expect(pngSize(42)).toBeNull();
    expect(pngSize("")).toBeNull();
    expect(pngSize("data:image/webp;base64,AAAA")).toBeNull();
    expect(pngSize("data:image/png;base64,AAAA")).toBeNull(); // too short to hold a header
    expect(pngSize("data:image/png;base64," + "!".repeat(60))).toBeNull(); // not base64
    expect(pngSize("data:image/png;base64," + "A".repeat(60))).toBeNull(); // no signature
    // A real signature but the first chunk is not IHDR.
    const bad = atob(pngUrl(96).slice("data:image/png;base64,".length));
    const notIhdr = bad.slice(0, 12) + "IDAT" + bad.slice(16);
    expect(pngSize(`data:image/png;base64,${btoa(notIhdr)}`)).toBeNull();
  });

  it("expects the capture size times the device pixel ratio, with rounding slack", () => {
    expect(POSE_CAPTURE_PX).toBe(96);
    expect(hasExpectedSize(pngUrl(96), 1)).toBe(true);
    expect(hasExpectedSize(pngUrl(95), 1)).toBe(true);
    expect(hasExpectedSize(pngUrl(99), 1)).toBe(false);
    expect(hasExpectedSize(pngUrl(192), 2)).toBe(true);
    expect(hasExpectedSize(pngUrl(96), 2)).toBe(false); // a 1x frame on a 2x screen is soft
    expect(hasExpectedSize(pngUrl(120), 1.25)).toBe(true);
    expect(hasExpectedSize(pngUrl(96, 64), 1)).toBe(false); // not square
  });
});

describe("what a frame must show to be kept", () => {
  it("accepts a resting pose", async () => {
    expect(contentShare(MASCOT)).toBe(50);
    expect(await frameIsGood(pngUrl(SIDE), 1, async () => MASCOT)).toBe(true);
  });

  it("rejects a blank tile — which has an opaque background, so opaque-pixel count would pass it", async () => {
    expect(contentShare(BLANK)).toBe(0);
    expect(await frameIsGood(pngUrl(SIDE), 1, async () => BLANK)).toBe(false);
  });

  it("rejects the rise-in's peek: below the floor, above nothing", async () => {
    const share = contentShare(PEEK);
    expect(share).toBeGreaterThan(20);
    expect(share).toBeLessThan(PERSISTED_MIN_SHARE);
    expect(await frameIsGood(pngUrl(SIDE), 1, async () => PEEK)).toBe(false);
  });

  it("rejects a frame that will not decode, or a probe that fails, or the wrong size", async () => {
    expect(await frameIsGood(pngUrl(SIDE), 1, async () => null)).toBe(false);
    expect(
      await frameIsGood(pngUrl(SIDE), 1, async () => {
        throw new Error("decode failed");
      }),
    ).toBe(false);
    expect(await frameIsGood(pngUrl(50), 1, async () => MASCOT)).toBe(false);
    expect(await frameIsGood("not a data url", 1, async () => MASCOT)).toBe(false);
  });
});

describe("what makes a stored frame stale", () => {
  it("the .riv fingerprint is the shipped file's — editing the animation must invalidate stored frames", () => {
    const sha = createHash("sha256")
      .update(readFileSync(resolve(frontend, "public/avatars/mascot-animated.riv")))
      .digest("hex");
    expect(sha.startsWith(MASCOT_RIV_FINGERPRINT)).toBe(true);
  });

  it("the renderer version is the installed Rive packages' — a dependency bump must be deliberate", () => {
    const installed = (name: string) =>
      JSON.parse(readFileSync(resolve(frontend, "node_modules", name, "package.json"), "utf8")).version as string;
    expect(MASCOT_RENDERER_VERSION).toBe(
      `canvas-${installed("@rive-app/canvas")}+react-canvas-${installed("@rive-app/react-canvas")}`,
    );
  });

  it("frames stored under any other version are deleted, never read", () => {
    localStorage.setItem("oc.mascot-pose.v0-oldfingerprint-oldrenderer@d1.00:key", JSON.stringify({ u: pngUrl(SIDE), t: 1 }));
    localStorage.setItem("unrelated", "kept");
    writeStoredPose("key", 1, pngUrl(SIDE));
    expect(storedKeys().some((k) => k.includes("v0-oldfingerprint"))).toBe(false);
    expect(storedKeys()).toHaveLength(1);
    expect(localStorage.getItem("unrelated")).toBe("kept");
    expect(MASCOT_CAPTURE_LOGIC_VERSION).toBeGreaterThanOrEqual(1);
  });

  it("keeps frames for different device pixel ratios apart", () => {
    writeStoredPose("key", 1, pngUrl(96));
    writeStoredPose("key", 2, pngUrl(192));
    expect(readStoredPose("key", 1)).toBe(pngUrl(96));
    expect(readStoredPose("key", 2)).toBe(pngUrl(192));
    expect(readStoredPose("key", 3)).toBeNull();
  });
});

describe("the storage", () => {
  it("round-trips a frame and forgets it on request", () => {
    expect(readStoredPose("key", 1)).toBeNull();
    writeStoredPose("key", 1, pngUrl(SIDE));
    expect(readStoredPose("key", 1)).toBe(pngUrl(SIDE));
    removeStoredPose("key", 1);
    expect(readStoredPose("key", 1)).toBeNull();
  });

  it("deletes an entry that does not parse, or is not a frame of the right size, when it reads it", () => {
    writeStoredPose("key", 1, pngUrl(SIDE));
    const [storageKey] = storedKeys();
    localStorage.setItem(storageKey, "{truncated");
    expect(readStoredPose("key", 1)).toBeNull();
    expect(localStorage.getItem(storageKey)).toBeNull();

    localStorage.setItem(storageKey, JSON.stringify({ u: pngUrl(40), t: 1 }));
    expect(readStoredPose("key", 1)).toBeNull();
    expect(localStorage.getItem(storageKey)).toBeNull();

    localStorage.setItem(storageKey, JSON.stringify({ u: "javascript:alert(1)", t: 1 }));
    expect(readStoredPose("key", 1)).toBeNull();
    expect(localStorage.getItem(storageKey)).toBeNull();
  });

  it("trims the least recently used frames first, by count and by size", () => {
    vi.useFakeTimers();
    for (let i = 0; i < 5; i += 1) {
      vi.setSystemTime(1000 + i);
      writeStoredPose(`k${i}`, 1, pngUrl(SIDE, SIDE, i));
    }
    expect(storedKeys()).toHaveLength(5);
    trimStoredPoses(localStorage, 3, Number.MAX_SAFE_INTEGER);
    expect(storedKeys()).toHaveLength(3);
    expect(readStoredPose("k0", 1)).toBeNull();
    expect(readStoredPose("k1", 1)).toBeNull();
    expect(readStoredPose("k4", 1)).not.toBeNull();

    const oneEntry = localStorage.getItem(storedKeys()[0])!.length;
    trimStoredPoses(localStorage, 99, oneEntry * 2);
    expect(storedKeys()).toHaveLength(2);
    expect(readStoredPose("k4", 1)).not.toBeNull(); // the newest survives
    expect(MAX_STORED_POSES).toBeGreaterThan(0);
    expect(MAX_STORED_CHARS).toBeGreaterThan(0);
  });

  it("keeps what was just used: touching an old frame protects it from the trim", () => {
    vi.useFakeTimers();
    vi.setSystemTime(1000);
    writeStoredPose("old", 1, pngUrl(SIDE, SIDE, 1));
    vi.setSystemTime(2000);
    writeStoredPose("new", 1, pngUrl(SIDE, SIDE, 2));
    vi.setSystemTime(3000);
    touchStoredPose("old", 1);
    vi.advanceTimersByTime(TOUCH_DELAY_MS + 1);
    trimStoredPoses(localStorage, 1, Number.MAX_SAFE_INTEGER);
    expect(readStoredPose("old", 1)).not.toBeNull();
    expect(readStoredPose("new", 1)).toBeNull();
  });

  it("survives a full quota: drops older frames and retries, and never throws", () => {
    let calls = 0;
    const real = Storage.prototype.setItem;
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(function (this: Storage, key: string, value: string) {
      calls += 1;
      if (calls === 1) throw new DOMException("quota", "QuotaExceededError");
      return real.call(this, key, value);
    });
    expect(() => writeStoredPose("key", 1, pngUrl(SIDE))).not.toThrow();
    expect(readStoredPose("key", 1)).toBe(pngUrl(SIDE));

    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new DOMException("quota", "QuotaExceededError");
    });
    expect(() => writeStoredPose("other", 1, pngUrl(SIDE))).not.toThrow();
  });

  it("behaves as empty and does not throw where storage cannot be reached at all", () => {
    const own = Object.getOwnPropertyDescriptor(window, "localStorage");
    Object.defineProperty(window, "localStorage", {
      configurable: true,
      get() {
        throw new Error("SecurityError");
      },
    });
    try {
      expect(readStoredPose("key", 1)).toBeNull();
      expect(() => writeStoredPose("key", 1, pngUrl(SIDE))).not.toThrow();
      expect(() => removeStoredPose("key", 1)).not.toThrow();
      expect(() => touchStoredPose("key", 1)).not.toThrow();
    } finally {
      if (own) Object.defineProperty(window, "localStorage", own);
      else delete (window as unknown as Record<string, unknown>).localStorage;
    }
  });
});

describe("the pose cache with stored frames", () => {
  const key = mascotPoseKey("headphones", undefined, undefined);

  it("restores a stored frame on the first look — after checking its pixels, not before", async () => {
    writeStoredPose(key, 1, pngUrl(SIDE));
    const notify = vi.fn();
    subscribeMascotPose(key, notify);
    expect(getMascotPose(key)).toBeUndefined(); // not shown until validated
    await flush();
    expect(getMascotPose(key)).toBe(pngUrl(SIDE));
    expect(notify).toHaveBeenCalled();
  });

  it("does not start a capture for a look whose stored frame is being checked, and drops it once it is good", async () => {
    writeStoredPose(key, 1, pngUrl(SIDE));
    const grant = vi.fn();
    requestMascotPoseCapture(key, "rest", grant);
    expect(grant).not.toHaveBeenCalled();
    await flush();
    expect(getMascotPose(key)).toBe(pngUrl(SIDE));
    expect(grant).not.toHaveBeenCalled();
  });

  it("throws a poisoned frame away and lets the capture through: right size, blank pixels", async () => {
    writeStoredPose(key, 1, pngUrl(SIDE));
    setMascotFrameProbe(async () => BLANK);
    const grant = vi.fn();
    requestMascotPoseCapture(key, "rest", grant);
    expect(grant).not.toHaveBeenCalled();
    await flush();
    expect(getMascotPose(key)).toBeUndefined();
    expect(readStoredPose(key, 1)).toBeNull();
    expect(grant).toHaveBeenCalledTimes(1);
  });

  it("throws away garbage at the door, without a capture waiting on a check that will never come", async () => {
    writeStoredPose(key, 1, pngUrl(SIDE));
    localStorage.setItem(storedKeys()[0], JSON.stringify({ u: "data:image/png;base64,garbage", t: 1 }));
    const grant = vi.fn();
    requestMascotPoseCapture(key, "rest", grant);
    expect(grant).toHaveBeenCalledTimes(1); // nothing to validate, so no waiting
    expect(storedKeys()).toHaveLength(0);
  });

  it("captures at once when nothing is stored", () => {
    const grant = vi.fn();
    requestMascotPoseCapture(key, "rest", grant);
    expect(grant).toHaveBeenCalledTimes(1);
  });

  it("stores a fresh capture for the next visit, and a reload then shows it", async () => {
    publishMascotPose(key, pngUrl(SIDE));
    expect(getMascotPose(key)).toBe(pngUrl(SIDE)); // in memory at once
    await flush();
    expect(storedKeys()).toHaveLength(1);

    resetMascotPoses(); // the reload: memory gone, storage kept
    const notify = vi.fn();
    subscribeMascotPose(key, notify);
    await flush();
    expect(getMascotPose(key)).toBe(pngUrl(SIDE));
  });

  it("does not store a capture whose pixels fail the check", async () => {
    setMascotFrameProbe(async () => BLANK);
    publishMascotPose(key, pngUrl(SIDE));
    await flush();
    expect(getMascotPose(key)).toBe(pngUrl(SIDE)); // the page keeps what it captured
    expect(storedKeys()).toHaveLength(0); // the next visit does not
  });

  it("does not store a capture of the wrong size", async () => {
    publishMascotPose(key, pngUrl(40));
    await flush();
    expect(storedKeys()).toHaveLength(0);
  });

  it("works with storage disabled: captures, publishes, never throws", async () => {
    const own = Object.getOwnPropertyDescriptor(window, "localStorage");
    Object.defineProperty(window, "localStorage", {
      configurable: true,
      get() {
        throw new Error("SecurityError");
      },
    });
    try {
      const grant = vi.fn();
      requestMascotPoseCapture(key, "rest", grant);
      expect(grant).toHaveBeenCalledTimes(1);
      expect(() => publishMascotPose(key, pngUrl(SIDE))).not.toThrow();
      await flush();
      expect(getMascotPose(key)).toBe(pngUrl(SIDE));
    } finally {
      if (own) Object.defineProperty(window, "localStorage", own);
      else delete (window as unknown as Record<string, unknown>).localStorage;
    }
  });
});
