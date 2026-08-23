// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { Login } from "@/views/Login";
import type { OpenCompanyClient } from "@/api/client";

/**
 * The sign-in screen names the company it is a sign-in to.
 *
 * This is the one moment a person confirms *what* they are about to hand a
 * credential to, and until issue #1334 the console said nothing: `Login` took a
 * `companyName` prop that no caller ever passed, so the heading read a bare
 * "Sign in" everywhere and the ` to <name>` branch was unreachable. On the
 * hosted platform every tenant is a separate company on its own URL, so someone
 * with two of them — or clicking a week-old link from an email — had no
 * confirmation on screen until the sidebar, by which point they were already in.
 *
 * The name now rides the `/auth/config` fetch this view already makes: it is
 * per company, unauthenticated by construction, and the only thing the host
 * tells the console before anyone signs in.
 */

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

/** A host answering `/auth/config` with `config`, and no hub buttons. */
function hostReporting(config: Record<string, unknown>): OpenCompanyClient {
  return {
    scopeFor: () => "/api/v1/company",
    get: vi.fn().mockImplementation(async (path: string) => {
      if (path.endsWith("/auth/config")) return config;
      if (path.endsWith("/auth/hub")) return { providers: [] };
      throw new Error(`unexpected GET ${path}`);
    }),
    post: vi.fn(),
  } as unknown as OpenCompanyClient;
}

async function render(client: OpenCompanyClient) {
  await act(async () => {
    root.render(createElement(Login, { client, company: "acme", onSignedIn: () => {} }));
    await Promise.resolve();
  });
}

function heading(): HTMLElement | null {
  return container.querySelector("[data-testid='login-heading']");
}

it("names the company in the heading", async () => {
  await render(hostReporting({ mode: "email", passwords: true, magicLink: true, name: "Acme" }));

  expect(heading()?.textContent).toBe("Sign in to Acme");
});

it("names it in wallet mode too, where the form is a single button", async () => {
  await render(hostReporting({ mode: "wallet", passwords: false, magicLink: false, name: "Acme" }));

  expect(heading()?.textContent).toBe("Sign in to Acme");
});

/**
 * A host predating the field. The screen must be exactly the one it always
 * was — a missing name is not a reason to render a blank heading.
 */
it("falls back to a bare Sign in when the host reports no name", async () => {
  await render(hostReporting({ mode: "email", passwords: true, magicLink: true }));

  expect(heading()?.textContent).toBe("Sign in");
});

/**
 * `""` and `"   "` reach the same place as "not reported": one normalisation,
 * in `fetchAuthConfig`, so no view has to decide what a blank name means.
 */
it("treats a blank name as no name", async () => {
  await render(hostReporting({ mode: "email", passwords: true, magicLink: true, name: "   " }));

  expect(heading()?.textContent).toBe("Sign in");
});

/**
 * `none` mode has no signing in to do — the card below says so in full, and a
 * heading that said "Sign in to Acme" over it would contradict it. The name
 * alone is what the original code was reaching for.
 */
it("shows the name alone where there is no sign-in", async () => {
  await render(hostReporting({ mode: "none", passwords: false, magicLink: false, name: "Acme" }));

  expect(heading()?.textContent).toBe("Acme");
  expect(container.textContent).toContain("There is no sign-in here");
});

/**
 * The regression that made this visible: in `none` mode both the heading and
 * the subtitle are empty, and the block holding them rendered anyway — an empty
 * `h1` above an empty `p`, a gap above the card and a page with no heading text
 * for a screen reader.
 */
it("renders no empty heading or subtitle when there is nothing to say", async () => {
  await render(hostReporting({ mode: "none", passwords: false, magicLink: false }));

  expect(heading()).toBeNull();
  for (const el of container.querySelectorAll("h1, p")) {
    expect(el.textContent?.trim()).not.toBe("");
  }
});
