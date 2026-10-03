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
 * The channel list is ONE flat list, with no caption and no doors
 * on the same row.
 *
 * Channels and Direct messages used to be two captioned, collapsible sections
 * with a door each. They are merged: no headings, no folds, channels first and
 * then DMs, each kind in the order it already had. The two doors that sat on
 * the caption row went with the caption — see the last case.
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
        ...props,
      }),
    ),
  );
}

describe("the merged conversation list", () => {
  it("is one flat list with no Channels or Direct messages heading", () => {
    render();
    expect(container.querySelectorAll("section")).toHaveLength(0);
    // Two lists in the DOM (the DMs slide on a re-sort and need their own), one
    // run on screen: nothing between them.
    expect(container.querySelectorAll("ul")).toHaveLength(2);
    expect(container.querySelectorAll("ul")[0].parentElement).toBe(
      container.querySelectorAll("ul")[1].parentElement,
    );
    expect(container.textContent).not.toContain("Channels");
    expect(container.textContent).not.toContain("Direct messages");
    // No fold toggle among the list's own rows (the two menu triggers carry
    // `aria-expanded` too, so look at the rows).
    expect(container.querySelectorAll("li button[aria-expanded]")).toHaveLength(0);
  });

  it("lists channels before DMs, each kind in its own order", () => {
    render();
    const names = [...container.querySelectorAll("li [data-testid=\"channel-name\"]")].map((li) => li.textContent?.trim());
    expect(names).toEqual(["general", "ops-desk", "Ada", "Bo"]);
  });

  it("says Nothing here yet when there is nothing to list", () => {
    render({ sections: [] });
    expect(container.textContent).toContain("Nothing here yet.");
  });
});

describe("no caption row and no doors", () => {
  // "Conversations" with a `+` (new channel / new agent) and a pencil (start a
  // conversation) used to head this list. New agents and desks are made on
  // Company > Agents, and every conversation is already a row here, so the row
  // had nothing left to open and was removed outright.
  it("draws no Conversations caption and no New or Start a conversation control", () => {
    render();
    expect(container.textContent).not.toContain("Conversations");
    expect(container.querySelector('[aria-label="New"]')).toBeNull();
    expect(container.querySelector('[aria-label="Start a conversation"]')).toBeNull();
  });
});
