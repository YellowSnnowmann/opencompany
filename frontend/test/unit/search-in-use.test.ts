import { describe, expect, it } from "vitest";

import { ApiError } from "@/api/types";
import {
  confirmInUseFor,
  guardedOutcome,
  isInUseRefusal,
} from "@/search-providers/in-use";

/**
 * Keys rework (issue #2306) — the pure state machine behind the search page's
 * confirm dialogs for disabling, removing, or clearing the key of the search
 * default (`docs/key-reworks/in-use-guards.md`). No component-test harness
 * exists in this project (see `test/unit/budget-pause-notice.test.ts`'s
 * header), so this pins the decisions `SearchView.tsx` is built from rather
 * than the markup around them. Mirrors `test/unit/composio-in-use.test.ts`
 * for the sibling module `@/composio/in-use` — the two are deliberately
 * separate files (see `@/search-providers/in-use`'s own header) but pin the
 * same shared contract.
 */
describe("isInUseRefusal", () => {
  it("recognises the host's 409 in_use envelope", () => {
    const err = new ApiError(409, "in_use", "Exa is the search default.", true);
    expect(isInUseRefusal(err)).toBe(true);
  });

  it("does not misfire on an ordinary refusal with a different code", () => {
    const err = new ApiError(400, "invalid_request", "bad request", true);
    expect(isInUseRefusal(err)).toBe(false);
  });

  it("does not misfire on a non-ApiError, such as a network failure", () => {
    expect(isInUseRefusal(new TypeError("failed to fetch"))).toBe(false);
    expect(isInUseRefusal(undefined)).toBe(false);
    expect(isInUseRefusal("in_use")).toBe(false);
  });
});

describe("guardedOutcome", () => {
  const inUse = () => new ApiError(409, "in_use", "Exa is the search default.", true);

  it("reopens with the host's message on a first, unconfirmed in-use refusal", () => {
    expect(guardedOutcome(inUse(), false)).toEqual({
      action: "reopen",
      message: "Exa is the search default.",
    });
  });

  it("carries the refusal's own usedBy through, fresher than whatever the dialog opened with (round-3 review, P1-2)", () => {
    const err = new ApiError(409, "in_use", "Exa is the search default.", true);
    err.usedBy = { default: true };
    expect(guardedOutcome(err, false)).toEqual({
      action: "reopen",
      message: "Exa is the search default.",
      usedBy: { default: true },
    });
  });

  it("closes when a CONFIRMED attempt is refused in_use again", () => {
    // The confirmed write itself failed — this is not the stale-UI case the
    // reopen exists for, so it must not loop forever showing the same notice.
    expect(guardedOutcome(inUse(), true)).toEqual({ action: "close" });
  });

  it("closes on success (no error at all)", () => {
    expect(guardedOutcome(undefined, false)).toEqual({ action: "close" });
  });

  it("closes on an unrelated failure, unconfirmed or not — the caller shows the real error instead", () => {
    const err = new ApiError(403, "forbidden", "admin only", true);
    expect(guardedOutcome(err, false)).toEqual({ action: "close" });
    expect(guardedOutcome(err, true)).toEqual({ action: "close" });
  });

  it("closes on a network failure that is not an ApiError at all", () => {
    expect(guardedOutcome(new TypeError("failed to fetch"), false)).toEqual({
      action: "close",
    });
  });
});

describe("confirmInUseFor", () => {
  it("is false before any refusal has been shown", () => {
    expect(confirmInUseFor(null)).toBe(false);
  });

  it("is true once the dialog is showing a host-reported reason", () => {
    expect(confirmInUseFor("Exa is the search default.")).toBe(true);
  });

  it("is true even for an empty-string notice — presence, not content, decides", () => {
    expect(confirmInUseFor("")).toBe(true);
  });
});
