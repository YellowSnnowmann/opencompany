// What a create request carries for the new teammate's look, and what is
// written after it when the host did not take it (`lib/new-member-look.ts`).

import { describe, expect, it, vi } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import type { TeamMemberDto } from "@/api/types";
import { birthLook, unechoedLook, writeUnechoedLook } from "@/lib/new-member-look";
import { newMember } from "@/lib/team";

const FULL = {
  avatar: "mascot:animated",
  mascotMode: "animated",
  mascotCostume: "headband",
  mascotSkinColor: "peach",
  mascotHandColor: "charcoal",
};

const created = (extra: Partial<TeamMemberDto> = {}): TeamMemberDto =>
  ({ id: "growth", name: "Growth", role: "Growth Marketer", ...extra }) as TeamMemberDto;

describe("birthLook", () => {
  it("keeps what was chosen and trims it", () => {
    expect(birthLook({ avatar: " mascot:animated ", mascotCostume: "cap" })).toEqual({
      avatar: "mascot:animated",
      mascotCostume: "cap",
    });
  });

  it("drops blanks and undefined rather than sending an empty string", () => {
    expect(birthLook({ avatar: "", mascotMode: "   ", mascotCostume: undefined })).toEqual({});
  });

  it("carries nothing but the five look fields", () => {
    const wide = { ...FULL, name: "x", role: "y" } as unknown as typeof FULL;
    expect(Object.keys(birthLook(wide)).sort()).toEqual(Object.keys(FULL).sort());
  });
});

describe("unechoedLook", () => {
  it("is null when the host echoed every field it was sent", () => {
    expect(unechoedLook(created(FULL), FULL)).toBeNull();
  });

  it("is null when nobody chose anything", () => {
    expect(unechoedLook(created(), {})).toBeNull();
  });

  it("names every field an older host ignored", () => {
    expect(unechoedLook(created(), FULL)).toEqual(FULL);
  });

  it("names only the fields that were not echoed", () => {
    const partial = created({ avatar: FULL.avatar, mascotMode: FULL.mascotMode });
    expect(unechoedLook(partial, FULL)).toEqual({
      mascotCostume: FULL.mascotCostume,
      mascotSkinColor: FULL.mascotSkinColor,
      mascotHandColor: FULL.mascotHandColor,
    });
  });

  it("treats an echo of a different value as not taken", () => {
    expect(unechoedLook(created({ ...FULL, mascotCostume: "cap" }), FULL)).toEqual({
      mascotCostume: "headband",
    });
  });
});

describe("writeUnechoedLook", () => {
  const clientWith = (updateAgent: unknown) => ({ updateAgent }) as unknown as OpenCompanyClient;

  it("writes nothing when the host took the whole look", async () => {
    const updateAgent = vi.fn();
    expect(await writeUnechoedLook(clientWith(updateAgent), "acme", created(FULL), FULL)).toBe(true);
    expect(updateAgent).not.toHaveBeenCalled();
  });

  it("patches only the missing fields, for the created id, in this company", async () => {
    const updateAgent = vi.fn().mockResolvedValue({});
    const echoed = created({ avatar: FULL.avatar });
    expect(await writeUnechoedLook(clientWith(updateAgent), "acme", echoed, FULL)).toBe(true);
    expect(updateAgent).toHaveBeenCalledTimes(1);
    const [id, patch, company] = updateAgent.mock.calls[0];
    expect(id).toBe("growth");
    expect(company).toBe("acme");
    expect(patch).not.toHaveProperty("avatar");
    expect(patch).toMatchObject({ mascotMode: "animated", mascotCostume: "headband" });
  });

  it("answers false, never throws, when the write fails", async () => {
    const updateAgent = vi.fn().mockRejectedValue(new Error("400 Pick one of"));
    expect(await writeUnechoedLook(clientWith(updateAgent), "acme", created(), FULL)).toBe(false);
  });
});

describe("newMember (no host to write to)", () => {
  const base = { name: " Ada ", role: "Analyst", description: "" };

  it("wears the look it was created with", () => {
    const row = newMember({ ...base, ...FULL });
    expect(row.avatar).toBe("mascot:animated");
    expect(row.mascotMode).toBe("animated");
    expect(row.mascotCostume).toBe("headband");
    expect(row.mascotSkinColor).toBe("peach");
    expect(row.mascotHandColor).toBe("charcoal");
  });

  it("falls back to the hashed face and the file's own look when nothing was chosen", () => {
    const row = newMember(base);
    expect(row.avatar).toBeTruthy();
    expect(row.mascotMode).toBeUndefined();
    expect(row.mascotCostume).toBeUndefined();
    expect(row.name).toBe("Ada");
  });
});
