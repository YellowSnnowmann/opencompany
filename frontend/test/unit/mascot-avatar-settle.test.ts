// @vitest-environment jsdom
//
// How a static (or reduced-motion) mascot comes to hold one pose, and how every
// mascot shares one parsed `.riv`. Every number below was measured live against
// the shipped file, not chosen: see `docs/issue/mascot-profile-avatar/
// open-questions.md` for the rise-in trace (peek plateau ~24%, pop to ~53%,
// overshoot settling by ~1.3 s) and the idle duck-out (~4 s in, ~6 s period).
//
// Mocked at the `@rive-app/react-canvas` boundary like `mascot-avatar.test.ts`:
// jsdom has no canvas, so the fake Rive here hands the component a plain canvas
// element and the tests supply the pixels its probe reads.

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const fake = vi.hoisted(() => ({
  fileConstructed: 0,
  filePinned: 0,
  riveParams: [] as unknown[],
  vmiOptions: [] as unknown[],
  advance: null as null | ((event: { data: number }) => void),
  canvas: null as null | HTMLCanvasElement,
  toDataURL: vi.fn(() => "data:image/png;base64,SETTLED"),
}));

vi.mock("@rive-app/react-canvas", () => {
  class RiveFile {
    private readonly params: { onLoad?: () => void };
    constructor(params: { onLoad?: () => void }) {
      fake.fileConstructed += 1;
      this.params = params;
    }
    init() {
      this.params.onLoad?.();
      return Promise.resolve();
    }
    getInstance() {
      fake.filePinned += 1;
      return {};
    }
  }
  return {
    RiveFile,
    EventType: { Advance: "advance" },
    useRive: (params: unknown) => {
      fake.riveParams.push(params);
      return {
        rive: params
          ? {
              on: (_type: string, cb: (event: { data: number }) => void) => {
                fake.advance = cb;
              },
              off: () => {
                fake.advance = null;
              },
            }
          : null,
        canvas: params ? fake.canvas : null,
        RiveComponent: () => createElement("canvas", { "data-testid": "live-canvas" }),
      };
    },
    useViewModel: () => ({}),
    useViewModelInstance: (_vm: unknown, options: unknown) => {
      fake.vmiOptions.push(options);
      return {};
    },
    useViewModelInstanceNumber: () => ({ value: 1, setValue: vi.fn() }),
    useViewModelInstanceColor: () => ({ setRgb: vi.fn() }),
  };
});

const avatar = await import("@/components/mascot-avatar");
const { MascotAvatar } = avatar;

let container: HTMLDivElement;
let root: Root;
/**
 * What the 16×16 probe reads: every pixel is the background colour, and with
 * `visible` every second one (never the corner, which the measurement takes as
 * the background) is the mascot's.
 */
let frame: { visible: boolean };

function stubProbeCanvas() {
  HTMLCanvasElement.prototype.getContext = function () {
    return {
      clearRect() {},
      drawImage() {},
      getImageData() {
        const data = new Uint8ClampedArray(16 * 16 * 4);
        for (let i = 0; i < 256; i++) {
          const mascot = frame.visible && i % 2 === 1;
          data[i * 4] = mascot ? 100 : 240;
          data[i * 4 + 1] = mascot ? 60 : 240;
          data[i * 4 + 2] = mascot ? 60 : 240;
          data[i * 4 + 3] = 255;
        }
        return { data };
      },
    } as unknown as CanvasRenderingContext2D;
  } as unknown as typeof HTMLCanvasElement.prototype.getContext;
}

beforeEach(() => {
  vi.useFakeTimers();
  fake.riveParams.length = 0;
  fake.vmiOptions.length = 0;
  fake.advance = null;
  fake.toDataURL.mockClear();
  fake.canvas = document.createElement("canvas");
  fake.canvas.width = 40;
  fake.canvas.height = 40;
  fake.canvas.toDataURL = fake.toDataURL as unknown as HTMLCanvasElement["toDataURL"];
  frame = { visible: true };
  stubProbeCanvas();
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    writable: true,
    value: () => ({
      matches: false,
      addEventListener() {},
      removeEventListener() {},
      addListener() {},
      removeListener() {},
    }),
  });
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => {
    root.unmount();
  });
  container.remove();
  vi.useRealTimers();
});

/** Runs `seconds` of fake animation: Rive advances 0.12 s a frame, and the probe fires every 120 ms. */
async function play(seconds: number, each?: () => void) {
  for (let t = 0; t < seconds; t += 0.12) {
    each?.();
    await act(async () => {
      fake.advance?.({ data: 0.12 });
      await vi.advanceTimersByTimeAsync(avatar.PROBE_EVERY_MS);
    });
  }
}

