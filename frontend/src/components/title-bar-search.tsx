// The search control: a glyph among the floating sidebar's foot tabs.
//
// A *trigger*, not a field: pressing it (or ⌘K) opens `SearchDialog`, which
// owns the actual input. One search box, in a modal, is what makes the results
// list possible at all — a dropdown hanging off an inline input has nowhere to
// put four groups of results.
//
// It was a capped field in the middle of the window's title row, then a
// full-width field at the head of the sidebar card. Since the palette opens on
// ⌘K from anywhere, the field earned no screen space of its own; a glyph in the
// tab row beside Company, Connections, Notifications and Settings is enough to
// say it exists, and the shortcut is in its tooltip.

import { useEffect, useState } from "react";
import { Search } from "lucide-react";

import type { OpenCompanyClient } from "@/api/client";
import { TITLE_BAR_ICON_BUTTON } from "@/components/window-title-bar";
import { isAppleKeyboard } from "@/connections/HostsContext";
import { SearchDialog } from "@/search/SearchDialog";

/** What the control is for — its accessible name and the start of its tooltip. */
const SEARCH_LABEL = "Search";

/** Opens the console-wide search palette, from its glyph or from ⌘K / Ctrl+K. */
export function TitleBarSearch({
  client,
  company,
}: {
  client: OpenCompanyClient;
  company: string | null;
}) {
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

  const shortcut = isAppleKeyboard() ? "⌘K" : "Ctrl K";
  return (
    <>
      <button
        type="button"
        onClick={() => setOpen(true)}
        data-testid="title-bar-search"
        aria-label={SEARCH_LABEL}
        aria-keyshortcuts="Meta+K Control+K"
        title={`${SEARCH_LABEL} (${shortcut})`}
        className={TITLE_BAR_ICON_BUTTON}
      >
        <Search aria-hidden="true" className="size-4" />
      </button>
      <SearchDialog client={client} company={company} open={open} onOpenChange={setOpen} />
    </>
  );
}
