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
// card — so those controls move into the card's foot: search and the places
// you jump to from anywhere as one row of glyphs, then who you are.

import type { ReactNode } from "react";

import { SidebarFooter } from "@/components/ui/sidebar";

/**
 * The card's foot: one row of icon tabs — Search, Company, Connections,
 * Notifications, Settings, Discord — over you. The card has no head: search
 * was its last occupant, and it is a glyph here now.
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
      className="gap-1 border-t border-sidebar-float-border px-2 py-2"
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