describe("shouldKeepFrame — the rule each measured phase of the animation demands", () => {
  const { shouldKeepFrame, EARLIEST_KEEP_S, QUIET_WINDOW_END_S, STEADY_PROBES } = avatar;

  it("never keeps a frame while the mascot is still rising in, even a perfectly steady one", () => {
    // The peek plateau holds ~24% coverage for ~0.25 s: still, and not the pose.
    expect(shouldKeepFrame(0.7, true, STEADY_PROBES + 5)).toBe(false);
    expect(shouldKeepFrame(EARLIEST_KEEP_S - 0.01, true, STEADY_PROBES + 5)).toBe(false);
  });

  it("keeps any visible frame in the quiet window, moving or not — the costumes that never stop bobbing", () => {
    expect(shouldKeepFrame(EARLIEST_KEEP_S, true, 0)).toBe(true);
    expect(shouldKeepFrame(QUIET_WINDOW_END_S - 0.01, true, 0)).toBe(true);
  });

  it("never keeps a frame with the mascot ducked out of view", () => {
    expect(shouldKeepFrame(2.5, false, STEADY_PROBES)).toBe(false);
    expect(shouldKeepFrame(8, false, STEADY_PROBES)).toBe(false);
  });

  it("past the quiet window, only trusts a frame that has held still", () => {
    expect(shouldKeepFrame(QUIET_WINDOW_END_S, true, STEADY_PROBES - 1)).toBe(false);
    expect(shouldKeepFrame(9, true, STEADY_PROBES)).toBe(true);
  });
});

describe("the frame measurements", () => {
  const { contentShare, motion } = avatar;
  const px = (n: number, fn: (i: number) => [number, number, number]) => {
    const out = new Uint8ClampedArray(n * 4);
    for (let i = 0; i < n; i++) {
      const [r, g, b] = fn(i);
      out.set([r, g, b, 255], i * 4);
    }
    return out;
  };

  it("reads a frame that is all background as 0% and a half-filled one as 50%", () => {
    expect(contentShare(px(256, () => [240, 240, 240]))).toBe(0);
    expect(contentShare(px(256, (i) => (i % 2 ? [240, 240, 240] : [200, 100, 50])))).toBe(50);
  });

  it("measures no motion between identical frames and a lot between different ones", () => {
    const a = px(256, () => [10, 20, 30]);
    expect(motion(a, new Uint8ClampedArray(a))).toBe(0);
    expect(motion(a, px(256, () => [110, 20, 30]))).toBeGreaterThan(30);
  });
});

describe("MascotAvatar holds a static pose by keeping the settled frame, not by pausing", () => {
  it("swaps a static mascot for an <img> of the settled frame and drops the live canvas", async () => {
    await act(async () => {
      root.render(createElement(MascotAvatar, { mode: "static", costume: "cap" }));
    });
    expect(container.querySelector('[data-testid="live-canvas"]')).not.toBeNull();
    await play(3);
    const img = container.querySelector("img");
    expect(img?.getAttribute("src")).toBe("data:image/png;base64,SETTLED");
    expect(container.querySelector('[data-testid="live-canvas"]')).toBeNull();
    expect(fake.toDataURL).toHaveBeenCalledTimes(1);
  });

  it("does not keep a frame taken before the rise-in has finished", async () => {
    await act(async () => {
      root.render(createElement(MascotAvatar, { mode: "static" }));
    });
    await play(1.5);
    expect(container.querySelector("img")).toBeNull();
    expect(fake.toDataURL).not.toHaveBeenCalled();
  });

  it("keeps waiting while the mascot is ducked out, and keeps the frame once it is back", async () => {
    await act(async () => {
      root.render(createElement(MascotAvatar, { mode: "static" }));
    });
    frame.visible = false;
    await play(3);
    expect(container.querySelector("img")).toBeNull();
    frame.visible = true;
    await play(1);
    expect(container.querySelector("img")).not.toBeNull();
  });

  it("never freezes an animated mascot", async () => {
    await act(async () => {
      root.render(createElement(MascotAvatar, { mode: "animated" }));
    });
    await play(6);
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector('[data-testid="live-canvas"]')).not.toBeNull();
    expect(fake.toDataURL).not.toHaveBeenCalled();
  });

  it("starts over when the costume changes, so the new pose is what gets kept", async () => {
    await act(async () => {
      root.render(createElement(MascotAvatar, { mode: "static", costume: "cap" }));
    });
    await play(3);
    expect(container.querySelector("img")).not.toBeNull();
    await act(async () => {
      root.render(createElement(MascotAvatar, { mode: "static", costume: "habibi" }));
    });
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector('[data-testid="live-canvas"]')).not.toBeNull();
  });
});

describe("every mascot shares one parsed file", () => {
  it("builds the RiveFile once, pins it, and hands it to each instance — none loading it themselves", async () => {
    await act(async () => {
      root.render(
        createElement("div", null, [
          createElement(MascotAvatar, { key: "a", costume: "cap" }),
          createElement(MascotAvatar, { key: "b", costume: "habibi" }),
        ]),
      );
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10);
    });
    expect(fake.fileConstructed).toBe(1);
    // The permanent reference that keeps the runtime from releasing the file the
    // moment the last mascot on a page unmounts (found live: "Problem loading
    // file; may be corrupt!" on the next page).
    expect(fake.filePinned).toBe(1);
    const loaded = fake.riveParams.filter(Boolean) as { riveFile?: unknown; src?: unknown }[];
    expect(loaded.length).toBeGreaterThan(0);
    for (const params of loaded) {
      expect(params.riveFile).toBeDefined();
      expect(params.src).toBeUndefined();
    }
  });

  it("gives each instance its own ViewModel instance, never the shared default", async () => {
    await act(async () => {
      root.render(createElement(MascotAvatar, {}));
    });
    expect(fake.vmiOptions.length).toBeGreaterThan(0);
    for (const options of fake.vmiOptions as { useNew?: boolean; useDefault?: boolean }[]) {
      expect(options.useNew).toBe(true);
      expect(options.useDefault).toBeUndefined();
    }
  });
});
