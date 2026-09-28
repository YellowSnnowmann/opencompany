// @vitest-environment jsdom
//
// `PoseMascot`, the tile every mascot on a mass-render surface is: it rests on a
// settled frame (an `<img>`, no canvas), captures that frame once per look
// through a hidden live instance, and plays a one-shot reaction when the
// enclosing row is hovered. Mocked at the `mascot-avatar` boundary — jsdom cannot
// draw Rive — so what is pinned here is the contract around the runtime: when a
// capture is mounted and what it is asked for, what the tile shows before and
// after the frame exists, and exactly when the reaction plays and re-arms. That
// the captured frame is the *settled* one is `mascot-avatar-settle.test.ts`'s
// job, and how it looks is only ever settled in a real browser.

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const live = vi.hoisted(() => ({
  reduced: false,
  instances: [] as { costume?: string; skinColor?: string; handColor?: string; mode?: string; onSettled?: (url: string) => void }[],
}));

vi.mock("@/components/mascot-avatar", async () => {
  const { createElement: h, useEffect } = await import("react");
  return {
    usePrefersReducedMotion: () => live.reduced,
    LiveMascot: (props: (typeof live.instances)[number]) => {
      useEffect(() => {
        live.instances.push(props);
        return () => {
          live.instances.splice(live.instances.indexOf(props), 1);
        };
      });
      return h("canvas", { "data-testid": "live", "data-costume": props.costume ?? "" });
    },
  };
});

const { PoseMascot } = await import("@/components/mascot-pose");
const pose = await import("@/lib/mascot-pose");

let container: HTMLDivElement;
let root: Root;
const animate = vi.fn();

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function mount(props: Record<string, any> = {}) {
  act(() => {
    root.render(
      createElement(
        "button",
        { id: "row" },
        createElement(PoseMascot, { "data-testid": "tile", ...props } as never),
      ),
    );
  });
}

const tile = () => container.querySelector<HTMLElement>('[data-testid="tile"]')!;
const row = () => container.querySelector<HTMLElement>("#row")!;
const imgs = () => Array.from(tile().querySelectorAll("img"));
const liveCanvases = () => document.body.querySelectorAll('[data-testid="live"]').length;
const enter = () => act(() => void row().dispatchEvent(new Event("pointerenter")));
const leave = () => act(() => void row().dispatchEvent(new Event("pointerleave")));
const advance = (ms: number) => act(() => void vi.advanceTimersByTime(ms));

const REST = "data:image/png;base64,REST";
const HOVER = "data:image/png;base64,HOVER";
const restKey = (costume?: string) => pose.mascotPoseKey(costume, undefined, undefined);
const hoverKey = () => pose.mascotPoseKey(pose.MASCOT_HOVER_COSTUME, undefined, undefined);

beforeEach(() => {
  vi.useFakeTimers();
  live.reduced = false;
  live.instances.length = 0;
  animate.mockClear();
  (HTMLElement.prototype as unknown as { animate: typeof animate }).animate = animate;
  pose.resetMascotPoses();
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  pose.resetMascotPoses();
  vi.useRealTimers();
});

describe("before the settled frame exists", () => {
  it("draws nothing, so the tile's initials underneath stay the only thing on screen", () => {
    mount({ costume: "glass2" });
    expect(tile().getAttribute("data-mascot-pose")).toBe("pending");
    expect(imgs()).toHaveLength(0);
  });

  it("mounts one hidden live instance, asked for exactly this look, held still", () => {
    mount({ costume: "glass2", skinColor: "coral", handColor: "forest" });
    expect(liveCanvases()).toBe(1);
    expect(live.instances[0]).toMatchObject({ costume: "glass2", skinColor: "coral", handColor: "forest", mode: "static" });
  });

  it("puts the capture in the body, not in the tile, where a clipping ancestor cannot pause it", () => {
    mount({ costume: "glass2" });
    expect(tile().querySelector("canvas")).toBeNull();
    expect(document.body.querySelector('[data-testid="live"]')).not.toBeNull();
  });
});

describe("when the settled frame arrives", () => {
  it("shows it as an image and lets the live instance go", () => {
    mount({ costume: "glass2" });
    act(() => live.instances[0].onSettled!(REST));
    expect(imgs().map((i) => i.getAttribute("src"))).toContain(REST);
    expect(tile().getAttribute("data-mascot-pose")).toBe("rest");
    // The resting capture is gone; only the background hover-costume one may remain.
    expect(live.instances.some((i) => i.costume === "glass2")).toBe(false);
  });

  it("is shared: a second tile with the same look needs no capture of its own", () => {
    mount({ costume: "glass2" });
    act(() => live.instances[0].onSettled!(REST));
    act(() => root.unmount());
    root = createRoot(container);
    mount({ costume: "glass2" });
    expect(tile().getAttribute("data-mascot-pose")).toBe("rest");
    expect(live.instances.filter((i) => i.costume === "glass2")).toHaveLength(0);
  });

  it("then captures the hover costume in the background — never before the resting frame", () => {
    mount({ costume: "glass2" });
    expect(live.instances.map((i) => i.costume)).toEqual(["glass2"]);
    act(() => live.instances[0].onSettled!(REST));
    expect(live.instances.map((i) => i.costume)).toEqual([pose.MASCOT_HOVER_COSTUME]);
  });

  it("does not capture a hover costume for a teammate who already wears it", () => {
    mount({ costume: pose.MASCOT_HOVER_COSTUME });
    act(() => live.instances[0].onSettled!(REST));
    expect(liveCanvases()).toBe(0);
    expect(imgs()).toHaveLength(1);
  });
});

