// @vitest-environment jsdom

// DefaultModelDialog's confirm step (keys rework, issue #2306, decision X4).
// Round-2 review, KR-L2-04: the description dropped a word — "moves to this
// the moment you confirm" instead of "moves to this default the moment you
// confirm" — found live on build b2f1848d5.

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import { DefaultModelDialog } from "@/inference/DefaultModelDialog";
import type { Provider } from "@/inference/types";

let container: HTMLDivElement;
let root: Root;

function provider(over: Partial<Provider> = {}): Provider {
  return {
    id: "prv_beta",
    slug: "beta",
    label: "Beta",
    kind: "openai_compatible",
    baseUrl: "https://beta.example/v1",
    models: {},
    enabled: true,
    keyConfigured: true,
    model: "beta/test-model",
    ...over,
  };
}

function stubClient(): OpenCompanyClient {
  return {
    scopeFor: () => "/api/v1/companies/acme",
    // A rejected catalog read settles ModelField into free text with no
    // catalog, quickly and deterministically — this test is about the
    // confirm step's own copy, not the model field.
    get: async () => {
      throw new Error("no catalog in this test");
    },
  } as unknown as OpenCompanyClient;
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

// Base UI's Dialog portals its content to `document.body`, not into
// `container` — the same reason `inference-add-dialog-close.test.ts` queries
// globally rather than scoping to the render root.
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

describe("DefaultModelDialog's replace-confirm description (KR-L2-04)", () => {
  it("reads 'moves to this default', not the word-dropped original", async () => {
    await act(async () => {
      root.render(
        createElement(DefaultModelDialog, {
          client: stubClient(),
          company: "acme",
          provider: provider(),
          providers: [provider(), { slug: "acme", label: "Acme" }],
          // A DIFFERENT, full default already stored — this is exactly what
          // triggers the confirm step (`replacesDifferentDefault`).
          defaultChoice: { provider: "acme", model: "acme/test-model" },
          busy: false,
          error: null,
          onCancel: () => {},
          onSubmit: () => {},
        }),
      );
    });
    await act(async () => {});

    // The model field seeds from the row's own model ("beta/test-model"),
    // so the submit button is ready without typing anything.
    await click(testId("inference-default-model-submit"));

    expect(testId("inference-default-model-confirm")).not.toBeNull();
    expect(document.body.textContent).toContain("moves to this default the moment you confirm");
    expect(document.body.textContent).not.toContain("moves to this the moment");
  });
});
