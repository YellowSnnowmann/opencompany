import { useEffect, useState } from "react";

/**
 * Whether a CSS media query currently matches, kept live.
 *
 * Reads `false` where `matchMedia` is unavailable. Subscribing has two
 * spellings: `MediaQueryList` only became an `EventTarget` in Safari 14, and
 * before that (and in the WebKitGTK builds of that vintage, which Tauri v2's
 * floor still admits) it carries the deprecated `addListener` alone. Calling
 * `addEventListener` there throws out of the effect and takes the view down,
 * so prefer the modern spelling and fall back.
 */
export function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState(
    () => typeof window !== "undefined" && typeof window.matchMedia === "function"
      ? window.matchMedia(query).matches
      : false,
  );

  useEffect(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") return;
    const mql = window.matchMedia(query);
    const onChange = () => setMatches(mql.matches);
    onChange();
    if (typeof mql.addEventListener === "function") {
      mql.addEventListener("change", onChange);
      return () => mql.removeEventListener("change", onChange);
    }
    mql.addListener(onChange);
    return () => mql.removeListener(onChange);
  }, [query]);

  return matches;
}
