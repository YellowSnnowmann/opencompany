// @vitest-environment jsdom
//
// `SidebarTitleRow` is the pencil/`+` actions' only home, on every platform,
// plus the overlay title bar's payload (the drag band and traffic-light
// inset from `window-chrome.tsx`) on macOS desktop specifically. The two
// buttons must not disappear anywhere the overlay title bar does not apply —
// that was the bug (tinysweeper, high — `cross-platform-functionality`): an
// early `if (!usesOverlayTitleBar()) return null` took compose-message and
// add-agent away from the web console, Windows, and Linux, since
// `WindowDragBar`/`WindowControlsInset` already gate themselves and need no
// help from this component to disappear off-overlay.
//
// This pins: the row (and both buttons) rendering identically off the
// overlay platform, with only the drag band/inset absent there; render order
// on macOS, where all four pieces are present; the pointer-events split that
// lets blank title-row space fall through to the drag band while the
// buttons stay clickable (tinysweeper, medium); and that each button is
// wired to its own callback rather than the other's.

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { WINDOW_CHROME_HEIGHT } from "@/components/window-chrome";
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

describe("SidebarTitleRow off the overlay platform", () => {
  it("still renders the row and both buttons — only the drag band and inset are absent", () => {
    // No `__TAURI__`: a browser console, same precondition
    // `WindowDragBar`/`WindowControlsInset` each check for themselves.
    render(createElement(SidebarTitleRow, { onComposeMessage: vi.fn(), onAddAgent: vi.fn() }));

    const row = host.querySelector('[data-testid="sidebar-title-row"]');
    expect(row).not.toBeNull();
    expect((row as HTMLElement).style.height).toBe(`${WINDOW_CHROME_HEIGHT}px`);
    expect(host.querySelector('[data-testid="window-drag-bar"]')).toBeNull();
    expect(host.querySelector('[data-testid="window-controls-inset"]')).toBeNull();
    expect(host.querySelector('[aria-label="Start a conversation"]')).not.toBeNull();
    expect(host.querySelector('[aria-label="Add"]')).not.toBeNull();
  });

  it("still wires both buttons to their callbacks off the overlay platform", () => {
    const onComposeMessage = vi.fn();
    const onAddAgent = vi.fn();
    render(createElement(SidebarTitleRow, { onComposeMessage, onAddAgent }));

    click(host.querySelector('[aria-label="Start a conversation"]'));
    expect(onComposeMessage).toHaveBeenCalledTimes(1);
    click(host.querySelector('[aria-label="Add"]'));
    expect(onAddAgent).toHaveBeenCalledTimes(1);
  });
});

describe("SidebarTitleRow on macOS desktop, where the title bar is an overlay", () => {
  it("renders the drag band, the inset, and both buttons, in that order", () => {
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

  it("lets blank title-row space fall through to the drag band while the buttons stay clickable", () => {
    // Pins the two concrete, falsifiable claims the component's doc comment
    // makes (tinysweeper, medium — "pin the z-30 and pointer-events claims"):
    // the buttons' layer is pointer-events-none so hit testing on its own
    // blank space does not win against `WindowDragBar` underneath, with
    // pointer-events-auto restored on each button individually so they stay
    // clickable despite that.
    asDesktop("MacIntel");
    render(createElement(SidebarTitleRow, { onComposeMessage: vi.fn(), onAddAgent: vi.fn() }));

    const buttonsLayer = host.querySelector('[aria-label="Start a conversation"]')!.parentElement!;
    expect(buttonsLayer.className).toContain("pointer-events-none");
    expect(buttonsLayer.className).toContain("z-30");
    for (const label of ["Start a conversation", "Add"]) {
      const button = host.querySelector(`[aria-label="${label}"]`)!;
      expect(button.className).toContain("pointer-events-auto");
    }
  });
});
