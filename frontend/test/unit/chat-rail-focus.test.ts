import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

/**
 * Expanding the compact rail keeps keyboard focus on a collapse control
 * (issue #1340 focus review).
 *
 * The compact rail's expand button only exists in the collapsed branch, so
 * expanding it unmounts the very control that carried focus and a keyboard user
 * falls out to the document. The expand action therefore hands focus to a
 * control that is mounted in both states.
 *
 * Which control that is changed with the four-row sidebar. It was a toggle in
 * the chat header; the rail is a section of the app sidebar now, so that toggle
 * became a duplicate of `SidebarCollapseButton` sitting forty pixels from it,
 * and was removed. The seam button is what survives the switch, and it is what
 * catches the focus.
 *
 * A jsdom render of `RoomView` cannot prove this — it needs the whole client
 * and every hook. So this guards the *wiring contract* the fix rests on, the
 * same source-contract idiom as `responsive-two-rail-band.test.ts`: the rail
 * fires `onExpand`, and the view hands focus to the one collapse control that
 * is mounted on both sides of the switch.
 */

const here = dirname(fileURLToPath(import.meta.url));
const read = (rel: string) => readFileSync(resolve(here, "../../src", rel), "utf8");

describe("expanding the compact rail preserves focus (issue #1340)", () => {
  const rail = read("views/room/ChannelRail.tsx");
  const chatView = read("views/RoomView.tsx");
  const controls = read("components/sidebar-controls.tsx");

  it("keeps the compact rail's expand button the only expand affordance in the collapsed branch", () => {
    // The button only renders while collapsed, which is exactly why expanding
    // it strips focus — the premise the rest of this wiring exists to fix.
    const idx = rail.indexOf('aria-label="Expand channels"');
    expect(idx).toBeGreaterThan(-1);
    const button = rail.slice(Math.max(0, idx - 400), idx);
    expect(button).toContain("onClick={onExpand}");
  });

  it("hands focus to the control that is mounted in BOTH density states", () => {
    // That control used to be a toggle in the chat header. It is not any more:
    // the rail lives in the app sidebar, so collapsing it IS collapsing the
    // sidebar, and the header's button became a second control doing the
    // sidebar's job forty pixels from the sidebar's own (issue #1177). The one
    // that survives the switch is `SidebarCollapseButton`, on the content
    // card's leading seam.
    expect(chatView).toContain('document.querySelector<HTMLElement>(\'[data-testid="sidebar-collapse"]\')');
    // Only on the expand half: collapsing from the seam button leaves that
    // button mounted, and the focus it already holds is the right place to stay.
    expect(chatView).toContain("const expanding = channelsCollapsed;");
    expect(chatView).toContain("if (expanding) {");
  });

  it("keeps that test id on the control the shell actually renders", () => {
    // The hand-off is a DOM query, so the id is a contract between two files.
    // `sidebar-toggle-reachable.spec.ts` pins the same id from the other side.
    expect(controls).toContain('data-testid="sidebar-collapse"');
    expect(controls).toContain("onClick={toggleSidebar}");
  });
});
