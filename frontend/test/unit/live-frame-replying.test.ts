import { describe, expect, it } from "vitest";

import { frameTurnMeta } from "@/lib/agent-presence";
import { foldTurnFrame, type LiveFrame, type LiveRow } from "@/lib/live-frame";

/**
 * The host's `replying` frame says an agent started writing its reply. It is a
 * live signal for the presence dot and never a step: the host's `fold_steps`
 * adds none for it, so a row here would make the live timeline and the folded
 * one disagree the moment the reply lands. These run the rule `onTurnEvent`
 * runs (`foldTurnFrame`, `frameTurnMeta`) over a real frame sequence.
 */

type Frame = LiveFrame | { type: "replying" };

/** Folds a sequence the way the shell does, row list and presence flag both. */
function run(frames: Frame[]) {
  let rows: LiveRow[] = [];
  const replying: boolean[] = [];
  for (const frame of frames) {
    rows = foldTurnFrame(rows, frame) ?? rows;
    replying.push(frameTurnMeta("rae", frame.type, 0).replying);
  }
  return { rows, replying };
}

describe("the replying frame", () => {
  const call: Frame = { type: "tool_call", toolCallId: "c1", label: "Search the docs" };
  const result: Frame = { type: "tool_result", toolCallId: "c1", status: "ok" };
  const thinking: Frame = { type: "thinking" };
  const replying: Frame = { type: "replying" };

  it("never adds a row: the rows equal the same run without it", () => {
    const withMarker = run([thinking, replying, call, result, replying]);
    const without = run([thinking, call, result]);
    expect(withMarker.rows).toEqual(without.rows);
    expect(withMarker.rows).toHaveLength(2);
  });

  it("answers null so the caller keeps the previous rows object", () => {
    const rows: LiveRow[] = [{ kind: "tool_call", status: "running", label: "x" }];
    expect(foldTurnFrame(rows, replying)).toBeNull();
    expect(foldTurnFrame([], replying)).toBeNull();
  });

  it("sets typing until a tool call or thinking frame resets it", () => {
    expect(run([replying, call, replying, thinking, replying]).replying).toEqual([
      true,
      false,
      true,
      false,
      true,
    ]);
  });

  it("carries the thread and the frame time for the age-out", () => {
    expect(frameTurnMeta("dm:rae", "replying", 42)).toEqual({
      chatId: "dm:rae",
      replying: true,
      lastFrameAt: 42,
    });
  });
});
