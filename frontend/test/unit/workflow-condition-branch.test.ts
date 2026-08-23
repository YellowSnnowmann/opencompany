import { describe, expect, it } from "vitest";

import { conditionBranchChoice } from "@/views/WorkflowCreateDialog";
import type { EdgeEndpoint } from "@/views/WorkflowCreateDialog";

/**
 * The condition branch-label control (issue #1074).
 *
 * The host refuses an edge out of a `condition` node unless its label reads
 * `yes`/`no` — or exactly `error`, and only when that node is also
 * `on_error = "route"`. The dialog used to render the label as a free-text box,
 * so the only way to learn the rule was to submit and be refused.
 *
 * These pin the OFFER, not a second copy of the rule: what the select may show,
 * which stored labels it can represent, and which it must surface rather than
 * quietly rewrite. The host stays the authority (`POST …/workflows/validate`).
 */

const condition = (over: Partial<EdgeEndpoint> = {}): EdgeEndpoint => ({
  id: "gate",
  kind: "condition",
  ...over,
});

describe("conditionBranchChoice — when it applies at all", () => {
  it("leaves a non-condition source on free text", () => {
    expect(conditionBranchChoice({ id: "worker", kind: "agent" }, "ok")).toBeNull();
    expect(conditionBranchChoice({ id: "start", kind: "trigger" }, "")).toBeNull();
  });

  it("leaves an edge with no source yet on free text", () => {
    expect(conditionBranchChoice(undefined, "anything")).toBeNull();
  });
});

describe("conditionBranchChoice — the options offered", () => {
  it("offers yes and no on an ordinary condition", () => {
    expect(conditionBranchChoice(condition(), "yes")?.options).toEqual(["yes", "no"]);
  });

  // The narrow exception, and the part a hand-written client check gets wrong:
  // `error` is legal only because THIS node routes its errors.
  it("adds error only when the condition itself is on_error = route", () => {
    expect(conditionBranchChoice(condition({ onError: "route" }), "yes")?.options).toEqual([
      "yes",
      "no",
      "error",
    ]);
    for (const onError of [undefined, "stop", "continue"]) {
      expect(conditionBranchChoice(condition({ onError }), "yes")?.options).toEqual([
        "yes",
        "no",
      ]);
    }
  });
});

describe("conditionBranchChoice — representing a label that is already there", () => {
  // The host lowercases and trims before matching yes/no, so a graph that
  // stored `Yes` is legal and must not be reported as unrepresentable.
  it("represents a stored Yes / ` no ` as the matching option, with no problem", () => {
    for (const stored of ["Yes", "YES", " yes ", "yes"]) {
      const choice = conditionBranchChoice(condition(), stored);
      expect(choice?.value).toBe("yes");
      expect(choice?.problem).toBeNull();
    }
    expect(conditionBranchChoice(condition(), " No ")?.value).toBe("no");
  });

  // The host compares `error` VERBATIM, unlike yes/no. Mirroring that asymmetry
  // is the whole reason this is one function rather than a lowercase compare.
  it("matches error verbatim, so Error is not the recovery branch", () => {
    const routed = condition({ onError: "route" });
    expect(conditionBranchChoice(routed, "error")?.problem).toBeNull();
    const shouted = conditionBranchChoice(routed, "Error");
    expect(shouted?.value).toBe("Error");
    expect(shouted?.problem).toContain("`Error` is not a branch");
  });

  it("refuses error on a condition that does not route its errors", () => {
    const choice = conditionBranchChoice(condition(), "error");
    expect(choice?.options).toEqual(["yes", "no"]);
    expect(choice?.problem).toContain("`error` is not a branch");
  });

  // Surfacing beats a quiet edit: `parse_workflow` is lenient on this rule
  // (issue #682) so a pre-#661 graph carrying `maybe` still loads and reaches
  // this dialog. The label is handed back unchanged, with the problem named.
  it("keeps an unrepresentable label and names the problem", () => {
    const choice = conditionBranchChoice(condition(), "maybe");
    expect(choice?.value).toBe("maybe");
    expect(choice?.problem).toContain("must be labeled `yes` or `no`");
  });

  it("asks for a choice on a condition edge with no label yet", () => {
    const choice = conditionBranchChoice(condition(), "");
    expect(choice?.value).toBe("");
    expect(choice?.problem).toContain("pick a branch");
  });
});
