// The drawer a FAILED run leaves behind (issue #1007).
//
// `RunResultPanel` next door is the surface for a run that produced something,
// and it can only mount when the settled body arrives — which on the failure
// path it never does. So a failed run had no drawer, no row and no error text:
// the console went back to its resting state behind a toast that lasted four
// seconds. This is the same slot, for the outcome that had nothing in it.
//
// Deliberately a panel and not a toast, on the same rule the version-conflict
// banner and the inference refusal already follow: an outcome the operator has
// to act on — read the message, look at the history row, fix the graph, run it
// again — must outlive the glance.

import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";

import { formatDuration } from "./run-health";
import { stripEnginePrefixes } from "./run-error-message";
import {
  failureDisposition,
  type FailureDisposition,
  type RunFailure,
} from "./run-failure";

const DISPOSITION_COPY: Record<FailureDisposition, string> = {
  transport:
    "The request didn't complete, so the host may or may not have started this run. Run history is the answer either way.",
  "refusal-inference":
    "The host did not start this run because inference needs attention. Update Connections → Inference, then try again.",
  "refusal-lifecycle":
    "This company is not running, so the host did not start this run. Resume it from the company's controls, then try again.",
  "refusal-not-wired": "This host cannot run automations, so it did not start this run.",
  journaled: "This console saw the run start. Run history has the step it stopped at.",
  cautious:
    "The host answered, but this console did not see the run start. Run history may have more detail if it started.",
};

export function RunFailurePanel({
  failure,
  onClose,
  onOpenHistory,
  onFixWithCopilot,
  fixing,
  failedStepName,
  onShowFailedStep,
}: {
  failure: RunFailure;
  onClose: () => void;
  /** Opens the durable record when the host supports history. */
  onOpenHistory?: () => void;
  /** Reuses the history row's copilot correction for this exact failed run. */
  onFixWithCopilot?: () => void;
  /** Keeps this panel's button consistent with the history row while fixing. */
  fixing?: boolean;
  /** The graph-authored name of the failed step, when the matching row arrived. */
  failedStepName?: string | null;
  /** Opens the failed run on canvas and selects the step that failed. */
  onShowFailedStep?: () => void;
}) {
  // How long the operator waited before this came back. The number a failure
  // most needs beside it: a run that died in 200ms was refused, and one that
  // died after four minutes got somewhere first.
  const ranFor = Math.max(0, failure.atMillis - failure.startedAtMillis);
  const disposition = failureDisposition(failure);
  const message = stripEnginePrefixes(failure.message);
  return (
    // Issue #1205: a right rail at `xl`, the bottom strip it used to always be
    // below that — same pattern as `RunResultPanel` next door, which is the
    // mutually exclusive sibling this panel replaces in the same slot.
    <aside
      aria-label="Run failure"
      className="flex h-full flex-col border-t bg-card/60 xl:border-0 xl:bg-transparent"
      data-testid="workflow-run-failure"
    >
      <div className="flex items-center justify-between px-4 py-2">
        <div className="flex flex-wrap items-center gap-2">
          {/*
            Issue B-037: a refusal is not a failure. All three refusal
            dispositions are host answers that returned before anything was
            spawned, so "Run failed" reads as "your automation broke" when the
            truth is that it never started and the operator can clear the
            reason. The body copy below already says which reason.
          */}
          <span className="text-sm font-medium">
            {disposition.startsWith("refusal-") ? "Run not started" : "Run failed"}
          </span>
          {failure.dryRun && (
            <Badge
              variant="outline"
              className="border-primary/40 bg-primary/10 text-primary"
            >
              Test run — nothing was sent
            </Badge>
          )}
          {/* Only a code the HOST gave us. See `RunFailure.code`. */}
          {failure.code && (
            <Badge
              variant="outline"
              className="h-4 px-1.5 font-mono text-3xs font-normal"
              data-testid="workflow-run-failure-code"
            >
              {failure.code}
            </Badge>
          )}
        </div>
        <Button variant="ghost" size="sm" onClick={onClose}>
          Dismiss
        </Button>
      </div>
      {/* Capped as a strip, growing as a rail — see `RunResultPanel`'s body. */}
      <div className="max-h-72 overflow-auto px-4 pb-3 xl:min-h-0 xl:max-h-none xl:flex-1">
        <Alert variant="destructive" className="py-2">
          <AlertDescription
            className="text-xs"
            data-testid="workflow-run-failure-message"
          >
            {message}
          </AlertDescription>
        </Alert>
        <details className="mt-2">
          <summary className="cursor-pointer text-2xs text-muted-foreground">
            Details
          </summary>
          <pre className="mt-1 overflow-auto rounded border bg-muted/40 p-2 font-mono text-2xs leading-snug text-foreground">
            {failure.message}
          </pre>
        </details>
        <p className="mt-2 text-2xs text-muted-foreground">
          Failed after {formatDuration(ranFor)} ·{" "}
          {new Date(failure.atMillis).toLocaleString()}
          {/* A status is worth printing only when it says something the message
              does not. `0` is the client's "the request never completed", which
              the sentence below already states in words. */}
          {failure.status != null && failure.status > 0
            ? ` · HTTP ${failure.status}`
            : ""}
        </p>
        {/* Issue #154's echo, for the same reason the result drawer carries it:
            the operator has usually typed something else by now, so the box is
            not the record of what this run was asked for. */}
        {failure.request && (
          <p className="mt-1 text-xs text-muted-foreground">
            <span className="font-medium text-foreground">Requested:</span>{" "}
            {failure.request}
          </p>
        )}
        {/* A host envelope proves only who answered. The structured code proves
            known pre-execution refusals, and the console's own start frame is
            the positive evidence needed before saying History has this run. */}
        <p
          className="mt-2 text-2xs text-muted-foreground"
          data-testid="workflow-run-failure-disposition"
        >
          {DISPOSITION_COPY[disposition]}
        </p>
        {(onOpenHistory || onFixWithCopilot || onShowFailedStep) && (
          <div className="mt-2 flex flex-wrap gap-2">
            {onOpenHistory && (
              <Button
                size="sm"
                variant="outline"
                className="h-6 px-2 text-3xs"
                onClick={onOpenHistory}
                data-testid="workflow-run-failure-open-history"
              >
                Open Run history
              </Button>
            )}
            {onShowFailedStep && failedStepName && (
              <Button
                size="sm"
                variant="outline"
                className="h-6 px-2 text-3xs"
                onClick={onShowFailedStep}
                data-testid="workflow-run-failure-show-step"
              >
                Show “{failedStepName}”
              </Button>
            )}
            {onFixWithCopilot && (
              <Button
                size="sm"
                variant="outline"
                className="h-6 px-2 text-3xs"
                disabled={fixing}
                onClick={onFixWithCopilot}
                data-testid="workflow-run-failure-fix-with-copilot"
              >
                {fixing ? "Fixing…" : "Fix with copilot"}
              </Button>
            )}
          </div>
        )}
      </div>
    </aside>
  );
}
