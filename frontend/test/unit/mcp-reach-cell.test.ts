// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { avatarFor } from "@/lib/team";
import { McpReachCell, fitCount } from "@/views/connections/mcp-reach-cell";

/**
 * Who reaches a server, as faces.
 *
 * Two things are worth pinning. The mark is the hashed **mascot**, never an
 * uploaded avatar: a mascot reference resolves synchronously from a static file,
 * so a row needs nothing but the id `reachableBy` already carries — no roster
 * read, no authenticated fetch, and no request per face on a list of twenty
 * servers. And the overflow control states that an answer is being withheld, so
 * it must never be shown on a guess: a company with two teammates sees no
 * overflow, and neither does a cell nothing has measured yet.
 */

describe("how many faces fit", () => {
  it("shows them all before anything has measured the column", () => {
    // The overflow is a claim that something is hidden. An unmeasured cell has no
    // grounds for it.
    expect(fitCount(7, null)).toBe(7);
    expect(fitCount(7, 0)).toBe(7);
  });

  it("never overflows a company small enough to fit", () => {
    // Two faces at 24px and an 18px pitch need 42px, which any column holds.
    expect(fitCount(2, 200)).toBe(2);
    expect(fitCount(2, 60)).toBe(2);
  });

  it("is a width question rather than a fixed three", () => {
    expect(fitCount(12, 400)).toBeGreaterThan(3);
    expect(fitCount(12, 140)).toBeLessThan(fitCount(12, 400));
  });

  it("keeps at least one face however narrow the column", () => {
    expect(fitCount(12, 20)).toBe(1);
  });
});

const AGENTS = [
  { id: "chief_executive", name: "Chief Executive" },
  { id: "engineer", name: "Engineer" },
  { id: "writer", name: "Writer" },
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

describe("the cell", () => {
  it("draws the mascot hashed from the id, not a letter and not an upload", () => {
    act(() => {
      root.render(
        createElement(McpReachCell, {
          agents: AGENTS,
          serverName: "notion",
          onOverflow: () => {},
        }),
      );
    });

    const images = [...container.querySelectorAll("img")];
    expect(images).toHaveLength(3);
    // `tiny:<flavour>` resolves to a file this origin already ships, so the row
    // costs no request per face.
    for (const agent of AGENTS) {
      expect(avatarFor(agent.id)).toMatch(/^tiny:/);
    }
    expect(images.every((img) => img.getAttribute("src")?.startsWith("http"))).toBe(
      false,
    );
  });

  it("says so, rather than showing an empty stack, when nobody reaches it", () => {
    act(() => {
      root.render(
        createElement(McpReachCell, {
          agents: [],
          serverName: "notion",
          onOverflow: () => {},
        }),
      );
    });

    expect(container.textContent).toContain("no teammate");
    expect(container.querySelector('[data-testid="mcp-reach-overflow"]')).toBeNull();
  });

  it("offers no overflow while every face is on screen", () => {
    act(() => {
      root.render(
        createElement(McpReachCell, {
          agents: AGENTS,
          serverName: "notion",
          onOverflow: () => {},
        }),
      );
    });

    expect(container.querySelector('[data-testid="mcp-reach-overflow"]')).toBeNull();
  });
});
