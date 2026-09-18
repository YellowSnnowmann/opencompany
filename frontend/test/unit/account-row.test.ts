import { describe, expect, it } from "vitest";

import type { CompanyBilling, CompanyCredentialStatus } from "@/api/credential";
import {
  accountShape,
  accountSubline,
  balanceLine,
  canRemoveKey,
  headerActions,
  keyVerdict,
  REJECTED_SUBLINE,
  REMOVAL_CONSEQUENCE,
  REMOVAL_AND_THINKING,
} from "@/views/connections/account";

function status(overrides: Partial<CompanyCredentialStatus> = {}): CompanyCredentialStatus {
  return {
    configured: true,
    source: "company",
    notice: "notice",
    hubLink: false,
    ...overrides,
  };
}

/** A billing read that came back with figures. */
function answered(): CompanyBilling {
  return {
    configured: true,
    summary: { balanceUsd: 4, plan: "free", activeSubscription: false },
  };
}

/** A billing read the host classified as a failure. */
function refused(
  reason: CompanyBilling["unavailableReason"],
  unavailable = "TinyHumans refused this company's key.",
): CompanyBilling {
  return reason === undefined
    ? { configured: true, unavailable }
    : { configured: true, unavailable, unavailableReason: reason };
}

describe("keyVerdict earns both of its verdicts and infers neither", () => {
  // The inversion this whole pass is about. "No failure reported" is not
  // success: a host with no hub reports no failure, and so does one too old to
  // classify. Only figures coming back prove the key was presented and taken.
  it("is working only where a summary actually came back", () => {
    expect(keyVerdict(status({ source: "company" }), answered())).toBe("working");
    expect(keyVerdict(status({ source: "company" }), { configured: true })).toBe("unknown");
    expect(keyVerdict(status({ configured: true, source: "company" }), null)).toBe("unknown");
    expect(keyVerdict(status({ source: "company" }), refused("noHub"))).toBe("unknown");
  });

  it("is rejected only where the host said so about this company's own key", () => {
    expect(keyVerdict(status({ source: "company" }), refused("rejected"))).toBe("rejected");
    expect(keyVerdict(status({ source: "company" }), refused("unreachable"))).toBe("unknown");
    expect(keyVerdict(status({ source: "company" }), refused(undefined))).toBe("unknown");
    expect(keyVerdict(null, refused("rejected"))).toBe("unknown");
  });

  // A refusal against the instance's platform identity is not this company's
  // key to replace — its admin does not hold that credential.
  it("never blames this company for a refusal on a borrowed identity", () => {
    for (const source of ["attested", "static"] as const) {
      expect(keyVerdict(status({ configured: false, source }), refused("rejected"))).toBe(
        "unknown",
      );
    }
  });
});

describe("accountShape keeps an unreadable store apart from an empty one", () => {
  it("is rejected when the hub refused this company's own key", () => {
    expect(accountShape("ready", status({ source: "company" }), refused("rejected"))).toBe(
      "rejected",
    );
  });

  // The most valuable case here. A hub outage must never read as "your key is
  // bad": one says wait, the other says revoke and re-mint, and the row is the
  // only thing telling an operator which.
  it("stays connected when the hub merely could not be reached", () => {
    expect(accountShape("ready", status({ source: "company" }), refused("unreachable"))).toBe(
      "connected",
    );
    expect(accountShape("ready", status({ source: "company" }), refused("noHub"))).toBe(
      "connected",
    );
    expect(accountShape("ready", status({ source: "company" }), refused("unknown"))).toBe(
      "connected",
    );
    expect(accountShape("ready", status({ source: "company" }), refused(undefined))).toBe(
      "connected",
    );
  });

  it("never calls a borrowed identity's refusal this company's problem", () => {
    for (const source of ["attested", "static"] as const) {
      expect(
        accountShape("ready", status({ configured: false, source }), refused("rejected")),
      ).toBe("connected");
    }
  });

  // The load guards outrank every verdict: a page that does not know whose
  // account this is cannot know whether that account works.
  it("is unknown while the host could not answer, whatever billing said", () => {
    expect(accountShape("error", status({ source: "company" }), refused("rejected"))).toBe(
      "unknown",
    );
    expect(accountShape("loading", status({ source: "company" }), refused("rejected"))).toBe(
      "unknown",
    );
    expect(accountShape("ready", null, refused("rejected"))).toBe("unknown");
  });


  it("is connected whenever anything resolves", () => {
    expect(accountShape("ready", status({ source: "company" }), null)).toBe("connected");
    expect(accountShape("ready", status({ configured: false, source: "attested" }), null)).toBe(
      "connected",
    );
    expect(accountShape("ready", status({ configured: false, source: "static" }), null)).toBe(
      "connected",
    );
  });

  it("is empty only when the chain resolves to nothing", () => {
    expect(accountShape("ready", status({ configured: false, source: "none" }), null)).toBe("empty");
  });

  // The trap the whole page is built around. `company_key::resolve` propagates
  // a store read error rather than falling through, because a connection made
  // under a silently-borrowed identity belongs to the wrong account invisibly
  // and permanently. A console that folded that into "empty" would throw the
  // distinction away at the last step and send an admin to set a key they may
  // already have set.
  it("is unknown — never empty — when the host could not answer", () => {
    expect(accountShape("error", null, null)).toBe("unknown");
    // Even holding a status from a previous good read: the load says the
    // current answer is not known, and that outranks a stale one.
    expect(accountShape("error", status({ source: "company" }), null)).toBe("unknown");
  });
});

