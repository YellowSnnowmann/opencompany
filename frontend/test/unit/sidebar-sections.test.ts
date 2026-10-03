// @vitest-environment jsdom

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  NAV_SECTIONS,
  SidebarNavigation,
  SidebarSectionTabs,
  childActive,
  childAnchor,
  sectionOwning,
  type NavSection,
} from "@/components/sidebar-navigation";
import { SidebarProvider } from "@/components/ui/sidebar";
import type { View } from "@/lib/console-routes";

/**
 * The sidebar is sections with sub-navigation under them, not a flat list.
 *
 * These assertions are written to survive mutation, which for a nav table means
 * two specific traps. `toContain('{ view: "x" }')` is satisfied by a **commented
 * out** row that renders nothing — the exact shape that left `#/pages`
 * unreachable for four months (issue #1311) — so nothing here reads source text.
 * And a label assertion that only checks membership is satisfied by a row that
 * is also still somewhere else, so the tables are compared whole and in order.
 */

let container: HTMLDivElement;
let root: Root;

function render(view: View) {
  act(() =>
    root.render(
      createElement(
        SidebarProvider,
        null,
        // No `pending` to pass: the approvals count is the title row's bell's
        // now, and `title-bar-jumps.test.ts` owns it.
        // The column (now only the conversation list's slot) and the foot's
        // section tabs, which are what Company and Connections became.
        createElement(SidebarNavigation),
        createElement(SidebarSectionTabs, { view, onNavigate: () => {} }),
      ),
    ),
  );
}

/** The section tabs, by accessible name, in document order. */
function tabs(): HTMLButtonElement[] {
  return [...container.querySelectorAll<HTMLButtonElement>("button[data-tour^='nav-']")];
}

/** The names of the tabs marked as the current page. */
function litTabs(): string[] {
  return tabs()
    .filter((el) => el.getAttribute("aria-current") === "page")
    .map((el) => el.getAttribute("aria-label") ?? "");
}

