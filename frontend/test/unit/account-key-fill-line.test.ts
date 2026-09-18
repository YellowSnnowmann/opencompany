// Pure coverage for the account-key dialog's one conditional line (Q9) and
// its "which slots would this save fill" decision — keys rework, issue
// #2306, slice 4b. No React, no host: `docs/key-reworks/phase-4b-account-dialog.md`
// §3.3, with the wording override (decision "X5", 2026-09-15) that the LLM
// branch never claims the key "connects" or "is connected" for LLM — the
// fan-out (4a) never creates a `tinyhumans` row without a model, so a key
// with no row behind it has done nothing for LLM yet.
//
// Round-3b review, P3-4: the LLM clause also hides outright, rather than just
// softening its wording, once `inferenceHasModel` says the row saving would
// fill already has a model — there is then no "next" step left to promise.

import { describe, expect, it } from "vitest";

import type { CompanyCredentialStatus } from "@/api/credential";
import {
  COMPOSIO_PAGE_HREF,
  LLM_PAGE_HREF,
  SEARCH_PAGE_HREF,
  accountFillLine,
  accountFills,
  modelStepTitle,
} from "@/views/connections/account-fill";

function status(overrides: Partial<CompanyCredentialStatus> = {}): CompanyCredentialStatus {
  return {
    configured: true,
    source: "company",
    notice: "notice",
    hubLink: false,
    searchHasOwnKey: false,
    ...overrides,
  };
}

describe("accountFills", () => {
  it("reads false HasOwnKey as a slot this save would fill", () => {
    expect(
      accountFills(status({ inferenceHasOwnKey: false, composioHasOwnKey: false })),
    ).toEqual({ llm: true, composio: true, search: true, llmHasModel: false });
  });

  it("reads true HasOwnKey as a slot this save would leave alone", () => {
    expect(
      accountFills(
        status({
          inferenceHasOwnKey: true,
          composioHasOwnKey: true,
          searchHasOwnKey: true,
        }),
      ),
    ).toEqual({ llm: false, composio: false, search: false, llmHasModel: false });
  });

  it("mixes the two independently", () => {
    expect(accountFills(status({ inferenceHasOwnKey: true, composioHasOwnKey: false }))).toEqual({
      llm: false,
      composio: true,
      search: true,
      llmHasModel: false,
    });
  });

  it("is null when any field is missing (an older host)", () => {
    expect(accountFills(status({ inferenceHasOwnKey: true, composioHasOwnKey: undefined }))).toBe(
      null,
    );
    expect(accountFills(status({ inferenceHasOwnKey: undefined, composioHasOwnKey: true }))).toBe(
      null,
    );
    expect(
      accountFills(
        status({
          inferenceHasOwnKey: true,
          composioHasOwnKey: true,
          searchHasOwnKey: undefined,
        }),
      ),
    ).toBe(null);
    expect(accountFills(null)).toBe(null);
  });

  // Round-3b review, P3-4: absent on a host that has not landed the field —
  // reads as "we don't know", the same direction `inferenceHasOwnKey` itself
  // falls back to, so an older host's line keeps today's wording rather than
  // guessing the row has no model.
  it("reads inferenceHasModel as false when the host does not say", () => {
    expect(
      accountFills(status({ inferenceHasOwnKey: false, composioHasOwnKey: false })),
    ).toEqual(
      expect.objectContaining({ llmHasModel: false }),
    );
  });

  it("reads a true inferenceHasModel through", () => {
    expect(
      accountFills(
        status({ inferenceHasOwnKey: false, composioHasOwnKey: false, inferenceHasModel: true }),
      ),
    ).toEqual({ llm: true, composio: true, search: true, llmHasModel: true });
  });
});

