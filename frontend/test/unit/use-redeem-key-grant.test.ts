// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import { captureKeyLink } from "@/lib/pending-key-link";
import { useRedeemKeyGrant } from "@/views/connections/use-redeem-key-grant";

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  captureKeyLink(null, false);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.restoreAllMocks();
});

function client(post: unknown) {
  return {
    scopeFor: (company: string | null) =>
      company ? `/api/v1/companies/${company}` : "/api/v1/company",
    post,
  } as unknown as OpenCompanyClient;
}

function Harness(props: { client: OpenCompanyClient; onConnected?: () => void }) {
  useRedeemKeyGrant(props.client, "acme", props.onConnected);
  return null;
}

async function mount(props: { client: OpenCompanyClient; onConnected?: () => void }) {
  await act(async () => {
    root.render(createElement(Harness, props));
  });
}

describe("coming back finishes the exchange", () => {
  it("redeems a captured grant exactly once", async () => {
    const post = vi.fn(async () => ({
      status: { configured: true, source: "company", notice: "" },
      note: "connected",
    }));
    captureKeyLink({ state: "s1", code: "c1" }, false);

    const onConnected = vi.fn();
    await mount({ client: client(post), onConnected });

    expect(post).toHaveBeenCalledTimes(1);
    expect(post).toHaveBeenCalledWith("/api/v1/companies/acme/credential/link/finish", {
      state: "s1",
      code: "c1",
    });
    expect(onConnected).toHaveBeenCalledTimes(1);
  });

  it("redeems nothing when the person cancelled on the hub", async () => {
    const post = vi.fn();
    captureKeyLink(null, true);

    await mount({ client: client(post) });

    expect(post).not.toHaveBeenCalled();
  });

  it("redeems nothing on an ordinary load", async () => {
    const post = vi.fn();
    await mount({ client: client(post) });

    expect(post).not.toHaveBeenCalled();
  });
});
