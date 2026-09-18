// `rowSubline` — the one fact a connected-list row shows, and specifically
// that no category is ever shown as "set" without a chosen model (keys
// rework, issue #2306, decision X5, round-2 review P2-4: a local runtime and
// a keyless cloud row used to return before the model check ran at all).

import { describe, expect, it } from "vitest";

import { rowSubline } from "@/inference/ProviderList";
import type { Provider } from "@/inference/types";

function provider(over: Partial<Provider> = {}): Provider {
  return {
    id: "prv_acme",
    slug: "acme",
    label: "Acme",
    kind: "openai_compatible",
    baseUrl: "https://acme.example/v1",
    models: {},
    enabled: true,
    keyConfigured: true,
    ...over,
  };
}

describe("rowSubline — entry zero", () => {
  it("always names the original config, model or not", () => {
    expect(rowSubline(provider({ origin: "entryZero" }))).toBe("This company's original configuration");
    expect(rowSubline(provider({ origin: "entryZero", model: "acme/test-model" }))).toBe(
      "This company's original configuration",
    );
  });
});

describe("rowSubline — a provider is never shown as set without a model (P2-4)", () => {
  it("a keyed cloud row with no model says so, never '•••• configured'", () => {
    expect(rowSubline(provider({ keyConfigured: true, model: null }))).toBe("Key added — choose a model");
  });

  it("a local runtime with no model says so, never 'Runs on this machine' alone", () => {
    expect(rowSubline(provider({ kind: "ollama", keyConfigured: false, model: null }))).toBe(
      "Runs on this machine — choose a model",
    );
  });

  it("a CLI login with no model says so, never the borrowed-credential line alone", () => {
    expect(rowSubline(provider({ kind: "claude-code", keyConfigured: false, model: null }))).toBe(
      "Uses a login another CLI already holds — choose a model",
    );
  });

  it("a keyless cloud row with no model says so, never its bare endpoint host", () => {
    expect(rowSubline(provider({ keyConfigured: false, model: null }))).toBe("Choose a model");
  });

  it("an ambiguous row counts as having 'a model' for this line — its own chip says the rest", () => {
    expect(rowSubline(provider({ model: null, modelAmbiguous: true }))).toBe("•••• configured");
  });
});

describe("rowSubline — a chosen model restores the ordinary per-category line", () => {
  it("cloud, keyed", () => {
    expect(rowSubline(provider({ model: "acme/test-model" }))).toBe("•••• configured");
  });

  it("local", () => {
    expect(rowSubline(provider({ kind: "ollama", keyConfigured: false, model: "llama3" }))).toBe(
      "Runs on this machine",
    );
  });

  it("cli", () => {
    expect(rowSubline(provider({ kind: "claude-code", keyConfigured: false, model: "gpt-5" }))).toBe(
      "Uses a login another CLI already holds",
    );
  });

  it("keyless cloud shows its endpoint host, or 'no key' when that cannot be read", () => {
    expect(
      rowSubline(provider({ keyConfigured: false, model: "acme/test-model", baseUrl: "https://acme.example/v1" })),
    ).toBe("acme.example");
    expect(rowSubline(provider({ keyConfigured: false, model: "acme/test-model", baseUrl: "" }))).toBe("no key");
  });
});