describe("accountFillLine", () => {
  it("names both slots without claiming LLM is connected", () => {
    const line = accountFillLine({ llm: true, composio: true, search: false, llmHasModel: false });
    expect(line).toBe(
      "Saving also adds this key to TinyHumans on the LLM page, with the model you choose next — and connects it for Composio.",
    );
    expect(line?.toLowerCase()).not.toContain("connects tinyhumans for llm");
    expect(line?.toLowerCase()).not.toContain("llm is connected");
  });

  it("names only the LLM slot, and says a model is still needed", () => {
    const line = accountFillLine({ llm: true, composio: false, search: false, llmHasModel: false });
    expect(line).toBe(
      "Saving also adds this key to TinyHumans on the LLM page, with the model you choose next.",
    );
    expect(line?.toLowerCase()).not.toContain("connects");
  });

  it("names only the Composio slot — unaffected by the LLM wording override", () => {
    expect(accountFillLine({ llm: false, composio: true, search: false, llmHasModel: false })).toBe(
      "Saving also connects TinyHumans for Composio.",
    );
  });

  it("is null when saving would fill neither slot", () => {
    expect(
      accountFillLine({ llm: false, composio: false, search: false, llmHasModel: false }),
    ).toBe(null);
  });

  it("is null when the host did not say (accountFills returned null)", () => {
    expect(accountFillLine(null)).toBe(null);
  });

  // Round-3b review, P3-4's actual fix: a row that already has a model gets
  // no LLM clause at all, not just softer wording — there is no "next" step
  // left to promise.
  it("drops the LLM clause entirely once the row already has a model", () => {
    expect(
      accountFillLine({ llm: true, composio: false, search: false, llmHasModel: true }),
    ).toBe(null);
  });

  it("keeps only the Composio clause when LLM already has a model but Composio would still be filled", () => {
    expect(accountFillLine({ llm: true, composio: true, search: false, llmHasModel: true })).toBe(
      "Saving also connects TinyHumans for Composio.",
    );
  });

  it("names the company-controlled managed Search credential", () => {
    expect(accountFillLine({ llm: false, composio: false, search: true, llmHasModel: false })).toBe(
      "Saving also uses this key as the company's managed Search credential.",
    );
  });

  it("names all three slots when a save would fill all three", () => {
    const line = accountFillLine({ llm: true, composio: true, search: true, llmHasModel: false });
    expect(line).toBe(
      "Saving also adds this key to TinyHumans on the LLM page, with the model you choose next — connects it for Composio, and uses it as this company's managed Search credential.",
    );
    expect(line?.toLowerCase()).not.toContain("connects tinyhumans for llm");
  });

  it("names LLM and Search when only Composio holds its own key", () => {
    expect(accountFillLine({ llm: true, composio: false, search: true, llmHasModel: false })).toBe(
      "Saving also adds this key to TinyHumans on the LLM page, with the model you choose next — and uses it as this company's managed Search credential.",
    );
  });

  it("names Composio and Search when only LLM holds its own key", () => {
    expect(accountFillLine({ llm: false, composio: true, search: true, llmHasModel: false })).toBe(
      "Saving also connects TinyHumans for Composio and uses this key as the company's managed Search credential.",
    );
  });

  it("drops the LLM clause from the three-slot line once the row already has a model", () => {
    expect(accountFillLine({ llm: true, composio: true, search: true, llmHasModel: true })).toBe(
      "Saving also connects TinyHumans for Composio and uses this key as the company's managed Search credential.",
    );
  });
});

describe("modelStepTitle", () => {
  it("names the default when this model would also become one", () => {
    expect(modelStepTitle(true)).toBe("Choose the model new work uses");
  });

  it("names TinyHumans plainly otherwise", () => {
    expect(modelStepTitle(false)).toBe("Choose the model TinyHumans uses");
  });
});

describe("the dialog's three links", () => {
  it("point at the LLM, Composio, and Search connection pages", () => {
    expect(LLM_PAGE_HREF).toBe("#/connections/inference");
    expect(COMPOSIO_PAGE_HREF).toBe("#/connections/composio");
    expect(SEARCH_PAGE_HREF).toBe("#/connections/search");
  });
});
