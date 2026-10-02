// The workflow DETAIL view's column layout — and the convention for where a
// panel mounts (issue #1107, extended by #1205).
//
// Scope first, because it is load-bearing since issue #1110: this is the shell
// for `#/workflows/<id>`, never for `#/workflows`. The index is a list of
// workflows, so it has no single run to show history for and no single graph to
// inspect — every slot below is per-workflow chrome. `WorkflowsView` therefore
// renders this on the detail side of its one branch and a plain column on the
// index side, which is what makes "no run chrome on the index" structural
// rather than a rule each panel has to remember.
//
// Before #1107, every panel the view grew was appended to one vertical stack
// under the canvas, and each one ate the dimension a workflow graph needs most.
// Run history was the worst case: tall rows in a `max-h-72` horizontal strip,
// showing two runs at a time while spending the full width of the screen. #1205
// found the run-result drawer in the same state and gave it the same fix.
//
// The detail view now has three slots — two rails and one overlay — and each
// is a decision about the panel's LIFETIME, not about where there happens to be
// room:
//
//   leftRail  — a floating overlay at `xl`, a strip below the canvas beneath
//               it. It was in-flow and shrank the canvas; with the app's 240px
//               sidebar and Company's 240px section rail (Automations is a
//               Company page now) beside it, that left a 626px canvas at 1440px
//               — under the 640px floor `workflow-run-history-rail.spec.ts`
//               defends, and too narrow for a short graph to fit at
//               `LEGIBLE_FIT_ZOOM` (`graph.ts`). So at `xl` it is `absolute
//               left-3 top-3 bottom-3 z-10` against the SHELL, mirroring the
//               right overlay below: the canvas keeps its full width and the
//               rail covers it, dismissed with its own close control. Run
//               history is the only occupant. Single occupancy.
//
//               The canvas's zoom controls sit bottom-left, which is exactly
//               where this lands, so `WorkflowsView` nudges them clear of it
//               while it is open.
//
//   rightRail — a floating overlay at `xl`, mirrored from `leftRail`: `absolute
//               right-3 top-3 bottom-3 z-10` against the shell, a strip below
//               the canvas beneath it. Issue #1205's answer for
//               `RunResultPanel` and `RunFailurePanel`: a receipt for something
//               that just happened, dismissed when read, but tall and
//               vertically stacking. It was in-flow too, and with Company's
//               section rail beside the canvas it left 626px — under the 640px
//               floor — so it floats and the canvas keeps its width. It is
//               rendered after the canvas, so where it meets the right overlay
//               below (copilot, inspector) it sits on top of it until
//               dismissed. Single occupancy; the two panels are mutually
//               exclusive by construction in `WorkflowsView` (a run either
//               produces a result or a failure, never both).
//
//   right overlay — a floating overlay, mounted `absolute right-3 top-3
//               bottom-3 z-10` INSIDE the canvas (see `CopilotPanel` and
//               `NodeDetailPanel`, unchanged by #1205). A transient focus that
//               covers the graph and is dismissed: the canvas keeps its full
//               width underneath, so closing the panel restores it instantly.
//               Single occupancy, arbitrated by `WorkflowsView` with a
//               ternary — opening the copilot clears the selected node.
//
//               Issue #1231 drew the line this slot was missing. "Covers the
//               graph" is a fair trade for the copilot, which is a conversation
//               about the workflow as a whole. It is not a fair trade for the
//               node inspector, whose entire subject is one node and which is
//               opened BY clicking that node — an overlay on the right ~344px
//               routinely landed squarely on it. The slot's convention is
//               unchanged; what changed is that the inspector now brings its
//               subject out from under itself, by panning the canvas rather
//               than by shrinking it (`RevealSelectedNode`, `node-reveal.ts`).
//               A panel that mounts here and describes ONE thing on the canvas
//               owes the operator the same.
//
// **Where the overlays meet.** All three floating panels sit over the canvas's
// edges now (`leftRail` left; `rightRail` and the right overlay both right), so
// none of them costs the canvas width. `rightRail` and the right overlay share
// an edge: `rightRail` is rendered after the canvas and so paints over the
// copilot or inspector until it is dismissed, which then reappears underneath.
// It is as wide as the copilot (`w-96`), the widest of them, so nothing peeks
// out beside it.
// It is as wide as the copilot (`w-96`), the widest of them, so nothing peeks
// out beside it.
// The left rail and the right panels only meet on a canvas narrower than their
// two widths, which `xl` (≥1280px) is wide enough to avoid.
//
// Below `xl` each rail falls back to the bottom strip it has always been,
// stacking canvas → leftRail's strip → rightRail's strip, which never competes
// with the overlay slot.

import type { ReactNode } from "react";

export function CanvasShell({
  leftRail,
  rightRail,
  children,
}: {
  /**
   * The left rail, or nothing. Rendered AFTER the canvas in the DOM and moved
   * left with `order` at `xl`: keeping the canvas first means the narrow
   * layout's reading order is unchanged, and the rail names itself as a
   * landmark so assistive tech can reach it directly either way.
   */
  leftRail?: ReactNode;
  /**
   * The right rail, or nothing (issue #1205). Rendered AFTER the canvas and
   * the left rail in the DOM, with NO `order` override: at `xl` (flex-row)
   * that natural position already reads as "on the right" (canvas, then the
   * reordered-first left rail, then this); below `xl` (flex-col) it naturally
   * stacks last, under both the canvas and the left rail's own strip.
   */
  rightRail?: ReactNode;
  /** The canvas. Positioned, because the right overlay slot mounts inside it. */
  children: ReactNode;
}) {
  return (
    <div className="relative flex min-h-0 flex-1 flex-col xl:flex-row">
      <div className="relative min-h-0 flex-1">{children}</div>
      {leftRail && (
        <div className="shrink-0 xl:absolute xl:top-3 xl:bottom-3 xl:left-3 xl:z-10 xl:w-80 xl:overflow-hidden xl:rounded-xl xl:border xl:bg-card/95 xl:shadow-lg xl:backdrop-blur">
          {leftRail}
        </div>
      )}
      {rightRail && (
        <div className="shrink-0 xl:absolute xl:top-3 xl:right-3 xl:bottom-3 xl:z-10 xl:w-96 xl:overflow-hidden xl:rounded-xl xl:border xl:bg-card/95 xl:shadow-lg xl:backdrop-blur">
          {rightRail}
        </div>
      )}
    </div>
  );
}