beforeEach(() => {
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  // jsdom ships no `matchMedia`, and `SidebarProvider` asks it whether this is a
  // phone. Answer "no" — the sub-navigation this file is about is the desktop
  // column, not the mobile sheet.
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("the sidebar's section table", () => {
  it("is exactly these top-level rows, in this order", () => {
    // Room and Automations left the table: the conversation list is the front
    // of the sidebar rather than a row, and Automations is a row on Company's
    // rail. Company and Connections are the only sections left.
    expect(NAV_SECTIONS.map((section) => section.label)).toEqual(["Company", "Connections"]);
  });

  it("keeps the surfaces that lost their row out of the table entirely", () => {
    // Overview moved up into the window's title row; Observatory moved down
    // into Settings. A row left behind as a comment is the failure this
    // codebase has already had once (#1311), so the assertion above is a
    // whole-table equality — a commented row is not a member of it — and this
    // one says the same thing from the other side.
    //
    // Approvals is NOT in this list. It is the Approvals tab of the
    // Notifications page now, reached from a bell in the window's title row
    // beside Overview and Settings — so the column draws neither the row nor
    // the `SidebarMenuBadge`/`SidebarMenuDot` pair that used to carry its
    // count. See `components/notifications-button.tsx` for why this position
    // settles an argument the previous two did not.
    const views = NAV_SECTIONS.map((section) => section.view as string);
    expect(views).not.toContain("overview");
    expect(views).not.toContain("observatory");
    expect(views).not.toContain("approvals");
    expect(views).not.toContain("notifications");
  });

  it("files the company's surfaces under Company, in this order, Automations before Finance", () => {
    const company = NAV_SECTIONS.find((section) => section.label === "Company")!;
    expect(company.children?.map((child) => [child.label, child.view])).toEqual([
      ["Agents", "company"],
      ["Work", "ledgers"],
      ["Workspace", "workspace"],
      ["Brain", "brain"],
      // A plain top-level child, not inside the Finance caption: `sectionOwning`
      // reads only direct children, and a plain row after the caption would
      // read as one of its pages.
      ["Automations", "workflows"],
      ["Finance", "finances"],
    ]);
  });

  it("calls the automation surface Automations, over the view id every address uses", () => {
    // A view id is an address — every `#/workflows/<id>` a run row points at —
    // and renaming a row is not a reason to break them. It is a child of
    // Company now, and still the `workflows` view.
    const company = NAV_SECTIONS.find((section) => section.label === "Company")!;
    const automations = company.children!.find((child) => child.label === "Automations")!;
    expect(automations.view).toBe("workflows");
    expect(automations.group).toBeFalsy();
    // And Room is not a section: the `chat` route stays, only the row went.
    expect(NAV_SECTIONS.some((section) => section.view === "chat")).toBe(false);
    expect(NAV_SECTIONS.some((section) => section.view === "workflows")).toBe(false);
  });

  it("keeps the Agents row and the page it reaches called the same thing", () => {
    // A row named one thing leading to a page headed another is the reader's
    // problem, not the code's. "Company > Company" said the word twice and
    // named nothing; both halves are Agents now.
    const company = NAV_SECTIONS.find((section) => section.label === "Company")!;
    expect(company.children?.[0].label).toBe("Agents");
    const source = readFileSync(
      resolve(dirname(fileURLToPath(import.meta.url)), "../../src/views/TeamView.tsx"),
      "utf8",
    );
    expect(source).toContain('title="Agents"');
    expect(source).not.toContain('title="Company"');
  });

  it("never puts two rows under one tour anchor", () => {
    // Anchors follow the address, so the child that lands on its section's own
    // address — Agents on `#/company` — would name itself what the section row
    // is already called. Two nodes answering one selector is worse than none:
    // a spec that clicked `nav-company` stops clicking anything and fails as a
    // strict-mode violation, which is how the Console E2E lane found this.
    const anchors: string[] = [];
    for (const section of NAV_SECTIONS) {
      anchors.push(`nav-${section.view}`);
      for (const child of section.children ?? []) {
        const a = childAnchor(section, child);
        if (a) anchors.push(a);
      }
    }
    expect(new Set(anchors).size, anchors.join(", ")).toBe(anchors.length);

    // Agents used to land on its section's own address (`#/company`) with no
    // sub, so its anchor would have been `nav-company` — the collision this
    // test is about, and why `childAnchor` returned nothing for it. It has its
    // own address now (`#/company/agents`, because a row reading "Agents" over
    // an address reading "company" was the mismatch the prefix work removed),
    // so it gets an anchor of its own and there is nothing to collide with.
    const company = NAV_SECTIONS.find((s) => s.view === "company")!;
    expect(childAnchor(company, company.children![0])).toBe("nav-agents");
    expect(childAnchor(company, company.children![1])).toBe("nav-ledgers");
  });

  it("gives every section a distinct view, so two rows can never light at once", () => {
    const views = NAV_SECTIONS.map((section) => section.view);
    expect(new Set(views).size).toBe(views.length);
  });
});

describe("which section an address belongs to", () => {
  it("claims the deep-link surfaces that have no row of their own", () => {
    // A task card is a card on Work's board; a teammate is a seat on the org
    // chart. Each keeps its section open rather than emptying the sidebar.
    expect(sectionOwning("tasks")?.label).toBe("Company");
    expect(sectionOwning("team")?.label).toBe("Company");
  });

  it("claims a section's children for that section", () => {
    for (const view of ["company", "ledgers", "workspace", "brain", "workflows", "finances"] as View[]) {
      expect(sectionOwning(view)?.label).toBe("Company");
    }
  });

  it("claims Company for Automations, and nothing for Room", () => {
    // `#/workflows` lights Company and draws its rail with Automations current.
    // `#/chat` belongs to no section: the conversation list is not a row.
    expect(sectionOwning("workflows")?.label).toBe("Company");
    expect(sectionOwning("chat")).toBeUndefined();
  });

  it("claims Connections for both of its sub-pages", () => {
    // Its children share the parent's view and differ only by hash segment, so
    // the section is claimed by the view and the child by the segment.
    expect(sectionOwning("connections")?.label).toBe("Connections");
  });

  it("claims nothing for the surfaces that are deliberately not in the nav", () => {
    // Settings and Feedback live in the sidebar's footer; Overview in the
    // window's title row; Observatory under Settings; Pages is direct-URL only
    // (#1171, #1172); `not-found` is nowhere by design. Approvals is absent
    // from this list only because it shares a page with `notifications`: the
    // two heads are asserted unowned together, below, rather than here.
    for (const view of [
      "settings",
      "feedback",
      "pages",
      "overview",
      "observatory",
      "not-found",
    ] as View[]) {
      expect(sectionOwning(view)).toBeUndefined();
    }
  });

  it("owns neither half of the Notifications page", () => {
    // Both heads render the same page, and it is reached from chrome rather
    // than from this column — so no section lights up for either. A section
    // that claimed one would light a row an operator did not come from.
    expect(sectionOwning("approvals" as View)).toBeUndefined();
    expect(sectionOwning("notifications" as View)).toBeUndefined();
  });
});

describe("which child row is open", () => {
  const company = NAV_SECTIONS.find((section) => section.label === "Company") as NavSection;
  const child = (label: string) => company.children!.find((c) => c.label === label)!;

  it("keeps a child lit across every sub-page of its own view", () => {
    // `#/ledgers/goals` is a declared list on the same Work surface.
    expect(childActive(company, child("Work"), "ledgers", "goals")).toBe(true);
    expect(childActive(company, child("Workspace"), "workspace", "node-7")).toBe(true);
    expect(childActive(company, child("Automations"), "workflows", "wf-1")).toBe(true);
  });

  it("lights exactly one child per address", () => {
    const lit = company.children!.filter((c) => childActive(company, c, "brain", null));
    expect(lit.map((c) => c.label)).toEqual(["Brain"]);
  });

  it("lights the child a deep-link surface belongs to", () => {
    expect(childActive(company, child("Work"), "tasks", "task-1")).toBe(true);
    expect(childActive(company, child("Agents"), "team", "agent-1")).toBe(true);
  });
});

describe("the rendered sidebar", () => {
  it("is the conversation list's slot and nothing else, on every section", () => {
    // The two labelled rows (Company, Connections) that sat under the list are
    // icon tabs on the floating sidebar's foot now (`SidebarSectionTabs`), so
    // the column paints no menu rows at all — only the list.
    for (const view of ["chat", "company", "connections", "workflows"] as View[]) {
      render(view);
      expect(container.querySelectorAll("[data-sidebar='menu-button']"), view).toHaveLength(0);
      expect(container.querySelectorAll("[data-testid='room-rail-slot']"), view).toHaveLength(1);
    }
  });

  it("lets the list take the column's height and scroll inside itself", () => {
    render("company");
    const groups = [...container.querySelectorAll("[data-sidebar='group']")];
    expect(groups).toHaveLength(1);
    expect(groups[0].className).toContain("min-h-0");
    expect(groups[0].className).toContain("flex-1");
    const slot = groups[0].querySelector("[data-testid='room-rail-slot']")!;
    expect(slot.className).toContain("overflow-y-auto");
    // Vertical padding on the scroller, not the group: padding outside it
    // clipped the scrolled rows against a hard edge.
    expect(groups[0].className).toContain("py-0");
    expect(slot.className).toContain("py-2");
  });

  it("draws one tab per section, Company then Connections", () => {
    render("chat");
    expect(tabs().map((el) => el.getAttribute("aria-label"))).toEqual(["Company", "Connections"]);
    expect(tabs().map((el) => el.dataset.tour)).toEqual(["nav-company", "nav-connections"]);
  });

  it("marks the section an address is in, and only that", () => {
    render("workspace");
    expect(litTabs()).toEqual(["Company"]);
    render("connections");
    expect(litTabs()).toEqual(["Connections"]);
  });

  it("lights Company, and nothing on Room, for the Automations address", () => {
    render("workflows");
    expect(litTabs()).toEqual(["Company"]);
    // And on the Room route no tab is lit: the list is not a section.
    render("chat");
    expect(litTabs()).toEqual([]);
  });

  it("renders one node per tour anchor, whichever section is open", () => {
    for (const view of ["chat", "company", "connections", "workflows"] as View[]) {
      render(view);
      const seen = [...container.querySelectorAll("[data-tour]")].map((el) =>
        el.getAttribute("data-tour"),
      );
      expect(new Set(seen).size, `${view}: ${seen.join(", ")}`).toBe(seen.length);
    }
  });

  it("puts no heading in the sidebar, at runtime and not only in source", () => {
    // The complement of `nav-rail-headings.test.ts`, which is a source guard and
    // cannot see what a portal or a child component contributes (issue #1392).
    render("company");
    expect(container.querySelectorAll("h1, h2, h3, h4, h5, h6")).toHaveLength(0);
  });
});
