import { describe, expect, it } from "vitest";

import { OpenCompanyClient } from "@/api/client";
import { lifecycleAffordances } from "@/lib/lifecycle-controls";

/**
 * Which lifecycle buttons the console is allowed to offer (issue #1401).
 *
 * The bug this pins was not a wrong result, it was a *reachable* control: the
 * console rendered `Archive` — destructive, behind a dialog calling itself
 * permanent — to an operator signed in with a magic link, took the
 * confirmation, and then answered `401 unauthorized`, because `archive` is a
 * `PlatformScope` route a session cookie can never reach. So every assertion
 * here is about a button *not* existing, which is the only form the fix can
 * take.
 */

/** A client as the console builds one for a person: cookie session, no bearer. */
function humanClient(): OpenCompanyClient {
  return new OpenCompanyClient({
    baseUrl: "",
    company: "acme",
    operatorToken: null,
    sessionHeader: null,
  });
}

/** A client as a hosting console builds one: `?token=` / `VITE_OC_TOKEN`. */
function platformClient(): OpenCompanyClient {
  return new OpenCompanyClient({
    baseUrl: "",
    company: "acme",
    operatorToken: "platform-jwt",
    sessionHeader: null,
  });
}

describe("what the console knows about its own credential", () => {
  it("reports no platform bearer for the ordinary signed-in human", () => {
    // The normal deployment. The session rides in an HttpOnly cookie that
    // nothing in the bundle can read, and `resolve_claims` cannot turn it into
    // platform claims whatever it contains.
    expect(humanClient().carriesPlatformBearer).toBe(false);
  });

  it("reports a platform bearer when one was configured", () => {
    expect(platformClient().carriesPlatformBearer).toBe(true);
  });

  it("treats an empty token as no token", () => {
    // `?token=` with nothing after it resolves to `""`, which would be sent as
    // `Bearer ` and refused. A truthiness check keeps the button decision and
    // the header decision reading the same value the same way.
    const blank = new OpenCompanyClient({
      baseUrl: "",
      company: "acme",
      operatorToken: "",
      sessionHeader: null,
    });
    expect(blank.carriesPlatformBearer).toBe(false);
  });
});

describe("a console without a platform bearer", () => {
  it("never offers suspend or archive, in any lifecycle", () => {
    // The regression test proper. Not "archive is disabled" — disabled would
    // still assert the action belongs here.
    for (const state of ["running", "paused", "suspended"]) {
      const { actions } = lifecycleAffordances(state, false);
      expect(actions).not.toContain("suspend");
      expect(actions).not.toContain("archive");
    }
  });

  it("still offers pause on a running company", () => {
    // The point of the fix is not to empty the card. `pause` is `CompanyAuth`
    // and is the operator's real, reversible stop.
    expect(lifecycleAffordances("running", false).actions).toEqual(["pause"]);
  });

  it("still offers resume on a paused company", () => {
    expect(lifecycleAffordances("paused", false).actions).toEqual(["resume"]);
  });

  it("withholds resume on a platform-suspended company, and says why", () => {
    // `resume` is a `CompanyAuth` route, so the button *is* reachable — and the
    // handler refuses a non-platform caller specifically when the lifecycle is
    // `suspended`. Rendering it there is the same dishonesty as Archive.
    const { actions, explainPlatformSuspended } = lifecycleAffordances("suspended", false);
    expect(actions).toEqual([]);
    expect(explainPlatformSuspended).toBe(true);
  });

  it("explains the withheld controls rather than dropping them silently", () => {
    // A missing button with no explanation sends an operator who read the docs
    // hunting for a control that was never theirs.
    expect(lifecycleAffordances("running", false).explainPlatformOnly).toBe(true);
  });
});

describe("a console holding a platform bearer", () => {
  it("offers suspend and archive", () => {
    const { actions, explainPlatformOnly } = lifecycleAffordances("running", true);
    expect(actions).toEqual(["pause", "suspend", "archive"]);
    expect(explainPlatformOnly).toBe(false);
  });

  it("offers resume on a suspended company, because it can lift one", () => {
    expect(lifecycleAffordances("suspended", true).actions).toContain("resume");
    expect(lifecycleAffordances("suspended", true).explainPlatformSuspended).toBe(false);
  });
});

describe("an archived company", () => {
  it("offers nothing to anyone, and explains nothing away", () => {
    // Terminal: the host removes it from the registry. Even a platform bearer
    // has no transition left, and the banners would be noise next to the
    // "This company is archived." line the card already shows.
    for (const platform of [false, true]) {
      const shown = lifecycleAffordances("archived", platform);
      expect(shown.actions).toEqual([]);
      expect(shown.archived).toBe(true);
      expect(shown.explainPlatformOnly).toBe(false);
      expect(shown.explainPlatformSuspended).toBe(false);
    }
  });
});

describe("a lifecycle string the console does not know", () => {
  it("offers no transition rather than guessing one", () => {
    // A host newer than this bundle can report a state that is not in the set.
    // Showing nothing is recoverable; showing Archive is not.
    expect(lifecycleAffordances("provisioning", true).actions).toEqual(["suspend", "archive"]);
    expect(lifecycleAffordances("provisioning", false).actions).toEqual([]);
  });
});
