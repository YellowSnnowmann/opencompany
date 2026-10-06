//! The floating sidebar's own title row — the overlay title bar's payload.
//!
//! `window-chrome.tsx` reserves the strip the traffic lights float over
//! (`WindowControlsInset`) and makes the rest of it draggable
//! (`WindowDragBar`); neither renders anything to look at. This is what sits
//! beside them: a pencil ("start a conversation") and a `+` ("add"), the two
//! actions that used to live in `ChannelRail`'s own header before the
//! sidebar's sections lost their captions (see that file's history) — and
//! that caption/menu pair was later retired outright, not merely hidden; two
//! e2e specs pin its absence everywhere this row does not apply:
//! `sidebar-conversations-layout.spec.ts`'s "renders no captions and no
//! new-conversation doors" (no button named "Start a conversation" anywhere
//! in the sidebar on the web) and `connections-authority.spec.ts`'s "a member
//! sees what is connected but is offered nothing that changes it" (no button
//! named "Add", for a member viewing Settings on any platform). Rendering
//! these two buttons unconditionally — tried once, reverted — breaks both:
//! this row is mounted at the `AppShell` level, so it would put "Start a
//! conversation" and "Add" in front of every page and every role, not only
//! the macOS title bar it was built to fill. They move here, scoped to the
//! overlay title bar specifically, rather than back onto the rail or loose
//! on every platform, because the rail is one of several things this sidebar
//! shows now — Company, Connections and Notifications get the same floating
//! card — and the only space this PR actually has to fill is the one beside
//! the floating traffic lights.
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";

interface Props {
  /** "Start a conversation in a channel", from the pencil's menu. */
  onStartChannelConversation: () => void;
  /** "Start a conversation with the agent", from the pencil's menu. */
  onComposeMessage: () => void;
  /** "Create a new channel", from the `+` menu. */
  onCreateChannel: () => void;
  /** "Create a new agent", from the `+` menu. */
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
 * before the traffic lights are known to be floating. See the module doc
 * for why the buttons share that gate rather than rendering everywhere: the
 * two e2e specs that pin their absence off this one platform.
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
 *
 * `TITLE_BAR_ICON_BUTTON`'s own `size-8` (32px) is 4px taller than this
 * row's 28px `WINDOW_CHROME_HEIGHT` (tinysweeper, medium — the buttons
 * centre past the row and clip into whatever sits below it). Overridden to
 * `size-7` (28px exactly) here only, through `cn`'s tailwind-merge so the
 * later class wins rather than leaving both in the string for the stylesheet
 * to pick between — the shared constant stays `size-8` for every other title
 * bar context, where the row itself is taller.
 */
export function SidebarTitleRow({
  onStartChannelConversation,
  onComposeMessage,
  onCreateChannel,
  onAddAgent,
}: Props) {
  if (!usesOverlayTitleBar()) return null;
  return (
    <div
      data-testid="sidebar-title-row"
      className="relative flex shrink-0 items-center"
      style={{ height: WINDOW_CHROME_HEIGHT }}
    >
      <WindowDragBar />
      <WindowControlsInset />
      <div className="pointer-events-none relative z-30 flex min-w-0 flex-1 items-center justify-end gap-0.5 pr-1.5">
        {/* Pencil: who to talk to, not what to make — a channel already in
            the sidebar, or an agent not yet DM'd. Same two-item split
            PR #2545 built into `ChannelRail`'s own compose door, just
            relocated up here with it. */}
        <DropdownMenu>
          <DropdownMenuTrigger
            aria-label="Start a conversation"
            title="Start a conversation"
            className={cn(TITLE_BAR_ICON_BUTTON, "pointer-events-auto size-7")}
          >
            <SquarePen aria-hidden="true" className="size-4" />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-auto min-w-56">
            <DropdownMenuItem onClick={onStartChannelConversation}>
              Start a conversation in a channel
            </DropdownMenuItem>
            <DropdownMenuItem onClick={onComposeMessage}>
              Start a conversation with the agent
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        {/* `+`: make a new one — same split. */}
        <DropdownMenu>
          <DropdownMenuTrigger
            aria-label="Add"
            title="Add"
            className={cn(TITLE_BAR_ICON_BUTTON, "pointer-events-auto size-7")}
          >
            <Plus aria-hidden="true" className="size-4" />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-auto min-w-48">
            <DropdownMenuItem onClick={onCreateChannel}>Create a new channel</DropdownMenuItem>
            <DropdownMenuItem onClick={onAddAgent}>Create a new agent</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </div>
  );
}
