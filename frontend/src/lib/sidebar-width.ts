// The floating sidebar's width — dragged by its right edge, remembered per
// browser like the other appearance preferences.
//
// The default is the 18rem `ui/sidebar.tsx` ships (the width the two-line
// conversation rows were laid out at); the bounds keep a row's name readable at
// the narrow end and stop the sidebar crowding the page at the wide one.

/** The sidebar's width when nothing is stored, in px — `SIDEBAR_WIDTH`'s 18rem. */
export const DEFAULT_SIDEBAR_WIDTH = 288;
/** Narrowest: below this a conversation's name truncates after a few letters. */
export const MIN_SIDEBAR_WIDTH = 240;
/** Widest: past this the sidebar takes more of the window than the page it serves. */
export const MAX_SIDEBAR_WIDTH = 480;

/** Where the width is persisted, beside the other appearance keys. */
export const SIDEBAR_WIDTH_STORAGE_KEY = "oc.appearance.sidebarWidth";

/** A width forced into bounds and onto whole pixels. */
export function clampSidebarWidth(width: number): number {
  if (!Number.isFinite(width)) return DEFAULT_SIDEBAR_WIDTH;
  return Math.round(Math.min(MAX_SIDEBAR_WIDTH, Math.max(MIN_SIDEBAR_WIDTH, width)));
}

/** The stored width, clamped, or the default when nothing valid is stored. */
export function readStoredSidebarWidth(): number {
  try {
    const stored = window.localStorage.getItem(SIDEBAR_WIDTH_STORAGE_KEY);
    return stored === null ? DEFAULT_SIDEBAR_WIDTH : clampSidebarWidth(Number(stored));
  } catch {
    return DEFAULT_SIDEBAR_WIDTH;
  }
}

/** Persist a width; the default clears the key. A refused write is ignored. */
export function storeSidebarWidth(width: number): void {
  try {
    if (width === DEFAULT_SIDEBAR_WIDTH) window.localStorage.removeItem(SIDEBAR_WIDTH_STORAGE_KEY);
    else window.localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, String(width));
  } catch {
    // Storage refused; the width still holds for this session.
  }
}
