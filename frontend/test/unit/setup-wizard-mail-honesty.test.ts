// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import type { SetupStatus } from "@/api/setup";
import { SetupWizard } from "@/views/setup/SetupWizard";

/**
 * What the wizard says about mail, when it cannot send any.
 *
 * `email` sign-in and a working mailbox are two different questions — hub OAuth
 * and passwords sign people in with no transport at all — so the flow must
 * neither hide the mode nor promise a link it cannot deliver. The host answers
 * both halves in `mail`: `wired` is "a link is genuinely sent", `echoes_code` is
 * "the code comes back in the response instead", and only a host with neither
 * is one where a link goes nowhere.
 *
 * The hand-off at the end used to infer all of this from whether `requestCode`
 * echoed a `dev_code`, which is only ever true on a loopback bind — so a
 * routable host with no SMTP finished setup by telling its operator to check an
 * inbox that would stay empty forever. That is the bug these tests hold shut.
 */

function status(over: Partial<SetupStatus> = {}): SetupStatus {
  return {
    complete: false,
    config_path: "/data/config.toml",
    fields: [],
    templates: [],
    auth_modes: ["email"],
    build: {
      acp_in_build: false,
      acp_transport_mounted: false,
      mcp_in_build: false,
      harness_in_build: false,
      oauth_in_build: false,
    },
    companies: [],
    inference: { ready: false, provider: null, base_url: null },
    mail: { wired: false, echoes_code: false },
    ...over,
  };
}

/**
 * Routed by path: the wizard makes four different calls through `post`, and the
 * one under test here is the last of them.
 */
function clientWith(
  s: SetupStatus,
  over: { requestCode?: () => Promise<unknown> } = {},
): OpenCompanyClient {
  return {
    scopeFor: (company: string | null) => `/api/v1/companies/${company}`,
    get: async () => s,
    post: async (path: string) => {
      if (path.endsWith("/setup/roster")) {
        return {
          agents: [{ name: "Ada", role: "Operations", description: "Runs the desk." }],
          template: "ecommerce",
          source: "fallback",
        };
      }
      if (path.endsWith("/auth/request")) {
        return over.requestCode ? await over.requestCode() : { sent: true };
      }
      return {
        complete: true,
        config_path: s.config_path,
        restart_required: [],
        seeded_company: "acme",
      };
    },
  } as unknown as OpenCompanyClient;
}

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

async function show(client: OpenCompanyClient) {
  await act(async () => {
    root.render(createElement(SetupWizard, { client, onDone: () => {} }));
  });
}

const find = (testId: string) => container.querySelector(`[data-testid="${testId}"]`);

async function click(testId: string) {
  const el = find(testId) as HTMLElement | null;
  expect(el, `no element ${testId}`).toBeTruthy();
  await act(async () => {
    el!.click();
  });
}

/**
 * Gets past step 0 onto step 1, and is a no-op once already there.
 *
 * The flow opens on the setup-way choice, and step 1 sits behind
 * "Set it up yourself".
 */
async function chooseSelfManaged() {
  if (!find("setup-way-self-managed")) return;
  await click("setup-way-self-managed");
  await next();
}

/**
 * Gets past step 1 without connecting anything.
 *
 * The self-managed branch's step 1 is the real add-provider sequence now, and
 * both of its connections are optional — so leaving it unanswered is the whole
 * of skipping it, and Next is not gated. This presses the "set this up later"
 * affordance rather than choosing a "No model" the step no longer offers.
 */
async function skipConnect() {
  await chooseSelfManaged();
  await click("setup-provider-later");
}

const next = async () =>
  act(async () => {
    const match = Array.from(container.querySelectorAll("button")).find((b) =>
      ["Next", "Looks good"].includes(b.textContent?.trim() ?? ""),
    );
    expect(match, "no advance button").toBeTruthy();
    match!.click();
  });

