import { describe, expect, it } from "vitest";

import { NAV_SECTIONS, childAnchor } from "@/components/sidebar-navigation";
import { TOUR } from "@/tour/steps";
import { VIEWS } from "@/lib/console-routes";

/**
 * Every stop on the guided tour has something to point at.
 *
 * This is the check the tour has never had, and the reason it needs one is that
 * the failure is **silent**. `waitForTarget` resolves `false` after five seconds
 * when a step's anchor never mounts, and the controller treats that as a stop to
 * skip rather than an error — deliberately, so a lazy chunk that is slow to land
 * cannot wedge the tour. The cost of that decision is that a stop pointed at a
 * nav row somebody deleted looks exactly like a stop that loaded slowly: the
 * tour runs, the operator is shown fewer things than the product has, and no
 * test, log line or exception says so.
 *
 * The four-row restructure is exactly the change that would have done it. Half
 * the tour's stops named `nav-overview`, `nav-approvals`, `nav-workflows` and
 * `overview-graph`; two of those rows no longer exist and one was renamed. Under
 * the old arrangement the whole suite would have stayed green.
 *
 * So the anchors are asserted against the tables that decide what renders — the
 * nav model and the route allow-list — rather than against source text or a
 * browser. A row that is not in `NAV_SECTIONS` renders no `data-tour`, and a
 * view that is not in `VIEWS` is an address the shell will not resolve.
 */

/** The `data-tour` value a selector names, or `null` if it is not that shape. */
function anchorOf(target: string): string | null {
  return /^\[data-tour="([^"]+)"\]$/.exec(target)?.[1] ?? null;
}

/** Anchors the sidebar renders: one per section, one per child of a section. */
function sidebarAnchors(): Set<string> {
  const anchors = new Set<string>(["sidebar"]);
  for (const section of NAV_SECTIONS) {
    anchors.add(`nav-${section.view}`);
    for (const child of section.children ?? []) anchors.add(`nav-${child.sub ?? child.view}`);
  }
  return anchors;
}

/**
 * Anchors that are page content rather than sidebar rows, with the view each
 * one is rendered by. Listed here because nothing else in the codebase pairs
 * them, and an anchor whose view is wrong fails in the same silent way.
 */
const CONTENT_ANCHORS: Record<string, string> = {
  "chat-composer": "chat",
  "overview-graph": "overview",
  // The sidebar's own content region, rendered on every view.
  sidebar: "*",
  // The conversation list at the top of the sidebar, on every view.
  conversations: "*",
};

describe("every guided-tour stop can actually anchor", () => {
  it("names a view the console will route to", () => {
    for (const stop of TOUR) {
      expect(VIEWS, `the "${stop.title}" stop names view "${stop.view}"`).toContain(stop.view);
    }
  });

  it("targets an anchor something on that view renders", () => {
    const sidebar = sidebarAnchors();
    for (const stop of TOUR) {
      const anchor = anchorOf(stop.target);
      expect(anchor, `the "${stop.title}" stop's target is not a data-tour selector`).not.toBeNull();
      const known = sidebar.has(anchor!) || anchor! in CONTENT_ANCHORS;
      expect(
        known,
        `the "${stop.title}" stop points at "${anchor}", which nothing renders — ` +
          "a missing anchor is SKIPPED, not reported, so this would ship as a tour " +
          "that silently teaches less than the product does",
      ).toBe(true);
    }
  });

  it("navigates to the view its content anchor is rendered by", () => {
    for (const stop of TOUR) {
      const anchor = anchorOf(stop.target)!;
      const owner = CONTENT_ANCHORS[anchor];
      if (owner === undefined || owner === "*") continue;
      expect(stop.view, `the "${stop.title}" stop spotlights ${anchor}`).toBe(owner);
    }
  });

  it("names none of the rows the restructure removed", () => {
    // Overview and Approvals are chrome in the window's title row; Observatory
    // is a Settings rail row. None of the three renders a `nav-` anchor, and a
    // stop still pointing at one is the silent-skip case above.
    const anchors = TOUR.map((stop) => anchorOf(stop.target));
    expect(anchors).not.toContain("nav-overview");
    expect(anchors).not.toContain("nav-approvals");
    expect(anchors).not.toContain("nav-observatory");
  });

  it("says Automations where the sidebar does", () => {
    // The tour's prose is the other half of a rename. The anchors follow view
    // ids and so survive one silently — which is exactly how a step titled
    // "Workflows" would have gone on spotlighting a row labelled "Flows".
    const automations = TOUR.find((stop) => stop.view === "workflows")!;
    expect(automations.title).toBe("Automations");
    // Both earlier spellings, so a half-finished rename fails here rather than
    // shipping a step that spotlights a row by another name.
    expect(TOUR.some((stop) => stop.title === "Workflows")).toBe(false);
    expect(TOUR.some((stop) => stop.title === "Flows")).toBe(false);

    // Automations is a row on Company's rail now, so its label is a child of a
    // section rather than a section of its own — and the stop's anchor is the
    // one `childAnchor` gives that child.
    const labelled = NAV_SECTIONS.flatMap((section) =>
      (section.children ?? []).map((child) => ({ section, child })),
    ).find(({ child }) => child.label === automations.title);
    expect(labelled, "no sidebar row is called Automations").toBeDefined();
    expect(anchorOf(automations.target)).toBe(childAnchor(labelled!.section, labelled!.child));
  });

  it("no longer points at the Room row, which is not in the sidebar", () => {
    expect(TOUR.map((stop) => anchorOf(stop.target))).not.toContain("nav-chat");
  });

  it("still opens on the welcome and closes on the composer", () => {
    expect(TOUR[0].title).toBe("Welcome to your company");
    expect(TOUR[0].target).toBe('[data-tour="sidebar"]');
    expect(TOUR[TOUR.length - 1].target).toBe('[data-tour="chat-composer"]');
  });
});
