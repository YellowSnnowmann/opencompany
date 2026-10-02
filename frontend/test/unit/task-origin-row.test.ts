// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { originLabel } from "@/lib/task-origin";
import { OriginThreadRow } from "@/views/TaskDetailView";

/**
 * The row itself, rendered — the half `task-origin-conversation.test.ts`
 * cannot reach.
 *
 * What it guards is an affordance that must match reality: the jump is offered
 * only when a Room channel actually carries the card's origin thread, and it
 * carries that channel id to the click. A row that always offered the jump is
 * how the pre-fix screen sent operators to a surface with nothing on it.
 */

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

function render(props: Parameters<typeof OriginThreadRow>[0]) {
  act(() => root.render(createElement(OriginThreadRow, props)));
}

describe("the Opened-from-chat row", () => {
  it("renders nothing for a card with no conversation behind it", () => {
    render({ chatChannelByThread: { t1: "desk-eng" }, onOpenChannel: () => {} });
    expect(container.textContent).toBe("");
  });

  it("offers the jump when a channel carries the origin thread", () => {
    render({
      originChatId: "t1",
      chatChannelByThread: { t1: "desk-eng" },
      onOpenChannel: () => {},
    });
    const button = container.querySelector("button");
    expect(button).not.toBeNull();
    expect(button?.textContent).toContain("Open the conversation");
  });

  it("carries the resolved channel id to the click, not the thread id", () => {
    const opened: string[] = [];
    render({
      originChatId: "t1",
      chatChannelByThread: { t1: "desk-eng" },
      onOpenChannel: (channelId) => opened.push(channelId),
    });
    act(() => {
      container.querySelector("button")?.dispatchEvent(
        new MouseEvent("click", { bubbles: true }),
      );
    });
    // `desk-eng`, not `t1`: `#/chat/<channelId>` is the address, and the card
    // records a host thread id.
    expect(opened).toEqual(["desk-eng"]);
  });

  it("states the origin without a jump when no channel carries the thread", () => {
    render({
      originChatId: "gone",
      chatChannelByThread: { t1: "desk-eng" },
      onOpenChannel: () => {},
    });
    expect(container.querySelector("button")).toBeNull();
    expect(container.textContent).toContain("Opened from chat");
  });

  it("states the origin without a jump when the host screen offers none", () => {
    render({ originChatId: "t1", chatChannelByThread: { t1: "desk-eng" } });
    expect(container.querySelector("button")).toBeNull();
    expect(container.textContent).toContain("Opened from chat");
  });

  it("names the teammate who opened the card from a HiveMind room", () => {
    render({
      originChatId: "t1",
      openedBy: { agentId: "writer", episodeId: "ep-1" },
      names: new Map([["writer", "Wren"]]),
      chatChannelByThread: { t1: "desk-eng" },
      onOpenChannel: () => {},
    });
    expect(container.textContent).toContain("Opened in chat by Wren");
    expect(container.textContent).toContain("Open the conversation");
  });

  it("falls back to the teammate's id when the roster does not name them", () => {
    render({
      originChatId: "gone",
      openedBy: { agentId: "writer" },
      chatChannelByThread: {},
    });
    expect(container.textContent).toContain("Opened in chat by writer");
  });
});

describe("originLabel", () => {
  it("says only where a card came from when no teammate opened it", () => {
    expect(originLabel(undefined)).toBe("Opened from chat");
    expect(originLabel({ agentId: "" })).toBe("Opened from chat");
  });

  it("prefers the roster name, ignoring a blank one", () => {
    expect(originLabel({ agentId: "ceo" }, new Map([["ceo", "Ada"]]))).toBe(
      "Opened in chat by Ada",
    );
    expect(originLabel({ agentId: "ceo" }, new Map([["ceo", "  "]]))).toBe(
      "Opened in chat by ceo",
    );
  });
});