describe("accountSubline says which tier actually answers", () => {
  it("names the company's own account", () => {
    expect(accountSubline("ready", status({ source: "company" }), null)).toContain(
      "this company's own TinyHumans account",
    );
  });

  // The hosted case. `configured` is false here and a row built on it would
  // read "not configured" while the server's identity is the one answering.
  it("names the server's account for both fallback identities", () => {
    for (const source of ["attested", "static"] as const) {
      expect(accountSubline("ready", status({ configured: false, source }), null)).toBe(
        "Acting as the account of whoever runs this server",
      );
    }
  });

  // `source` comes from `company_key::resolve` and reports which TinyHumans
  // identity won — nothing about who pays for thinking, which `inference/config`
  // and `inference/key` decide independently on the LLM page. A company on
  // `source: "company"` can be thinking on its own OpenRouter key; one on the
  // instance's identity can be paying a provider direct. A payer named from
  // this value would send an operator chasing spend to the wrong account.
  it("claims no payer, in any state it can reach", () => {
    for (const source of ["company", "attested", "static", "none"] as const) {
      for (const money of [null, answered(), refused("rejected"), refused("unreachable")]) {
        const line = accountSubline("ready", status({ source }), money).toLowerCase();
        expect(line).not.toContain("billed");
        expect(line).not.toContain("pays");
        expect(line).not.toContain("paying");
      }
    }
    expect(accountSubline("loading", null, null).toLowerCase()).not.toContain("billed");
    expect(accountSubline("error", null, null).toLowerCase()).not.toContain("billed");
  });

  // The defect this page's pass exists to remove: the row said "Acting as this
  // company's own TinyHumans account" directly above the hub's refusal of that
  // same key. `source` says which tier is stored; only the billing read has
  // ever presented the key to anyone.
  it("stops claiming the company is acting as its own account once the key is refused", () => {
    const line = accountSubline("ready", status({ source: "company" }), refused("rejected"));
    expect(line).not.toContain("Acting as this company's own");
    expect(line).toContain("set");
    expect(line).toContain("refus");
    expect(line).toBe(REJECTED_SUBLINE);
  });

  // "is set", not "is connected": the key exists, so the move is to replace it
  // rather than to set a first one — which is what separates this row from the
  // empty state.
  it("keeps the refused row apart from having no key at all", () => {
    expect(REJECTED_SUBLINE).not.toContain("No TinyHumans account");
    expect(accountSubline("ready", status({ source: "company" }), refused("unreachable"))).toContain(
      "Acting as this company's own",
    );
  });

  // Nothing the hub returned reaches the row.
  it("puts none of the hub's own words in the line", () => {
    const body = '{"success":false,"error":"Invalid API key"}';
    const line = accountSubline("ready", status({ source: "company" }), refused("rejected", body));
    expect(line).not.toContain(body);
    expect(line).not.toContain("{");
    expect(line).not.toContain("success");
  });

  // Narrow on purpose. "Agents cannot think" is what this line said first, and
  // it is **false** on a company whose LLM page holds a provider key of its
  // own — `inference/key` resolves without this credential. The sub-line states
  // the absence; the empty state carries the consequence with its exception
  // named.
  it("states the absence without claiming the company has stopped", () => {
    const line = accountSubline("ready", status({ configured: false, source: "none" }), null);
    expect(line).toBe("No TinyHumans account for this company");
  });

  it("never claims agents cannot think, in any state", () => {
    for (const load of ["ready", "error", "loading"] as const) {
      for (const source of ["company", "attested", "static", "none"] as const) {
        const line = accountSubline(load, status({ source }), null).toLowerCase();
        expect(line, `${load}/${source}`).not.toContain("cannot think");
      }
    }
  });

  it("does not claim there is no key when the host could not answer", () => {
    const line = accountSubline("error", null, null);
    expect(line).toContain("not the same as having no key");
    expect(line).not.toContain("No TinyHumans account");
  });

  it("falls back to what the row is when a host names an unknown tier", () => {
    const unknown = status({ source: "something-new" as CompanyCredentialStatus["source"] });
    expect(accountSubline("ready", unknown, null)).toBe(
      "The account this company acts and spends through",
    );
  });
});

