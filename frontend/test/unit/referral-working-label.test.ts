import { describe, expect, it } from "vitest";

import { referralWorkingLabel, type ReferralWorking } from "@/views/room/model";

/**
 * A crossing cannot be described as "<name> is working…", and the two kinds
 * are different events rather than two spellings of one (#2341 live report).
 */
describe("referralWorkingLabel", () => {
  const nameOf = (id: string) =>
    ({ cancellations: "Cancellations", amendments: "Amendments", returns: "Returns" })[id] ?? id;

  it("names both seats of a person crossing, because both are spending turns", () => {
    const crossing: ReferralWorking = {
      direct: true,
      asker: "cancellations",
      target: "amendments",
    };

    expect(referralWorkingLabel(crossing, nameOf)).toBe(
      "Cancellations and Amendments are talking",
    );
  });

  it("names the desk of a desk crossing, never the seat the library picked", () => {
    // The host resolves `@#returns` to that desk's FIRST eligible member, so a
    // seat name here would credit one member with a whole room's work.
    const crossing: ReferralWorking = { direct: false, desk: "returns" };

    expect(referralWorkingLabel(crossing, nameOf)).toBe("the Returns desk is answering");
  });

  it("falls back to the id when a name is not known yet", () => {
    const crossing: ReferralWorking = { direct: true, asker: "a", target: "b" };

    expect(referralWorkingLabel(crossing, (id) => id)).toBe("a and b are talking");
  });
});
