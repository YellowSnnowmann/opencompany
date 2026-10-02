// What makes a captured mascot frame worth keeping.
//
// A settled pose is captured once per look and shared by every tile; persisting
// it (`lib/mascot-pose-store.ts`) means a bad frame — blank, half-risen, the
// wrong size — would come back on every reload until something noticed. So the
// same checks run on the way *in* to storage and on the way *out*, and they live
// here, with no Rive import, so the main bundle can use them without pulling in
// the lazy `mascot-avatar` chunk.

/**
 * What share (0–100) of a probe is not its own background colour.
 *
 * The mascot sits on an opaque artboard fill, so "how much of this frame is
 * something other than the corner pixel" says whether the character is on
 * screen at all: ~0 when ducked out, roughly half when it is up.
 */
export function contentShare(px: Uint8ClampedArray): number {
  let differing = 0;
  for (let i = 0; i < px.length; i += 4) {
    const delta = Math.abs(px[i] - px[0]) + Math.abs(px[i + 1] - px[1]) + Math.abs(px[i + 2] - px[2]);
    if (delta > 40) differing += 1;
  }
  return (differing / (px.length / 4)) * 100;
}

/**
 * CSS pixels on a side of the hidden capture instance. The captured frame is
 * this times the device pixel ratio, which is what a stored frame's size is
 * checked against.
 */
export const POSE_CAPTURE_PX = 96;

/**
 * A stored frame must show at least this share of non-background. A settled
 * mascot covers 48–61%, the rise-in's peek ~21–24% and a ducked-out or blank
 * frame ~0%, so 30 rejects everything that is not a resting pose with margin on
 * both sides. (Opaque-pixel count cannot do this job: the tile has an opaque
 * background, so an empty tile reads as fully opaque.)
 */
export const PERSISTED_MIN_SHARE = 30;

/** Side of the square a frame is downscaled to for measuring. */
const PROBE_PX = 16;
/** Slack, in pixels, between a frame's size and `POSE_CAPTURE_PX × dpr` (rounding in the renderer). */
const SIZE_TOLERANCE = 2;

const PNG_PREFIX = "data:image/png;base64,";
const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

/**
 * The pixel size a PNG data URL declares in its header, or `null` if it is not a
 * PNG data URL. Reads the 33 leading bytes only — no decoding — so it is cheap
 * enough to run synchronously on everything that comes out of storage.
 */
export function pngSize(url: unknown): { width: number; height: number } | null {
  if (typeof url !== "string" || !url.startsWith(PNG_PREFIX)) return null;
  const head = url.slice(PNG_PREFIX.length, PNG_PREFIX.length + 44);
  if (head.length < 44) return null;
  let bin: string;
  try {
    bin = atob(head);
  } catch {
    return null;
  }
  for (let i = 0; i < PNG_SIGNATURE.length; i += 1) {
    if (bin.charCodeAt(i) !== PNG_SIGNATURE[i]) return null;
  }
  if (bin.slice(12, 16) !== "IHDR") return null;
  const u32 = (at: number) =>
    ((bin.charCodeAt(at) << 24) | (bin.charCodeAt(at + 1) << 16) | (bin.charCodeAt(at + 2) << 8) | bin.charCodeAt(at + 3)) >>> 0;
  return { width: u32(16), height: u32(20) };
}

/** The frame size a capture produces at this device pixel ratio. */
export function expectedPosePx(dpr: number): number {
  return Math.round(POSE_CAPTURE_PX * dpr);
}

/** Whether `url` is a square PNG of the size a capture at `dpr` produces. */
export function hasExpectedSize(url: unknown, dpr: number): boolean {
  const size = pngSize(url);
  if (!size || size.width !== size.height) return false;
  return Math.abs(size.width - expectedPosePx(dpr)) <= SIZE_TOLERANCE;
}

/** Reads a frame's pixels downscaled to a probe, or `null` if it cannot be decoded. */
export type FrameProbe = (url: string) => Promise<Uint8ClampedArray | null>;

/** The browser's probe: decode, draw at {@link PROBE_PX}, read back. */
export const domFrameProbe: FrameProbe = async (url) => {
  if (typeof Image === "undefined" || typeof document === "undefined") return null;
  const img = new Image();
  img.src = url;
  try {
    await img.decode();
  } catch {
    return null;
  }
  const canvas = document.createElement("canvas");
  canvas.width = PROBE_PX;
  canvas.height = PROBE_PX;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) return null;
  ctx.drawImage(img, 0, 0, PROBE_PX, PROBE_PX);
  return ctx.getImageData(0, 0, PROBE_PX, PROBE_PX).data;
};

/**
 * Whether a frame is fit to store or to show: the right size, decodes, and has
 * a mascot in it. Any doubt is a `false` — the caller discards and re-captures.
 */
export async function frameIsGood(
  url: string,
  dpr: number,
  probe: FrameProbe = domFrameProbe,
): Promise<boolean> {
  if (!hasExpectedSize(url, dpr)) return false;
  try {
    const px = await probe(url);
    return px !== null && contentShare(px) >= PERSISTED_MIN_SHARE;
  } catch {
    return false;
  }
}
