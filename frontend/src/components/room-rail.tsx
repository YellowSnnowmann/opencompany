import { createContext, useContext, useMemo, useState, type ReactNode } from "react";

import { useSidebar } from "@/components/ui/sidebar";

/**
 * The seam that lets Room's channel list live in the app sidebar while its
 * data keeps living in `RoomView`.
 *
 * The sidebar and the content pane are siblings under `SidebarProvider`, so the
 * sidebar cannot reach into `RoomView`'s state and `RoomView` cannot render into
 * the sidebar's tree. Two ways out of that: lift the whole rail model — sections,
 * unread, mentions, the roster, the fold set, the two dialogs it opens — up into
 * the shell, or leave it exactly where it is and portal the rendered rail into a
 * slot the sidebar owns.
 *
 * This is the second. `RoomView` stays the one owner of the chat model (it is
 * 2,400 lines of it), the rail keeps rendering from that state on the same
 * render pass, and anything the rail opens still mounts inside `RoomView`'s
 * React tree even though the rail is painted in the sidebar — a portal moves
 * the DOM node, not the component tree, so context, events and focus
 * management all still resolve against Chat.
 *
 * Lifting the state instead would have meant an effect in `RoomView` writing the
 * model up to the shell and a second render of the whole console every time an
 * unread count changed.
 *
 * ## The rail is permanent, so `RoomView` is mounted permanently (issue #2130)
 *
 * The slot used to be `null` whenever Room was not the open section, and
 * `RoomView` rendered no rail at all then — the intended behaviour, while the
 * sidebar's middle region belonged to whichever section you were in. It belongs
 * to the channel list now, on every section, so that gap had to close, and there
 * were the same two ways out as before.
 *
 * It closed the same way, for the same reason: the shell keeps `RoomView`
 * mounted on every route and hands it `routeOpen`, which is false off Room. The
 * rail portals as it always did; the transcript, the header and the members pane
 * render only when `routeOpen`. Lifting the model into the shell was rejected
 * twice now on the same grounds, and the second time it would have been worse —
 * the console would re-render on every unread tick from *every* section rather
 * than only from Room.
 *
 * What it costs is honest and worth stating: ~2,400 lines of chat model stay
 * mounted while an operator is on Company or Flows, with its polls and its SSE
 * subscriptions live. That was always true *of the data* — the shell owns the
 * transcripts, the mention feed and the unread map precisely because `RoomView`
 * used to unmount — so what is newly resident is the view's own state and its
 * desks/roster reads, not the traffic. In exchange the channel list is never a
 * round trip away, and returning to Room no longer refetches what it just had.
 *
 * `routeOpen` also gates `chatPaneVisible`, which is what stops a mention being
 * marked read while the operator is somewhere else entirely — a mounted-but-off
 * -screen transcript must not clear anything, exactly as the phone's covering
 * sheet must not.
 *
 * The slot is still `null` in one case: the mobile sheet is closed and has
 * unmounted its contents. `RoomView` renders no rail then, which remains the
 * intended behaviour rather than a fallback.
 *
 * The sidebar's own density travels the same way. `RoomView` used to keep a
 * `collapsed` flag of its own in `localStorage` (`lib/chat-rail.ts`), because the
 * rail was its own column and nothing else governed it. It is a section of the
 * app sidebar now, so the sidebar's state IS the rail's state — one control, one
 * persisted preference (the sidebar's cookie), and no way for the two to
 * disagree.
 */
interface RoomRailSlot {
  /**
   * The sidebar's mount point — present on every section since #2130, and
   * `null` only while the mobile sheet is closed and has unmounted its contents.
   */
  element: HTMLElement | null;
  /** Called by the sidebar with its slot node, as a ref callback. */
  setElement: (element: HTMLElement | null) => void;
  /** The sidebar is a 3rem icon rail: the channel list renders compact. */
  collapsed: boolean;
  /** Put the sidebar back to a full column. */
  expand: () => void;
  /**
   * Whether the rail is currently covering the transcript — true only on a
   * phone, where the sidebar is a sheet over the whole screen. `RoomView` gates
   * mention-clearing on it: a mention that lands while the operator is looking
   * at the channel list must not be marked read behind their back.
   */
  covering: boolean;
  /**
   * Opens the rail from the chat header, or `undefined` where the rail is
   * already on screen beside the transcript. Absent rather than disabled, the
   * rule this codebase follows for a control that would do nothing.
   */
  reveal?: () => void;
  /**
   * Closes the sheet after the rail has been used to go somewhere — the other
   * half of `reveal`, and `undefined` at every width where the rail is a column
   * beside the transcript rather than a sheet over it.
   *
   * Every other destination in the sidebar already does this: `SidebarNavigation`
   * calls `setOpenMobile(false)` on each row it navigates. The channel list is a
   * section of that same sidebar now, so picking a channel has to behave like
   * picking a section — otherwise the sheet stays up covering the transcript it
   * just switched to, and the operator has to dismiss it by hand to see what
   * they chose (codex P2 review on #1987).
   */
  dismiss?: () => void;
}

const RoomRailSlotContext = createContext<RoomRailSlot | null>(null);

export function RoomRailSlotProvider({ children }: { children: ReactNode }) {
  const [element, setElement] = useState<HTMLElement | null>(null);
  const { state, isMobile, openMobile, setOpenMobile, toggleSidebar } = useSidebar();
  // `state` tracks the DESKTOP open flag; the sheet has its own. Reading it
  // unguarded would render the compact rail inside an open sheet whenever the
  // desktop sidebar happened to be collapsed — the same trap
  // `SidebarCollapseButton` documents.
  const collapsed = !isMobile && state === "collapsed";
  const covering = isMobile && openMobile;
  const value = useMemo<RoomRailSlot>(
    () => ({
      element,
      setElement,
      collapsed,
      expand: toggleSidebar,
      covering,
      reveal: isMobile ? () => setOpenMobile(true) : undefined,
      dismiss: isMobile ? () => setOpenMobile(false) : undefined,
    }),
    [element, collapsed, covering, isMobile, setOpenMobile, toggleSidebar],
  );
  return <RoomRailSlotContext.Provider value={value}>{children}</RoomRailSlotContext.Provider>;
}

/**
 * The Room slot, for the sidebar (which sets it) and for `RoomView` (which
 * portals into it).
 *
 * Returns a nulled slot outside the provider so a standalone `RoomView` — the
 * unit tests, a future embed — renders no rail rather than throwing.
 */
export function useRoomRailSlot(): RoomRailSlot {
  return useContext(RoomRailSlotContext) ?? NO_SLOT;
}

const NO_SLOT: RoomRailSlot = {
  element: null,
  setElement: () => {},
  collapsed: false,
  expand: () => {},
  covering: false,
};
