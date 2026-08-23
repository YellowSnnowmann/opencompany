// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import { SendInvoiceDialog } from "@/views/finance/SendInvoiceDialog";

/**
 * The due-days guard in the invoice dialog.
 *
 * `due_days` is the one field that can silently become "no due date" (a `NaN`
 * serializes as `null`) or reach the wire in a shape the server rejects (a
 * decimal) — so the dialog has to parse it once and refuse to send until it is
 * either empty or a valid non-negative safe integer.
 */

const sent = vi.fn();
const CLIENT = {
  scopeFor: () => "/api/v1/companies/acme",
} as unknown as OpenCompanyClient;

// The dialog owns its own field state, so rather than trying to drive React
// through a portal we mount a fresh instance per input value. The `due` value
// is used as the key so a brand-new component mounts for each case.
function Mount({ due }: { due: string }) {
  return createElement(
    SendInvoiceDialog,
    {
      key: due,
      client: CLIENT,
      company: "acme",
      site: "acme-test",
      open: true,
      onOpenChange: () => {},
      onSent: sent,
    },
  );
}

let container: HTMLDivElement;
let root: Root;

function at(testid: string): HTMLElement | null {
  return document.querySelector<HTMLElement>(`[data-testid="${testid}"]`);
}

async function mountWith(due: string) {
  await act(async () => {
    root.render(createElement(Mount, { due }));
  });
  // Type all the required fields.
  await fill("invoice-email", "alan@example.com");
  await fill("invoice-description", "Consulting");
  await fill("invoice-amount", "1250.00");
  await fill("invoice-due-days", due);
}

/** Types into a field the way an operator does, so React's state updates. */
async function fill(field: string, value: string) {
  const input = document.querySelector<HTMLInputElement>(`[data-testid="${field}"]`);
  if (!input) throw new Error(`no input ${field}`);
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(
      window.HTMLInputElement.prototype,
      "value",
    )?.set;
    setter?.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

beforeEach(() => {
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => {
    root.unmount();
  });
  container.remove();
  document.body.innerHTML = "";
  vi.clearAllMocks();
});

describe("the due-days field", () => {
  it("accepts an empty field", async () => {
    await mountWith("");
    expect(at("invoice-send")?.hasAttribute("disabled")).toBe(false);
  });

  it("accepts a whole non-negative number", async () => {
    await mountWith("7");
    expect(at("invoice-send")?.hasAttribute("disabled")).toBe(false);
  });

  it("disables send on non-numeric input", async () => {
    await mountWith("abc");
    expect(at("invoice-send")?.hasAttribute("disabled")).toBe(true);
  });

  it("disables send on a decimal", async () => {
    await mountWith("1.5");
    expect(at("invoice-send")?.hasAttribute("disabled")).toBe(true);
  });

  it("disables send on a negative number", async () => {
    await mountWith("-1");
    expect(at("invoice-send")?.hasAttribute("disabled")).toBe(true);
  });

  it("disables send on an out-of-safe-range number", async () => {
    await mountWith("999999999999999999999");
    expect(at("invoice-send")?.hasAttribute("disabled")).toBe(true);
  });
});
