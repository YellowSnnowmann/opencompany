import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

/**
 * The 768–1023px two-rail band (issue #1383).
 *
 * The app sidebar comes on at `md` (≥768). Chat's channel rail and Settings'
 * sub-rail used to come on *earlier or at the same* breakpoint, so from
 * 768–1023px the window carried two rails plus content and the working pane
 * collapsed to ~290px. In Chat that stranded the composer's Send button off
 * the right edge with no scroll to reach it; in Settings it clipped the SMTP
 * card on both sides. The fix pushes both second rails to `lg` (≥1024) — so
 * 768–1023 is single-rail — and lets the composer's action row wrap so Send
 * can never leave the flow.
 *
 * A jsdom render cannot prove this: the whole failure is a media query, and
 * jsdom does not evaluate them. So this guards the *class contract* the fix
 * rests on — the same source-contract idiom as `shell-chrome-tokens.test.ts`.
 * The pixel-accurate proof (Send clickable, SMTP legible across 768–1024px)
 * is a manual/e2e concern noted in the PR.
 */

const here = dirname(fileURLToPath(import.meta.url));
const read = (rel: string) => readFileSync(resolve(here, "../../src", rel), "utf8");

describe("chat rails collapse to single-rail below lg (issue #1383)", () => {
  const chatView = read("views/ChatView.tsx");
  const chatHeader = read("views/chat/ChatHeader.tsx");

  it("shows the channel rail only from lg, toggled below it", () => {
    // The rail: on from lg, otherwise governed by the mobile pane toggle.
    expect(chatView).toContain(
      'cn("lg:flex", mobilePane === "rail" ? "flex" : "hidden")',
    );
    // Regression guard: the `md` rail is what produced the two-rail band.
    expect(chatView).not.toContain(
      'cn("md:flex", mobilePane === "rail" ? "flex" : "hidden")',
    );
  });

  it("shows the chat pane full-width below lg (single pane), split at lg", () => {
    expect(chatView).toContain('mobilePane === "chat" ? "flex" : "hidden lg:flex"');
    expect(chatView).not.toContain('mobilePane === "chat" ? "flex" : "hidden md:flex"');
  });

  it("keeps the header's rail toggle visible across the single-rail band", () => {
    // The "Show channels" affordance must reach up to lg, matching the rail.
    expect(chatHeader).toContain("size-8 lg:hidden");
    expect(chatHeader).not.toContain("size-8 md:hidden");
  });
});

describe("settings sub-rail collapses to chips below lg (issue #1383)", () => {
  const settings = read("views/SettingsSection.tsx");

  it("shows the sub-rail only from lg", () => {
    expect(settings).toContain(
      "hidden w-60 shrink-0 flex-col gap-0.5 overflow-y-auto border-r p-3 lg:flex",
    );
    // Regression guard: the `sm` rail overlapped the app sidebar at 768–1023.
    expect(settings).not.toContain(
      "hidden w-60 shrink-0 flex-col gap-0.5 overflow-y-auto border-r p-3 sm:flex",
    );
  });

  it("shows the chip-row fallback below lg, so the pane gets full width", () => {
    expect(settings).toContain("flex gap-1 overflow-x-auto border-b p-2 lg:hidden");
    expect(settings).not.toContain("flex gap-1 overflow-x-auto border-b p-2 sm:hidden");
  });
});

describe("composer keeps Send in-flow in a narrow pane (issue #1383)", () => {
  const composer = read("views/chat/MessageComposer.tsx");

  it("lets the action row wrap instead of overflowing", () => {
    expect(composer).toContain('className="flex flex-wrap items-center gap-0.5 px-2 pb-1.5"');
    // Regression guard: the non-wrapping row pushed Send off-screen.
    expect(composer).not.toContain('className="flex items-center gap-0.5 px-2 pb-1.5"');
  });

  it("keeps Send in normal flow (ml-auto), never absolutely positioned", () => {
    // Anchor on the Send button and read the className just above its
    // `aria-label`: it must right-align with `ml-auto` and carry no out-of-flow
    // escape. If Send were pulled from the flow it could clip again exactly the
    // way #1383 describes.
    const idx = composer.indexOf('aria-label="Send"');
    expect(idx).toBeGreaterThan(-1);
    const sendButton = composer.slice(Math.max(0, idx - 300), idx);
    expect(sendButton).toContain("ml-auto");
    expect(sendButton).not.toMatch(/\babsolute\b|\bfixed\b/);
  });
});
