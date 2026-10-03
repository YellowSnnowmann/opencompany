// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { ContentSurface } from "@/components/content-surface";

/**
 * The content sheet: the card half of the two-layer shell (issue #1178), unframed
 * since the sidebar became a floating card over a full-bleed page.
 *
 * Every page renders on this one card — there is no full-bleed escape hatch,
 * and the component's own docblock says why.
 *
 * These cases pin the contract: which classes the card carries, and that it
 * keeps the scroll container every view's `overflow-y-auto` depends on. They
 * cannot tell 12px from 1px; that is a fact about pixels, and
 * `test/e2e/shell-two-layer.spec.ts` measures it in a real browser. What they
 * catch is the frame, the radius or the edge quietly going missing, and
 * `min-h-0` being dropped — which moves every view's scroll to the window.
 */

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

function render() {
  act(() =>
    root.render(
      createElement(ContentSurface, { children: createElement("p", null, "page content") }),
    ),
  );
  const surface = container.querySelector<HTMLElement>('[data-testid="content-surface"]');
  expect(surface).not.toBeNull();
  return surface!;
}

describe("ContentSurface", () => {
  it("runs full-bleed under the floating sidebar: no frame, no card, no halo", () => {
    // The floating sidebar (`app-shell.tsx`) is the one card in the shell now.
    // The content runs the full window behind it, so the margins, rounded
    // corners, hairline and orbiting halo that framed it as a second card are
    // gone — and must not creep back, or the page reads as a card beside a card.
    const surface = render();
    const classes = surface.className.split(/\s+/);
    const frameClasses = surface.parentElement!.className.split(/\s+/);

    for (const cls of ["mr-(--frame-inset)", "mb-(--frame-inset)", "mt-0.5"]) {
      expect(frameClasses).not.toContain(cls);
    }
    expect(surface.parentElement!.querySelector(".content-orbit")).toBeNull();
    expect(classes).not.toContain("rounded-2xl");
    expect(classes).not.toContain("border-chrome-border");
    // Still the opaque sheet everything a page draws stacks on.
    expect(classes).toContain("bg-page");
  });

  it("is the scroll container every view depends on", () => {
    // A view's own `overflow-y-auto` only scrolls because this box refuses to
    // grow past its share of the shell. Losing `min-h-0` moves the scroll to the
    // window, which is a whole-app regression rather than a styling one.
    const classes = render().className.split(/\s+/);
    expect(classes).toEqual(
      expect.arrayContaining(["flex", "min-h-0", "flex-1", "overflow-hidden"]),
    );
  });

  it("renders its children", () => {
    expect(render().textContent).toBe("page content");
  });
});
