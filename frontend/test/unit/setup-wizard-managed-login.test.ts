// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import type { SetupStatus } from "@/api/setup";
import { SetupWizard } from "@/views/setup/SetupWizard";

/**
 * The managed branch's step 1: the Account page's "Connect to TinyHumans"
 * ask, and the key it collects going where that page's key goes.
 *
 * Two different features used to hide behind the same screen. Connecting
 * TinyHumans on the Account page wrote the **company's** account key and let
 * the host fan it out — the Composio copy, the LLM copy, the row, the
 * default. Connecting TinyHumans in onboarding wrote the instance-wide
 * `tinyhumans_api_key` into `config.toml`, a boot-time setting that fills none
 * of those, and told the operator to restart. This file pins that there is now
 * one mechanism: what the step collects is submitted as the company's account
 * key, is not written as host configuration, and the host's own account of
 * what the fan-out did is shown rather than summarised away.
 *
 * Mounted rather than pure: every claim here is about what a submit carries
 * after the operator has walked the branch, which only exists while the
 * component is rendering.
 */

const TEMPLATE = {
  id: "law_firm",
  name: "Agentic Law Firm",
  agent_count: 5,
  output: "Filings and advice",
};

const KEY = "th-not-a-real-key";
const MODEL = "acme/test-model";
const NOTE = "Key saved. Composio now uses this key. TinyHumans is set up for LLM with " +
  `${MODEL}. It is now the default for new work.`;

