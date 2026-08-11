import { describe, expect, it } from "vitest";

import { SKILLS_READ_ONLY_NOTE, skillReachLabel } from "@/lib/skills";

/**
 * The Skills tab's honesty about what a skill is (issue #569).
 *
 * A desk agent lists, describes and reads an installed skill and can never run
 * one — `dispatched_belt_excludes_every_deferred_family` keeps `run_skill`,
 * `skill_run`, `run_workflow` and `await_workflow` off every dispatched belt,
 * and only the orchestrator holds `RunWorkflowTool`. The behaviour is intended;
 * the bug was that the console said nothing, while offering install / enable /
 * disable — the vocabulary of turning a capability on.
 *
 * These assert the *claim*, not the layout: that the note names reading and the
 * orchestrator, and that neither it nor the per-card line tells an operator a
 * teammate will execute anything. A future copy edit is free to move words
 * around; it is not free to quietly put the promise back.
 */

describe("SKILLS_READ_ONLY_NOTE", () => {
  it("says teammates read skills", () => {
    expect(SKILLS_READ_ONLY_NOTE).toMatch(/\bread\b/i);
  });

  it("names the orchestrator as what executes a workflow", () => {
    expect(SKILLS_READ_ONLY_NOTE).toMatch(/orchestrator/i);
  });

  it("never promises that enabling a skill makes a teammate run it", () => {
    // The exact shape of the old implication: enabling/installing framed as
    // handing a teammate something it will carry out.
    expect(SKILLS_READ_ONLY_NOTE).not.toMatch(/teammates? (can|will) (run|execute|use)/i);
    expect(SKILLS_READ_ONLY_NOTE).not.toMatch(/agents? (can|will) (run|execute)/i);
  });
});

describe("skillReachLabel", () => {
  it("describes an enabled skill as readable, not as runnable", () => {
    const label = skillReachLabel(true);
    expect(label).toMatch(/read/i);
    expect(label).not.toMatch(/run|execute/i);
  });

  it("describes a disabled skill as out of a teammate's sight", () => {
    expect(skillReachLabel(false)).toMatch(/hidden/i);
  });

  it("distinguishes the two states", () => {
    expect(skillReachLabel(true)).not.toBe(skillReachLabel(false));
  });
});
