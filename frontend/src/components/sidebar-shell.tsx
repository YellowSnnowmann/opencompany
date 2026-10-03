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
// card — so those controls move into the card: what you are in and where you
// can jump at its head, who you are and the console's utilities at its foot.

import type { ReactNode } from "react";

import { SidebarFooter, SidebarHeader } from "@/components/ui/sidebar";

/**
 * The card's head: the search field beside the collapse toggle.
 *
 * The company switcher sat here first and lives on the Settings rail now: which
 * company a window is pointed at is set once and rarely, and the head of the
 * column is where you go many times a minute. On the collapsed icon rail the
 * two stack, the same width rule the channel list's compact rows follow.
 */
export function SidebarShellHeader({
  toggle,
  search,
}: {
  toggle: ReactNode;
  search: ReactNode;
}) {
  return (
    <SidebarHeader
      data-testid="sidebar-shell-header"
      className="flex-row items-center gap-1 px-2 pt-2 pb-1 group-data-[collapsible=icon]:flex-col"
    >
      <div className="min-w-0 flex-1 group-data-[collapsible=icon]:flex-none">{search}</div>
      {toggle}
    </SidebarHeader>
  );
}

/**
 * The card's foot: one row of icon tabs — Company, Connections, Overview,
 * Notifications, Settings — over you.
 *
 * The tabs replaced two labelled rows (Company, Connections) above the foot
 * and a separate cluster of utility glyphs beside the profile: five
 * destinations an operator reaches from anywhere, read as one strip rather
 * than two lists in two styles. You and the Discord link sit under them. On
 * the collapsed rail both rows turn into columns so every glyph keeps its 32px.
 */
export function SidebarShellFooter({
  tabs,
  profile,
  aside,
}: {
  tabs: ReactNode;
  profile: ReactNode;
  /** Trailing the profile row: the Discord link. */
  aside?: ReactNode;
}) {
  return (
    <SidebarFooter
      data-testid="sidebar-shell-footer"
      className="gap-1 border-t border-sidebar-border px-2 py-2"
    >
      <nav
        aria-label="Sections"
        data-testid="sidebar-shell-tabs"
        className="flex items-center justify-between gap-0.5 group-data-[collapsible=icon]:flex-col"
      >
        {tabs}
      </nav>
      <div className="flex items-center gap-1 group-data-[collapsible=icon]:flex-col">
        <div className="flex min-w-0 flex-1 items-center empty:hidden group-data-[collapsible=icon]:flex-none">
          {profile}
        </div>
        {aside}
      </div>
    </SidebarFooter>
  );
}
