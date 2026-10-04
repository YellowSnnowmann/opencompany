// Settings and Discord, as glyphs on the floating sidebar's foot.
//
// They lived in the window's title row — chrome holding the controls that are
// about the console rather than the page — until that row was removed and the
// sidebar became a floating card (`sidebar-shell.tsx`, after OpenHuman's
// shell). Settings is one of the foot's icon tabs now, beside Company,
// Connections, Overview and Notifications; Discord leaves the product
// altogether, so it trails your profile instead of sitting among the tabs.
//
// **Glyphs, not labels.** Each keeps `aria-label` and `title` — the whole of
// what a screen reader and a hovering pointer respectively get from an
// icon-only control — so only the pixels go. The file keeps its name, and the
// controls their `title-bar-*` test ids, so the e2e specs that find them by id
// still do.

import { Settings } from "lucide-react";

import { DiscordIcon } from "@/components/discord-icon";
import { TITLE_BAR_ICON_BUTTON } from "@/components/window-title-bar";
import { DISCORD_INVITE_URL } from "@/lib/links";
import type { View } from "@/lib/console-routes";
import { cn } from "@/lib/utils";

/**
 * Discord's brand blurple, lifted a step in dark mode so it clears the chrome
 * instead of sinking into it.
 *
 * Named tokens rather than raw hex — the colour is deliberately not ours, and
 * saying so in the token name is what stops it being "fixed" into the palette
 * later. Carried over from `sidebar-controls.tsx`, which had it for the same
 * reason and no longer draws this control.
 */
const DISCORD_BLURPLE = "text-(--brand-discord-on-light) dark:text-(--brand-discord-on-dark)";

/** The accessible names, stated once so a test and a button cannot drift. */
export const SETTINGS_LABEL = "Settings";
export const DISCORD_LABEL = "Join our Discord";

/**
 * The Settings tab. A tab on the floating sidebar's foot now
 * (`sidebar-shell.tsx`), and current on `#/settings` as well as on the
 * Feedback page it carries a rail row for.
 */
export function SettingsButton({
  view,
  onNavigate,
}: {
  /** The active view, so Settings can show as current. */
  view: View;
  onNavigate: (view: View) => void;
}) {
  return (
    <button
      type="button"
      data-testid="title-bar-settings"
      data-tour="nav-settings"
      onClick={() => onNavigate("settings")}
      aria-current={view === "settings" ? "page" : undefined}
      aria-label={SETTINGS_LABEL}
      title={SETTINGS_LABEL}
      className={TITLE_BAR_ICON_BUTTON}
    >
      <Settings aria-hidden="true" className="size-4" />
    </button>
  );
}

/**
 * The Discord invite. An anchor, not a button: it leaves the product, so it
 * has to behave like a link — middle-click, copy address, open in a new tab.
 * Beside your profile on the sidebar's foot, not among the tabs, because it is
 * not a place in the console.
 */
export function DiscordLink() {
  return (
    <a
      data-testid="title-bar-discord"
      href={DISCORD_INVITE_URL}
      target="_blank"
      rel="noreferrer"
      aria-label={DISCORD_LABEL}
      title={DISCORD_LABEL}
      className={cn(
        TITLE_BAR_ICON_BUTTON,
        DISCORD_BLURPLE,
        "hover:text-(--brand-discord-on-light) dark:hover:text-(--brand-discord-on-dark)",
      )}
    >
      <DiscordIcon className="size-4" />
    </a>
  );
}
