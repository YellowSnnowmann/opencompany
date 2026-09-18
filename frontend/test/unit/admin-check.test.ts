import { describe, expect, it } from "vitest";

import { ApiError } from "@/api/types";
import { resolveAdminCheckError } from "@/lib/admin-check";

/**
 * The distinction this classifier exists for: only a definitive `401` is an
 * answer about who the signed-in user is. Everything else is a failure to get
 * an answer, and a caller that pins it as "not an admin" stays wrong for the
 * rest of the mount.
 */
describe("resolveAdminCheckError", () => {
  it("settles to non-admin on a definitive 401 — no session on this host", () => {
    expect(resolveAdminCheckError(new ApiError(401, "no_session", "no session"))).toEqual({
      settled: true,
      isAdmin: false,
    });
  });

  it("does not settle on a network failure", () => {
    expect(resolveAdminCheckError(new TypeError("Failed to fetch"))).toEqual({ settled: false });
  });

  it("does not settle on a 5xx — the host, not the session, is the problem", () => {
    expect(resolveAdminCheckError(new ApiError(503, "unavailable", "quiescing"))).toEqual({ settled: false });
  });

  it("does not settle on a non-ApiError throw", () => {
    expect(resolveAdminCheckError("nope")).toEqual({ settled: false });
  });
});
