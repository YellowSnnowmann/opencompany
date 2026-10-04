// @vitest-environment jsdom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import type { MemoryEntry, MemoryList } from "@/api/memory";
import { MemoryView } from "@/views/MemoryView";

let container: HTMLDivElement;
let root: Root;

const VIEWPORT = 660;
const ROW = 132;

function entries(count: number): MemoryEntry[] {
  return Array.from({ length: count }, (_, i) => ({
    id: `m-${i}`,
    kind: "learning" as const,
    learningKind: "fact" as const,
    title: `memory ${i}`,
    body: "",
    namespace: "team:acme",
    tags: [],
    updatedAt: 0,
    editable: true,
  }));
}

function clientFor(count: number): OpenCompanyClient {
  const list: MemoryList = { items: entries(count) };
  return {
    scopeFor: () => "/api/v1/companies/acme",
    get: async (path: string) => {
      if (path.endsWith("/memory/status")) return { root: "team:acme", on: true };
      if (path.endsWith("/memory/agents")) return { root: "team:acme", agents: [] };
      if (path.endsWith("/memory/brain")) return { root: "team:acme", sources: [], unfiled: 0 };
      if (path.endsWith("/memory")) return list;
      throw new Error(`unexpected GET ${path}`);
    },
  } as unknown as OpenCompanyClient;
}

async function mount(client: OpenCompanyClient) {
  await act(async () => {
    root.render(createElement(MemoryView, { client, company: "acme" }));
  });
  await act(async () => {});
  await act(async () => {});
}

beforeEach(() => {
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, value: VIEWPORT });
  Object.defineProperty(HTMLElement.prototype, "getBoundingClientRect", {
    configurable: true,
    value() {
      return {
        top: 0,
        left: 0,
        right: 300,
        bottom: ROW,
        width: 300,
        height: ROW,
        x: 0,
        y: 0,
        toJSON() {},
      };
    },
  });
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("Brain memory list virtualization", () => {
  it("renders only a viewport-sized window of a large memory set, not every row", async () => {
    const count = 1000;
    await mount(clientFor(count));

    const cards = container.querySelectorAll('[data-testid="memory-card"]');
    // The bug this guards: the old view mapped the full `filtered` array, so a
    // 1000-item set mounted 1000 cards and locked the page. A windowed list
    // mounts only the rows the viewport can show (plus overscan). This is the
    // assertion that fails on the pre-fix view, which renders all `count`.
    expect(cards.length).toBeGreaterThan(0);
    expect(cards.length).toBeLessThan(count);
    // A viewport of ~5 rows plus overscan is comfortably under 50; asserting a
    // hard ceiling keeps this honest if the window math ever regresses to "all".
    expect(cards.length).toBeLessThanOrEqual(50);

    expect(container.querySelector('[data-testid="memory-list"]')).not.toBeNull();
  });

  it("mounts every card when the set is small enough to fit", async () => {
    await mount(clientFor(3));
    const cards = container.querySelectorAll('[data-testid="memory-card"]');
    expect(cards.length).toBe(3);
  });

  it("renders the empty state and no windowed list when there are no memories", async () => {
    await mount(clientFor(0));
    expect(container.querySelector('[data-testid="memory-list"]')).toBeNull();
    expect(container.querySelectorAll('[data-testid="memory-card"]').length).toBe(0);
    expect(container.textContent).toContain("No memories yet.");
  });
});
