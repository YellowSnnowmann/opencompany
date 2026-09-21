// @vitest-environment jsdom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { UtteranceChip } from "@/components/episode/UtteranceChip";
import type { MessageEpisodeDto } from "@/api/types";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let host: HTMLDivElement;
let root: Root;
beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});
afterEach(() => {
  act(() => root.unmount());
  host.remove();
});

function render(episode: MessageEpisodeDto, audience?: string[]) {
  act(() => {
    root.render(createElement(UtteranceChip, { episode, audience, agentNames: { engineer: "Engineer", ceo: "CEO" } }));
  });
  return host.querySelector('[data-testid="utterance-chip"]') as HTMLElement;
}

describe("UtteranceChip", () => {
  it("names each speech act by word, and stamps the episode and round", () => {
    for (const [kind, word] of [
      ["post", "post"],
      ["broadcast", "broadcast"],
      ["dm", "dm"],
      ["complete_episode", "complete"],
    ] as const) {
      const chip = render({ id: "ep-1", revision: 2, kind });
      expect(chip.dataset.kind).toBe(kind);
      expect(chip.dataset.episodeId).toBe("ep-1");
      expect(chip.dataset.roundRevision).toBe("2");
      expect(chip.textContent).toContain(word);
    }
  });

  it("addresses a dm to its recipients by display name", () => {
    const chip = render({ id: "ep-1", revision: 1, kind: "dm", to: ["engineer"] });
    expect(chip.querySelector('[data-testid="utterance-audience"]')?.textContent).toBe("→ @Engineer");
  });

  it("falls back to the audience when a dm names no recipients, and to the id when unnamed", () => {
    const chip = render({ id: "ep-1", revision: 1, kind: "dm" }, ["writer"]);
    expect(chip.querySelector('[data-testid="utterance-audience"]')?.textContent).toBe("→ @writer");
    expect(render({ id: "ep-1", revision: 0, kind: "post" }, ["writer"]).querySelector('[data-testid="utterance-audience"]')).toBeNull();
  });

  it("shows how a broadcast was routed onward", () => {
    const chip = render({
      id: "ep-1",
      revision: 1,
      kind: "broadcast",
      routedBy: { plan: { kind: "hive", primaryId: "ceo", invitedIds: ["engineer"] }, router: "jev" },
    });
    const plan = chip.querySelector('[data-testid="routing-plan-chip"]') as HTMLElement;
    expect(plan.dataset.planKind).toBe("hive");
    expect(plan.dataset.router).toBe("jev");
    expect(plan.textContent).toContain("→ CEO + Engineer");
  });
});