describe("the reaction on hover", () => {
  const cached = (costume = "glass2") => {
    act(() => {
      pose.publishMascotPose(restKey(costume), REST);
      pose.publishMascotPose(hoverKey(), HOVER);
    });
    mount({ costume });
  };

  it("starts at the enclosing row's pointer-enter — not the tile's — and swaps to the hover costume", () => {
    cached();
    expect(tile().getAttribute("data-mascot-pose")).toBe("rest");
    enter();
    expect(tile().getAttribute("data-mascot-pose")).toBe("hover");
    expect(animate).toHaveBeenCalledTimes(1);
  });

  it("returns to the identical resting frame once it has played", () => {
    cached();
    enter();
    advance(900);
    expect(tile().getAttribute("data-mascot-pose")).toBe("rest");
    expect(imgs()[0].getAttribute("src")).toBe(REST);
  });

  it("plays once while the pointer stays — no loop, no retrigger", () => {
    cached();
    enter();
    advance(5000);
    enter();
    advance(5000);
    expect(animate).toHaveBeenCalledTimes(1);
    expect(tile().getAttribute("data-mascot-pose")).toBe("rest");
  });

  it("plays again after the pointer has left and come back", () => {
    cached();
    enter();
    advance(1100);
    leave();
    enter();
    expect(animate).toHaveBeenCalledTimes(2);
    expect(tile().getAttribute("data-mascot-pose")).toBe("hover");
  });

  it("ignores a re-entry while a reaction is still playing", () => {
    cached();
    enter();
    advance(300);
    leave();
    enter();
    expect(animate).toHaveBeenCalledTimes(1);
  });

  it("with no hover frame yet, still pops — it just has nothing to swap to", () => {
    act(() => pose.publishMascotPose(restKey("glass2"), REST));
    mount({ costume: "glass2" });
    enter();
    expect(animate).toHaveBeenCalledTimes(1);
    expect(tile().getAttribute("data-mascot-pose")).toBe("rest");
  });

  it("for a teammate already in the hover costume, is the pop alone", () => {
    act(() => pose.publishMascotPose(restKey(pose.MASCOT_HOVER_COSTUME), REST));
    mount({ costume: pose.MASCOT_HOVER_COSTUME });
    enter();
    expect(animate).toHaveBeenCalledTimes(1);
    expect(imgs()).toHaveLength(1);
    expect(tile().getAttribute("data-mascot-pose")).toBe("rest");
  });

  it("does not react before there is a resting frame to react from", () => {
    mount({ costume: "glass2" });
    enter();
    expect(animate).not.toHaveBeenCalled();
  });
});

describe("what never reacts", () => {
  const cachedThenMount = (props: Record<string, unknown>) => {
    act(() => {
      pose.publishMascotPose(restKey("glass2"), REST);
      pose.publishMascotPose(hoverKey(), HOVER);
    });
    mount({ costume: "glass2", ...props });
  };

  it("a static teammate, whatever the surface asks for", () => {
    cachedThenMount({ mode: "static", animate: "hover" });
    expect(tile().getAttribute("data-mascot-trigger")).toBe("none");
    enter();
    expect(animate).not.toHaveBeenCalled();
    expect(tile().getAttribute("data-mascot-pose")).toBe("rest");
  });

  it("reduced motion", () => {
    live.reduced = true;
    cachedThenMount({});
    expect(tile().getAttribute("data-mascot-trigger")).toBe("none");
    enter();
    expect(animate).not.toHaveBeenCalled();
  });

  it("a surface that asks for none", () => {
    cachedThenMount({ animate: "none" });
    enter();
    expect(animate).not.toHaveBeenCalled();
  });

  it("and none of them capture a hover costume they will never show", () => {
    act(() => pose.publishMascotPose(restKey("glass2"), REST));
    mount({ costume: "glass2", mode: "static" });
    expect(liveCanvases()).toBe(0);
  });
});

describe("cleanup", () => {
  it("stops listening to the row when the tile unmounts", () => {
    act(() => {
      pose.publishMascotPose(restKey("glass2"), REST);
      pose.publishMascotPose(hoverKey(), HOVER);
    });
    mount({ costume: "glass2" });
    const el = row();
    act(() => root.unmount());
    root = createRoot(container);
    act(() => void el.dispatchEvent(new Event("pointerenter")));
    expect(animate).not.toHaveBeenCalled();
  });
});