describe("canRemoveKey offers Remove only where it would remove something", () => {
  it("is true for the company's own key", () => {
    expect(canRemoveKey(status({ source: "company" }))).toBe(true);
  });

  // The instance's identity is not this row's to take away, and a Remove that
  // clears nothing is the control-that-cannot-act the LLM page's pass deleted
  // a toggle over.
  it("is false for a fallback identity, for nothing, and for an unknown state", () => {
    expect(canRemoveKey(status({ configured: false, source: "attested" }))).toBe(false);
    expect(canRemoveKey(status({ configured: false, source: "static" }))).toBe(false);
    expect(canRemoveKey(status({ configured: false, source: "none" }))).toBe(false);
    expect(canRemoveKey(null)).toBe(false);
  });
});

describe("headerActions offers the page's one way to connect", () => {
  const none = { key: false };

  it("offers nothing to a member", () => {
    expect(headerActions(status({ source: "none", hubLink: true }), false)).toEqual(none);
    expect(headerActions(status({ source: "none", hubLink: false }), false)).toEqual(none);
  });

  // One option whatever the host: the sign-in grant was removed from the page
  // (operator request, 2026-09-14), so `hubLink` no longer changes the offer.
  it("offers Connect to TinyHumans alone, with or without a hub", () => {
    expect(headerActions(status({ configured: false, source: "none", hubLink: true }), true)).toEqual(
      { key: true },
    );
    expect(headerActions(status({ configured: false, source: "none", hubLink: false }), true)).toEqual(
      { key: true },
    );
    expect(
      headerActions(status({ configured: false, source: "static", hubLink: undefined }), true),
    ).toEqual({ key: true });
  });

  // Connected: the row carries Replace and Remove, and a Connect button above
  // it would be a second path to the same write.
  it("offers no connect action once this company has a key of its own", () => {
    expect(headerActions(status({ source: "company", hubLink: true }), true)).toEqual(none);
  });

  // A null status is the read having failed or not yet landed — not "no key".
  // The row beside this button says so in words, and an enabled "Add a key"
  // under that sentence opens a blind overwrite of a write-only credential the
  // console has just admitted it cannot see. The value it would replace cannot
  // be read back from anywhere, which is what makes this worse than an ordinary
  // control-that-cannot-act.
  it("offers nothing at all while the credential state is unknown", () => {
    expect(headerActions(null, true)).toEqual({ key: false });
    expect(headerActions(null, false)).toEqual({ key: false });
  });
});

