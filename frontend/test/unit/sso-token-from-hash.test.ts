import { describe, expect, it } from "vitest";

import { ssoTokenFromHash } from "@/views/login/sso";

// A JWT-shaped value — three base64url segments — so the test exercises the
// characters a real token carries (`.`, `-`, `_`) rather than a placeholder.
const JWT = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhZGFAZXhhbXBsZS5jb20ifQ.abc-_DEF123";

describe("ssoTokenFromHash", () => {
  it("reads the token from the canonical #/sso?token=… fragment", () => {
    expect(ssoTokenFromHash(`#/sso?token=${JWT}`)).toBe(JWT);
  });

  it("tolerates a missing leading slash on the route", () => {
    expect(ssoTokenFromHash(`#sso?token=${JWT}`)).toBe(JWT);
  });

  it("tolerates a missing leading hash", () => {
    expect(ssoTokenFromHash(`/sso?token=${JWT}`)).toBe(JWT);
  });

  it("finds the token among other query params in any order", () => {
    expect(ssoTokenFromHash(`#/sso?from=dashboard&token=${JWT}`)).toBe(JWT);
    expect(ssoTokenFromHash(`#/sso?token=${JWT}&from=dashboard`)).toBe(JWT);
  });

  it("returns null for a different route", () => {
    expect(ssoTokenFromHash(`#/overview?token=${JWT}`)).toBeNull();
    expect(ssoTokenFromHash("#/company")).toBeNull();
  });

  it("returns null when there is no fragment at all", () => {
    expect(ssoTokenFromHash("")).toBeNull();
  });

  it("returns null on the sso route with no token", () => {
    expect(ssoTokenFromHash("#/sso")).toBeNull();
    expect(ssoTokenFromHash("#/sso?from=dashboard")).toBeNull();
  });

  it("returns null for a blank token", () => {
    expect(ssoTokenFromHash("#/sso?token=")).toBeNull();
    expect(ssoTokenFromHash("#/sso?token=%20%20")).toBeNull();
  });

  it("does not confuse a route that merely starts with sso", () => {
    expect(ssoTokenFromHash(`#/ssory?token=${JWT}`)).toBeNull();
  });
});
