import { describe, expect, it } from "vitest";

import { ApiError } from "@/api/types";
import {
  confirmInUseFor,
  guardedOutcome,
  isInUseRefusal,
} from "@/composio/in-use";

/**
 * Keys rework (issue #2306) — the pure state machine behind the Composio
 * confirm dialogs for clearing the managed token and switching route
 * (`docs/key-reworks/in-use-guards.md`). No component-test harness exists in
 * this project (see `test/unit/budget-pause-notice.test.ts`'s header), so
 * this pins the decisions `ComposioSection.tsx` is built from rather than the
 * markup around them.
 */
describe("isInUseRefusal", () => {
  it("recognises the host's 409 in_use envelope", () => {
    const err = new ApiError(409, "in_use", "Composio's key is used by Composio.", true);
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
  const inUse = () =>
    new ApiError(409, "in_use", "Composio's key is used by Composio.", true);

  it("reopens with the host's message on a first, unconfirmed in-use refusal", () => {
    expect(guardedOutcome(inUse(), false)).toEqual({
      action: "reopen",
      message: "Composio's key is used by Composio.",
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
    expect(confirmInUseFor("Composio's key is used by Composio.")).toBe(true);
  });

  it("is true even for an empty-string notice — presence, not content, decides", () => {
    expect(confirmInUseFor("")).toBe(true);
  });
});
