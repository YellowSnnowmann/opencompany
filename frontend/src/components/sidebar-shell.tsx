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
// card — so those controls move into the card's foot: search and the places
// you jump to from anywhere as one row of glyphs, then who you are.

import type { KeyboardEvent, PointerEvent as ReactPointerEvent, ReactNode } from "react";

import { SidebarFooter } from "@/components/ui/sidebar";

/**
 * The card's foot: one row of icon tabs — Search, Company, Connections,
 * Notifications, Settings, Discord — over you. The card has no head: search
 * was its last occupant, and it is a glyph here now.
 *
 * The tabs replaced two labelled rows (Company, Connections) above the foot
 * and a separate cluster of utility glyphs beside the profile: the
 * destinations an operator reaches from anywhere, read as one strip rather
 * than two lists in two styles. You sit under them.
 */
export function SidebarShellFooter({
  tabs,
  profile,
}: {
  tabs: ReactNode;
  profile: ReactNode;
}) {
  return (
    <SidebarFooter
      data-testid="sidebar-shell-footer"
      className="gap-1 border-t border-foreground/15 px-2 py-2"
    >
      <nav
        aria-label="Sections"
        data-testid="sidebar-shell-tabs"
        className="flex items-center justify-between gap-0.5"
      >
        {tabs}
      </nav>
      <div className="flex min-w-0 items-center empty:hidden">{profile}</div>
    </SidebarFooter>
  );
}

/**
 * The floating sidebar's right edge, as a resize handle.
 *
 * Drag it to set the width, use ←/→ (16px a step, Home/End for the bounds)
 * from the keyboard, double-click to reset. It is a `role="separator"` with the
 * width as its value, which is the ARIA pattern for a splitter. The width
 * itself is owned by the shell (`app-shell.tsx`), which feeds it to
 * `SidebarProvider` as `--sidebar-width` and persists it on release
 * (`lib/sidebar-width.ts`); this only reports where the edge was moved to.
 *
 * The column is flush to the window's left edge, so the width under a pointer
 * at `clientX` is `clientX` itself. `onResizing` lets the shell switch the sidebar's width
 * transition off for the drag, so the edge tracks the pointer instead of
 * easing behind it.
 */
export function SidebarResizeHandle({
  width,
  min,
  max,
  defaultWidth,
  onWidthChange,
  onCommit,
  onResizing,
}: {
  width: number;
  min: number;
  max: number;
  defaultWidth: number;
  /** Every move, live. */
  onWidthChange: (width: number) => void;
  /** Once, when a drag or a key press settles — the moment to persist. */
  onCommit: (width: number) => void;
  onResizing: (resizing: boolean) => void;
}) {
  const clamp = (w: number) => Math.round(Math.min(max, Math.max(min, w)));

  function onPointerDown(event: ReactPointerEvent<HTMLDivElement>) {
    if (event.button !== 0) return;
    event.preventDefault();
    const handle = event.currentTarget;
    handle.setPointerCapture(event.pointerId);
    onResizing(true);
    let latest = width;
    const move = (e: PointerEvent) => {
      latest = clamp(e.clientX - SIDEBAR_CARD_INSET);
      onWidthChange(latest);
    };
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      handle.removeEventListener("pointercancel", up);
      onResizing(false);
      onCommit(latest);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
    handle.addEventListener("pointercancel", up);
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    const step = event.shiftKey ? 64 : 16;
    const next =
      event.key === "ArrowLeft"
        ? width - step
        : event.key === "ArrowRight"
          ? width + step
          : event.key === "Home"
            ? min
            : event.key === "End"
              ? max
              : null;
    if (next === null) return;
    event.preventDefault();
    const w = clamp(next);
    onWidthChange(w);
    onCommit(w);
  }

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize sidebar"
      aria-valuenow={width}
      aria-valuemin={min}
      aria-valuemax={max}
      tabIndex={0}
      title="Drag to resize · double-click to reset"
      data-testid="sidebar-resize-handle"
      onPointerDown={onPointerDown}
      onKeyDown={onKeyDown}
      onDoubleClick={() => {
        onWidthChange(defaultWidth);
        onCommit(defaultWidth);
      }}
      // On the column's right border, 6px wide, invisible until hovered or
      // focused.
      className="group/resize absolute inset-y-0 right-0 z-20 w-1.5 cursor-col-resize touch-none outline-none"
    >
      <span className="absolute inset-y-0 right-0.5 w-0.5 rounded-full bg-transparent transition-colors group-hover/resize:bg-primary/40 group-focus-visible/resize:bg-primary/60 group-active/resize:bg-primary/60" />
    </div>
  );
}

/** How far the sidebar sits from the window's left edge (`app-shell.tsx`): flush. */
const SIDEBAR_CARD_INSET = 0;
