// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import { InvoicingView } from "@/views/finance/InvoicingView";

/**
 * A provider's credentials must not survive a company switch.
 *
 * This suite is normally for pure functions. The exception is earned the same
 * way it was when this file tested `SettingsSection`: the behaviour is not a
 * function anybody can call, it is *composition* — a parent giving a
 * credential-holding page a `key` that changes with the company. A unit test of
 * either alone would pass with the key removed, which is exactly the regression
 * that happened once: clearing fields by hand covered the ones somebody
 * remembered and left the API key, webhook secret and both PayPal halves
 * behind, so an operator who typed a key, switched company, and pressed Save
 * wrote that credential into the wrong company's secret store.
 *
 * The forms moved from Settings → Billing to Finance → Invoicing / Wallet
 * (docs/spec/runtime/finance-console.md), and `FinanceSection` carries the same
 * `key={company ?? "self"}`. What is driven here is the page itself, keyed the
 * way the section keys it — `FinanceSection` lazily imports the Recharts-backed
 * Overview, which this runner has no reason to stand up.
 */

/** A client that answers the Chargebee status read and fails the data read. */
function clientFor(company: string): OpenCompanyClient {
  return {
    scopeFor: () => `/api/v1/companies/${company}`,
    get: async (path: string) => {
      if (path.endsWith("/billing/chargebee"))
        return {
          apiKeyConfigured: false,
          site: null,
          webhookConfigured: false,
          webhookUrl: null,
          granted: true,
          inBuild: true,
        };
      // Unconfigured, so there are no invoices to list. The page must still
      // render the form — that is the point of the state being a 409.
      throw Object.assign(new Error("not configured"), { code: "not_configured" });
    },
  } as unknown as OpenCompanyClient;
}

let container: HTMLDivElement;
let root: Root;

/** Renders the page exactly as `FinanceSection` does: keyed by company. */
async function showInvoicing(company: string) {
  await act(async () => {
    root.render(
      createElement(InvoicingView, {
        key: company,
        client: clientFor(company),
        company,
      }),
    );
  });
}

function apiKeyBox(): HTMLInputElement {
  const box = container.querySelector<HTMLInputElement>('[data-testid="billing-api-key"]');
  if (!box) throw new Error("the API key input is not on the page");
  return box;
}

/** Types into the field the way an operator does, so React's state updates. */
async function type(box: HTMLInputElement, value: string) {
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(
      window.HTMLInputElement.prototype,
      "value",
    )?.set;
    setter?.call(box, value);
    box.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

beforeEach(() => {
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("invoicing credentials across a company switch", () => {
  it("opens the credential form on an unconfigured company", () => {
    // The precondition every assertion below rests on, and a behaviour in its
    // own right: an unconfigured provider is the one state this form fixes, so
    // the panel arrives expanded rather than making the operator find it.
    return showInvoicing("acme").then(() => {
      expect(apiKeyBox()).not.toBeNull();
    });
  });

  it("drops a typed-but-unsaved credential when the company changes", async () => {
    await showInvoicing("acme");
    await type(apiKeyBox(), "cb_live_for_acme");
    expect(apiKeyBox().value).toBe("cb_live_for_acme");

    // The operator switches company without saving.
    await showInvoicing("globex");

    // The key must NOT still be sitting in the box, where the next Save would
    // send it to globex.
    expect(apiKeyBox().value).toBe("");
  });

  it("drops it again on a switch back, not just the first time", async () => {
    // A `key` that only changed once — or a clear that ran on mount only —
    // would pass the test above and fail this one.
    await showInvoicing("acme");
    await type(apiKeyBox(), "cb_live_for_acme");
    await showInvoicing("globex");
    await type(apiKeyBox(), "cb_live_for_globex");
    expect(apiKeyBox().value).toBe("cb_live_for_globex");

    await showInvoicing("acme");
    expect(apiKeyBox().value).toBe("");
  });
});
