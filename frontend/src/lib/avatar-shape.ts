// The avatar shape — a per-browser appearance preference, like the theme and
// the accent preset (`accent-presets.ts`), set on Settings → Appearance.
//
// Every teammate face in the console (`TeammateAvatar`) draws its corners from
// one CSS variable, `--avatar-radius`, and this module picks its value by
// setting `data-avatar-shape` on `<html>`. Round is the default — the face
// reads as a person rather than as an app icon, which is what the conversation
// list and the transcript are full of. The rounded square the console drew
// before is the alternative, kept for anyone who preferred it.
//
// Same mechanics as the accent preset, on purpose: applied before first paint
// from `main.tsx`, persisted to `localStorage` (and surviving a refused write
// for the tab), synced across tabs by the `storage` event, read by React
// through `useSyncExternalStore`.

import { useSyncExternalStore } from "react";

/** The shapes an avatar can take, and how the Appearance card names them. */
export const AVATAR_SHAPES = [
  { id: "round", label: "Round" },
  { id: "rounded", label: "Rounded square" },
] as const;

/** One of {@link AVATAR_SHAPES}' ids. */
export type AvatarShape = (typeof AVATAR_SHAPES)[number]["id"];

/** Round, unless the operator chose otherwise. */
export const DEFAULT_AVATAR_SHAPE: AvatarShape = "round";

/** Where the choice is persisted — namespaced beside the accent preset's key. */
export const AVATAR_SHAPE_STORAGE_KEY = "oc.appearance.avatarShape";

function isAvatarShape(value: unknown): value is AvatarShape {
  return AVATAR_SHAPES.some((shape) => shape.id === value);
}

/** The stored shape, or the default when nothing (or nothing valid) is stored. */
export function readStoredAvatarShape(): AvatarShape {
  try {
    const stored = window.localStorage.getItem(AVATAR_SHAPE_STORAGE_KEY);
    return isAvatarShape(stored) ? stored : DEFAULT_AVATAR_SHAPE;
  } catch {
    // Storage disabled or the getter threw: the default is a safe answer.
    return DEFAULT_AVATAR_SHAPE;
  }
}

/**
 * Sets `data-avatar-shape` on `<html>`, which is what `index.css` keys
 * `--avatar-radius` on. The default removes the attribute rather than setting
 * it, so the CSS default is the one source of the round value.
 */
export function applyAvatarShape(shape: AvatarShape): void {
  const root = document.documentElement;
  if (shape === DEFAULT_AVATAR_SHAPE) delete root.dataset.avatarShape;
  else root.dataset.avatarShape = shape;
}

/** Apply whatever is stored — once, before first paint, from `main.tsx`. */
export function applyStoredAvatarShape(): void {
  applyAvatarShape(readStoredAvatarShape());
}

const listeners = new Set<() => void>();

function emit() {
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** The live `dataset`, not storage: a refused write still changed the page. */
function getSnapshot(): AvatarShape {
  const value = document.documentElement.dataset.avatarShape;
  return isAvatarShape(value) ? value : DEFAULT_AVATAR_SHAPE;
}

function getServerSnapshot(): AvatarShape {
  return DEFAULT_AVATAR_SHAPE;
}

/** The current avatar shape, re-rendering when it changes here or in another tab. */
export function useAvatarShape(): AvatarShape {
  return useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot);
}

/** Choose a shape: applied to the page at once, then persisted if storage allows. */
export function setAvatarShape(shape: AvatarShape): void {
  applyAvatarShape(shape);
  try {
    if (shape === DEFAULT_AVATAR_SHAPE) window.localStorage.removeItem(AVATAR_SHAPE_STORAGE_KEY);
    else window.localStorage.setItem(AVATAR_SHAPE_STORAGE_KEY, shape);
  } catch {
    // Storage refused; the choice still holds for this tab.
  }
  emit();
}

if (typeof window !== "undefined") {
  // Cross-tab sync, for this one key only — a theme or accent change must
  // never re-apply the avatar shape.
  window.addEventListener("storage", (event) => {
    if (event.key !== AVATAR_SHAPE_STORAGE_KEY) return;
    applyStoredAvatarShape();
    emit();
  });
}
