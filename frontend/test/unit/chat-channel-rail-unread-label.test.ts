// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { ChannelRail } from "@/views/room/ChannelRail";
import type { ChannelSection } from "@/views/room/model";

/**
 * The compact rail keeps an unread channel's count in its accessible name
 * (issue #364, P2 review).
 *
 * The expanded row announces unread because the count is text inside the
 * button; the collapsed row draws it as a bare dot, which is invisible to
 * screen readers. The fix puts the same count in the compact button's
 * `aria-label`, so collapsing the rail does not strip the fact from the
 * accessibility tree. These pin that label directly.
 */

const SECTIONS: ChannelSection[] = [
  {
    id: "s1",
    label: "Company",
    channels: [
      { id: "front-desk", name: "Front desk", kind: "channel", purpose: "The front line." },
      { id: "ops", name: "Ops", kind: "channel", purpose: "Where work lands." },
    ],
  },
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

const channelButtons = () =>
  [...container.querySelectorAll<HTMLButtonElement>('nav[aria-label="Channels"] button')].filter(
    (b) => b.getAttribute("aria-label") !== "Expand channels",
  );

describe("collapsed ChannelRail unread labels", () => {
  it("names an unread channel with its count", () => {
    act(() =>
      root.render(
        createElement(ChannelRail, {
          sections: SECTIONS,
          activeId: null,
          unread: { "front-desk": 3 },
          onSelect: () => {},
          collapsed: true,
        }),
      ),
    );

    const buttons = channelButtons();
    expect(buttons.map((b) => b.getAttribute("aria-label"))).toEqual([
      "Front desk, 3 unread",
      "Ops",
    ]);
  });

  it("includes mention and unread counts in the compact accessible name", () => {
    act(() =>
      root.render(
        createElement(ChannelRail, {
          sections: SECTIONS,
          activeId: null,
          unread: { "front-desk": 3 },
          mentions: { "front-desk": 2 },
          onSelect: () => {},
          collapsed: true,
        }),
      ),
    );

    expect(channelButtons()[0].getAttribute("aria-label")).toBe("Front desk, 2 mentions, 3 unread");
    expect(container.querySelectorAll('[data-testid="channel-mentions"]')).toHaveLength(1);
  });
  it("caps a huge count the way the expanded badge does", () => {
    act(() =>
      root.render(
        createElement(ChannelRail, {
          sections: SECTIONS,
          activeId: null,
          unread: { "front-desk": 142 },
          onSelect: () => {},
          collapsed: true,
        }),
      ),
    );

    expect(channelButtons()[0].getAttribute("aria-label")).toBe("Front desk, 99+ unread");
  });

  it("keeps the active channel's label bare even when unread", () => {
    act(() =>
      root.render(
        createElement(ChannelRail, {
          sections: SECTIONS,
          activeId: "front-desk",
          unread: { "front-desk": 7 },
          onSelect: () => {},
          collapsed: true,
        }),
      ),
    );

    // The unread dot does not render on the channel you are already reading,
    // so the label must not claim unread either.
    expect(channelButtons()[0].getAttribute("aria-label")).toBe("Front desk");
  });
});

describe("the rail has no section folds", () => {
  // Channels and Direct messages were collapsible sections; they are one flat
  // list now, headed by a caption that is not a control. Nothing in it can be
  // folded, so there is no fold state to survive a collapse and expand, and no
  // `aria-expanded` toggle left among the list's own buttons.
  it("renders no fold toggle, expanded or after a collapse and expand", () => {
    const rail = (collapsed: boolean) =>
      createElement(ChannelRail, {
        sections: SECTIONS,
        activeId: null,
        unread: {},
        onSelect: () => {},
        collapsed,
      });
    act(() => root.render(rail(false)));
    expect(container.querySelectorAll("section button[aria-expanded]")).toHaveLength(0);
    act(() => root.render(rail(true)));
    act(() => root.render(rail(false)));
    expect(container.querySelectorAll("section button[aria-expanded]")).toHaveLength(0);
    // Both rows are still listed.
    expect(container.textContent).toContain("Front desk");
    expect(container.textContent).toContain("Ops");
  });
});
