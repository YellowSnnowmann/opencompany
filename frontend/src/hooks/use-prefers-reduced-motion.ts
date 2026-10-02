import { useMediaQuery } from "./use-media-query";

const REDUCED_MOTION = "(prefers-reduced-motion: reduce)";

/**
 * Whether the viewer asked for reduced motion, kept live.
 *
 * One shared spelling for what four call sites each wrote by hand. Reads
 * `false` where `matchMedia` is unavailable (jsdom without a stub, an older
 * embedded webview): a pulse for a viewer who never asked for stillness is the
 * lesser failure.
 *
 * `index.css` honours the preference for CSS animations and transitions only.
 * Anything driven from script (the Web Animations API, a d3 timer) has to ask
 * for itself, and this is how.
 */
export function usePrefersReducedMotion(): boolean {
  return useMediaQuery(REDUCED_MOTION);
}
