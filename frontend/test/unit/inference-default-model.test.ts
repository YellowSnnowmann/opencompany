// The company default's shape and the model-required flow — keys rework,
// issue #2306, slices 2b/2c/2d. Every provider add/edit/set-default now
// carries exactly one validated, non-tier model id, and the default is a full
// `{provider, model}` pair rather than a bare slug.

import { describe, expect, it } from "vitest";

import {
  checkModelId,
  defaultBadgeLabel,
  defaultBrokenCopy,
  defaultModelPrefill,
  defaultNeedsModel,
  isFullDefault,
  MAX_MODEL_ID_CHARS,
  modelAskFromProbe,
  modelIdErrorCopy,
  modelRequiredCopy,
  providerState,
  replacesDifferentDefault,
  rowNeedsModel,
} from "@/inference/connect";
import type { DefaultChoice, Provider } from "@/inference/types";

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

describe("checkModelId", () => {
  it("trims and accepts an ordinary id", () => {
    expect(checkModelId("  acme/test-model \n")).toBeNull();
  });

  it("refuses empty", () => {
    expect(checkModelId("")).toBe("empty");
    expect(checkModelId("   ")).toBe("empty");
  });

  it("refuses a control character", () => {
    expect(checkModelId("testmodel")).toBe("control");
  });

  it("refuses inner whitespace", () => {
    expect(checkModelId("test model")).toBe("whitespace");
  });

  it("is bounded in code points, not bytes", () => {
    expect(checkModelId("a".repeat(MAX_MODEL_ID_CHARS))).toBeNull();
    expect(checkModelId("a".repeat(MAX_MODEL_ID_CHARS + 1))).toBe("tooLong");
  });

  it("refuses every workload tier name — never presented or stored as a model (D-no-tier)", () => {
    for (const tier of ["chat-v1", "reasoning-v1", "agentic-v1", "vision-v1"]) {
      expect(checkModelId(tier)).toBe("tier");
      expect(checkModelId(` ${tier} `)).toBe("tier");
    }
  });

  it("accepts every shape a real model id takes", () => {
    for (const id of ["acme/test-model", "acme/test-model:free", "test-model:8b", "test-model", "test.deployment-1"]) {
      expect(checkModelId(id)).toBeNull();
    }
  });
});

describe("modelIdErrorCopy", () => {
  it("gives each error its own sentence", () => {
    expect(modelIdErrorCopy("empty")).toBe("Choose a model.");
    expect(modelIdErrorCopy("control")).toContain("control characters");
    expect(modelIdErrorCopy("whitespace")).toContain("spaces");
    expect(modelIdErrorCopy("tooLong")).toContain(String(MAX_MODEL_ID_CHARS));
    expect(modelIdErrorCopy("tier")).toContain("workload name");
  });
});

describe("modelRequiredCopy (decision X9)", () => {
  it("names the provider, verbatim", () => {
    expect(modelRequiredCopy("Anthropic")).toBe("Choose a model for Anthropic before saving.");
  });
});

describe("modelAskFromProbe — the model step always opens, in every state (D-model)", () => {
  const azure = (url: string | null | undefined) => url === "https://x.openai.azure.com/v1";

  it("is empty with no error for a kind that is not probed (a CLI login)", () => {
    expect(modelAskFromProbe(null, null, azure)).toEqual({ models: [], freeTextOnly: false });
  });

  it("opens in free text with the reason, on a failed probe — never skipped", () => {
    const ask = modelAskFromProbe("https://acme.example/v1", { ok: false, message: "connection refused" }, azure);
    expect(ask.models).toEqual([]);
    expect(ask.freeTextOnly).toBe(false);
    expect(ask.error).toContain("connection refused");
  });

  it("opens with an empty list and a reason when there was no probe at all despite a url", () => {
    const ask = modelAskFromProbe("https://acme.example/v1", null, azure);
    expect(ask.models).toEqual([]);
    expect(ask.error).toContain("no answer");
  });

  it("lists exactly what a successful probe published, at the real cap", () => {
    const many = Array.from({ length: 500 }, (_, i) => `acme/model-${i}`);
    const ask = modelAskFromProbe("https://acme.example/v1", { ok: true, models: many }, azure);
    expect(ask.models).toHaveLength(500);
    expect(ask.error).toBeUndefined();
  });

  it("forces free text at an Azure endpoint, even on a successful probe", () => {
    const ask = modelAskFromProbe("https://x.openai.azure.com/v1", { ok: true, models: ["gpt-5"] }, azure);
    expect(ask.freeTextOnly).toBe(true);
  });
});

describe("defaultNeedsModel — a bare-slug default from before this rework (Q1)", () => {
  it("is true only for a provider chosen with no model", () => {
    expect(defaultNeedsModel({ provider: "acme", model: null })).toBe(true);
  });

  it("is false for a full default, or no default at all", () => {
    expect(defaultNeedsModel({ provider: "acme", model: "acme/test-model" })).toBe(false);
    expect(defaultNeedsModel(null)).toBe(false);
    expect(defaultNeedsModel(undefined)).toBe(false);
  });
});

