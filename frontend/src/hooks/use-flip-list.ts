import { useCallback, useLayoutEffect, useRef } from "react";

import { usePrefersReducedMotion } from "./use-prefers-reduced-motion";

/** Options for {@link useFlipList}. */
export interface FlipListOptions {
  /** How long a row takes to slide to its new slot. Defaults to `--duration-slow`'s 260ms. */
  durationMs?: number;
  /**
   * Suppress animation without forgetting positions. Used to hold still while
   * a list is still hydrating, so a cold load does not play a storm of moves.
   * The first commit after this clears is also skipped: it is the one that
   * lands the final order, not a reorder the operator should watch.
   */
  disabled?: boolean;
}

/**
 * Slides list rows to their new slot when the list reorders (FLIP: First, Last,
 * Invert, Play).
 *
 * Rows are keyed, so React moves the DOM node rather than remounting it: focus
 * and hover survive a reorder, and the node is still the same element we
 * measured. On every commit we compare each row's `offsetTop` within its list
 * (see `slotTop`) with what it was after the previous commit and, when it
 * moved, play a Web Animations API
 * `translateY` from the old spot to the new one. `offsetTop` is used rather
 * than `getBoundingClientRect` because it ignores both transforms (a mid-flight
 * animation does not corrupt the measurement) and scroll (scrolling the sidebar
 * between commits is not a reorder).
 *
 * Skipped, while still recording positions: the first commit, `disabled`,
 * the preference for reduced motion (`index.css` only reaches CSS animations,
 * never Web Animations calls, so this hook asks itself), rows that are new to
 * the list, and rows whose slot did not change.
 *
 * `keys` should be the rendered order. Returns `ref(key)`, a stable per-key
 * callback ref to put on each row's outermost element.
 */
export function useFlipList<K extends string | number>(
  keys: readonly K[],
  { durationMs = 260, disabled = false }: FlipListOptions = {},
): (key: K) => (node: HTMLElement | null) => void {
  const reduced = usePrefersReducedMotion();
  const nodes = useRef(new Map<K, HTMLElement>());
  const tops = useRef(new Map<K, number>());
  const running = useRef(new Map<K, Animation>());
  const refs = useRef(new Map<K, (node: HTMLElement | null) => void>());
  const wasDisabled = useRef(true);

  const ref = useCallback((key: K) => {
    let cb = refs.current.get(key);
    if (!cb) {
      cb = (node) => {
        if (node) {
          nodes.current.set(key, node);
        } else {
          nodes.current.delete(key);
          tops.current.delete(key);
          running.current.get(key)?.cancel();
          running.current.delete(key);
          refs.current.delete(key);
        }
      };
      refs.current.set(key, cb);
    }
    return cb;
  }, []);

  // Deliberately no dependency array: row heights and slots change without the
  // key order changing (a badge wraps, the window resizes), and a stale
  // recorded position would turn the next reorder into a wrong-sized slide.
  // Reading `offsetTop` for a few dozen rows once per commit is cheap.
  useLayoutEffect(() => {
    const animate = !disabled && !reduced && !wasDisabled.current;
    const easing =
      typeof document !== "undefined"
        ? getComputedStyle(document.documentElement).getPropertyValue("--ease-standard").trim() || "ease-out"
        : "ease-out";
    const next = new Map<K, number>();
    for (const key of keys) {
      const el = nodes.current.get(key);
      if (!el) continue;
      const top = slotTop(el);
      next.set(key, top);
      const before = tops.current.get(key);
      // An unchanged slot is skipped before touching any running animation: a
      // commit that re-renders the same order mid-slide (a badge, a presence
      // tick) must let the slide finish rather than restart it at full length.
      if (!animate || before === undefined || before === top || typeof el.animate !== "function") continue;
      // A row already sliding restarts from where it visibly is, not from
      // where it was headed, so back-to-back reorders do not jump.
      const inFlight = running.current.get(key);
      let carried = 0;
      if (inFlight) {
        carried = currentTranslateY(el);
        inFlight.cancel();
        running.current.delete(key);
      }
      const delta = before - top + carried;
      if (delta === 0) continue;
      const anim = el.animate(
        [{ transform: `translateY(${delta}px)` }, { transform: "none" }],
        { duration: durationMs, easing },
      );
      running.current.set(key, anim);
      const done = () => {
        if (running.current.get(key) === anim) running.current.delete(key);
      };
      anim.onfinish = done;
      anim.oncancel = done;
    }
    tops.current = next;
    wasDisabled.current = disabled || reduced;
  });

  return ref;
}

/**
 * The row's top within its own list, not within the page.
 *
 * `offsetTop` is measured from the nearest positioned ancestor, which for a rail
 * section is far above the list: collapsing the Channels section, or a banner
 * appearing, moves every DM row by the same amount without reordering anything,
 * and read raw that played a slide on every row. Subtracting the list's own
 * `offsetTop` (both are measured from the same ancestor) leaves only the slot,
 * so a reorder is the one thing that moves it. When the list is itself the
 * positioned ancestor, `offsetTop` is already relative to it.
 */
function slotTop(el: HTMLElement): number {
  const list = el.parentElement;
  if (!list || list === el.offsetParent) return el.offsetTop;
  return el.offsetTop - list.offsetTop;
}

/** The row's current `translateY` in px, 0 when it has no transform (or no DOMMatrix). */
function currentTranslateY(el: HTMLElement): number {
  if (typeof DOMMatrixReadOnly === "undefined") return 0;
  const transform = getComputedStyle(el).transform;
  if (!transform || transform === "none") return 0;
  try {
    return new DOMMatrixReadOnly(transform).m42;
  } catch {
    return 0;
  }
}
