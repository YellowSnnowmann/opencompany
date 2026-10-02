// Keeps the node inspector's subject on screen (issue #1231).
//
// Renders nothing. It is a child of `<ReactFlow>` purely so it can reach the
// canvas store — the same reason `WorkflowMiniMap` is one — and it exists as a
// component rather than as a hook called from `WorkflowsView` because
// `useReactFlow` needs a provider, and the only provider in this view is the
// one `<ReactFlow>` creates for its own children.
//
// The behaviour, and the two halves of it that matter equally:
//
//   opening — pan just far enough that the selected node clears the overlay.
//             "Just far enough" is the point: a node that is already visible
//             does not move the canvas at all, so the common click is still a
//             click and not an animation.
//
//   closing — pan back to exactly the viewport the operator was looking at,
//             which is the property `CanvasShell`'s convention comment already
//             promises ("closing the panel restores it instantly") and which a
//             pan on open would otherwise quietly break. Unless the operator
//             has panned or zoomed since, in which case their view is theirs.
//
// **Never read the live viewport while our own animation is running.** React
// Flow animates a viewport change through d3-zoom's smooth interpolation, which
// arcs through a LOWER ZOOM even when the start and the end zoom are identical
// — a 264px pan at scale 1.291 was measured passing through 1.271. Two things
// break if that arc is mistaken for the operator's view: closing the panel
// mid-animation reads a zoom that matches neither end and abandons the restore,
// and selecting a second node mid-animation computes its pan against the arc's
// zoom and then LANDS there, silently rescaling the canvas. So while a reveal
// is in flight this reasons about the viewport it asked for, not the one on
// screen. `settledAtRef` is when that stops being true.
//
// That in-flight window is also why the canvas has to TELL this component when
// the operator takes over. While a reveal is settling its own opinion of the
// viewport outranks the live one, so an operator who pans mid-animation would
// otherwise be invisible here — and closing the panel would put the canvas back
// over the top of what they just did. `onMove` on `<ReactFlow>` distinguishes
// the two for us: React Flow forwards d3-zoom's `sourceEvent`, which is null
// for a programmatic transition (ours) and a real pointer or wheel event for
// theirs. `operatorTookOver` is the seam; `WorkflowsView` wires it.
//
// See `node-reveal.ts` for the arithmetic and for why this pans rather than
// zooms or reflows.

import { useCallback, useEffect, useImperativeHandle, useRef } from "react";
import { useReactFlow, useStore } from "@xyflow/react";

import { usePrefersReducedMotion } from "@/hooks/use-prefers-reduced-motion";

import { NODE_H, NODE_W } from "./graph";
import { revealViewport, shouldRestore, type Viewport } from "./node-reveal";

/** How long the reveal and its undo take. Long enough to read as the canvas
 * moving rather than cutting, short enough not to be in the way of the panel
 * that is opening over it. */
const REVEAL_MS = 200;

/** Slack on top of the animation before the viewport counts as the operator's
 * again — one or two frames of transition teardown. */
const SETTLE_SLACK_MS = 80;

/** What `WorkflowsView` calls when the operator moves the canvas themselves. */
export interface RevealSelectedNodeHandle {
  operatorTookOver(): void;
}

export function RevealSelectedNode({
  nodeId,
  handleRef,
}: {
  /**
   * The node the inspector is open on, or `null` when it is closed.
   *
   * Deliberately the INSPECTOR's node rather than `selectedNodeId`: the copilot
   * shares this overlay slot and wins while it is open (issue #303), and a
   * conversation about the whole workflow has no one node it must not hide.
   */
  nodeId: string | null;
  /** Where this component publishes {@link RevealSelectedNodeHandle}. */
  handleRef?: React.RefObject<RevealSelectedNodeHandle | null>;
}) {
  const { getInternalNode, getViewport, setViewport } = useReactFlow();
  const paneWidth = useStore((s) => s.width);
  // React Flow moves the viewport from a d3 timer, which the global CSS
  // reduced-motion rule cannot shorten, so ask for the preference here. Reduced
  // motion makes the reveal a cut, not a removal.
  const duration = usePrefersReducedMotion() ? 0 : REVEAL_MS;

  /** The viewport to go back to — captured before the FIRST reveal of a run of
   * selections, so walking node to node with the panel open and then closing it
   * returns to where the walk started, not to the last node's stop. */
  const restoreRef = useRef<Viewport | null>(null);
  /** The viewport the canvas was last asked for — the destination of whatever
   * animation is running, and the truth about where it is heading. */
  const appliedRef = useRef<Viewport | null>(null);
  /** When that animation stops being ours. See the note at the top. */
  const settledAtRef = useRef(0);

  /** The operator has panned or zoomed: the canvas is theirs now, so drop every
   * claim on it. Nothing is restored on close, and the next reveal anchors on
   * the view they chose rather than on one from two selections ago. */
  const operatorTookOver = useCallback(() => {
    restoreRef.current = null;
    appliedRef.current = null;
    settledAtRef.current = 0;
  }, []);
  useImperativeHandle(handleRef, () => ({ operatorTookOver }), [operatorTookOver]);

  useEffect(() => {
    const applied = appliedRef.current;
    const inFlight = applied !== null && Date.now() < settledAtRef.current;
    /** What the canvas is showing, or heading to — see the note at the top. */
    const current = inFlight && applied ? applied : getViewport();

    if (!nodeId) {
      const from = restoreRef.current;
      restoreRef.current = null;
      appliedRef.current = null;
      settledAtRef.current = 0;
      // Nothing was moved, so there is nothing to put back — the overwhelmingly
      // common case, since most nodes are nowhere near the overlay.
      if (!from || !applied) return;
      // Their pan, their zoom, their view: leave it.
      if (!shouldRestore(current, from, applied)) return;
      // The restore is itself an animation, so the canvas is heading somewhere
      // again and the next reveal has to reason about `from`, not the arc.
      appliedRef.current = from;
      settledAtRef.current = Date.now() + duration + SETTLE_SLACK_MS;
      void setViewport(from, { duration });
      return;
    }

    // `getInternalNode`, not `getNode` — the internal node is where React Flow
    // keeps what it MEASURED, and `fitView` reads the same thing. `getNode`
    // hands back the object `layout` built, which carries only the nominal
    // `initialWidth`/`initialHeight` hints: a real node runs 180-204px wide
    // against that hint's 190 (issue #1230), so clearing the panel by the
    // nominal width left a real node up to ~14px * zoom still under it.
    const node = getInternalNode(nodeId);
    if (!node) return;
    const next = revealViewport({
      node: {
        x: node.internals.positionAbsolute.x,
        y: node.internals.positionAbsolute.y,
        // The hints are still the fallback: they cover the frame between a node
        // entering the graph and React Flow measuring it.
        width: node.measured.width ?? node.initialWidth ?? NODE_W,
        height: node.measured.height ?? node.initialHeight ?? NODE_H,
      },
      viewport: current,
      paneWidth,
    });
    if (!next) return;

    // Re-anchor if the operator has panned or zoomed since the last reveal:
    // "where they were before the inspector started moving things" is now their
    // view, not the one captured two nodes ago.
    const anchor = restoreRef.current;
    if (!anchor || !applied || !shouldRestore(current, anchor, applied)) {
      restoreRef.current = current;
    }
    appliedRef.current = next;
    settledAtRef.current = Date.now() + duration + SETTLE_SLACK_MS;
    void setViewport(next, { duration });
  }, [nodeId, paneWidth, duration, getInternalNode, getViewport, setViewport]);

  return null;
}
