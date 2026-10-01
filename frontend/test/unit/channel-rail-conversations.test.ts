// @vitest-environment jsdom

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ChannelRail } from "@/views/room/ChannelRail";
import type { Channel, ChannelSection } from "@/views/room/model";

/**
 * The channel list is ONE flat list headed "Conversations", with two icon doors
 * on the same row.
 *
 * Channels and Direct messages used to be two captioned, collapsible sections
 * with a door each. They are merged: no headings, no folds, channels first and
 * then DMs, each kind in the order it already had. The `+` makes things; the
 * pencil starts a conversation. `Search` is deliberately NOT among its items —
 * the title bar owns search (⌘K), and a synthetic keydown from the rail was the
 * workaround this removed.
 */

const CHANNELS: Channel[] = [
  { id: "general", name: "general", kind: "channel", purpose: "Everyone." },
  { id: "ops-desk", name: "ops-desk", kind: "channel", purpose: "Ops." },
];
const DMS: Channel[] = [
  { id: "dm:ada", name: "Ada", kind: "dm", purpose: "" },
  { id: "dm:bo", name: "Bo", kind: "dm", purpose: "" },
];
const SECTIONS: ChannelSection[] = [
  { id: "channels", label: "Channels", channels: CHANNELS },
  { id: "dms", label: "Direct messages", channels: DMS },
];

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

function render(props: Partial<Parameters<typeof ChannelRail>[0]> = {}) {
  act(() =>
    root.render(
      createElement(ChannelRail, {
        sections: SECTIONS,
        activeId: null,
        unread: {},
        onSelect: () => {},
        directMessages: DMS,
        onStartDirectMessage: () => {},
        onAddChannel: () => {},
        onAddAgent: () => {},
        ...props,
      }),
    ),
  );
}

const byLabel = (label: string) =>
  container.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;

function open(label: string) {
  act(() => {
    byLabel(label).dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

/** The menu's items, which a menu portals out to `document.body`. */
const menuItems = () =>
  [...document.querySelectorAll('[role="menuitem"]')].map((el) => el.textContent?.trim());
const menuItem = (label: string) =>
  [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find(
    (el) => el.textContent?.trim() === label,
  )!;

describe("the merged conversation list", () => {
  it("is one flat list with no Channels or Direct messages heading", () => {
    render();
    expect(container.querySelectorAll("section")).toHaveLength(0);
    expect(container.querySelectorAll("ul")).toHaveLength(1);
    expect(container.textContent).not.toContain("Channels");
    expect(container.textContent).not.toContain("Direct messages");
    // No fold toggle among the list's own rows (the two menu triggers carry
    // `aria-expanded` too, so look at the rows).
    expect(container.querySelectorAll("li button[aria-expanded]")).toHaveLength(0);
  });

  it("lists channels before DMs, each kind in its own order", () => {
    render();
    const names = [...container.querySelectorAll("li")].map((li) => li.textContent?.trim());
    expect(names).toEqual(["general", "ops-desk", "Ada", "Bo"]);
  });

  it("says Nothing here yet when there is nothing to list", () => {
    render({ sections: [] });
    expect(container.textContent).toContain("Nothing here yet.");
  });
});

describe("the Conversations caption", () => {
  const caption = () =>
    [...container.querySelectorAll("div")].find(
      (el) => el.textContent?.trim() === "Conversations" && !el.querySelector("div"),
    )!;

  it("is a plain label: not a heading and not pressable", () => {
    render();
    expect(caption()).toBeDefined();
    expect(container.querySelectorAll("h1, h2, h3, h4, h5, h6")).toHaveLength(0);
    expect(caption().tagName).toBe("DIV");
    expect(caption().closest("button")).toBeNull();
    expect(caption().getAttribute("role")).toBeNull();
    expect(caption().className).toContain("text-xs");
    expect(caption().className).toContain("text-muted-foreground");
  });

  it("shares a row with the two doors, which stay on the right", () => {
    render();
    const row = caption().parentElement!;
    expect(row.contains(byLabel("New"))).toBe(true);
    expect(row.contains(byLabel("Start a conversation"))).toBe(true);
    // The caption is the elastic member, so the doors sit at the right edge.
    expect(caption().className).toContain("flex-1");
  });
});

describe("the + menu", () => {
  it("has exactly Create a new channel and Create a new agent", () => {
    render();
    open("New");
    expect(menuItems()).toEqual(["Create a new channel", "Create a new agent"]);
  });

  it("opens the channel creator and the real Add agent flow, not the DM picker", () => {
    const onAddChannel = vi.fn();
    const onAddAgent = vi.fn();
    render({ onAddChannel, onAddAgent });
    open("New");
    act(() => menuItem("Create a new agent").click());
    expect(onAddAgent).toHaveBeenCalledTimes(1);
    expect(onAddChannel).not.toHaveBeenCalled();
    // `NewMessageDialog` ("New message") is the compose menu's agent picker; this
    // item must not open it.
    expect(document.body.textContent).not.toContain("Choose an agent to start a direct message.");

    open("New");
    act(() => menuItem("Create a new channel").click());
    expect(onAddChannel).toHaveBeenCalledTimes(1);
  });

  it("holds an item back when the rail was not given its handler", () => {
    render({ onAddChannel: undefined, onAddAgent: undefined });
    open("New");
    expect(menuItem("Create a new channel").hasAttribute("data-disabled")).toBe(true);
    expect(menuItem("Create a new agent").hasAttribute("data-disabled")).toBe(true);
  });

  it("is wired in RoomView to the same AddMemberDialog the Agents page opens", () => {
    // A source guard: `RoomView` owns the dialog and its handler, and the rail
    // only asks. Reading the wiring here is what stops the item quietly pointing
    // back at the DM picker (it did, once).
    const source = readFileSync(
      resolve(dirname(fileURLToPath(import.meta.url)), "../../src/views/RoomView.tsx"),
      "utf8",
    );
    expect(source).toContain("onAddAgent={() => setAddOpen(true)}");
    expect(source).toMatch(/<AddMemberDialog\s+open=\{addOpen\}/);
  });
});

describe("the compose menu", () => {
  it("has exactly the two Start a conversation items, and no Search", () => {
    render();
    open("Start a conversation");
    expect(menuItems()).toEqual([
      "Start a conversation in a channel",
      "Start a conversation with the agent",
    ]);
    expect(menuItems().some((label) => /search/i.test(label ?? ""))).toBe(false);
  });

  it("opens the channel picker, then the agent picker", () => {
    render();
    open("Start a conversation");
    act(() => menuItem("Start a conversation in a channel").click());
    expect(document.body.textContent).toContain("Choose a channel to talk in.");
    expect(document.body.textContent).toContain("ops-desk");
  });

  it("holds the agent picker back when nobody can be messaged", () => {
    render({ directMessages: [] });
    open("Start a conversation");
    expect(menuItem("Start a conversation with the agent").hasAttribute("data-disabled")).toBe(true);
  });
});
