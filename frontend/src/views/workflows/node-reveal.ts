// The canvas arithmetic that keeps the node inspector from hiding its own
// subject (issue #1231).
//
// `NodeDetailPanel` mounts as the canvas's right overlay — `absolute right-3
// top-3 bottom-3`, `w-72 sm:w-80` — which is the right call for its LIFETIME
// (see `CanvasShell`'s convention comment: a transient focus, dismissed, with
// the canvas kept at full width underneath). It is the wrong call for its
// GEOMETRY: the panel is opened BY clicking a node, so an overlay that lands on
// the right-hand ~344px of the canvas routinely covers the very node it is
// describing. On `customer-escalation` at 1440px the last node sits at
// x=1159..1338 and the panel at x=1108..1428 — the node is entirely inside the
// panel's footprint, and because the panel's surface is translucent what the
// operator sees is a smeared ghost of it bleeding through, which reads as a
// rendering fault rather than as an overlay.
//
// The fix keeps the overlay and moves the GRAPH: on selection the viewport pans
// just far enough left that the node clears the panel, and on close it pans
// back to exactly where it was. Panning rather than zooming is deliberate — the
// zoom is the operator's (and, at load, `fitView`'s within the #1261 `minZoom`
// floor), and a fix that silently rescales the graph would be a second surprise
// on top of the one it is fixing.
//
// **What a pan costs, and why it is still the right trade.** When the graph is
// wider than the canvas minus the panel — `ticket_pipeline` at 1440px measures
// 1022px of graph against 1224px of canvas, and the panel wants 344 of those,
// so the reveal's 244px pan runs the first node 142px past the left edge — no
// viewport at that zoom can show every node, so revealing one on the right
// pushes the leftmost off. The two alternatives both cost more:
//
//   re-fit with asymmetric padding — `fitView({ padding: { right: 344 } })`
//     keeps every node on screen, but rescales the WHOLE graph on every
//     selection and again on every close. A lot of motion for a click, and on a
//     long pipeline it shrinks the node labels toward illegibility, bounded
//     only by #1261's 0.1 floor.
//
//   shrink the canvas — make the slot in-flow, as the rails are at `xl`. That
//     is a reflow of everything rather than an animation of one thing, and
//     `CanvasShell`'s arithmetic already rules it out below `xl`: two rails
//     plus the app's 240px nav is most of a laptop window before the canvas
//     enters into it.
//
// A pan moves the least, keeps the operator's zoom, and what it pushes off the
// left is still reachable — it is a pan away, and the panel is transient. What
// the overlay covered was reachable only by closing the panel.
//
// The math is pure and lives here so it can be asserted directly. What broke is
// arithmetic about two rectangles, not a React tree.

/** Viewport as React Flow models it: a translation in screen pixels plus a scale. */
export interface Viewport {
  x: number;
  y: number;
  zoom: number;
}

