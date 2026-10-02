// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useFlipList, type FlipListOptions } from "@/hooks/use-flip-list";

/**
 * `useFlipList` slides keyed rows to their new slot when a list reorders.
 *
 * jsdom does no layout, so `offsetTop` is faked: each row sits at
 * `index * ROW` in whatever order it is rendered, which is exactly what a real
 * stacked list does. `Element.animate` is a spy so the assertions are on the
 * keyframes the hook asked for rather than on pixels.
 */

const ROW = 30;
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root;
let host: HTMLElement;
let animate: ReturnType<typeof vi.fn>;
let cancel: ReturnType<typeof vi.fn>;
let reduced = false;

/**
 * `shift` moves the whole list down, as collapsing a section above it does: the
 * list's own `offsetTop` and every row's (measured from the same positioned
 * ancestor, which jsdom leaves null) grow by the same amount.
 */
function Harness({ keys, options, shift = 0 }: { keys: string[]; options?: FlipListOptions; shift?: number }) {
  const rowRef = useFlipList(keys, options);
  return createElement(
    "ul",
    { "data-top": String(shift) },
    keys.map((key, index) =>
      createElement(
        "li",
        { key, ref: rowRef(key), "data-top": String(shift + index * ROW) },
        createElement("button", { type: "button", "data-testid": `row-${key}` }, key),
      ),
    ),
  );
}

function render(keys: string[], options?: FlipListOptions, shift = 0) {
  act(() => root.render(createElement(Harness, { keys, options, shift })));
}

beforeEach(() => {
  reduced = false;
  cancel = vi.fn();
  animate = vi.fn(() => ({ cancel, onfinish: null, oncancel: null }));
  Object.defineProperty(HTMLElement.prototype, "offsetTop", {
    configurable: true,
    get(this: HTMLElement) {
      return Number(this.getAttribute("data-top") ?? 0);
    },
  });
  (HTMLElement.prototype as unknown as { animate: unknown }).animate = animate;
  window.matchMedia = ((query: string) => ({
    matches: reduced && query.includes("reduce"),
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
  })) as unknown as typeof window.matchMedia;
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root.unmount());
  host.remove();
  delete (HTMLElement.prototype as unknown as { animate?: unknown }).animate;
});

const enabled = { disabled: false };

describe("useFlipList", () => {
  it("does not animate the first mount", () => {
    render(["a", "b", "c"], enabled);
    expect(animate).not.toHaveBeenCalled();
  });

  it("skips the commit that first enables it, then animates a reorder", () => {
    render(["a", "b", "c"], { disabled: true });
    render(["c", "a", "b"], { disabled: true });
    render(["b", "c", "a"], enabled);
    expect(animate).not.toHaveBeenCalled();
    render(["a", "b", "c"], enabled);
    expect(animate).toHaveBeenCalled();
  });

  it("never animates while disabled", () => {
    render(["a", "b", "c"], { disabled: true });
    render(["c", "a", "b"], { disabled: true });
    render(["b", "c", "a"], { disabled: true });
    expect(animate).not.toHaveBeenCalled();
  });

  it("does not animate under prefers-reduced-motion", () => {
    reduced = true;
    render(["a", "b", "c"], enabled);
    render(["c", "a", "b"], enabled);
    render(["b", "c", "a"], enabled);
    expect(animate).not.toHaveBeenCalled();
  });

  it("slides a moved row from its old slot to its new one with the right delta", () => {
    render(["a", "b", "c"], enabled);
    render(["a", "b", "c"], enabled);
    render(["c", "a", "b"], enabled);
    // c: 60 -> 0 starts +60px below its new slot; a and b each move down one row.
    const deltas = animate.mock.calls.map(
      ([frames]) => (frames as { transform: string }[])[0].transform,
    );
    expect(deltas).toHaveLength(3);
    expect(deltas).toContain("translateY(60px)");
    expect(deltas.filter((d) => d === `translateY(${-ROW}px)`)).toHaveLength(2);
    const frames = animate.mock.calls[0][0] as { transform: string }[];
    expect(frames[1].transform).toBe("none");
  });

  it("does not animate a row that is new to the list", () => {
    render(["a", "b"], enabled);
    render(["a", "b"], enabled);
    render(["n", "a", "b"], enabled);
    // n has no previous slot; a and b were pushed down one row.
    expect(animate).toHaveBeenCalledTimes(2);
  });

  it("does not animate rows whose slot did not change", () => {
    render(["a", "b", "c"], enabled);
    render(["a", "b", "c"], enabled);
    render(["a", "b", "c"], enabled);
    expect(animate).not.toHaveBeenCalled();
  });

  it("does not animate when something above the list moves every row equally", () => {
    // Collapsing the Channels section above the DM list: nothing re-sorted.
    render(["a", "b", "c"], enabled);
    render(["a", "b", "c"], enabled);
    render(["a", "b", "c"], enabled, -120);
    render(["a", "b", "c"], enabled, 40);
    expect(animate).not.toHaveBeenCalled();
  });

  it("still slides a real reorder that lands while the list has moved", () => {
    render(["a", "b", "c"], enabled);
    render(["a", "b", "c"], enabled);
    render(["c", "a", "b"], enabled, -120);
    const deltas = animate.mock.calls.map(
      ([frames]) => (frames as { transform: string }[])[0].transform,
    );
    expect(deltas).toHaveLength(3);
    expect(deltas).toContain("translateY(60px)");
  });

  it("cancels a running animation before starting the next one", () => {
    render(["a", "b"], enabled);
    render(["a", "b"], enabled);
    render(["b", "a"], enabled);
    expect(animate).toHaveBeenCalledTimes(2);
    render(["a", "b"], enabled);
    expect(cancel).toHaveBeenCalledTimes(2);
    expect(animate).toHaveBeenCalledTimes(4);
  });

  it("leaves a running slide alone when a commit renders the same order", () => {
    render(["a", "b"], enabled);
    render(["a", "b"], enabled);
    render(["b", "a"], enabled);
    expect(animate).toHaveBeenCalledTimes(2);
    // An unrelated re-render mid-slide: no row changed slot.
    render(["b", "a"], enabled);
    expect(cancel).not.toHaveBeenCalled();
    expect(animate).toHaveBeenCalledTimes(2);
  });

  it("keeps focus on the row's button across a reorder", () => {
    render(["a", "b", "c"], enabled);
    const button = host.querySelector<HTMLButtonElement>('[data-testid="row-c"]')!;
    button.focus();
    expect(document.activeElement).toBe(button);
    render(["c", "a", "b"], enabled);
    expect(document.activeElement).toBe(host.querySelector('[data-testid="row-c"]'));
    expect(host.querySelector('[data-testid="row-c"]')).toBe(button);
  });
});
