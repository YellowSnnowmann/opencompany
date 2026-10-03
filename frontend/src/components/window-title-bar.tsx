// The shared shape of the console's icon buttons.
//
// This file drew the window's title row — a full-width 52px band above the
// sidebar and the content, carrying the company switcher, search, Overview,
// the notifications bell, Settings, Discord and your profile. That row is gone:
// the sidebar is a floating card over a full-height page (after OpenHuman's
// shell), its controls live in the card's head and foot (`sidebar-shell.tsx`),
// and the switcher moved to the Settings rail. What survives is the one
// decision every one of those glyphs still shares, kept under its old name so
// none of the importers had to move.

import { cn } from "@/lib/utils";

/**
 * The shape the console's icon-only buttons take — the floating sidebar's foot
 * tabs (Company, Connections, Notifications, Settings, Discord) and the small
 * page-level glyph buttons beside them.
 *
 * Exported so they are one decision rather than copies that drift. It is
 * `relative` because the notifications count is positioned against it, and it
 * has no fill at rest: it announces itself on hover and on focus.
 */
export const TITLE_BAR_ICON_BUTTON = cn(
  "relative inline-flex size-8 flex-none items-center justify-center rounded-lg",
  "text-muted-foreground transition",
  "hover:bg-rail-hover hover:text-foreground",
  "focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none",
  // The view you are already on. Keyed off `aria-current` so the appearance and
  // the announced state cannot disagree.
  "aria-[current=page]:bg-rail-selected aria-[current=page]:text-rail-selected-foreground",
);