function status(over: Partial<SetupStatus> = {}): SetupStatus {
  return {
    complete: false,
    config_path: "/data/config.toml",
    fields: [
      {
        key: "tinyhumans_api_key",
        value: null,
        layer: "default",
        editable: true,
        requires_restart: true,
        secret: true,
      },
    ],
    templates: [TEMPLATE],
    // `none` keeps the walk short — it removes the address step, the only one
    // that would demand an answer this file is not about.
    auth_modes: ["none", "email"],
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

interface Sent {
  probe?: { provider?: string; key?: string | null };
  body?: Record<string, unknown>;
}

function clientWith(s: SetupStatus, sent: Sent, note: string | null = NOTE): OpenCompanyClient {
  return {
    scopeFor: () => "/api/v1/company",
    get: async () => s,
    post: async (path: string, body: unknown) => {
      if (path.endsWith("/inference/test")) {
        sent.probe = body as Sent["probe"];
        return { ok: true, baseUrl: "https://api.tinyhumans.ai/v1", model: MODEL };
      }
      if (path.includes("/setup/roster")) {
        return {
          agents: [{ name: "Partner", role: "Partner", description: "Advises." }],
          template: TEMPLATE.id,
          source: "preset",
          jobs: [],
          uncovered: [],
          reason: "ok",
        };
      }
      if (path === "/api/v1/setup") {
        sent.body = body as Record<string, unknown>;
        return {
          complete: true,
          config_path: s.config_path,
          restart_required: [],
          seeded_company: "agentic-law-firm",
          credential_note: note,
        };
      }
      return {};
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

const find = (testId: string) => container.querySelector(`[data-testid="${testId}"]`);

async function click(testId: string) {
  const el = find(testId) as HTMLElement | null;
  expect(el, `no element ${testId}`).toBeTruthy();
  await act(async () => {
    el!.click();
  });
}

function labelled(...wanted: string[]): HTMLButtonElement {
  const match = Array.from(container.querySelectorAll("button")).find((b) =>
    wanted.includes(b.textContent?.trim() ?? ""),
  );
  expect(match, `no button labeled ${wanted.join("/")}`).toBeTruthy();
  return match as HTMLButtonElement;
}

const next = async () =>
  act(async () => {
    labelled("Next", "Looks good").click();
  });

async function fill(testId: string, value: string) {
  const field = find(testId) as HTMLInputElement | null;
  expect(field, `no field ${testId}`).toBeTruthy();
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(field, value);
    field!.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function pickTemplate(id: string) {
  const select = container.querySelector("select") as HTMLSelectElement;
  expect(select, "no template dropdown").toBeTruthy();
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")!.set!.call(select, id);
    select.dispatchEvent(new Event("change", { bubbles: true }));
  });
}

async function show(client: OpenCompanyClient) {
  await act(async () => {
    root.render(createElement(SetupWizard, { client, onDone: () => {} }));
  });
}

/** Step 0 (managed) -> step 1, key pasted and proved. */
async function connect(client: OpenCompanyClient) {
  await show(client);
  await click("setup-way-managed");
  await next();
  await fill("setup-field-key", KEY);
  await click("setup-test-connection");
}

/** …and on through business -> sign-in -> review -> build. */
async function build(client: OpenCompanyClient) {
  await connect(client);
  await next();
  await pickTemplate(TEMPLATE.id);
  await next();
  await click("auth-mode-none");
  await next();
  await act(async () => {
    labelled("Build my company").click();
  });
}

describe("the managed branch's step 1", () => {
  it("asks for the account key, not for a provider", async () => {
    await show(clientWith(status(), {}));
    await click("setup-way-managed");
    await next();

    expect(find("setup-question")?.textContent).toContain("Connect to TinyHumans");
    expect(find("setup-field-key"), "the key field is the step").toBeTruthy();
    // Taking the managed way already answered "which provider", so asking
    // again would be offering a way out of the branch from inside it.
    expect(find("setup-provider-select"), "no provider picker on this branch").toBeNull();
    // The key is minted on the operator's own dashboard and pasted back. A
    // one-click grant is company-scoped, and no company exists yet.
    expect(find("setup-key-get-link"), "a way to get a key").toBeTruthy();
  });

  it("proves the key against TinyHumans rather than whatever the host runs", async () => {
    const sent: Sent = {};
    await connect(clientWith(status({ inference: { ready: false, provider: "openrouter", base_url: null } }), sent));

    expect(sent.probe?.provider).toBe("managed");
    expect(sent.probe?.key).toBe(KEY);
    expect(find("setup-test-ok")?.textContent).toContain(MODEL);
  });

  it("holds the step until the key has been proved", async () => {
    await show(clientWith(status(), {}));
    await click("setup-way-managed");
    await next();
    await fill("setup-field-key", KEY);

    await next();
    expect(find("setup-problem"), "an untested key must hold the step").toBeTruthy();
    expect(find("setup-field-key"), "and must not have left it").toBeTruthy();
  });
});

describe("where the key the managed branch collected goes", () => {
  it("is submitted as the company's account key, with the model that answered", async () => {
    const sent: Sent = {};
    await build(clientWith(status(), sent));

    expect(sent.body?.tinyhumans_key).toBe(KEY);
    // Sent with the key, so the fan-out can finish the row in one pass rather
    // than leaving it unmade for want of a model nobody was asked for.
    expect(sent.body?.tinyhumans_model).toBe(MODEL);
  });

  it("is never written as host configuration", async () => {
    const sent: Sent = {};
    await build(clientWith(status(), sent));

    // `tinyhumans_api_key` is the instance's identity for companies that have
    // none of their own, read once at boot. Writing the same bytes there
    // duplicates the secret and reports a restart that buys nothing — the
    // company's own key is what the fan-out acts on.
    const fields = sent.body?.fields as Record<string, unknown> | undefined;
    expect(fields?.tinyhumans_api_key).toBeUndefined();
  });

  it("does not ride the manifest as a declared provider", async () => {
    const sent: Sent = {};
    await build(clientWith(status(), sent));

    // A TinyHumans key is an account identity, not one provider's credential.
    // Declaring `managed` on the manifest is the legacy shape the fan-out
    // refuses to create a row underneath.
    const company = sent.body?.company as { inference?: unknown } | null | undefined;
    expect(company?.inference ?? null).toBeNull();
  });
});

describe("what the operator is told the key actually did", () => {
  it("shows the host's own account of the fan-out", async () => {
    await build(clientWith(status(), {}));

    // Verbatim. The fan-out honestly reports the slots it left alone and the
    // row it could not finish, and those are exactly the parts a cheerful
    // "you're set up" would bury.
    expect(find("setup-done"), "the apply landed").toBeTruthy();
    expect(find("setup-credential-note")?.textContent).toBe(NOTE);
  });

  it("says nothing where the host said nothing", async () => {
    await build(clientWith(status(), {}, null));

    expect(find("setup-done")).toBeTruthy();
    expect(find("setup-credential-note"), "no note, no line invented for it").toBeNull();
  });
});
