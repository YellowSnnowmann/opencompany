/**
 * The bottom-anchoring predicate, shared by every scrolling transcript.
 *
 * Pure on purpose — no React, no document. It is the one part of the anchoring
 * machinery a unit test can reach, and it is also the condition a "jump to the
 * end" control's visibility is the negation of, so both readings come from one
 * definition instead of two that can drift.
 */

/**
 * How close to the bottom still counts as "parked at the bottom", in CSS
 * pixels. Sub-pixel layout and a fractional `clientHeight` mean the arithmetic
 * rarely lands on exactly zero, so a strict test would read a view that is
 * visibly at the bottom as scrolled away and stop following.
 */
export const BOTTOM_SLACK_PX = 32;

/** The three numbers a scroller reports about where it is parked. */
export interface ScrollMetrics {
  scrollHeight: number;
  scrollTop: number;
  clientHeight: number;
}

/** Whether a scroller at these metrics counts as parked at the bottom. */
export function isAtBottom({ scrollHeight, scrollTop, clientHeight }: ScrollMetrics): boolean {
  return scrollHeight - scrollTop - clientHeight <= BOTTOM_SLACK_PX;
}

/** What a scroll handler knows besides the metrics: see {@link readsAsFollowing}. */
export interface GlideState {
  /** A smooth scroll this pane started towards the bottom may still be travelling. */
  gliding: boolean;
  /** `scrollTop` at the previous scroll event (or when the glide started). */
  previousTop: number;
}

/**
 * Whether a scroll event leaves the pane following its newest row.
 *
 * Parked at the bottom always does. Short of it, the answer depends on who is
 * scrolling. A smooth scroll the pane started itself emits scroll events all the
 * way down, every one of them short of the bottom, and read as "the reader
 * scrolled away" they switched following off mid-travel. A row arriving in that
 * window (the reply landing while the pane still glides to the question just
 * sent) was then not followed, and the glide stopped where the old bottom had
 * been, one reply short, for the rest of the session. So while a glide is under
 * way, movement towards the bottom is the glide and keeps following; only
 * movement up, which a glide never makes, is the reader leaving.
 */
export function readsAsFollowing(metrics: ScrollMetrics, { gliding, previousTop }: GlideState): boolean {
  if (isAtBottom(metrics)) return true;
  return gliding && metrics.scrollTop >= previousTop;
}
