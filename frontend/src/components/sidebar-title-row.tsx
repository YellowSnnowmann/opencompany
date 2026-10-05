//! The floating sidebar's own title row — the overlay title bar's payload.
//!
//! `window-chrome.tsx` reserves the strip the traffic lights float over
//! (`WindowControlsInset`) and makes the rest of it draggable
//! (`WindowDragBar`); neither renders anything to look at. This is what sits
//! beside them: a pencil ("start a conversation") and a `+` ("add"), the two
//! actions that used to live in `ChannelRail`'s own header before the
//! sidebar's sections lost their captions (see that file's history). They
//! move here rather than back onto the rail because the rail is one of
//! several things this sidebar shows now — Company, Connections and
//! Notifications get the same floating card — and both actions apply
//! regardless of which of those is on screen.
//!
//! Mounted once, at the head of the sidebar card, by `app-shell.tsx`.

import { Plus, SquarePen } from "lucide-react";

import {
  WINDOW_CHROME_HEIGHT,
  WindowControlsInset,
  WindowDragBar,
  usesOverlayTitleBar,
} from "@/components/window-chrome";
import { TITLE_BAR_ICON_BUTTON } from "@/components/window-title-bar";

interface Props {
  /** Opens the picker for "start a conversation" — the pencil. */
  onComposeMessage: () => void;
  /** Opens the add-agent dialog — the `+`. */
  onAddAgent: () => void;
}

/**
 * The sidebar's title row: a {@link WindowDragBar} and a
 * {@link WindowControlsInset} under the hood, a pencil and a `+` on top.
 *
 * `usesOverlayTitleBar()` already folds in the desktop-runtime and
 * macOS-platform checks `WindowDragBar`/`WindowControlsInset` gate on
 * themselves, so this renders nothing anywhere the overlay title bar itself
 * does not apply — Windows, Linux, the web console, and the desktop app
 * before the traffic lights are known to be floating.
 *
 * The two buttons sit in a `relative z-30` layer **after** the drag band in
 * the DOM. `WindowDragBar` is `absolute … z-20` over the whole row on
 * purpose (its own doc: a band that has to swallow the press to start a
 * drag) — a button painted at the same stacking level as ordinary row
 * content would lose every click under it. Explicit `z-30`, not DOM order
 * alone, is what actually wins here: `WindowDragBar` carries its own
 * positive `z-index`, so nothing below that level can out-paint it no
 * matter where it sits in the markup.
 */
export function SidebarTitleRow({ onComposeMessage, onAddAgent }: Props) {
  if (!usesOverlayTitleBar()) return null;
  return (
    <div
      data-testid="sidebar-title-row"
      className="relative flex shrink-0 items-center"
      style={{ height: WINDOW_CHROME_HEIGHT }}
    >
      <WindowDragBar />
      <WindowControlsInset />
      <div className="relative z-30 flex min-w-0 flex-1 items-center justify-end gap-0.5 pr-1.5">
        <button
          type="button"
          onClick={onComposeMessage}
          aria-label="Start a conversation"
          title="Start a conversation"
          className={TITLE_BAR_ICON_BUTTON}
        >
          <SquarePen aria-hidden="true" className="size-4" />
        </button>
        <button
          type="button"
          onClick={onAddAgent}
          aria-label="Add"
          title="Add"
          className={TITLE_BAR_ICON_BUTTON}
        >
          <Plus aria-hidden="true" className="size-4" />
        </button>
      </div>
    </div>
  );
}
