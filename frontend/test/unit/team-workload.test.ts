// `workloadByAssignee` — the derivation behind the Company cards' status dot
// and open-task count (issue #1141).
//
// The card says two things no host field answers, so every rule that keeps them
// honest is asserted here: what counts as open, what counts as working, whose
// work it is, and what happens when the host cannot say.

import { describe, expect, it } from "vitest";

import type { Task } from "@/api/tasks";
import type { TaskColumn } from "@/lib/board-columns";
import { workloadByAssignee } from "@/lib/team-workload";

/** The board's real columns, as the `tasks` ledger reports them (issue #1512). */
const COLUMNS: TaskColumn[] = [
  { id: "pending", label: "Pending", closed: false },
  { id: "working", label: "Working", closed: false },
  { id: "done", label: "Done", closed: true },
];

/**
 * A card in one of the host's six **stages**, phased the way the API phases it.
 *
 * The tests are written in stages because that is what the distinctions here
 * are about — paused is not in progress — and the derivation must read them off
 * `stage` now that all four share the `working` column.
 */
const PHASE_OF: Record<string, string> = {
  todo: "pending",
  planning: "working",
  in_progress: "working",
  paused: "working",
  in_review: "working",
  done: "done",
};

function task(assignee: string, stage: string, id = `${assignee}-${stage}`): Task {
  const column = PHASE_OF[stage] ?? stage;
  return {
    id,
    title: id,
    column,
    ...(column === "working" ? { stage } : {}),
    priority: "medium",
    assignee,
    updatedAt: 0,
  };
}

describe("workloadByAssignee", () => {
  it("counts a teammate's cards in every column the host has not closed", () => {
    const loads = workloadByAssignee(
      [
        task("maya", "todo"),
        task("maya", "paused"),
        task("maya", "in_review"),
        task("maya", "done"),
      ],
      COLUMNS,
    );

    // Three open, and the finished one is not one of them.
    expect(loads.get("maya")).toEqual({ open: 3, status: "idle" });
  });

  it("is idle while every open card is waiting on a person", () => {
    // Paused and In review are the host's "stopped, not finished" stages.
    // Work sitting in them is not work a teammate is doing.
    const loads = workloadByAssignee([task("maya", "paused"), task("maya", "in_review")], COLUMNS);

    expect(loads.get("maya")?.status).toBe("idle");
  });

  it("is working once a card is planning or in progress", () => {
    for (const stage of ["planning", "in_progress"]) {
      const loads = workloadByAssignee([task("maya", "todo"), task("maya", stage)], COLUMNS);
      expect(loads.get("maya")).toEqual({ open: 2, status: "working" });
    }
  });

  it("does not attribute a desk's card to the people on that desk", () => {
    // `assignee` is a desk id or a teammate id, and the host deliberately never
    // resolves a desk assignment to its lead. Nor does this.
    const loads = workloadByAssignee([task("research-desk", "in_progress")], COLUMNS);

    expect(loads.get("maya")).toBeUndefined();
    expect(loads.get("research-desk")).toEqual({ open: 1, status: "working" });
  });

  it("ignores the unassigned wire value, blank or whitespace", () => {
    const loads = workloadByAssignee([task("", "todo", "a"), task("   ", "todo", "b")], COLUMNS);

    expect(loads.size).toBe(0);
  });

  it("counts a column it has never heard of as open, but not as working", () => {
    // An id this build does not know is outstanding work by any reading, and
    // says nothing about an attempt running.
    const loads = workloadByAssignee([task("maya", "escalated")], COLUMNS);

    expect(loads.get("maya")).toEqual({ open: 1, status: "idle" });
  });

  it("says nothing at all when the column vocabulary is unknown", () => {
    // Without the host's columns there is no way to tell finished from open, so
    // the honest answer is silence — the caller draws no status and no count
    // rather than a zero that would claim every teammate is free.
    const loads = workloadByAssignee([task("maya", "in_progress")], []);

    expect(loads.size).toBe(0);
  });
});
