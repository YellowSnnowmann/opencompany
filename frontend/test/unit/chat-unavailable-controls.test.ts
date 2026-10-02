// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { ChannelRail } from "@/views/room/ChannelRail";
import { MessageComposer } from "@/views/room/MessageComposer";

let container: HTMLDivElement;
let root: Root;

function render(element: ReturnType<typeof createElement>) {
  act(() => root.render(element));
}

function action(label: string) {
  return container.querySelector(`[aria-label="${label}"]`);
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

describe("chat only renders controls it can perform (issue #1336)", () => {
  // The compose door is the pencil on the "Conversations" row now, and the agent
  // picker is an item of the menu it opens. The rail has to be given someone to
  // message for that item to be live.
  const DMS = [{ id: "dms", label: "Direct messages", channels: [] }];

  const agentItem = () =>
    [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find(
      (el) => el.textContent?.trim() === "Start a conversation with the agent",
    );

  function openCompose() {
    act(() => {
      action("Start a conversation")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
  }

  it("offers the agent picker in the compose menu when there is someone to message", () => {
    render(
      createElement(ChannelRail, {
        sections: DMS,
        activeId: null,
        unread: {},
        onSelect: () => {},
        directMessages: [{ id: "dm:2", name: "Ade", kind: "dm", purpose: "" }],
        onStartDirectMessage: () => {},
      }),
    );

    openCompose();
    expect(agentItem()).toBeDefined();
    expect(agentItem()!.hasAttribute("data-disabled")).toBe(false);
  });

  it("holds the agent picker back when nobody can be messaged", () => {
    render(
      createElement(ChannelRail, {
        sections: DMS,
        activeId: null,
        unread: {},
        onSelect: () => {},
        directMessages: [],
        onStartDirectMessage: () => {},
      }),
    );

    openCompose();
    expect(agentItem()).toBeDefined();
    expect(agentItem()!.hasAttribute("data-disabled")).toBe(true);
  });

  it("keeps working composer controls and holds unavailable ones back", () => {
    render(
      createElement(MessageComposer, {
        placeholder: "Message",
        onSend: () => {},
      }),
    );

    expect(action("Mention someone")).not.toBeNull();
    expect(action("Formatting")).not.toBeNull();
    expect(action("Attach a file")).toBeNull();
    expect(action("Add an emoji")).toBeNull();

    act(() => (action("Formatting") as HTMLButtonElement).click());
    expect(action("Bold")).not.toBeNull();
    expect(action("Bulleted list")).toBeNull();
    expect(action("Link")).toBeNull();
  });
});
