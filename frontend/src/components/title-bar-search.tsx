// The search control in the middle of the window's title row.
//
// A *trigger*, not a field: clicking it (or pressing ⌘K) opens
// `SearchDialog`, which owns the actual input. One search box, in a modal, is
// what makes the results list possible at all — a dropdown hanging off a
// title-row input has nowhere to put four groups of results.
//
// **The button is capped, the wrapper is not.** The wrapper stays the row's one
// elastic member because it is also the only thing left to grab the window by
// across the middle: it carries `data-tauri-drag-region`, the button
// deliberately does not, so the band stays draggable while the control keeps
// its clicks. Capping the button rather than the wrapper is what lets the
// control be a sensible width without giving that up.

import { useEffect, useState } from "react";
import { Search } from "lucide-react";

import type { OpenCompanyClient } from "@/api/client";
import { SearchDialog } from "@/search/SearchDialog";
import { isAppleKeyboard } from "@/connections/HostsContext";

/** What the field is for. */
const SEARCH_PLACEHOLDER = "Search";

export function TitleBarSearch({
  client,
  company,
  variant = "titlebar",
}: {
  client: OpenCompanyClient;
  company: string | null;
  /**
   * `sidebar` is the field at the head of the floating sidebar: the column's
   * full width, and a bare glyph when the column is collapsed to its icon rail.
   * `titlebar` is the capped, centred field the old window title row drew.
   */
  variant?: "titlebar" | "sidebar";
}) {
  const inSidebar = variant === "sidebar";
  const [open, setOpen] = useState(false);

  // ⌘K / Ctrl+K, the shortcut a palette is reached by in every tool this
  // console sits beside. `⌘1`–`⌘9` belong to the host switcher
  // (`HostsContext`), and this takes none of them.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "k" && event.key !== "K") return;
      if (!event.metaKey && !event.ctrlKey) return;
      event.preventDefault();
      setOpen((was) => !was);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div
      data-tauri-drag-region={inSidebar ? undefined : true}
      // Tighter gutters below `sm`: on a 390px phone this is what is left once
      // the fixed controls are placed, and 24px of padding was most of it.
      className={
        inSidebar
          ? "flex w-full min-w-0 items-center group-data-[collapsible=icon]:justify-center"
          : "flex min-w-0 flex-1 items-center justify-center self-stretch px-3 max-sm:px-1"
      }
    >
      <button
        type="button"
        onClick={() => setOpen(true)}
        data-testid="title-bar-search"
        aria-label={SEARCH_PLACEHOLDER}
        aria-keyshortcuts="Meta+K Control+K"
        className={
          inSidebar
            ? "flex h-9 w-full min-w-0 items-center gap-2 overflow-hidden rounded-lg border border-sidebar-border " +
              "bg-background/70 pr-2 pl-3 text-sm text-muted-foreground hover:bg-sidebar-accent " +
              "focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none " +
              "group-data-[collapsible=icon]:size-8 group-data-[collapsible=icon]:justify-center " +
              "group-data-[collapsible=icon]:border-0 group-data-[collapsible=icon]:bg-transparent group-data-[collapsible=icon]:p-0"
            : // Capped, not elastic: a search control that grows to fill a 1440px
          // window reads as a text field somebody stretched by accident.
          // `min-w-0` + `overflow-hidden` so a squeezed row clips the label
          // inside the control instead of painting it over the glyphs beside it.
          "flex h-9 w-full min-w-0 max-w-sm items-center gap-2 overflow-hidden rounded-lg border border-chrome-border " +
          "bg-background pr-2 pl-3 text-sm text-muted-foreground max-sm:justify-center max-sm:px-0 " +
          "hover:bg-accent/40 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
        }
      >
        <Search aria-hidden="true" className="size-4 shrink-0" />
        <span className="min-w-0 flex-1 truncate text-left max-sm:hidden group-data-[collapsible=icon]:hidden">{SEARCH_PLACEHOLDER}</span>
        <kbd className="hidden shrink-0 rounded border border-chrome-border px-1.5 py-0.5 font-sans text-2xs sm:inline group-data-[collapsible=icon]:hidden">
          {isAppleKeyboard() ? "⌘K" : "Ctrl K"}
        </kbd>
      </button>
      <SearchDialog client={client} company={company} open={open} onOpenChange={setOpen} />
    </div>
  );
}
