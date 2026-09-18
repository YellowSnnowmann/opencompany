import { describe, expect, it } from "vitest";

import { runningCrossingRows, type ReferralWorking } from "@/views/room/model";

/**
 * "asked @amendments · 1 message" describes a crossing that finished after one
 * reply. Shown while the pair were still talking, it was a running exchange
 * presented as a completed one — the stale count was only how it showed
 * (#2341 live report).
 */
describe("runningCrossingRows", () => {
  it("names the durable row each running crossing folds onto", () => {
    const working: Record<string, ReferralWorking> = {
      order_ops: { direct: true, asker: "cancellations", target: "amendments", row: 46 },
      eng: { direct: false, desk: "platform", row: 12 },
    };

    expect(runningCrossingRows(working).sort()).toEqual(["h12", "h46"]);
  });

  it("skips a crossing whose row the host did not name", () => {
    // An older host omits `sequence`; the chip then reads as it always did
    // rather than claiming a crossing is running with no row to point at.
    const working: Record<string, ReferralWorking> = {
      order_ops: { direct: true, asker: "a", target: "b" },
    };

    expect(runningCrossingRows(working)).toEqual([]);
  });

  it("is empty once the desk has spoken again and the entry is cleared", () => {
    expect(runningCrossingRows({})).toEqual([]);
  });
});
