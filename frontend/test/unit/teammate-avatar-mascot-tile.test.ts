// @vitest-environment jsdom
//
// What `TeammateAvatar` does with a `mascot:animated` teammate: mount a mascot
// component with that teammate's own look, but only while the tile is near the
// viewport — a transcript has a tile per message, so the number that matters is
// how many are near the screen, not how many exist — and pick which one by the
// surface's `animate` trigger (the settled-frame tile by default, the live loop
// only when asked for).

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const seen = vi.hoisted(() => ({ props: [] as Record<string, unknown>[] }));

// Both mascot components record what they were given and mount the same marker;
// `via` says which one the tile chose, which is the `animate` contract under test.
vi.mock("@/components/mascot-avatar", () => ({
  MascotAvatar: (props: Record<string, unknown>) => {
    seen.props.push({ ...props, via: "loop" });
    return createElement("span", { "data-testid": "mascot" });
  },
}));

vi.mock("@/components/mascot-pose", () => ({
  PoseMascot: (props: Record<string, unknown>) => {
    seen.props.push({ ...props, via: "pose" });
    return createElement("span", { "data-testid": "mascot" });
  },
}));

vi.mock("@/lib/console-context", () => ({
  useConsole: () => ({ client: null, company: null }),
}));

const { TeammateAvatar } = await import("@/components/teammate-avatar");

type ObserverCallback = (entries: { isIntersecting: boolean }[]) => void;

let container: HTMLDivElement;
let root: Root;
let observers: { callback: ObserverCallback; disconnected: boolean; options?: { rootMargin?: string } }[];

function installIntersectionObserver() {
  (globalThis as unknown as { IntersectionObserver: unknown }).IntersectionObserver = class {
    private readonly entry: (typeof observers)[number];
    constructor(callback: ObserverCallback, options?: { rootMargin?: string }) {
      this.entry = { callback, disconnected: false, options };
      observers.push(this.entry);
    }
    observe() {}
    disconnect() {
      this.entry.disconnected = true;
    }
  };
}

const mascotProps = {
  name: "Brand Designer",
  avatar: "mascot:animated",
  mascotCostume: "headphones",
  mascotSkinColor: "coral",
  mascotHandColor: "forest",
  mascotMode: "static",
} as const;

async function render(props: Record<string, unknown>) {
  await act(async () => {
    root.render(createElement(TeammateAvatar, props as never));
  });
  // Let the lazy chunk resolve.
  await act(async () => {
    await Promise.resolve();
  });
}

const mounted = () => container.querySelector('[data-testid="mascot"]') !== null;
const setIntersecting = async (isIntersecting: boolean) => {
  await act(async () => {
    observers.at(-1)!.callback([{ isIntersecting }]);
  });
};

beforeEach(() => {
  vi.useFakeTimers();
  seen.props.length = 0;
  observers = [];
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
  delete (globalThis as unknown as { IntersectionObserver?: unknown }).IntersectionObserver;
});

describe("a mascot tile's live canvas follows the viewport", () => {
  it("does not mount the mascot until the tile is near the viewport", async () => {
    installIntersectionObserver();
    await render(mascotProps);
    expect(mounted()).toBe(false);
    await setIntersecting(true);
    await act(async () => {
      await Promise.resolve();
    });
    expect(mounted()).toBe(true);
  });

  it("observes with a margin, so a tile is ready just before it scrolls in", async () => {
    installIntersectionObserver();
    await render(mascotProps);
    expect(observers.at(-1)!.options?.rootMargin).toBe("300px");
  });

  it("lets go of the canvas only after the tile has been out of range for a grace period", async () => {
    installIntersectionObserver();
    await render(mascotProps);
    await setIntersecting(true);
    await act(async () => {
      await Promise.resolve();
    });
    await setIntersecting(false);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });
    expect(mounted()).toBe(true);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(700);
    });
    expect(mounted()).toBe(false);
  });

  it("keeps the canvas when the tile comes back inside the grace period", async () => {
    installIntersectionObserver();
    await render(mascotProps);
    await setIntersecting(true);
    await act(async () => {
      await Promise.resolve();
    });
    await setIntersecting(false);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(800);
    });
    await setIntersecting(true);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(mounted()).toBe(true);
  });

  it("stops observing when the tile unmounts", async () => {
    installIntersectionObserver();
    await render(mascotProps);
    const observer = observers.at(-1)!;
    await act(async () => {
      root.unmount();
    });
    expect(observer.disconnected).toBe(true);
    root = createRoot(container);
  });

  it("treats everything as near where there is no IntersectionObserver", async () => {
    await render(mascotProps);
    expect(mounted()).toBe(true);
  });
});

describe("a mascot tile shows the teammate's own look", () => {
  it("forwards the costume, both colors and the mode to the mascot", async () => {
    await render(mascotProps);
    const last = seen.props.at(-1)!;
    expect(last).toMatchObject({
      costume: "headphones",
      skinColor: "coral",
      handColor: "forest",
      mode: "static",
    });
  });

  it("leaves the look undefined — the file's default — when the roster sent none", async () => {
    await render({ name: "Nova", avatar: "mascot:animated" });
    const last = seen.props.at(-1)!;
    expect(last.costume).toBeUndefined();
    expect(last.skinColor).toBeUndefined();
    expect(last.handColor).toBeUndefined();
    expect(last.mode).toBeUndefined();
  });

  it("mounts no mascot for a shipped flavour, an unmarked avatar, or a mark-only tile", async () => {
    await render({ name: "Nova", avatar: "tiny:teal" });
    expect(mounted()).toBe(false);
    await render({ name: "Nova" });
    expect(mounted()).toBe(false);
    await render({ ...mascotProps, markOnly: true });
    expect(mounted()).toBe(false);
  });
});

describe("a mascot tile picks its component by the surface's animate trigger", () => {
  it("defaults to the settled-frame tile that reacts on hover", async () => {
    await render({ name: "Nova", avatar: "mascot:animated" });
    expect(seen.props.at(-1)).toMatchObject({ via: "pose", animate: "hover" });
  });

  it("animate=\"loop\" mounts the live looping mascot, with no trigger passed down", async () => {
    await render({ name: "Nova", avatar: "mascot:animated", animate: "loop" });
    const last = seen.props.at(-1)!;
    expect(last.via).toBe("loop");
    expect(last.animate).toBeUndefined();
  });

  it("animate=\"none\" is the settled frame with no reaction", async () => {
    await render({ name: "Nova", avatar: "mascot:animated", animate: "none" });
    expect(seen.props.at(-1)).toMatchObject({ via: "pose", animate: "none" });
  });

  it("passes the teammate's own mode down either way, so a static teammate can stay still", async () => {
    await render({ ...mascotProps });
    expect(seen.props.at(-1)).toMatchObject({ via: "pose", mode: "static" });
    await render({ ...mascotProps, animate: "loop" });
    expect(seen.props.at(-1)).toMatchObject({ via: "loop", mode: "static" });
  });
});
