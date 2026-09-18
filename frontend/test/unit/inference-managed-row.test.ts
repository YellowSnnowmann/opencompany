// The legacy managed row must not claim availability it does not have.
//
// It used to carry a permanent `Always on` badge, ported from a design where the
// same company runs the managed backend — there it is true. Here the managed
// tier needs a credential and can resolve to nothing, and a row saying "always
// on" while agents cannot think is exactly the dishonesty the five-state
// cognition model exists to prevent.
//
// Keys rework (issue #2306, slice 2a): TinyHumans is an ordinary catalogue row
// now, so most of what this file tested (`offersManaged`, the special add-list
// entry) is gone with it — see `inference-catalogue.test.ts` and
// `inference-connect.test.ts` for the ordinary-row behaviour that replaces it.
// What remains here is the transitional legacy row (`showsLegacyManagedRow`)
// and its copy, both @deprecated and both moved to `managed-copy.ts`.

import { describe, expect, it } from "vitest";

import {
  NO_CREDENTIAL_RESOLVES,
  legacyManagedShowsSwitch,
  legacyManagedSubline,
  managedRow,
  showsLegacyManagedRow,
} from "@/inference/ProviderList";
import { MANAGED_NOT_SET_UP, MANAGED_SWITCHED_OFF, managedFallbackNote } from "@/inference/managed-copy";
import type { ManagedState } from "@/api/inference";

const managed = (source: ManagedState["source"]): ManagedState => ({
  source,
  configured: source !== "none",
  baseUrl: "https://api.tinyhumans.ai/agent-integrations/openrouter",
});

describe("what the legacy managed row says", () => {
  it("never claims permanent availability", () => {
    // The badge that used to say "Always on" is gone — the row carries a real
    // toggle now, like any other provider — and no sub-line may smuggle the
    // same claim back in as prose.
    for (const source of ["provider_key", "company_account", "instance", "none"] as const) {
      expect(managedRow(source).toLowerCase(), source).not.toContain("always");
    }
  });

  it("keeps the two paying states apart", () => {
    // One bills the company's own account, the other bills whoever runs the
    // server, and that is the decision an operator is on this page to make.
    expect(managedRow("company_account")).not.toBe(managedRow("instance"));
    expect(managedRow("company_account")).toContain("company");
    expect(managedRow("instance")).toContain("server");
  });

  it("says outright that agents cannot think when nothing resolves", () => {
    expect(managedRow("none")).toContain("cannot think");
  });

  it("keeps that sentence reachable from the states that need it", () => {
    expect(managedRow("none")).toBe(NO_CREDENTIAL_RESOLVES);
  });
});

describe("showsLegacyManagedRow (keys rework, decision Q3: exactly one TinyHumans row)", () => {
  it("shows once the chain resolves and no row exists yet", () => {
    expect(showsLegacyManagedRow(managed("company_account"))).toBe(true);
    expect(showsLegacyManagedRow(managed("instance"))).toBe(true);
  });

  it("hides once a tinyhumans row exists — the host says so with legacyRow: false", () => {
    expect(showsLegacyManagedRow({ ...managed("provider_key"), legacyRow: false })).toBe(false);
  });

  it("reads legacyRow absent as true, matching every host before this rework", () => {
    expect(showsLegacyManagedRow(managed("provider_key"))).toBe(true);
  });

  it("never shows when nothing resolves", () => {
    expect(showsLegacyManagedRow(managed("none"))).toBe(false);
  });

  it("never shows when the host did not say anything at all", () => {
    expect(showsLegacyManagedRow(undefined)).toBe(false);
  });
});

describe("legacyManagedSubline / legacyManagedShowsSwitch (round-2 review, P1-3, decision X5)", () => {
  it("shows the X5 sub-line, never the connected-sounding one, once the host says needsModel", () => {
    expect(legacyManagedSubline({ ...managed("company_account"), needsModel: true })).toBe(
      "Key added — choose a model",
    );
    expect(legacyManagedSubline({ ...managed("instance"), needsModel: true })).toBe(
      "Key added — choose a model",
    );
  });

  it("falls back to the ordinary managedRow text once a model is chosen", () => {
    expect(legacyManagedSubline({ ...managed("company_account"), needsModel: false })).toBe(
      managedRow("company_account"),
    );
    expect(legacyManagedSubline(managed("instance"))).toBe(managedRow("instance"));
  });

  it("hides the live switch exactly when needsModel is true", () => {
    expect(legacyManagedShowsSwitch({ needsModel: true })).toBe(false);
    expect(legacyManagedShowsSwitch({ needsModel: false })).toBe(true);
    expect(legacyManagedShowsSwitch({})).toBe(true);
  });
});

describe("the line that says managed is not set up", () => {
  it("carries no navigation on the page that holds the action", () => {
    // The Providers page has the button at the top of it. Telling an operator
    // to go to the page they are looking at is a sentence that has stopped
    // reading its own surroundings.
    expect(MANAGED_NOT_SET_UP).not.toContain("tab");
    expect(MANAGED_NOT_SET_UP).toContain("not a fallback");
  });
});

describe("the fallback line under the Connected card", () => {
  it("says nothing when the row above has already said it all", () => {
    // Set up and on: the row names the step that answers and who it bills.
    // Repeating that here is duplication, and the line it replaces claimed
    // "always available", which was never true here.
    expect(managedFallbackNote({ configured: true, enabled: true })).toBeNull();
    expect(managedFallbackNote({ configured: true })).toBeNull();
  });

  it("covers the one state the row's badge cannot express", () => {
    // Set up, resolving, and still not a fallback — because it is switched out
    // of routing. The credential is untouched, and the line says so.
    expect(managedFallbackNote({ configured: true, enabled: false })).toBe(MANAGED_SWITCHED_OFF);
    expect(MANAGED_SWITCHED_OFF).toContain("credential is untouched");
  });

  it("says so when nothing in the chain answers", () => {
    expect(managedFallbackNote({ configured: false })).toBe(MANAGED_NOT_SET_UP);
  });

  it("says nothing at all when the host did not say", () => {
    // "Unknown" is not "unavailable", and it is not "on" either.
    expect(managedFallbackNote(undefined)).toBeNull();
  });
});