async function fill(testId: string, value: string) {
  const field = find(testId) as HTMLInputElement | HTMLTextAreaElement | null;
  expect(field, `no field ${testId}`).toBeTruthy();
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(
      field instanceof HTMLTextAreaElement
        ? HTMLTextAreaElement.prototype
        : HTMLInputElement.prototype,
      "value",
    )!.set!;
    setter.call(field!, value);
    field!.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

/** Lets the design, apply and sign-in requests settle. */
const settle = async () =>
  act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });

/** Walks the whole flow and presses the finish button. */
async function finish() {
  await skipConnect();
  await next(); // -> business
  await fill("setup-field-industry", "E-commerce — homeware");
  await next(); // -> sign-in
  await next(); // -> account
  await fill("setup-field-email", "ada@example.com");
  await next(); // -> review
  await settle();
  await click("setup-finish");
  await settle();
}

describe("the sign-in step, on a host that cannot send mail", () => {
  it("says the link is handed over here when the host echoes the code", async () => {
    await show(clientWith(status({ mail: { wired: false, echoes_code: true } })));
    await skipConnect();
    await next();
    await fill("setup-field-industry", "Homeware");
    await next(); // -> sign-in

    const note = find("setup-mail-note");
    expect(note?.textContent).toContain("browser");
    // Still an offer, not a warning off it: the card is the control, and the
    // note sits beside it.
    expect((find("auth-mode-email") as HTMLButtonElement).disabled).toBe(false);
  });

  it("says a link would arrive nowhere on a routable host with no transport", async () => {
    await show(clientWith(status({ mail: { wired: false, echoes_code: false } })));
    await skipConnect();
    await next();
    await fill("setup-field-industry", "Homeware");
    await next(); // -> sign-in

    const note = find("setup-mail-note");
    expect(note?.textContent).toMatch(/no.*mail|won't arrive|nothing will arrive/i);
    // Email sign-in is not broken here — hub buttons and passwords work without
    // a transport — so the mode must stay offered rather than be hidden from an
    // operator who may wire SMTP ten minutes later.
    expect(find("auth-mode-email")).toBeTruthy();
    expect((find("auth-mode-email") as HTMLButtonElement).disabled).toBe(false);
  });

  it("says nothing when the host has a mail transport", async () => {
    await show(clientWith(status({ mail: { wired: true, echoes_code: false } })));
    await skipConnect();
    await next();
    await fill("setup-field-industry", "Homeware");
    await next(); // -> sign-in

    expect(find("setup-mail-note")).toBeNull();
  });
});

describe("the hand-off after setup applies", () => {
  it("hands over the link when the host returned the code", async () => {
    await show(
      clientWith(status({ mail: { wired: false, echoes_code: true } }), {
        requestCode: async () => ({ sent: true, dev_code: "abc123" }),
      }),
    );
    await finish();

    expect(find("setup-handoff-link")).toBeTruthy();
    expect(find("setup-signin")?.getAttribute("data-handoff-url")).toBe(
      "/login?company=acme&code=abc123#/company?from=setup",
    );
  });

  it("points at the inbox only when the host can actually send", async () => {
    await show(clientWith(status({ mail: { wired: true, echoes_code: false } })));
    await finish();

    expect(find("setup-handoff-mailed")?.textContent).toContain("ada@example.com");
  });

  /**
   * The bug this file exists for. No `dev_code` used to be read as "mailed",
   * and on a routable host with no transport that is a link nobody will ever
   * receive — announced as if it were on its way.
   */
  it("says no link was sent when the host has no way to send one", async () => {
    await show(clientWith(status({ mail: { wired: false, echoes_code: false } })));
    await finish();

    expect(find("setup-handoff-mailed")).toBeNull();
    expect(find("setup-handoff-unmailable")).toBeTruthy();
    // And a way in regardless: the console still opens.
    expect(find("setup-open-console")).toBeTruthy();
  });
});