describe("isFullDefault / defaultBadgeLabel", () => {
  it("is full only when the slug matches and a model is chosen", () => {
    const choice: DefaultChoice = { provider: "acme", model: "acme/test-model" };
    expect(isFullDefault(provider({ slug: "acme" }), choice)).toBe(true);
    expect(isFullDefault(provider({ slug: "beta" }), choice)).toBe(false);
    expect(isFullDefault(provider({ slug: "acme" }), { provider: "acme", model: null })).toBe(false);
    expect(isFullDefault(provider({ slug: "acme" }), null)).toBe(false);
  });

  it("shows the model on the badge only for the full default", () => {
    const choice: DefaultChoice = { provider: "acme", model: "acme/test-model" };
    expect(defaultBadgeLabel(provider({ slug: "acme" }), choice)).toBe("Default · acme/test-model");
    expect(defaultBadgeLabel(provider({ slug: "acme" }), null)).toBe("Default");
  });
});

describe("rowNeedsModel", () => {
  it("is true for an ambiguous row", () => {
    expect(rowNeedsModel(provider({ modelAmbiguous: true }), null)).toBe(true);
  });

  it("is true for the row a bare-slug default names", () => {
    expect(rowNeedsModel(provider({ slug: "acme" }), { provider: "acme", model: null })).toBe(true);
  });

  it("is false for an unrelated row, or a full default", () => {
    expect(rowNeedsModel(provider({ slug: "beta" }), { provider: "acme", model: null })).toBe(false);
    expect(rowNeedsModel(provider({ slug: "acme" }), { provider: "acme", model: "x" })).toBe(false);
  });
});

describe("defaultModelPrefill", () => {
  it("prefers the row's own model", () => {
    expect(defaultModelPrefill(provider({ slug: "acme", model: "acme/one" }), { provider: "acme", model: "acme/two" })).toBe(
      "acme/one",
    );
  });

  it("falls back to the stored choice's model when the row has none itself (the bare-slug case)", () => {
    expect(defaultModelPrefill(provider({ slug: "acme", model: null }), { provider: "acme", model: "acme/two" })).toBe(
      "acme/two",
    );
  });

  it("is blank when neither has one", () => {
    expect(defaultModelPrefill(provider({ slug: "acme", model: null }), null)).toBe("");
  });
});

describe("replacesDifferentDefault (round-2 review, P2-1)", () => {
  it("is false with no stored default at all", () => {
    expect(replacesDifferentDefault(null, "acme", "acme/one")).toBe(false);
    expect(replacesDifferentDefault(undefined, "acme", "acme/one")).toBe(false);
  });

  it("is true for a bare-slug default naming a DIFFERENT provider — the gap the old check missed", () => {
    expect(replacesDifferentDefault({ provider: "beta", model: null }, "acme", "acme/one")).toBe(true);
  });

  it("is false for completing THIS row's own bare-slug default", () => {
    expect(replacesDifferentDefault({ provider: "acme", model: null }, "acme", "acme/one")).toBe(false);
  });

  it("is true for a full default naming a different provider or a different model", () => {
    expect(replacesDifferentDefault({ provider: "beta", model: "beta/x" }, "acme", "acme/one")).toBe(true);
    expect(replacesDifferentDefault({ provider: "acme", model: "acme/old" }, "acme", "acme/one")).toBe(true);
  });

  it("is false for re-saving the exact pair already stored", () => {
    expect(replacesDifferentDefault({ provider: "acme", model: "acme/one" }, "acme", "acme/one")).toBe(false);
    expect(replacesDifferentDefault({ provider: "acme", model: "acme/one" }, "acme", "  acme/one  ")).toBe(false);
  });
});

describe("providerState / defaultBrokenCopy (decisions X9, X14)", () => {
  const providers = [
    provider({ slug: "acme", label: "Acme", enabled: true }),
    provider({ slug: "beta", label: "Beta", enabled: false }),
  ];

  it("classifies removed, disabled, keyless, and ok", () => {
    expect(providerState("ghost", providers)).toBe("removed");
    expect(providerState("beta", providers)).toBe("disabled");
    expect(providerState("acme", providers)).toBe("ok");
    expect(
      providerState("acme", [provider({ slug: "acme", enabled: true, keyConfigured: false })]),
    ).toBe("noKey");
  });

  it("says nothing for a keyless-but-enabled default — the host has no default_broken sentence for that; only a turn error naming an agent does", () => {
    const keyless = [provider({ slug: "acme", label: "Acme", enabled: true, keyConfigured: false })];
    expect(defaultBrokenCopy({ provider: "acme", model: "x" }, keyless)).toBeNull();
  });

  it("says nothing when the default is healthy", () => {
    expect(defaultBrokenCopy({ provider: "acme", model: "x" }, providers)).toBeNull();
  });

  it("says nothing for a bare-slug default — that is defaultNeedsModel's banner instead", () => {
    expect(defaultBrokenCopy({ provider: "acme", model: null }, providers)).toBeNull();
  });

  it("says nothing when there is no default at all", () => {
    expect(defaultBrokenCopy(null, providers)).toBeNull();
    expect(defaultBrokenCopy(undefined, providers)).toBeNull();
  });

  it("names a removed provider, verbatim per decision X9", () => {
    expect(defaultBrokenCopy({ provider: "ghost", model: "x" }, providers)).toBe(
      "The company default uses ghost, which is removed. Choose a new default in Connections → API Keys → LLM.",
    );
  });

  it("names a disabled provider by its label", () => {
    expect(defaultBrokenCopy({ provider: "beta", model: "x" }, providers)).toBe(
      "The company default uses Beta, which is turned off. Choose a new default in Connections → API Keys → LLM.",
    );
  });
});
