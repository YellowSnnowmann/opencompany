import { describe, expect, it } from "vitest";

import {
  agentEdits,
  draftFrom,
  draftIsValid,
  isEditable,
  summarizeGrants,
  tierLabel,
} from "@/lib/agent";
import type { AgentDetailDto } from "@/api/types";

/**
 * The derivations behind the agent detail view (issue #264).
 *
 * Each of these is a place where being subtly wrong looks completely normal on
 * screen: a patch body that erases the instructions it meant to leave alone, a
 * tool list that reports the opposite of an agent's real grant, a tier that
 * reads "Worker" for the agent actually running the company. None of them
 * throws when it is wrong, which is why they are unit-tested rather than left
 * to the browser suite to notice.
 */

function agent(over: Partial<AgentDetailDto> = {}): AgentDetailDto {
  return {
    id: "jamie",
    name: "Jamie",
    role: "Growth",
    description: "Runs paid acquisition.",
    source: "overlay",
    editable: ["name", "role", "description"],
    isOrchestrator: false,
    tools: { requested: [], companyAllow: ["workspace.*"], effective: ["workspace.*"] },
    desks: [],
    inboxEnabled: false,
    ...over,
  };
}

const manifest = () =>
  agent({ id: "ceo", name: undefined, role: "Chief Executive", source: "manifest", editable: [] });

describe("what an edit sends", () => {
  it("sends nothing at all when nothing changed", () => {
    const detail = agent();
    expect(agentEdits(detail, draftFrom(detail))).toBeNull();
  });

  it("sends only the fields that changed", () => {
    const detail = agent();
    expect(agentEdits(detail, { ...draftFrom(detail), role: "Head of Growth" })).toEqual({
      role: "Head of Growth",
    });
  });

  it("distinguishes clearing the instructions from leaving them alone", () => {
    const detail = agent();

    // Emptied on purpose: `null` is the only value the host reads as "clear
    // it". Sending `undefined` would leave the old text in place and the
    // operator would watch their deletion come back on the next load.
    expect(agentEdits(detail, { ...draftFrom(detail), description: "   " })).toEqual({
      description: null,
    });

    // Untouched: the key is absent, so the host leaves the instructions be.
    const untouched = agentEdits(detail, { ...draftFrom(detail), name: "Jamie R" });
    expect(untouched).toEqual({ name: "Jamie R" });
    expect(untouched && "description" in untouched).toBe(false);
  });

  it("trims, so whitespace alone is not a change", () => {
    const detail = agent();
    expect(agentEdits(detail, { ...draftFrom(detail), name: "  Jamie  " })).toBeNull();
  });

  it("never sends a field the host says is read-only", () => {
    // A manifest teammate's fields are rendered, disabled, so the draft still
    // holds their values. Echoing one back would earn a 409 for the whole save.
    const detail = manifest();
    expect(agentEdits(detail, { name: "Nope", role: "Chief Vibes", description: "New", instructions: "" })).toBeNull();
    expect(isEditable(detail, "role")).toBe(false);
  });
});

describe("whether a draft can be saved", () => {
  it("requires a name and a role, but not instructions", () => {
    const detail = agent();
    expect(draftIsValid(detail, { name: "Jamie", role: "Growth", description: "", instructions: "" })).toBe(true);
    expect(draftIsValid(detail, { name: "  ", role: "Growth", description: "x", instructions: "" })).toBe(false);
    expect(draftIsValid(detail, { name: "Jamie", role: "", description: "x", instructions: "" })).toBe(false);
  });

  it("does not block on a read-only field being blank", () => {
    // A manifest teammate carries no name of its own, so the disabled name box
    // is empty. That must not make Save unreachable for a form that has no
    // editable fields to begin with.
    expect(draftIsValid(manifest(), { name: "", role: "Chief Executive", description: "", instructions: "" })).toBe(
      true,
    );
  });
});

describe("what an agent's tools amount to", () => {
  it("names the standard grant rather than showing an empty request", () => {
    // The inversion worth guarding: an agent that lists no tools holds
    // everything the company allows. A surface that rendered `requested` would
    // tell the operator this agent is powerless.
    const summary = summarizeGrants({
      requested: [],
      companyAllow: ["workspace", "composio"],
      effective: ["workspace", "composio"],
    });
    expect(summary.standardGrant).toBe(true);
    expect(summary.effective).toEqual(["workspace", "composio"]);
    expect(summary.dropped).toEqual([]);
  });

  it("shows what was asked for and not granted", () => {
    const summary = summarizeGrants({
      requested: ["workspace.read", "email.send"],
      companyAllow: ["workspace.*"],
      effective: ["workspace.read"],
    });
    expect(summary.standardGrant).toBe(false);
    expect(summary.dropped).toEqual(["email.send"]);
  });

  it("reports an agent that holds nothing", () => {
    const summary = summarizeGrants({
      requested: ["email.send"],
      companyAllow: ["workspace"],
      effective: [],
    });
    expect(summary.effective).toEqual([]);
    expect(summary.dropped).toEqual(["email.send"]);
    expect(summary.standardGrant).toBe(false);
  });
});

describe("how a tier reads", () => {
  it("follows the host's resolution rather than the tier string", () => {
    // A company that tags nobody still has an orchestrator: the first agent
    // declared. The host resolves it; reading `tier` here would call that
    // agent a worker.
    expect(tierLabel(agent({ tier: undefined, isOrchestrator: true }))).toBe("Orchestrator");
    expect(tierLabel(agent({ tier: "orchestrator", isOrchestrator: true }))).toBe("Orchestrator");
    expect(tierLabel(agent())).toBe("Worker");
  });
});

describe("the draft a detail view starts from", () => {
  it("shows a manifest teammate by its role, since it carries no name", () => {
    expect(draftFrom(manifest()).name).toBe("");
    expect(draftFrom(manifest()).role).toBe("Chief Executive");
  });
});
