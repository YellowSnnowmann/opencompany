// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import type { CompanyStatus } from "@/api/types";
import type { CompanyFeed } from "@/hooks/use-company";
import { LifecycleControls } from "@/views/SettingsView";

/**
 * This file intentionally does not mock `product-scope`. The flow tests enable
 * company switching so their pre-archive behavior remains exercised; this test
 * instead pins the shipped feature gate at the surface that can archive a
 * company and provision its replacement.
 */

const status: CompanyStatus = {
  id: "acme",
  name: "Acme Robotics",
  lifecycle: "running",
  pending_approvals: 0,
};

const feed: CompanyFeed = {
  status,
  approvals: [],
  queue: "ready",
  now: Date.now(),
  refresh: () => Promise.resolve(),
};

const platformClient = { carriesPlatformBearer: true } as OpenCompanyClient;

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

describe("the shipped Reset / Start clean gate", () => {
  it("does not render the destructive reset control for a platform bearer", async () => {
    await act(async () => {
      root.render(
        createElement(LifecycleControls, {
          client: platformClient,
          company: "acme",
          feed,
          onReset: () => {},
        }),
      );
    });

    expect(container.textContent).not.toContain("Reset / Start clean");
  });
});
