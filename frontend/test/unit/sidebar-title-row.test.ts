// @vitest-environment jsdom
//
// `SidebarTitleRow` is the overlay title bar's payload — the drag band and
// the traffic-light inset from `window-chrome.tsx`, plus the pencil and `+`
// buttons that sit beside them. Both are gated on the same
// `usesOverlayTitleBar()` check, so this pins the two things that check does
// not already cover on its own: that the row renders nothing where the
// overlay title bar does not apply, and that each button is wired to its own
// callback rather than the other's (a copy-paste `onClick` would still render
// correctly and only fail at the click).

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { SidebarTitleRow } from "@/components/sidebar-title-row";

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

function click(el: Element | null) {
  if (!el) throw new Error("no such element");
  act(() => {
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

describe("SidebarTitleRow", () => {
  it("renders nothing where the overlay title bar does not apply", () => {
    // No `__TAURI__`: a browser console, same as `WindowDragBar`/
    // `WindowControlsInset` on their own.
    render(createElement(SidebarTitleRow, { onComposeMessage: vi.fn(), onAddAgent: vi.fn() }));
    expect(host.querySelector('[data-testid="sidebar-title-row"]')).toBeNull();
  });

  it("renders the drag band, the inset, and both buttons, in that order, on macOS desktop", () => {
    asDesktop("MacIntel");
    render(createElement(SidebarTitleRow, { onComposeMessage: vi.fn(), onAddAgent: vi.fn() }));

    const row = host.querySelector('[data-testid="sidebar-title-row"]');
    expect(row).not.toBeNull();
    // Render order matters here, not just presence: `WindowDragBar` has to
    // precede the buttons in the DOM for its doc-comment's "after the drag
    // band" to be a true statement about what is actually painted beneath
    // the buttons' explicit `z-30`.
    const tags = Array.from(row!.querySelectorAll("[data-testid], button")).map(
      (el) => el.getAttribute("data-testid") ?? el.tagName.toLowerCase(),
    );
    expect(tags).toEqual(["window-drag-bar", "window-controls-inset", "button", "button"]);
  });

  it("wires the pencil to onComposeMessage and the + to onAddAgent, not to each other", () => {
    asDesktop("MacIntel");
    const onComposeMessage = vi.fn();
    const onAddAgent = vi.fn();
    render(createElement(SidebarTitleRow, { onComposeMessage, onAddAgent }));

    click(host.querySelector('[aria-label="Start a conversation"]'));
    expect(onComposeMessage).toHaveBeenCalledTimes(1);
    expect(onAddAgent).not.toHaveBeenCalled();

    click(host.querySelector('[aria-label="Add"]'));
    expect(onAddAgent).toHaveBeenCalledTimes(1);
    expect(onComposeMessage).toHaveBeenCalledTimes(1);
  });
});