/** A laid-out node's box in GRAPH coordinates. */
export interface NodeBox {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * The inspector's width at `sm` and above (`sm:w-80` — 20rem).
 *
 * Below `sm` the panel is `w-72` (288px), so reserving this over-reserves by
 * 32px there. Deliberate: the reservation is a FLOOR on clearance, not a
 * measurement of the panel, and 32px of extra daylight between a node and the
 * panel edge is not a defect. Reading the panel's real width instead would mean
 * this module depended on a DOM node that mounts in the same commit as the pan.
 */
export const INSPECTOR_PANEL_W = 320;

/** The panel's inset from the canvas's right edge (`right-3` — 0.75rem). */
export const INSPECTOR_INSET = 12;

/** Daylight left between the node's right edge and the panel's left edge, so
 * "cleared" reads as cleared rather than as flush against the seam. */
export const INSPECTOR_GAP = 12;

/**
 * Screen pixels at the canvas's right edge that the inspector owns.
 *
 * The x below which a node's right edge has to sit to be fully visible:
 * `paneWidth - INSPECTOR_RESERVED`.
 */
export const INSPECTOR_RESERVED =
  INSPECTOR_PANEL_W + INSPECTOR_INSET + INSPECTOR_GAP;

/** The left margin a node is never pushed past when clearing the panel. */
const LEFT_MARGIN = 12;

/** Below this, a shift is not worth an animation. */
const MIN_SHIFT = 0.5;

/**
 * The viewport that brings `node` out from under the inspector, or `null` when
 * it is already clear and the canvas should not move at all.
 *
 * Horizontal only, and only ever a pan: the overlay spans the canvas's full
 * height (`top-3 bottom-3`), so there is no vertical position that helps, and
 * `zoom` is passed through untouched so this composes with #1261's `minZoom`
 * floor and with whatever the operator has zoomed to.
 *
 * When the node is too wide for the strip left of the panel — only reachable at
 * a zoom where a 190px node fills most of the canvas — the shift is clamped so
 * the node's LEFT edge stays on screen. Half a visible node beats a node pushed
 * off the other side.
 */
export function revealViewport({
  node,
  viewport,
  paneWidth,
  reserved = INSPECTOR_RESERVED,
}: {
  node: NodeBox;
  viewport: Viewport;
  paneWidth: number;
  /** Overridable for tests and for a future second overlay of another width. */
  reserved?: number;
}): Viewport | null {
  if (!(paneWidth > 0) || !(viewport.zoom > 0)) return null;

  const left = node.x * viewport.zoom + viewport.x;
  const right = (node.x + node.width) * viewport.zoom + viewport.x;
  const limit = paneWidth - reserved;

  // Already clear of the panel: the canvas must not twitch. Most clicks on most
  // graphs land here, and a pan on every one of them would be its own bug.
  if (right <= limit) return null;

  // Negative — the graph slides left so the node comes out from under the panel.
  let dx = limit - right;
  // …but never so far that the node's own left edge leaves the canvas.
  dx = Math.max(dx, LEFT_MARGIN - left);
  if (dx > -MIN_SHIFT) return null;

  return { x: viewport.x + dx, y: viewport.y, zoom: viewport.zoom };
}

/**
 * Whether two viewports are the same to within a pixel.
 *
 * Used to tell "the canvas is still where the reveal put it" from "the operator
 * has panned or zoomed since", which is what decides whether closing the panel
 * restores the previous view or leaves the operator's own view alone. Yanking
 * the canvas back from under someone who moved it deliberately would be worse
 * than not restoring at all.
 */
export function sameViewport(
  a: Viewport,
  b: Viewport,
  epsilon = 1,
  zoomEpsilon = 0.001,
): boolean {
  return (
    Math.abs(a.x - b.x) <= epsilon &&
    Math.abs(a.y - b.y) <= epsilon &&
    Math.abs(a.zoom - b.zoom) <= zoomEpsilon
  );
}

/**
 * Whether closing the inspector should pan the canvas back to `from`.
 *
 * Yes when the canvas is still where the reveal put it (`to`) — and also when
 * it is anywhere ALONG the pan from `from` to `to`, which is the case a plain
 * equality check gets wrong: the reveal is animated, so an operator who clicks
 * a node and closes the panel a moment later lands mid-flight, and "not exactly
 * at the target" would strand the canvas somewhere it was never asked to be.
 * The path is a pure horizontal translation, so `y` and `zoom` still have to
 * match exactly; a pan or a zoom of the operator's own leaves the path and
 * their view is theirs to keep.
 */
export function shouldRestore(
  current: Viewport,
  from: Viewport,
  to: Viewport,
  epsilon = 1,
  zoomEpsilon = 0.001,
): boolean {
  if (sameViewport(current, to, epsilon, zoomEpsilon)) return true;
  if (Math.abs(current.y - to.y) > epsilon) return false;
  if (Math.abs(current.zoom - to.zoom) > zoomEpsilon) return false;
  const lo = Math.min(from.x, to.x) - epsilon;
  const hi = Math.max(from.x, to.x) + epsilon;
  return current.x >= lo && current.x <= hi;
}
