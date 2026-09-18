// @vitest-environment jsdom

// The routes-not-carried banner (keys rework, issue #2306, phase 5a) — the
// one-release bridge for a stored routing table the boot-time carry could not
// fold into a single default. Round-2 review, P2-6: neither the copy nor the
// component had a test of its own.

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { describeRoute, describeRows, routesNotCarriedCopy } from "@/inference/routes-not-carried";
import { RoutesNotCarriedBanner } from "@/inference/RoutesNotCarriedBanner";

describe("describeRoute", () => {
  it("names a slug and model", () => {
    expect(describeRoute("acme:test-model")).toBe("acme · test-model");
  });

  it("names Managed for the legacy chain, capitalised", () => {
    expect(describeRoute("managed")).toBe("Managed");
  });

  it("names a bare slug with no model", () => {
    expect(describeRoute("acme")).toBe("acme");
    expect(describeRoute("acme:")).toBe("acme");
  });
});

describe("describeRows", () => {
  it("joins tier and route, using the tier's plain word", () => {
    expect(
      describeRows([
        { tier: "chat-v1", route: "acme:test-model" },
        { tier: "reasoning-v1", route: "managed" },
      ]),
    ).toBe("chat → acme · test-model, reasoning → Managed");
  });

  it("falls back to the raw tier id for one it does not recognise", () => {
    expect(describeRows([{ tier: "future-v1", route: "acme:x" }])).toBe("future-v1 → acme · x");
  });
});

describe("routesNotCarriedCopy", () => {
  it("is null with nothing to say", () => {
    expect(routesNotCarriedCopy(null)).toBeNull();
    expect(routesNotCarriedCopy(undefined)).toBeNull();
    expect(routesNotCarriedCopy([])).toBeNull();
  });

  it("names every row and the one action that fixes it", () => {
    const copy = routesNotCarriedCopy([{ tier: "chat-v1", route: "acme:test-model" }]);
    expect(copy).toContain("Routing is going away");
    expect(copy).toContain("chat → acme · test-model");
    expect(copy).toContain("Choose one default provider and model");
  });
});

describe("RoutesNotCarriedBanner", () => {
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

  function testId(id: string) {
    return container.querySelector(`[data-testid="${id}"]`);
  }

  it("renders nothing when there is nothing to carry", () => {
    act(() => {
      root.render(createElement(RoutesNotCarriedBanner, { rows: null, canManage: true }));
    });
    expect(testId("inference-routes-not-carried-banner")).toBeNull();
  });

  it("renders the banner and the action for a manager", () => {
    act(() => {
      root.render(
        createElement(RoutesNotCarriedBanner, {
          rows: [{ tier: "chat-v1", route: "acme:test-model" }],
          canManage: true,
          onChooseDefault: () => {},
        }),
      );
    });
    expect(testId("inference-routes-not-carried-banner")?.textContent).toContain("Routing is going away");
    expect(testId("inference-routes-not-carried-choose")).not.toBeNull();
  });

  it("withholds the action from a member who cannot change it", () => {
    act(() => {
      root.render(
        createElement(RoutesNotCarriedBanner, {
          rows: [{ tier: "chat-v1", route: "acme:test-model" }],
          canManage: false,
          onChooseDefault: () => {},
        }),
      );
    });
    expect(testId("inference-routes-not-carried-banner")).not.toBeNull();
    expect(testId("inference-routes-not-carried-choose")).toBeNull();
  });

  it("fires onChooseDefault when the button is pressed", () => {
    const onChooseDefault = vi.fn();
    act(() => {
      root.render(
        createElement(RoutesNotCarriedBanner, {
          rows: [{ tier: "chat-v1", route: "acme:test-model" }],
          canManage: true,
          onChooseDefault,
        }),
      );
    });
    const button = testId("inference-routes-not-carried-choose") as HTMLButtonElement | null;
    expect(button).not.toBeNull();
    act(() => button?.click());
    expect(onChooseDefault).toHaveBeenCalledTimes(1);
  });
});