describe("balanceLine", () => {
  function billing(overrides: Partial<CompanyBilling> = {}): CompanyBilling {
    return { configured: true, ...overrides };
  }

  it("renders no row for a company with no account of its own", () => {
    expect(balanceLine(null)).toBeNull();
    expect(balanceLine(billing({ configured: false }))).toBeNull();
  });

  it("shows the figure and the plan", () => {
    const line = balanceLine(
      billing({ summary: { balanceUsd: 12.5, plan: "pro", activeSubscription: true } }),
    );
    expect(line?.amount).toBe("$12.50");
    expect(line?.detail).toBe("on the pro plan · subscription active");
    expect(line?.low).toBe(false);
  });

  // `!balanceUsd` would hide exactly the figure somebody needs to see, and a
  // zero balance is the state the page exists to make loud.
  it("shows zero rather than hiding it, and marks it low", () => {
    const line = balanceLine(
      billing({ summary: { balanceUsd: 0, plan: "free", activeSubscription: false } }),
    );
    expect(line?.amount).toBe("$0.00");
    expect(line?.low).toBe(true);
  });

  // "We could not ask" and "there is nothing left" look identical on a row and
  // call for opposite actions, so the figure is dropped rather than invented.
  it("does not render an unanswered hub as a zero balance", () => {
    const line = balanceLine(billing({ unavailable: "x", unavailableReason: "unreachable" }));
    expect(line?.amount).toBeNull();
    expect(line?.low).toBe(false);
    expect(line?.detail).toContain("The key is set");
  });

  // The defect that put `{"success":false,"error":"Invalid API key",…}` under
  // somebody's balance. Asserted as substrings rather than against one body, so
  // it fails for ANY interpolation of what the hub returned.
  it("interpolates nothing the hub said", () => {
    const body = '{"success":false,"error":"Invalid API key","statusCode":401}';
    for (const reason of ["rejected", "unreachable", "noHub", "unknown", undefined] as const) {
      const line = balanceLine(billing({ unavailable: body, unavailableReason: reason }));
      expect(line?.detail, `${reason}`).not.toContain(body);
      expect(line?.detail, `${reason}`).not.toContain("{");
      expect(line?.detail, `${reason}`).not.toContain("success");
    }
  });

  // One sentence per reason, each naming a different next move — which is the
  // only thing that makes the classification worth carrying.
  it("says something different for each reason, and stays cautious where none was given", () => {
    const say = (reason: CompanyBilling["unavailableReason"]) =>
      balanceLine(billing({ unavailable: "x", unavailableReason: reason }))?.detail ?? "";

    expect(say("rejected")).toContain("refused it");
    expect(say("rejected")).toContain("replace");
    expect(say("unreachable")).toContain("could not be reached");
    expect(say("unreachable")).not.toContain("refused");
    expect(say("noHub")).toContain("not part of a TinyHumans ecosystem");
    expect(say("unknown")).toContain("could not be read");

    // An older host that classified nothing gets the cautious line, never the
    // one that tells somebody their key is dead.
    expect(say(undefined)).toBe(say("unknown"));
    expect(say(undefined)).not.toContain("refused");
  });
});

describe("the removal confirmation says only what removal does", () => {
  // `set_key("")` clears `tinyhumans/key` and stops. `finish_link` writes the
  // granted value into `inference/key` as well and declares the `managed`
  // provider, so on a company that took the one-click path — the path the
  // button beside this dialog recommends — every agent turn is still billed to
  // this account after the key is removed. The old sentence promised the
  // opposite, on the one screen where being wrong costs a credential.
  it("never claims the billing stops", () => {
    const all = `${REMOVAL_CONSEQUENCE} ${REMOVAL_AND_THINKING}`;
    expect(all).not.toContain("stops being billed");
  });

  // The sentence that has now been wrong in both directions. Before #2266 the
  // grant copied the key into `inference/key`, so removal here left a second
  // copy thinking on the same account; it copies nothing now, and a managed
  // turn resolves through `tinyhumans/key` itself. So the honest sentence is
  // the fallback the resolver actually takes, not a reassurance — and not the
  // opposite overclaim either, since a TinyHumans key on the LLM page outranks
  // this one and goes on working.
  it("describes what the resolver does next, not a reassurance", () => {
    expect(REMOVAL_AND_THINKING).toContain("resolve through this same key");
    expect(REMOVAL_AND_THINKING).toContain("whoever runs this server");
    expect(REMOVAL_AND_THINKING).toContain("nothing at all if this instance carries none");
    expect(REMOVAL_AND_THINKING).toContain("outranks this one and keeps working");
    // The claim the old copy made, which the merged inference rework falsified.
    expect(REMOVAL_AND_THINKING).not.toContain("puts the same key on the LLM page");
  });

  // Both fallbacks, because `GET …/credential` reports the tier that won and
  // that is `company` whichever way it went. Naming one would be a guess
  // dressed as a fact.
  it("offers both fallbacks rather than guessing which applies", () => {
    expect(REMOVAL_CONSEQUENCE).toContain("whoever runs this server");
    expect(REMOVAL_CONSEQUENCE).toContain("no account at all");
  });

  // Conditional at the top, because the outcome is: only a company whose
  // models are set to TinyHumans is standing on this key at all.
  it("states the thinking half as the conditional it is", () => {
    expect(REMOVAL_AND_THINKING).toContain("where this company's models are set to TinyHumans");
  });
});
