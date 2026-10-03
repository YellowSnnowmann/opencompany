// The head and foot of the floating sidebar: everything the window's title row
// used to carry, now inside the column it was always about.
//
// ## Why the title row went
//
// The console drew a full-width 52px row above both the sidebar and the
// content: the company switcher, the sidebar toggle, search, Overview, the
// notifications bell, Settings, Discord and your profile. The native window
// title bar (with the traffic lights) sits above it on the desktop, so the page
// started two bars down. Following OpenHuman's shell, the content now runs the
// full height of the window and the sidebar floats over its left edge as a
// card — so those controls move into the card: search at its head, and the
// places you jump to from anywhere, then who you are, at its foot.

import type { ReactNode } from "react";

import { SidebarFooter, SidebarHeader } from "@/components/ui/sidebar";

/**
 * The card's head: the search field, alone.
 *
 * The company switcher sat here first and lives on the Settings rail now: which
 * company a window is pointed at is set once and rarely, and the head of the
 * column is where you go many times a minute. The collapse toggle beside it is
 * gone too — the column is held open (`app-shell.tsx`).
 */
export function SidebarShellHeader({ search }: { search: ReactNode }) {
  return (
    <SidebarHeader data-testid="sidebar-shell-header" className="px-2 pt-2 pb-0">
      {search}
    </SidebarHeader>
  );
}

/**
 * The card's foot: one row of icon tabs — Company, Connections, Overview,
 * Notifications, Settings, Discord — over you.
 *
 * The tabs replaced two labelled rows (Company, Connections) above the foot
 * and a separate cluster of utility glyphs beside the profile: the
 * destinations an operator reaches from anywhere, read as one strip rather
 * than two lists in two styles. You sit under them.
 */
export function SidebarShellFooter({
  tabs,
  profile,
}: {
  tabs: ReactNode;
  profile: ReactNode;
}) {
  return (
    <SidebarFooter
      data-testid="sidebar-shell-footer"
      className="gap-1 border-t border-sidebar-border px-2 py-2"
    >
      <nav
        aria-label="Sections"
        data-testid="sidebar-shell-tabs"
        className="flex items-center justify-between gap-0.5"
      >
        {tabs}
      </nav>
      <div className="flex min-w-0 items-center empty:hidden">{profile}</div>
    </SidebarFooter>
  );
}
