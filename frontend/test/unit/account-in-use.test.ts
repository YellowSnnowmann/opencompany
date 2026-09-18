import { describe, expect, it } from "vitest";

import { ApiError } from "@/api/types";
import {
  accountKeyUsedByMessage,
  confirmInUseFor,
  guardedOutcome,
  isInUseRefusal,
} from "@/views/connections/account-in-use";

/**
 * Keys rework (issue #2306), bug KR-L3-01 — the pure state machine behind the
 * Account page's Remove-key confirm dialog
 * (`docs/key-reworks/in-use-guards.md`). No component-test harness exists in
 * this project (see `test/unit/budget-pause-notice.test.ts`'s header), so this
 * pins the decisions `ApiKeyView.tsx` is built from rather than the markup
 * around them — mirrors `test/unit/composio-in-use.test.ts` and
 * `test/unit/search-in-use.test.ts` for the two decisions this page shares
 * with those, plus `accountKeyUsedByMessage`, the one decision unique to this
 * page (the upfront, no-round-trip reason).
 */
describe("isInUseRefusal", () => {
  it("recognises the host's 409 in_use envelope", () => {
    const err = new ApiError(
      409,
      "in_use",
      "Used by TinyHumans on the LLM page.",
      true,
    );
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
    new ApiError(409, "in_use", "Used by TinyHumans on the LLM page.", true);

  it("reopens with the host's message on a first, unconfirmed in-use refusal", () => {
    expect(guardedOutcome(inUse(), false)).toEqual({
      action: "reopen",
      message: "Used by TinyHumans on the LLM page.",
    });
  });

  it("closes when a CONFIRMED attempt is refused in_use again", () => {
    // The confirmed write itself failed — not the stale-UI case the reopen
    // exists for, so it must not loop forever showing the same notice.
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
  it("is false before anything has said the key is in use", () => {
    expect(confirmInUseFor(null)).toBe(false);
  });

  it("is true once the dialog is showing a reason, from either source", () => {
    expect(confirmInUseFor("Used by TinyHumans on the LLM page.")).toBe(true);
  });

  it("is true even for an empty-string reason — presence, not content, decides", () => {
    expect(confirmInUseFor("")).toBe(true);
  });
});

describe("accountKeyUsedByMessage", () => {
  it("is null when usedBy is absent — the fallback-to-generic-text signal", () => {
    expect(accountKeyUsedByMessage(undefined)).toBeNull();
    expect(accountKeyUsedByMessage(null)).toBeNull();
  });

  it("is null when usedBy carries no surfaces — the account key's usedBy never sets default/agents", () => {
    expect(accountKeyUsedByMessage({})).toBeNull();
  });

  it("names one surface", () => {
    expect(accountKeyUsedByMessage({ surfaces: ["composio"] })).toBe(
      "Used by Composio.",
    );
  });

  it("names every surface, in the wire order, with display labels rather than the raw ids", () => {
    expect(accountKeyUsedByMessage({ surfaces: ["llm", "composio"] })).toBe(
      "Used by TinyHumans on the LLM page and by Composio.",
    );
  });
});
