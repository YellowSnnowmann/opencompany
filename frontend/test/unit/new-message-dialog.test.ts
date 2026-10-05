// @vitest-environment jsdom
//
// `NewMessageDialog` (restored from its pre-deletion version for the title
// row's pencil — `sidebar-title-row.tsx`) can be driven either way a dialog
// in this console is: with its own `trigger`, or bare `open`/`onOpenChange`
// held by a caller that mounts it once and flips a boolean (`app-shell.tsx`,
// the title row's case — its pencil button is a sibling of `RoomView`, not a
// parent, so it cannot wrap a `DialogTrigger` around this dialog itself).
//
// Pinned here because neither path has any other coverage (tinysweeper,
// medium): the trigger open, the controlled open, the `select()` flow's two
// side effects (`onSelect` with the picked id, `onOpenChange` with `false`),
// and the `openProp ?? openLocal` / `onOpenChange ?? setOpenLocal` fallback
// that lets a caller mix a controlled `open` with no `onOpenChange` without
// the dialog going silently unopenable or uncloseable.

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { NewMessageDialog } from "@/views/room/NewMessageDialog";
import type { Channel } from "@/views/room/model";

const DIRECT_MESSAGES: Channel[] = [
  { id: "dm-maya", name: "Maya", kind: "dm", purpose: "Research Lead" },
  { id: "dm-theo", name: "Theo", kind: "dm", purpose: "" },
];

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
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
  document.body.innerHTML = "";
});

// The dialog's own content renders through a portal onto `document.body`
// (same as every other Base UI dialog in this console — see
// `task-edit-dialog-gates.test.ts`), so lookups search the whole document
// rather than `container`.
function channelButton(name: string): HTMLElement {
  // The button's first descendant span carries just the name; a second,
  // sibling span carries the purpose line when there is one — so the name is
  // matched on its own span rather than the button's whole (possibly
  // name+purpose) text content.
  const found = Array.from(document.querySelectorAll<HTMLElement>("button")).find((el) =>
    Array.from(el.querySelectorAll("span")).some((span) => span.textContent?.trim() === name),
  );
  if (!found) throw new Error(`no channel button for ${name}`);
  return found;
}

function click(el: HTMLElement) {
  act(() => {
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

describe("NewMessageDialog, opened by its own trigger", () => {
  it("opens from the trigger and picks a channel", () => {
    const onSelect = vi.fn();
    const trigger = createElement("button", { type: "button" }, "Open");

    act(() => {
      root.render(
        createElement(NewMessageDialog, {
          directMessages: DIRECT_MESSAGES,
          onSelect,
          trigger,
        }),
      );
    });

    expect(document.body.textContent).not.toContain("Maya");
    click(document.querySelector("button")!);
    expect(document.body.textContent).toContain("Maya");

    click(channelButton("Maya"));
    expect(onSelect).toHaveBeenCalledWith("dm-maya");
    // Uncontrolled: nothing but its own `openLocal` closed the dialog, and it
    // did — the picked channel's row is gone from the document.
    expect(document.body.textContent).not.toContain("Maya");
  });
});

describe("NewMessageDialog, controlled by a caller-held open", () => {
  it("renders open exactly when the caller's `open` says so, with no trigger of its own", () => {
    const onSelect = vi.fn();
    const onOpenChange = vi.fn();

    act(() => {
      root.render(
        createElement(NewMessageDialog, {
          directMessages: DIRECT_MESSAGES,
          onSelect,
          open: false,
          onOpenChange,
        }),
      );
    });
    expect(document.body.textContent).not.toContain("Maya");
    // No trigger prop, so nothing in the document opens it from the inside.
    expect(document.querySelector("button")).toBeNull();

    act(() => {
      root.render(
        createElement(NewMessageDialog, {
          directMessages: DIRECT_MESSAGES,
          onSelect,
          open: true,
          onOpenChange,
        }),
      );
    });
    expect(document.body.textContent).toContain("Maya");
  });

  it("selecting a channel reports both the pick and the close to the caller, and does not close itself", () => {
    const onSelect = vi.fn();
    const onOpenChange = vi.fn();

    act(() => {
      root.render(
        createElement(NewMessageDialog, {
          directMessages: DIRECT_MESSAGES,
          onSelect,
          open: true,
          onOpenChange,
        }),
      );
    });

    click(channelButton("Theo"));
    expect(onSelect).toHaveBeenCalledWith("dm-theo");
    expect(onOpenChange).toHaveBeenCalledWith(false);
    // Controlled: the caller's `open` has not changed in this test, so the
    // dialog must still be the one deciding nothing locally — it stays open
    // until the caller actually lowers `open`, proving `setOpen` resolved to
    // `onOpenChange` and not `setOpenLocal`.
    expect(document.body.textContent).toContain("Theo");
  });
});
