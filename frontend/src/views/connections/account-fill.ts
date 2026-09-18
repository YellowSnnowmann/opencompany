// What the account-key dialog's one conditional line says (Q9), and where its
// links point. Pure — no React, no host, no browser — so the four cases in
// `accountFillLine`'s table can each carry a unit test without mounting the
// dialog.
//
// Keys rework, issue #2306, slice 4b:
// `docs/key-reworks/phase-4b-account-dialog.md` §3.3, with one override
// (decision "X5"/"the line must never claim LLM is connected", 2026-09-15):
// the fan-out (4a) never creates a `tinyhumans` row without a model, so a key
// with no row is not "set" for LLM — saving only ADDS the key there, and a
// model still needs choosing before it does anything. The line says exactly
// that rather than "connects"/"is connected".
//
// Round-3b review, P3-4: the old wording ("choose a model there to finish")
// showed even when the `tinyhumans` row saving would fill already has a
// model of its own — a save that only rotates an already-working row, where
// there is nothing left to "finish". `inferenceHasModel` (speculative field,
// not yet on every host — see `@/api/credential`) is what tells the two
// cases apart; `llmHasModel` below hides the LLM clause entirely rather than
// promising a step two the save will not actually need.

import type { CompanyCredentialStatus } from "@/api/credential";
import { connectionsHref } from "@/views/connection-pages";

/** Where the conditional links in the dialog's fill line go. */
export const LLM_PAGE_HREF = connectionsHref("inference");
export const COMPOSIO_PAGE_HREF = connectionsHref("composio");
export const SEARCH_PAGE_HREF = connectionsHref("search");

/** Which of the two derived slots a save would actually fill. */
export interface AccountFills {
  llm: boolean;
  composio: boolean;
  search: boolean;
  /**
   * Whether the `tinyhumans` row `llm` would fill already has a model of its
   * own — `false` on a host that has not landed `inferenceHasModel` yet,
   * which keeps today's wording rather than assuming a row has no model when
   * the host simply did not say (round-3b review, P3-4).
   */
  llmHasModel: boolean;
}

/**
 * Which slots saving would fill, from the status the dialog already has —
 * `null` when the host did not say (predates slice 4a's `*HasOwnKey` fields),
 * which {@link accountFillLine} reads as "render no line" rather than
 * guessing.
 */
export function accountFills(status: CompanyCredentialStatus | null): AccountFills | null {
  if (
    typeof status?.inferenceHasOwnKey !== "boolean" ||
    typeof status?.composioHasOwnKey !== "boolean" ||
    typeof status?.searchHasOwnKey !== "boolean"
  ) {
    return null;
  }
  return {
    llm: !status.inferenceHasOwnKey,
    composio: !status.composioHasOwnKey,
    search: !status.searchHasOwnKey,
    llmHasModel: status.inferenceHasModel === true,
  };
}

/**
 * The one conditional line Q9 asks for, naming only the slots this save would
 * fill — or `null` for no line at all (neither slot would be touched, the
 * only slot it would touch already has a model, or the host did not say).
 *
 * The LLM branch never says "connects" or "is connected": the fan-out adds
 * the key to `provider/tinyhumans/key` but creates no `tinyhumans` row
 * without a model (decision X5, `docs/key-reworks/README.md`), and a key with
 * no row behind it does nothing for LLM yet. Composio has no such gate — a
 * bearer with no row concept still serves live calls — so its wording keeps
 * the plain "connects".
 *
 * `llmHasModel` (round-3b review, P3-4) drops the LLM clause instead of
 * softening it: once a model is already chosen there is no "next" step to
 * promise, and "with the model you choose next" would simply be false.
 */
export function accountFillLine(fills: AccountFills | null): string | null {
  if (!fills) return null;
  const llm = fills.llm && !fills.llmHasModel;
  if (llm && fills.composio) {
    if (fills.search) {
      return "Saving also adds this key to TinyHumans on the LLM page, with the model you choose next — connects it for Composio, and uses it as this company's managed Search credential.";
    }
    return "Saving also adds this key to TinyHumans on the LLM page, with the model you choose next — and connects it for Composio.";
  }
  if (llm) {
    if (fills.search) {
      return "Saving also adds this key to TinyHumans on the LLM page, with the model you choose next — and uses it as this company's managed Search credential.";
    }
    return "Saving also adds this key to TinyHumans on the LLM page, with the model you choose next.";
  }
  if (fills.composio && fills.search) {
    return "Saving also connects TinyHumans for Composio and uses this key as the company's managed Search credential.";
  }
  if (fills.composio) {
    return "Saving also connects TinyHumans for Composio.";
  }
  if (fills.search) {
    return "Saving also uses this key as the company's managed Search credential.";
  }
  return null;
}

/** The model step's heading: whether the model chosen there also becomes the company default. */
export function modelStepTitle(setsDefault: boolean): string {
  return setsDefault ? "Choose the model new work uses" : "Choose the model TinyHumans uses";
}
