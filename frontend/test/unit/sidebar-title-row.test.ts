// @vitest-environment jsdom
//
// `SidebarTitleRow` is the overlay title bar's payload — the drag band and
// the traffic-light inset from `window-chrome.tsx`, plus a pencil and a `+`
// that sit beside them, each a `DropdownMenu` with two items (the pencil:
// "...in a channel" / "...with the agent"; the `+`: "Create a new channel" /
// "Create a new agent" — PR #2545's original split, relocated here). Both
// triggers are gated on the same `usesOverlayTitleBar()` check (see the
// component's module doc for why they share it rather than rendering
// everywhere — two e2e specs pin their absence off this platform:
// `sidebar-conversations-layout.spec.ts`'s "no new-conversation doors" and
// `connections-authority.spec.ts`'s "offered nothing that changes it"). This
// pins the things that check does not already cover on its own: that the row
// renders nothing where the overlay title bar does not apply, and that each
// menu item is wired to its own callback rather than one of the other three
// (a copy-paste `onClick` would still render correctly and only fail at the
// click).

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { SidebarTitleRow } from "@/components/sidebar-title-row";

let host: HTMLDivElement;
let root: Root | null = null;

// `navigator.platform` is a shared global `defineProperty` can only override,
// never scope to one test — jsdom defines it on the prototype, so `asDesktop`
// shadows that with an own property on `navigator` itself. Restoring means
// deleting that own property in `afterEach` below, not redefining it, so the
// prototype's own getter answers again rather than leaking "MacIntel" into
// whatever runs next in this worker (tinysweeper, medium).
const hadOwnPlatform = Object.prototype.hasOwnProperty.call(navigator, "platform");

/** Present the runtime as the Tauri desktop, on the given platform. */
function asDesktop(platform: string) {
  (window as unknown as Record<string, unknown>).__TAURI__ = {};
  Object.defineProperty(navigator, "platform", {
    configurable: true,
    value: platform,
  });
}

/** Undoes `asDesktop`'s override, leaving `navigator.platform` as it was. */
function restorePlatform() {
  if (!hadOwnPlatform) delete (navigator as unknown as Record<string, unknown>).platform;
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
  restorePlatform();
});

function click(el: Element | null) {
  if (!el) throw new Error("no such element");
  act(() => {
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

/** Both menus portal their content to `document.body`, not into `host`. */
function menuItem(label: string): Element | null {
  return Array.from(document.body.querySelectorAll('[role="menuitem"]')).find(
    (el) => el.textContent === label,
  ) ?? null;
}

const NOOP_PROPS = {
  onStartChannelConversation: () => {},
  onComposeMessage: () => {},
  onCreateChannel: () => {},
  onAddAgent: () => {},
};

describe("SidebarTitleRow", () => {
  it("renders nothing where the overlay title bar does not apply", () => {
    // No `__TAURI__`: a browser console, same as `WindowDragBar`/
    // `WindowControlsInset` on their own — and the configuration
    // `sidebar-conversations-layout.spec.ts`/`connections-authority.spec.ts`
    // assert neither trigger exists in.
    render(createElement(SidebarTitleRow, NOOP_PROPS));
    expect(host.querySelector('[data-testid="sidebar-title-row"]')).toBeNull();
    expect(host.querySelector('[aria-label="Start a conversation"]')).toBeNull();
    expect(host.querySelector('[aria-label="Add"]')).toBeNull();
  });

  it("renders the drag band, the inset, and both menu triggers, in that order, on macOS desktop", () => {
    asDesktop("MacIntel");
    render(createElement(SidebarTitleRow, NOOP_PROPS));

    const row = host.querySelector('[data-testid="sidebar-title-row"]');
    expect(row).not.toBeNull();
    // Render order matters here, not just presence: `WindowDragBar` has to
    // precede the triggers in the DOM for its doc-comment's "after the drag
    // band" to be a true statement about what is actually painted beneath
    // the triggers' explicit `z-30`.
    const tags = Array.from(row!.querySelectorAll("[data-testid], button")).map(
      (el) => el.getAttribute("data-testid") ?? el.tagName.toLowerCase(),
    );
    expect(tags).toEqual(["window-drag-bar", "window-controls-inset", "button", "button"]);
  });

  it("the pencil's menu wires its two items to onStartChannelConversation and onComposeMessage, not to each other or the + menu's pair", () => {
    asDesktop("MacIntel");
    const onStartChannelConversation = vi.fn();
    const onComposeMessage = vi.fn();
    const onCreateChannel = vi.fn();
    const onAddAgent = vi.fn();
    render(
      createElement(SidebarTitleRow, {
        onStartChannelConversation,
        onComposeMessage,
        onCreateChannel,
        onAddAgent,
      }),
    );

    click(host.querySelector('[aria-label="Start a conversation"]'));
    click(menuItem("Start a conversation with the agent"));
    expect(onComposeMessage).toHaveBeenCalledTimes(1);
    expect(onStartChannelConversation).not.toHaveBeenCalled();
    expect(onCreateChannel).not.toHaveBeenCalled();
    expect(onAddAgent).not.toHaveBeenCalled();

    click(host.querySelector('[aria-label="Start a conversation"]'));
    click(menuItem("Start a conversation in a channel"));
    expect(onStartChannelConversation).toHaveBeenCalledTimes(1);
    expect(onComposeMessage).toHaveBeenCalledTimes(1);
  });

  it("the +'s menu wires its two items to onCreateChannel and onAddAgent, not to each other or the pencil menu's pair", () => {
    asDesktop("MacIntel");
    const onStartChannelConversation = vi.fn();
    const onComposeMessage = vi.fn();
    const onCreateChannel = vi.fn();
    const onAddAgent = vi.fn();
    render(
      createElement(SidebarTitleRow, {
        onStartChannelConversation,
        onComposeMessage,
        onCreateChannel,
        onAddAgent,
      }),
    );

    click(host.querySelector('[aria-label="Add"]'));
    click(menuItem("Create a new agent"));
    expect(onAddAgent).toHaveBeenCalledTimes(1);
    expect(onCreateChannel).not.toHaveBeenCalled();
    expect(onStartChannelConversation).not.toHaveBeenCalled();
    expect(onComposeMessage).not.toHaveBeenCalled();

    click(host.querySelector('[aria-label="Add"]'));
    click(menuItem("Create a new channel"));
    expect(onCreateChannel).toHaveBeenCalledTimes(1);
    expect(onAddAgent).toHaveBeenCalledTimes(1);
  });

  it("lets blank title-row space fall through to the drag band while both triggers stay clickable", () => {
    // Pins the two concrete, falsifiable claims the component's doc comment
    // makes (tinysweeper, medium — "pin the z-30 and pointer-events claims"):
    // the triggers' layer is pointer-events-none so hit testing on its own
    // blank space does not win against `WindowDragBar` underneath, with
    // pointer-events-auto restored on each trigger individually so they stay
    // clickable despite that.
    asDesktop("MacIntel");
    render(createElement(SidebarTitleRow, NOOP_PROPS));

    const buttonsLayer = host.querySelector('[aria-label="Start a conversation"]')!.parentElement!;
    expect(buttonsLayer.className).toContain("pointer-events-none");
    expect(buttonsLayer.className).toContain("z-30");
    for (const label of ["Start a conversation", "Add"]) {
      const button = host.querySelector(`[aria-label="${label}"]`)!;
      expect(button.className).toContain("pointer-events-auto");
    }
  });
});
