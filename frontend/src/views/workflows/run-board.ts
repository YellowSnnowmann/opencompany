// The label and badge tone for each board action a run can report.

import type { WorkflowBoardAction } from "@/api/workflows";

export interface BoardActionMeta {
  label: string;
  tone: string;
  failed: boolean;
}

const FAILED_TONE = "border-status-failed/40 bg-status-failed-soft";
const NEUTRAL_TONE = "border-border bg-muted/60";

export const BOARD_ACTION: Record<WorkflowBoardAction, BoardActionMeta> = {
  spawned: { label: "Opened", tone: NEUTRAL_TONE, failed: false },
  assigned: { label: "Assigned", tone: NEUTRAL_TONE, failed: false },
  spawnFailed: { label: "Open failed", tone: FAILED_TONE, failed: true },
  assignFailed: { label: "Assign failed", tone: FAILED_TONE, failed: true },
  boardUnwired: { label: "No board", tone: FAILED_TONE, failed: true },
};

const UNKNOWN_ACTION: BoardActionMeta = {
  label: "Unknown",
  tone: FAILED_TONE,
  failed: true,
};

/** The meta for `action`, or a failed-looking fallback for an action this
 * console does not know. */
export function boardActionMeta(action: string): BoardActionMeta {
  return Object.prototype.hasOwnProperty.call(BOARD_ACTION, action)
    ? BOARD_ACTION[action as WorkflowBoardAction]
    : UNKNOWN_ACTION;
}
