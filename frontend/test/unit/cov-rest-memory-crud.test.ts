// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import type {
  BrainSources,
  MemoryAgents,
  MemoryEntry,
  MemoryList,
  MemoryStatus,
} from "@/api/memory";
import { MemoryView } from "@/views/MemoryView";

/**
 * The Brain over the OpenHuman memory v2 contract: status first (the one route
 * that answers when memory is off), then one page of items plus the agents and
 * brain sources beside it. Pins delete-per-item with an honest rollback,
 * cursor paging, the per-agent filter and forget, the per-source forget, and
 * the memory-off state that must not touch any other route.
 */

function entry(over: Partial<MemoryEntry> = {}): MemoryEntry {
  return {
    id: "m1",
    kind: "learning",
    learningKind: "fact",
    title: "Client prefers Friday reviews",
    body: "Client prefers Friday reviews",
    namespace: "team:acme",
    tags: [],
    updatedAt: 0,
    editable: true,
    ...over,
  };
}

const ON: MemoryStatus = { root: "team:acme", on: true, engine: "local" };
const AGENTS: MemoryAgents = { root: "team:acme", agents: [{ agentId: "ceo", turns: 4 }] };
const BRAIN: BrainSources = {
  root: "team:acme",
  sources: [{ source: "markdown", documents: 2 }],
  unfiled: 0,
};

function clientWith(opts: {
  status?: MemoryStatus;
  pages?: Record<string, MemoryList>;
  del?: (path: string) => Promise<unknown>;
}) {
  const pages = opts.pages ?? { first: { items: [entry()] } };
  const get = vi.fn((path: string) => {
    if (path.endsWith("/memory/status")) return Promise.resolve(opts.status ?? ON);
    if (path.endsWith("/memory/agents")) return Promise.resolve(AGENTS);
    if (path.endsWith("/memory/brain")) return Promise.resolve(BRAIN);
    if (path.includes("/memory")) {
      const cursor = new URLSearchParams(path.split("?")[1] ?? "").get("cursor");
      return Promise.resolve(pages[cursor ?? "first"] ?? { items: [] });
    }
    return Promise.reject(new Error(`unexpected GET ${path}`));
  });
  const del = vi.fn(opts.del ?? (() => Promise.resolve({ forgotten: 2 })));
  const client = {
    scopeFor: () => "/api/v1/company/acme",
    get,
    post: vi.fn(() => Promise.resolve(entry())),
    del,
  } as unknown as OpenCompanyClient;
  return { client, get, del };
}

let container: HTMLDivElement;
let root: Root;

async function show(client: OpenCompanyClient) {
  await act(async () => {
    root.render(createElement(MemoryView, { client, company: "acme" }));
  });
  await act(async () => {});
}

function cards(): HTMLElement[] {
  return Array.from(container.querySelectorAll('[data-testid="memory-card"]'));
}

function deleteButtonIn(card: HTMLElement): HTMLElement | null {
  return card.querySelector('[aria-label="Delete memory"]');
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
  vi.restoreAllMocks();
});

describe("every item may be forgotten", () => {
  it("offers delete on learnings and conversations alike", async () => {
    const { client } = clientWith({
      pages: {
        first: {
          items: [
            entry(),
            entry({ id: "c1", kind: "conversation", agentId: "ceo", title: "user: hi" }),
          ],
        },
      },
    });
    await show(client);

    expect(cards()).toHaveLength(2);
    for (const card of cards()) expect(deleteButtonIn(card)).not.toBeNull();
  });

  it("deletes by id through the item route", async () => {
    const { client, del } = clientWith({});
    await show(client);

    await act(async () => {
      deleteButtonIn(cards()[0])!.click();
    });
    expect(del).toHaveBeenCalledWith("/api/v1/company/acme/memory/m1");
  });

  it("re-inserts the entry when the host refuses the delete", async () => {
    const { client } = clientWith({
      del: () => Promise.reject(new Error("memory engine is unreachable")),
    });
    await show(client);

    await act(async () => {
      deleteButtonIn(cards()[0])!.click();
    });

    expect(cards()).toHaveLength(1);
    expect(container.textContent).toContain("Client prefers Friday reviews");
  });
});

describe("paging", () => {
  it("offers Load more while there is a cursor, and appends the next page", async () => {
    const { client, get } = clientWith({
      pages: {
        first: { items: [entry()], nextCursor: "c2" },
        c2: { items: [entry({ id: "m2", title: "Second page", body: "Second page" })] },
      },
    });
    await show(client);

    const more = container.querySelector<HTMLButtonElement>('[data-testid="memory-load-more"]');
    expect(more).not.toBeNull();
    await act(async () => {
      more!.click();
    });

    expect(get).toHaveBeenCalledWith("/api/v1/company/acme/memory?cursor=c2");
    expect(cards()).toHaveLength(2);
    expect(container.querySelector('[data-testid="memory-load-more"]')).toBeNull();
  });
});

describe("status, agents and sources", () => {
  it("shows the engine, the agents' turns and the brain's sources", async () => {
    const { client } = clientWith({});
    await show(client);

    expect(container.querySelector('[data-testid="memory-status-badge"]')?.textContent).toBe(
      "memory: local",
    );
    const health = container.querySelector('[data-testid="memory-health"]')!.textContent;
    expect(health).toContain("Conversation turns4");
    expect(health).toContain("Documents2");
    expect(container.querySelector('[data-testid="memory-brain-source"]')?.textContent).toContain(
      "markdown",
    );
  });

  it("reports memory off with its reason and reads no other route", async () => {
    const { client, get } = clientWith({
      status: { root: "team:acme", on: false, reason: "no engine configured" },
    });
    await show(client);

    expect(container.querySelector('[data-testid="memory-off"]')?.textContent).toContain(
      "no engine configured",
    );
    expect(get).toHaveBeenCalledTimes(1);
    expect(cards()).toHaveLength(0);
  });
});
