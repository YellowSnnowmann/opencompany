// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { ApprovalSummary, GrantScope, Verdict } from "@/api/types";
import { ApprovalCard } from "@/views/ApprovalsView";

/**
 * Issue #1406: Approve and Decline must not sit above the evidence and the
 * scope control that changes what Approve does.
 *
 * This suite is normally for pure functions — see `vitest.config.ts` — and it
 * earns the exception the same way `approval-batch-card` does: the claim is
 * about the DOM the operator's pointer travels through, and only a render can
 * show whether the commit affordance comes before or after the control that
 * redefines it. The old card put Approve in the headline's `actions` slot, level
 * with the title and ~200px above the "If you approve" fieldset; a pure test of
 * any helper cannot see that ordering at all.
 */

const T0 = new Date("2026-03-02T10:00:00Z").getTime();

// `broadly_grantable` so the scope control renders — it is the whole point of
// the issue — and a multi-line payload so the card is tall, the condition under
// which the scope control used to fall off-screen below the button.
const APPROVAL: ApprovalSummary = {
  id: "a1",
  kind: "shell",
  amount_usd: null,
  at_millis: T0,
  agent: "ops",
  broadly_grantable: true,
  payload: { command: "rm -rf /tmp/build && make release", cwd: "/srv/app" },
};

let container: HTMLDivElement;
let root: Root;

async function render(approval: ApprovalSummary) {
  await act(async () => {
    root.render(
      createElement(ApprovalCard, {
        approval,
        now: T0 + 60_000,
        askerNames: new Map([["ops", "Ops"]]),
        deciding: null,
        batchIndex: 1,
        batchTotal: 1,
        onDecide: (_verdict: Verdict, _scope: GrantScope) => {},
      }),
    );
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

/** The Approve button, wherever it ended up. */
function approveButton(): HTMLButtonElement {
  const btn = Array.from(container.querySelectorAll("button")).find((b) =>
    b.textContent?.includes("Approve"),
  );
  if (!btn) throw new Error("no Approve button rendered");
  return btn as HTMLButtonElement;
}

describe("ApprovalCard decide ordering (#1406)", () => {
  it("renders the scope control before the decide buttons in DOM order", async () => {
    await render(APPROVAL);

    const scope = container.querySelector("fieldset");
    const approve = approveButton();
    expect(scope, "the scope control should render for a broadly-grantable card").not.toBeNull();

    // `DOCUMENT_POSITION_FOLLOWING` on the scope element, tested against the
    // Approve button, means the button comes *after* the scope control — the
    // reading order #1406 requires: evidence and scope first, commit last.
    const position = scope!.compareDocumentPosition(approve);
    expect(position & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("keeps both decide buttons in the footer, below the scope control", async () => {
    await render(APPROVAL);

    const footer = container.querySelector('[data-testid="approval-decide"]');
    expect(footer, "the decide footer should exist").not.toBeNull();
    // Both verbs live in the footer — not one moved and one left behind.
    expect(footer!.textContent).toContain("Approve");
    expect(footer!.textContent).toContain("Decline");

    const scope = container.querySelector("fieldset")!;
    const position = scope.compareDocumentPosition(footer!);
    expect(position & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});
