// @vitest-environment jsdom

/**
 * Keys rework (issue #2306), slice 4c — the pure visibility/dismissal logic
 * behind the Composio "use the same key?" banner
 * (`docs/key-reworks/phase-4c-reuse-banner.md`). No component-test harness
 * exists in this project (see `test/unit/budget-pause-notice.test.ts`'s
 * header), so this pins the decision `ComposioSection.tsx` renders from
 * rather than the markup around it.
 *
 * `@/inference/reuse-banner` (round-3b review, item 6) is the shared module
 * that now also holds the LLM page's own `showsInferenceReuseBanner`, not
 * implemented yet — see that file's header. This file stays Composio-only
 * until that lands; it is not renamed to a shared `reuse-banner.test.ts` for
 * the same reason (a plainer name would read as covering both).
 */

import { describe, expect, it } from "vitest";

import {
  readDismissed,
  reuseDismissKey,
  showsComposioReuseBanner,
  writeDismissed,
} from "@/inference/reuse-banner";

describe("showsComposioReuseBanner", () => {
  const base = {
    canManage: true,
    accountConfigured: true,
    mode: "managed" as const,
    managedCredentialSource: "none" as string | undefined,
    dismissed: false,
  };

  it("is true once the account key exists, the route is managed, and nothing of its own resolves (none)", () => {
    expect(
      showsComposioReuseBanner({ ...base, managedCredentialSource: "none" }),
    ).toBe(true);
  });

  it("is true when the managed chain resolves to something the company did not itself paste (company)", () => {
    expect(
      showsComposioReuseBanner({ ...base, managedCredentialSource: "company" }),
    ).toBe(true);
  });

  it("is false when the managed slot already holds its own key (static)", () => {
    expect(
      showsComposioReuseBanner({ ...base, managedCredentialSource: "static" }),
    ).toBe(false);
  });

  it("is false under byok — this banner only ever offers to fill the managed slot", () => {
    expect(showsComposioReuseBanner({ ...base, mode: "byok" })).toBe(false);
  });

  it("is false with no `mode` on the wire (an older host)", () => {
    expect(
      showsComposioReuseBanner({ ...base, mode: undefined }),
    ).toBe(false);
  });

  it("is false for a non-admin, whatever else is true", () => {
    expect(showsComposioReuseBanner({ ...base, canManage: false })).toBe(false);
  });

  it("is false with no account key to reuse", () => {
    expect(
      showsComposioReuseBanner({ ...base, accountConfigured: false }),
    ).toBe(false);
  });

  it("is false once the operator has dismissed it", () => {
    expect(showsComposioReuseBanner({ ...base, dismissed: true })).toBe(false);
  });
});

describe("reuseDismissKey", () => {
  it("namespaces by page and company", () => {
    expect(reuseDismissKey("composio", "acme")).toBe(
      "oc.reuse-account-key.composio.acme",
    );
  });

  it("falls back to a fixed segment with no company", () => {
    expect(reuseDismissKey("composio", null)).toBe(
      "oc.reuse-account-key.composio._",
    );
  });
});

describe("readDismissed / writeDismissed", () => {
  it("is false before anything is written", () => {
    expect(readDismissed("oc.reuse-account-key.composio.unwritten")).toBe(
      false,
    );
  });

  it("round-trips through localStorage", () => {
    const key = "oc.reuse-account-key.composio.roundtrip";
    writeDismissed(key);
    expect(readDismissed(key)).toBe(true);
  });

  it("readDismissed returns false when localStorage itself throws", () => {
    const original = Object.getOwnPropertyDescriptor(window, "localStorage");
    Object.defineProperty(window, "localStorage", {
      configurable: true,
      get() {
        throw new Error("blocked");
      },
    });
    try {
      expect(readDismissed("oc.reuse-account-key.composio.acme")).toBe(false);
    } finally {
      if (original) Object.defineProperty(window, "localStorage", original);
    }
  });

  it("writeDismissed does not throw when localStorage itself throws", () => {
    const original = Object.getOwnPropertyDescriptor(window, "localStorage");
    Object.defineProperty(window, "localStorage", {
      configurable: true,
      get() {
        throw new Error("blocked");
      },
    });
    try {
      expect(() =>
        writeDismissed("oc.reuse-account-key.composio.acme"),
      ).not.toThrow();
    } finally {
      if (original) Object.defineProperty(window, "localStorage", original);
    }
  });
});
