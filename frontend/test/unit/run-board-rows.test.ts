import { describe, expect, it } from "vitest";

import { BOARD_ACTION, boardActionMeta } from "@/views/workflows/run-board";

describe("run board action labels", () => {
  it("labels a write with no board wired as a failed row", () => {
    expect(boardActionMeta("boardUnwired")).toEqual({
      label: "No board",
      tone: BOARD_ACTION.spawnFailed.tone,
      failed: true,
    });
  });

  it("keeps the success arms neutral and the failed arms failed", () => {
    expect(boardActionMeta("spawned").failed).toBe(false);
    expect(boardActionMeta("assigned").failed).toBe(false);
    expect(boardActionMeta("spawnFailed").failed).toBe(true);
    expect(boardActionMeta("assignFailed").failed).toBe(true);
  });

  it("falls back to a failed row for an action it does not know", () => {
    const meta = boardActionMeta("somethingNew");
    expect(meta.failed).toBe(true);
    expect(meta.label).toBe("Unknown");
    expect(boardActionMeta("toString").label).toBe("Unknown");
  });
});
