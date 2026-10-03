// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { MessageComposer } from "@/views/room/MessageComposer";

/**
 * Picking a conversation puts the cursor in its composer: `RoomView` passes the
 * open channel's id as `focusKey`, and the composer focuses its input whenever
 * that changes — for a fine pointer only, since on a touch screen focusing an
 * input raises the keyboard over the transcript that was just opened.
 */

let container: HTMLDivElement;
let root: Root;

function pointer(fine: boolean) {
  window.matchMedia = ((query: string) => ({
    matches: query === "(pointer: fine)" ? fine : false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
}

function render(focusKey: string | undefined) {
  act(() =>
    root.render(createElement(MessageComposer, { placeholder: "Message", onSend: () => {}, focusKey })),
  );
}

const textarea = () => container.querySelector("textarea")!;

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

describe("the composer takes focus when a conversation opens", () => {
  it("focuses on open and again on every switch", () => {
    pointer(true);
    render("general");
    expect(document.activeElement).toBe(textarea());
    act(() => textarea().blur());
    render("dm:ada");
    expect(document.activeElement).toBe(textarea());
  });

  it("does not refocus when the same conversation re-renders", () => {
    pointer(true);
    render("general");
    act(() => textarea().blur());
    render("general");
    expect(document.activeElement).not.toBe(textarea());
  });

  it("leaves focus alone without a focusKey, and on a touch screen", () => {
    pointer(true);
    render(undefined);
    expect(document.activeElement).not.toBe(textarea());
    pointer(false);
    render("general");
    expect(document.activeElement).not.toBe(textarea());
  });
});
