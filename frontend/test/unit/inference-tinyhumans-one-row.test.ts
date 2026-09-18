// @vitest-environment jsdom

// TinyHumans is exactly one row, however the company got there (keys rework,
// issue #2306, slice 2a, decision Q3). Round-2 review, P2-6: the pure
// predicate behind this (`showsLegacyManagedRow`) already had coverage, but
// nothing rendered the actual list and counted the rows it produced — which
// is the thing a caller wiring the two together wrong would break.

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { ManagedState } from "@/api/inference";
import { ProviderList } from "@/inference/ProviderList";
import type { Provider } from "@/inference/types";

function tinyhumansRow(over: Partial<Provider> = {}): Provider {
  return {
    id: "prv_tinyhumans",
    slug: "tinyhumans",
    label: "TinyHumans",
    kind: "tinyhumans",
    baseUrl: "https://api.tinyhumans.ai/agent-integrations/openrouter",
    models: {},
    enabled: true,
    keyConfigured: true,
    model: "acme/test-model",
    ...over,
  };
}

function managed(over: Partial<ManagedState> = {}): ManagedState {
  return {
    source: "provider_key",
    configured: true,
    baseUrl: "https://api.tinyhumans.ai/agent-integrations/openrouter",
    ...over,
  };
}

/** Every handler `ProviderList` requires — none of these tests click anything, so all are no-ops. */
const noop = () => {};
const handlers = {
  canManage: true,
  onToggle: noop,
  onEdit: noop,
  onChooseModel: noop,
  onTest: noop,
  onRemove: noop,
  onRemoveKey: noop,
  onReplaceKey: noop,
  onMakeDefault: noop,
  onAdd: noop,
  onManagedToggle: noop,
  onManagedTest: noop,
  onManagedReplaceKey: noop,
  testState: () => ({ kind: "idle" as const }),
};

let container: HTMLDivElement;
let root: Root;

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

function render(providers: Provider[], managedState: ManagedState | undefined) {
  act(() => {
    root.render(createElement(ProviderList, { providers, managed: managedState, ...handlers }));
  });
}

function rowCount(testid: string): number {
  return container.querySelectorAll(`[data-testid="${testid}"]`).length;
}

describe("TinyHumans is exactly one row (decision Q3)", () => {
  it("shows the legacy row alone when no tinyhumans row exists yet", () => {
    render([], managed());
    expect(rowCount("inference-provider-managed")).toBe(1);
    expect(rowCount("inference-provider-tinyhumans")).toBe(0);
  });

  it("shows the indexed row alone once one exists, with the legacy row gone", () => {
    render([tinyhumansRow()], managed({ legacyRow: false }));
    expect(rowCount("inference-provider-managed")).toBe(0);
    expect(rowCount("inference-provider-tinyhumans")).toBe(1);
  });

  it("still shows exactly one row alongside an unrelated provider", () => {
    render(
      [
        { id: "prv_acme", slug: "acme", label: "Acme", kind: "openai_compatible", baseUrl: "https://acme.example/v1", models: {}, enabled: true, keyConfigured: true, model: "acme/test-model" },
        tinyhumansRow(),
      ],
      managed({ legacyRow: false }),
    );
    expect(rowCount("inference-provider-managed")).toBe(0);
    expect(rowCount("inference-provider-tinyhumans")).toBe(1);
    expect(rowCount("inference-provider-acme")).toBe(1);
  });

  it("shows no TinyHumans row at all when nothing resolves and none is indexed", () => {
    render([], managed({ source: "none", configured: false }));
    expect(rowCount("inference-provider-managed")).toBe(0);
    expect(rowCount("inference-provider-tinyhumans")).toBe(0);
  });

  it("documents the known graceful-degradation case: an older host that sends an indexed row but omits legacyRow still shows both, until that host ships legacyRow: false", () => {
    // `legacyRow` absent reads as `configured` (`showsLegacyManagedRow`'s own
    // contract) — an older host that has not shipped the flag yet, not a
    // console bug. Pinned here so a future change to that default is a
    // deliberate one.
    render([tinyhumansRow()], managed({ legacyRow: undefined }));
    expect(rowCount("inference-provider-managed")).toBe(1);
    expect(rowCount("inference-provider-tinyhumans")).toBe(1);
  });
});
