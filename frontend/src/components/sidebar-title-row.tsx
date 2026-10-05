//! The floating sidebar's own title row — the overlay title bar's payload on
//! macOS desktop, and the pencil/`+` actions' only home on every platform.
//!
//! `window-chrome.tsx` reserves the strip the traffic lights float over
//! (`WindowControlsInset`) and makes the rest of it draggable
//! (`WindowDragBar`); neither renders anything to look at, and both already
//! gate themselves on `usesOverlayTitleBar()`. The pencil ("start a
//! conversation") and the `+` ("add") beside them do **not** gate on it
//! (tinysweeper, high — `cross-platform-functionality`): these are the two
//! actions that used to live in `ChannelRail`'s own header before the
//! sidebar's sections lost their captions (see that file's history), and
//! that header was not macOS-overlay-only. Folding the overlay check into
//! this component's own early return, rather than leaving it to the two
//! window-chrome pieces that actually need it, would silently take
//! compose-message and add-agent away from the web console, Windows, and
//! Linux — every platform except the one this PR's title bar change happens
//! to be about.
//!
//! Mounted once, at the head of the sidebar card, by `app-shell.tsx`.

import { Plus, SquarePen } from "lucide-react";

import { WINDOW_CHROME_HEIGHT, WindowControlsInset, WindowDragBar } from "@/components/window-chrome";
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
 * Always renders the row and its two buttons — `WindowDragBar` and
 * {@link WindowControlsInset} are each already gated on
 * `usesOverlayTitleBar()` internally and simply render nothing anywhere the
 * overlay title bar does not apply, so the row becomes a plain 28px header
 * strip with two right-aligned buttons there instead of disappearing.
 * Gating the actions themselves on the same check — the bug this replaces —
 * would take them away from every platform except macOS desktop.
 *
 * The two buttons sit in a `relative z-30` layer **after** the drag band in
 * the DOM. `WindowDragBar` is `absolute … z-20` over the whole row on
 * purpose (its own doc: a band that has to swallow the press to start a
 * drag) — a button painted at the same stacking level as ordinary row
 * content would lose every click under it. Explicit `z-30`, not DOM order
 * alone, is what actually wins here: `WindowDragBar` carries its own
 * positive `z-index`, so nothing below that level can out-paint it no
 * matter where it sits in the markup.
 *
 * That `z-30` layer is `pointer-events-none`, with `pointer-events-auto` put
 * back on each button individually (tinysweeper, medium). The layer spans
 * every pixel right of the lights' inset, not just the two buttons — it is a
 * `flex-1` wrapper so `justify-end` can pack the buttons against the right
 * edge — and an ordinary `auto` layer there would win every hit test in its
 * bounding box against `WindowDragBar` underneath, for exactly the reason
 * the paragraph above explains the buttons need `z-30` to begin with. Left
 * at the default, the blank space between the inset and the buttons stops
 * the window from being draggable from most of its own title row.
 */
export function SidebarTitleRow({ onComposeMessage, onAddAgent }: Props) {
  return (
    <div
      data-testid="sidebar-title-row"
      className="relative flex shrink-0 items-center"
      style={{ height: WINDOW_CHROME_HEIGHT }}
    >
      <WindowDragBar />
      <WindowControlsInset />
      <div className="pointer-events-none relative z-30 flex min-w-0 flex-1 items-center justify-end gap-0.5 pr-1.5">
        <button
          type="button"
          onClick={onComposeMessage}
          aria-label="Start a conversation"
          title="Start a conversation"
          className={`pointer-events-auto ${TITLE_BAR_ICON_BUTTON}`}
        >
          <SquarePen aria-hidden="true" className="size-4" />
        </button>
        <button
          type="button"
          onClick={onAddAgent}
          aria-label="Add"
          title="Add"
          className={`pointer-events-auto ${TITLE_BAR_ICON_BUTTON}`}
        >
          <Plus aria-hidden="true" className="size-4" />
        </button>
      </div>
    </div>
  );
}
