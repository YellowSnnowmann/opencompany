// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import type { Ingested } from "@/api/memory";
import { DropZone } from "@/views/memory/DropZone";

/**
 * `memory_ingest.rs` reports one status per dropped source rather than a
 * batch verdict, and `DropReport` (`DropZone.tsx`) is the only place that
 * renders it — a folder always contains something that could not be read, so
 * the per-item detail is what tells an operator WHICH files landed and which
 * did not. Nothing had pinned that the report actually renders each item's
 * real status and message, as opposed to a blanket "done" the moment any part
 * of a mixed batch stored successfully.
 */

const MIXED: Ingested = {
  chunks: 3,
  stored: 1,
  items: [
    { source: "https://acme.test/pricing", status: "stored", chunks: 3 },
    {
      source: "https://acme.test/dead-link",
      status: "failed",
      chunks: 0,
      detail: "the host answered 404",
    },
  ],
};

function clientAs(ingestLinks: () => Promise<Ingested>): OpenCompanyClient {
  return {
    scopeFor: () => "/api/v1/companies/acme",
    post: vi.fn((path: string) => {
      if (path.endsWith("/memory/ingest/links")) return ingestLinks();
      return Promise.reject(new Error(`unexpected POST ${path}`));
    }),
  } as unknown as OpenCompanyClient;
}

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  vi.spyOn(window, "prompt").mockReturnValue("https://acme.test/pricing");
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.restoreAllMocks();
});

function findButton(text: string): HTMLButtonElement | null {
  return (
    (Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes(text),
    ) as HTMLButtonElement | undefined) ?? null
  );
}

describe("DropZone's per-item report, on a mixed batch", () => {
  it("names the failed item and its real reason, and counts only the actual successes as remembered", async () => {
    const client = clientAs(() => Promise.resolve(MIXED));
    await act(async () => {
      root.render(
        createElement(DropZone, {
          client,
          company: "acme",
          onIngested: () => {},
          off: false,
        }),
      );
    });

    await act(async () => {
      findButton("Add link")!.click();
    });

    const report = container.querySelector('[data-testid="memory-drop-report"]')!;
    expect(report.textContent).toContain("1 remembered");
    expect(report.textContent).toContain("1 skipped");
    expect(report.textContent).toContain("https://acme.test/dead-link");
    expect(report.textContent).toContain("the host answered 404");
    // The item that actually stored must not also appear in the failure list.
    const failureRows = Array.from(report.querySelectorAll("li")).map((li) => li.textContent ?? "");
    expect(failureRows.some((row) => row.includes("pricing"))).toBe(false);
  });
});
