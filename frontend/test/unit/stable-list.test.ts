// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { useStableList, type StableList } from "@/hooks/use-stable-list";

/**
 * Issue #1414: the approvals queue must not reflow under the pointer on the 5s
 * poll. `useStableList` freezes the rendered order — and holds removals — while
 * the operator is interacting with the queue, so a poll that reorders or drops
 * a card cannot slide a different card's button under an in-flight click.
 *
 * A jsdom render rather than a pure test because the invariant is "the rendered
 * order does not change across a prop update", which only a rendered list has.
 * The interaction handlers are called directly rather than dispatched as DOM
 * events — they are plain callbacks, and calling them avoids jsdom's synthetic
 * pointer-event gaps while exercising exactly the freeze/thaw the view wires up.
 */

let captured: StableList<string>;

function Harness({ items }: { items: string[] }) {
  const stable = useStableList(items);
  captured = stable;
  return createElement(
    "div",
    { "data-testid": "queue", ...stable.containerProps },
    stable.items.map((id) => createElement("div", { key: id, "data-id": id }, id)),
  );
}

let container: HTMLDivElement;
let root: Root;

async function renderItems(items: string[]) {
  await act(async () => {
    root.render(createElement(Harness, { items }));
  });
}

/** The ids the DOM is actually showing, top to bottom. */
function domOrder(): string[] {
  return Array.from(container.querySelectorAll("[data-id]")).map((el) =>
    el.getAttribute("data-id"),
  ) as string[];
}

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

describe("useStableList (#1414)", () => {
  it("holds the rendered order while the pointer is over the queue", async () => {
    await renderItems(["a", "b", "c"]);
    expect(domOrder()).toEqual(["a", "b", "c"]);

    // Pointer enters the queue — the operator is now aiming a click.
    await act(async () => captured.containerProps.onPointerEnter());

    // A poll arrives that removes the top card and reorders the rest. Left to
    // React this would pull every card up under the pointer.
    await renderItems(["c", "b"]);

    // Frozen: the DOM must still show exactly what it did before the poll.
    expect(domOrder()).toEqual(["a", "b", "c"]);
  });

  it("reconciles to the latest poll once the pointer leaves", async () => {
    await renderItems(["a", "b", "c"]);
    await act(async () => captured.containerProps.onPointerEnter());
    await renderItems(["c", "b"]);
    expect(domOrder()).toEqual(["a", "b", "c"]);

    // Pointer leaves — nothing is being aimed at, so the held order is dropped
    // and the newest poll takes over.
    await act(async () => captured.containerProps.onPointerLeave());
    expect(domOrder()).toEqual(["c", "b"]);
  });

  it("flows live when nothing is interacting", async () => {
    await renderItems(["a", "b", "c"]);
    // No pointer, no focus: a poll updates the list immediately.
    await renderItems(["c", "b"]);
    expect(domOrder()).toEqual(["c", "b"]);
  });
});
