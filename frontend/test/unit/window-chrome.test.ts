// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { WindowControlsInset, WindowDragBar } from "@/components/window-chrome";

/**
 * The desktop window's own chrome.
 *
 * `tauri.conf.json` runs the main window with `titleBarStyle: "Overlay"` and
 * `hiddenTitle: true` again: macOS draws no title bar of its own and floats
 * the traffic lights over the web content, so the console draws a band that
 * opts into dragging and reserves 72px so the lights are not sitting on a
 * control underneath them.
 *
 * On macOS desktop, both pieces render: {@link WindowDragBar}'s drag region
 * and {@link WindowControlsInset}'s reserved, also-draggable strip. Elsewhere
 * — a browser (no `__TAURI__`), or a desktop platform that is not macOS (an
 * `Overlay` title bar is a macOS-only style) — neither renders anything,
 * because there is no window to drag, or the native title bar already has
 * the lights and a band here would only eat the top of every page.
 */

let host: HTMLDivElement;
let root: Root | null = null;

/** Present the runtime as the Tauri desktop, on the given platform. */
function asDesktop(platform: string) {
  (window as unknown as Record<string, unknown>).__TAURI__ = {};
  Object.defineProperty(navigator, "platform", {
    configurable: true,
    value: platform,
  });
}

function render(node: Parameters<Root["render"]>[0]) {
  act(() => root!.render(node));
}

beforeEach(() => {
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root?.unmount());
  root = null;
  host.remove();
  delete (window as unknown as Record<string, unknown>).__TAURI__;
});

describe("the window drag band", () => {
  it("renders nothing in a browser", () => {
    // No `__TAURI__`: there is no window to drag, and a band here would only
    // eat the top of every page.
    render(createElement(WindowDragBar));
    expect(host.querySelector("[data-tauri-drag-region]")).toBeNull();
  });

  it("renders nothing on a desktop that keeps its native title bar", () => {
    // `titleBarStyle: "Overlay"` is a macOS-only style — Windows and Linux draw
    // their real title bar, so reserving a band would waste 28px for nothing.
    asDesktop("Win32");
    render(createElement(WindowDragBar));
    expect(host.querySelector("[data-tauri-drag-region]")).toBeNull();
  });

  it("renders the drag band on macOS desktop, where the title bar is an overlay", () => {
    // macOS draws no title bar of its own again (`titleBarStyle: "Overlay"`),
    // so this band is the only thing that opts the top of the window back
    // into being draggable.
    asDesktop("MacIntel");
    render(createElement(WindowDragBar));
    expect(host.querySelector("[data-tauri-drag-region]")).not.toBeNull();
  });
});

describe("the traffic-light inset", () => {
  it("reserves nothing where the lights do not float", () => {
    render(createElement(WindowControlsInset));
    expect(host.querySelector("[data-tauri-drag-region]")).toBeNull();

    asDesktop("Linux x86_64");
    render(createElement(WindowControlsInset));
    expect(host.querySelector("[data-tauri-drag-region]")).toBeNull();
  });

  it("reserves 72px on macOS desktop, where the traffic lights float over the content", () => {
    // The lights land in this strip, not in a title bar of their own, so it
    // has to exist and has to be draggable rather than a dead hole in the row.
    asDesktop("MacIntel");
    render(createElement(WindowControlsInset));
    expect(host.querySelector("[data-tauri-drag-region]")).not.toBeNull();
  });
});
