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
 * The card's head: the company switcher beside the collapse toggle, then the
 * search field.
 *
 * On the collapsed icon rail the switcher has no room to name a company, so it
 * is hidden and the toggle and the search glyph stack in its place — the same
 * width rule the channel list's compact rows follow.
 */
export function SidebarShellHeader({
  switcher,
  toggle,
  search,
}: {
  switcher: ReactNode;
  toggle: ReactNode;
  search: ReactNode;
}) {
  return (
    <SidebarHeader
      data-testid="sidebar-shell-header"
      className="gap-2 px-2 pt-2 pb-1 group-data-[collapsible=icon]:items-center"
    >
      <div className="flex items-center gap-1 group-data-[collapsible=icon]:flex-col">
        <div className="min-w-0 flex-1 group-data-[collapsible=icon]:hidden">{switcher}</div>
        {toggle}
      </div>
      {search}
    </SidebarHeader>
  );
}

/**
 * The card's foot: you, then the console's own destinations — Overview, the
 * notifications bell, Settings and Discord — as one row of glyphs.
 *
 * These are about the console rather than the company the list above
 * enumerates, which is why they sit under it rather than among it. On the
 * collapsed rail the row turns into a column so every glyph keeps its 32px.
 */
export function SidebarShellFooter({
  profile,
  actions,
}: {
  profile: ReactNode;
  actions: ReactNode;
}) {
  return (
    <SidebarFooter
      data-testid="sidebar-shell-footer"
      className="flex-row items-center gap-1 border-t border-sidebar-border px-2 py-2 group-data-[collapsible=icon]:flex-col"
    >
      <div className="flex min-w-0 flex-1 items-center empty:hidden group-data-[collapsible=icon]:flex-none">
        {profile}
      </div>
      <div
        data-testid="sidebar-shell-actions"
        className="flex items-center gap-0.5 group-data-[collapsible=icon]:flex-col"
      >
        {actions}
      </div>
    </SidebarFooter>
  );
}
