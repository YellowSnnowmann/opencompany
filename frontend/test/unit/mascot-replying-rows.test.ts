// @vitest-environment jsdom
//
// When the rows that exist while a turn is open — the live receipt, the working
// row and the typing row — tell their teammate's avatar to reply. A row being
// on screen is not the same as the turn progressing: a queued turn is waiting on
// the per-company lock (the status dot stills for the same reason) and a stalled
// one has gone quiet, so neither should make the mascot look busy. The avatar
// itself is mocked — what it does with the prop is `mascot-pose-tile.test.ts`'s
// job.

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const avatar = vi.hoisted(() => ({ props: [] as Record<string, unknown>[] }));

vi.mock("@/components/teammate-avatar", () => ({
  TeammateAvatar: (props: Record<string, unknown>) => {
    avatar.props.push(props);
    return null;
  },
}));

const { ChatLiveReceipt, RECEIPT_STALL_AFTER_MS } = await import("@/views/room/ChatLiveReceipt");
const { MessageTimeline } = await import("@/views/room/MessageTimeline");

const BASE = 1_700_000_000_000;

const channel = {
  id: "dm:ada",
  name: "Ada",
  voice: "Ada",
  kind: "dm",
  purpose: "",
  tone: "sky",
  member: {
    avatar: "mascot:animated",
    mascotCostume: "glass2",
    mascotSkinColor: "mint",
    mascotHandColor: "charcoal",
    mascotMode: "animated",
  },
} as never;

let container: HTMLDivElement;
let root: Root;

async function render(props: { queued?: boolean; lastFrameAt?: number }) {
  avatar.props.length = 0;
  await act(async () => {
    root.render(
      createElement(ChatLiveReceipt, {
        channel,
        receipt: { startedAt: BASE - 1_000, lastFrameAt: props.lastFrameAt ?? BASE - 1_000 },
        steps: [],
        queued: props.queued,
      }),
    );
  });
  return avatar.props[avatar.props.length - 1];
}

beforeEach(() => {
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  vi.useFakeTimers();
  vi.setSystemTime(BASE);
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => {
    root.unmount();
  });
  container.remove();
  vi.useRealTimers();
});

describe("the live receipt's avatar", () => {
  it("replies while the turn is progressing", async () => {
    expect((await render({})).replying).toBe(true);
  });

  it("does not while the turn is queued behind another", async () => {
    expect((await render({ queued: true })).replying).toBe(false);
  });

  it("does not once the turn has gone quiet, and does again when a frame arrives", async () => {
    const quiet = BASE - RECEIPT_STALL_AFTER_MS - 1_000;
    expect((await render({ lastFrameAt: quiet })).replying).toBe(false);
    expect((await render({ lastFrameAt: BASE })).replying).toBe(true);
  });

  it("carries the teammate's own look, so the reply is in their costume and colors", async () => {
    const props = await render({});
    expect(props).toMatchObject({
      avatar: "mascot:animated",
      mascotCostume: "glass2",
      mascotSkinColor: "mint",
      mascotHandColor: "charcoal",
      mascotMode: "animated",
    });
  });
});

describe("the working and typing rows' avatars", () => {
  /** The avatars a timeline draws with a `replying` opinion — the intro mark has none. */
  async function rows(props: { typing: boolean; queued?: boolean; liveSteps?: unknown[] }) {
    avatar.props.length = 0;
    await act(async () => {
      root.render(
        createElement(MessageTimeline, {
          channel,
          items: [],
          historyPending: false,
          openThreadId: null,
          onOpenThread: () => {},
          onReact: () => {},
          onDismissCard: () => {},
          dismissingCardId: null,
          ...props,
        } as never),
      );
    });
    return avatar.props.filter((p) => "replying" in p);
  }

  it("the typing row replies while its turn is progressing", async () => {
    const drawn = await rows({ typing: true });
    expect(drawn).toHaveLength(1);
    expect(drawn[0].replying).toBe(true);
  });

  it("the typing row does not while its turn is queued", async () => {
    const drawn = await rows({ typing: true, queued: true });
    expect(drawn).toHaveLength(1);
    expect(drawn[0].replying).toBe(false);
  });

  it("the working row — a turn with live steps — replies", async () => {
    const drawn = await rows({ typing: true, liveSteps: [{ kind: "tool_call", status: "running", label: "Search" }] });
    expect(drawn).toHaveLength(1);
    expect(drawn[0].replying).toBe(true);
  });

  it("a queued turn with steps falls back to the typing row, which does not", async () => {
    const drawn = await rows({ typing: true, queued: true, liveSteps: [{ kind: "tool_call", status: "running", label: "Search" }] });
    expect(drawn).toHaveLength(1);
    expect(drawn[0].replying).toBe(false);
  });

  it("nothing replies when no turn is open", async () => {
    expect(await rows({ typing: false })).toHaveLength(0);
  });
});
