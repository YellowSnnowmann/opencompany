// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import { ApiError } from "@/api/types";
import type { MemoryEntry, MemoryList } from "@/api/memory";
import { MemoryView } from "@/views/MemoryView";

/**
 * The memory routes are `ScopedCompany` — no admin gate. What this file pins:
 * a plain member gets the same add-learning and delete affordances an admin
 * would, the add form posts the `{ text, kind }` learning body, and a delete
 * the host refuses puts the card straight back.
 */

function entry(over: Partial<MemoryEntry> = {}): MemoryEntry {
  return {
    id: "f1",
    kind: "learning",
    learningKind: "fact",
    title: "Acme renews every March.",
    body: "Acme renews every March.",
    namespace: "team:acme",
    tags: [],
    updatedAt: 1000,
    editable: true,
    ...over,
  };
}

function clientAs(opts: { del?: (path: string) => Promise<void> }) {
  const list: MemoryList = { items: [entry()] };
  const post = vi.fn(() => Promise.resolve(entry()));
  const client = {
    scopeFor: () => "/api/v1/companies/acme",
    get: (path: string) => {
      if (path.endsWith("/memory/status")) {
        return Promise.resolve({ root: "team:acme", on: true, engine: "local" });
      }
      if (path.endsWith("/memory/agents")) return Promise.resolve({ root: "team:acme", agents: [] });
      if (path.endsWith("/memory/brain")) {
        return Promise.resolve({ root: "team:acme", sources: [], unfiled: 0 });
      }
      return Promise.resolve(list);
    },
    post,
    del: vi.fn(opts.del ?? (() => Promise.resolve())),
  } as unknown as OpenCompanyClient;
  return { client, post };
}

let container: HTMLDivElement;
let root: Root;

async function show(element: React.ReactElement) {
  await act(async () => {
    root.render(element);
  });
  await act(async () => {});
}

function at(testid: string): HTMLElement | null {
  return container.querySelector<HTMLElement>(`[data-testid="${testid}"]`);
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

describe("MemoryView, learning CRUD with no admin gate", () => {
  it("offers a plain member both Add learning and per-card delete", async () => {
    const { client } = clientAs({});
    await show(createElement(MemoryView, { client, company: "acme", sub: "upload" }));
    expect(at("memory-add"), "the add panel is on the Upload tab").not.toBeNull();

    await show(createElement(MemoryView, { client, company: "acme" }));
    const card = at("memory-card")!;
    expect(card.querySelector("button[aria-label='Delete memory']")).not.toBeNull();
  });

  it("posts the typed text as a learning with its kind", async () => {
    const { client, post } = clientAs({});
    await show(createElement(MemoryView, { client, company: "acme", sub: "upload" }));

    const text = at("memory-text") as HTMLTextAreaElement;
    act(() => {
      const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
      setter?.call(text, "Ship on Fridays.");
      text.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => {
      at("memory-save")!.click();
    });

    expect(post).toHaveBeenCalledWith("/api/v1/companies/acme/memory", {
      text: "Ship on Fridays.",
      kind: "fact",
    });
  });

  it("puts the card back when the host refuses the delete", async () => {
    const { client } = clientAs({
      del: () => Promise.reject(new ApiError(409, "conflict", "this item was already removed")),
    });
    await show(createElement(MemoryView, { client, company: "acme" }));

    expect(container.textContent).toContain("Acme renews every March.");
    await act(async () => {
      container.querySelector<HTMLButtonElement>("button[aria-label='Delete memory']")!.click();
    });
    expect(container.textContent).toContain("Acme renews every March.");
  });
});
