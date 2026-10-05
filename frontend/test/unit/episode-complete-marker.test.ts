// @vitest-environment jsdom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { DeskEpisode } from "@/lib/hive";
import { describeSettle, EpisodeCompleteMarker } from "@/views/room/EpisodeCompleteMarker";

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

function episode(over: Partial<DeskEpisode> = {}): DeskEpisode {
  return { id: "ep-1", chatId: "engineering", messageIds: [], status: "settled", ...over };
}

function render(value: DeskEpisode) {
  act(() => {
    root.render(createElement(EpisodeCompleteMarker, { episode: value }));
  });
  return host.querySelector('[data-testid="episode-complete"]') as HTMLElement;
}

describe("EpisodeCompleteMarker", () => {
  it("says a settled episode is complete", () => {
    const marker = render(episode());
    expect(marker.dataset.episodeId).toBe("ep-1");
    expect(marker.dataset.episodeStatus).toBe("settled");
    expect(marker.textContent).toBe("Episode complete");
  });

  it("says a failed episode failed, with the host's reason when it gave one", () => {
    const marker = render(episode({ status: "failed", failure: "turn timed out" }));
    expect(marker.dataset.episodeStatus).toBe("failed");
    expect(marker.textContent).toBe("Episode failed · turn timed out");
    expect(describeSettle({ status: "failed" })).toBe("Episode failed");
  });
});
