// @vitest-environment jsdom

// Live lane 1, KR-L1-02: the outer "Add a provider" picker must close when a
// connect flow finishes — success or cancel — not linger open underneath the
// connect dialog and reappear (marking the rest of the page `aria-hidden`
// again) the instant that dialog unmounts. `ProvidersTab`'s `adding` boolean
// used to be set `true` in four places and reset only by the picker's own
// `onOpenChange`; choosing an option opened the connect dialog without ever
// closing the picker underneath it.

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import type { InferenceStatus, ProviderMutation } from "@/api/inference";
import { InferenceView } from "@/views/InferenceView";

let container: HTMLDivElement;
let root: Root;

function status(providers: InferenceStatus["providers"] = []): InferenceStatus {
  return {
    provider: "managed",
    slug: "managed",
    baseUrl: "https://openrouter.ai/api/v1",
    models: {},
    source: "runtime",
    keyConfigured: false,
    cognition: "echo",
    usageMetering: "none",
    restartRequired: false,
    harnessReachable: true,
    canRebuildInPlace: false,
    providers,
    routes: {},
    managed: { source: "none", configured: false, baseUrl: "" },
  };
}

function stubClient(): OpenCompanyClient {
  let current = status();
  return {
    scopeFor: (company: string | null) =>
      company ? `/api/v1/companies/${company}` : "/api/v1/company",
    get: async (path: string) =>
      path.endsWith("/auth/me")
        ? { user: { id: "u1", email: "admin@acme.test", role: "admin" }, role: "admin" }
        : current,
    post: async (path: string, body: unknown): Promise<unknown> => {
      if (path.endsWith("/inference/probe")) {
        return { ok: false, modelCount: 0, message: "connection refused" };
      }
      if (path.endsWith("/inference/providers")) {
        const draft = body as { kind: string; label?: string; model?: string };
        current = status([
          {
            id: "prv_e2e",
            slug: "e2e-custom",
            label: draft.label ?? draft.kind,
            kind: "openai_compatible",
            baseUrl: "http://127.0.0.1:9/v1",
            models: {},
            model: draft.model ?? null,
            enabled: true,
            keyConfigured: true,
            isDefault: true,
          },
        ]);
        return { status: current, note: "Provider connected." } satisfies ProviderMutation;
      }
      throw new Error(`unexpected POST ${path}`);
    },
  } as unknown as OpenCompanyClient;
}

async function mount(client: OpenCompanyClient) {
  await act(async () => {
    root.render(createElement(InferenceView, { client, company: "acme" }));
  });
  await act(async () => {});
}

function testId(id: string) {
  return document.querySelector(`[data-testid="${id}"]`);
}

async function click(el: Element | null) {
  if (!el) throw new Error("nothing to click");
  await act(async () => {
    (el as HTMLElement).click();
  });
  await act(async () => {});
}

async function typeInto(selector: string, value: string) {
  const el = document.querySelector(selector) as HTMLInputElement | null;
  if (el === null) throw new Error(`nothing to type into at ${selector}`);
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  await act(async () => {
    setter?.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
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

describe("the Add-a-provider picker closes when the connect flow finishes (KR-L1-02)", () => {
  it("closes on a successful custom-provider add, leaving no dialog open at all", async () => {
    await mount(stubClient());

    await click(testId("inference-add-open"));
    expect(testId("inference-add-provider")).not.toBeNull();

    await click(testId("inference-add-custom"));
    // The picker must already be gone the moment the connect dialog opens —
    // this is the state KR-L1-02 found stuck `true` underneath it.
    expect(testId("inference-add-provider")).toBeNull();
    expect(testId("inference-connect-provider")).not.toBeNull();

    await typeInto("#inference-connect-name", "E2E Custom");
    await typeInto("#inference-connect-url", "http://127.0.0.1:9/v1");
    await click(testId("inference-connect-submit")); // step 1 -> probe -> model step

    expect(testId("inference-connect-model-step")).not.toBeNull();
    await typeInto("#inference-connect-model", "acme/test-model");
    await click(testId("inference-connect-submit")); // step 2 -> add

    // Neither dialog remains — not the connect dialog, and not the picker
    // reappearing underneath it.
    expect(testId("inference-connect-provider")).toBeNull();
    expect(testId("inference-add-provider")).toBeNull();
    // The page is interactive again: the header button is reachable and
    // opens a fresh picker rather than something stuck behind an overlay.
    await click(testId("inference-add-open"));
    expect(testId("inference-add-provider")).not.toBeNull();
  });

  it("closes on Cancel too, not just on success", async () => {
    await mount(stubClient());

    await click(testId("inference-add-open"));
    await click(testId("inference-add-custom"));
    expect(testId("inference-add-provider")).toBeNull();
    expect(testId("inference-connect-provider")).not.toBeNull();

    // Found by its accessible text rather than a testid, to stay robust to
    // layout — it is one of several buttons in the dialog's footer.
    const buttons = Array.from(
      document.querySelectorAll('[data-testid="inference-connect-provider"] button'),
    );
    const cancelButton = buttons.find((b) => b.textContent?.trim() === "Cancel");
    expect(cancelButton, "expected a Cancel button on the details step").toBeTruthy();
    await click(cancelButton ?? null);

    expect(testId("inference-connect-provider")).toBeNull();
    expect(testId("inference-add-provider")).toBeNull();
  });
});
