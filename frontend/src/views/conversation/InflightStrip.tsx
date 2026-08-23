// The in-flight steer strip (issue #111), lifted out of the Conversation view
// unchanged. It sits between the transcript and the composer, and it is why
// this surface hands assistant-ui no `onCancel`: abandoning the last reply is
// not the action an operator wants here — pausing, redirecting or cancelling a
// named run is, and only this strip can name them.

import { useCallback, useEffect, useRef, useState } from "react";
import { CornerUpRight, Loader2, Pause, Send, X } from "lucide-react";
import { toast } from "sonner";

import type { OpenCompanyClient } from "@/api/client";
import { listInflight, steerTask, type InflightRun, type SteerAction } from "@/api/tasks";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

/* ---- in-flight steer strip (issue #111) ---- */

/** Past-tense badge copy while a steer of the given verb is in flight. */
const PENDING_LABEL: Record<string, string> = {
  pause: "pausing…",
  cancel: "cancelling…",
  redirect: "redirecting…",
};

/**
 * A strip above the composer listing the company's in-flight runs, so the
 * operator can steer them (issue #111) without leaving company chat: pause,
 * redirect, or cancel a dispatched task; cancel a sub-agent delegation. Reads
 * {@link listInflight} on mount and refetches on any successful steer and on
 * each task-lifecycle SSE tick. Renders nothing when nothing is in flight (or
 * when the host has no inflight route), so it stays out of the way.
 */
export function InflightStrip({
  client,
  company,
  taskEventTick,
}: {
  client: OpenCompanyClient;
  company: string | null;
  taskEventTick?: number;
}) {
  const [runs, setRuns] = useState<InflightRun[]>([]);
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const rows = await listInflight(client, company);
      if (mounted.current) setRuns(rows);
    } catch {
      // Best-effort surface: a host without the inflight route (404) just means
      // no strip. Clear rather than surface an error into the chat.
      if (mounted.current) setRuns([]);
    }
  }, [client, company]);

  useEffect(() => {
    mounted.current = true;
    void refresh();
    return () => {
      mounted.current = false;
    };
  }, [refresh]);

  // Live refetch when a task-lifecycle event rides the SSE stream.
  useEffect(() => {
    if (taskEventTick !== undefined) void refresh();
  }, [taskEventTick, refresh]);

  if (runs.length === 0) return null;

  return (
    <div className="border-t bg-muted/30">
      <div className="mx-auto w-full max-w-3xl px-4 py-2">
        <p className="mb-1.5 px-1 text-2xs font-medium uppercase tracking-wide text-muted-foreground">
          In flight · {runs.length}
        </p>
        <div className="flex flex-col gap-1.5">
          {runs.map((run) => (
            <InflightRow key={run.key} run={run} onSteer={refresh} client={client} company={company} />
          ))}
        </div>
      </div>
    </div>
  );
}

function InflightRow({
  run,
  onSteer,
  client,
  company,
}: {
  run: InflightRun;
  onSteer: () => Promise<void> | void;
  client: OpenCompanyClient;
  company: string | null;
}) {
  const [busy, setBusy] = useState(false);
  const [redirecting, setRedirecting] = useState(false);
  const [instruction, setInstruction] = useState("");

  // A pending server-side steer, or an optimistic local one, freezes the row.
  const pending = run.pendingAction ?? null;
  const disabled = busy || pending !== null;

  async function steer(action: SteerAction, opts?: { instruction?: string; confirm?: boolean }) {
    setBusy(true);
    try {
      await steerTask(client, company, run.key, { action, ...opts });
      setRedirecting(false);
      setInstruction("");
      await onSteer();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : "could not steer the task");
    } finally {
      setBusy(false);
    }
  }

  function onCancel() {
    // Cancel is destructive — the backend also requires `confirm: true`.
    if (!window.confirm(`Cancel “${run.title}”? This stops the run.`)) return;
    void steer("cancel", { confirm: true });
  }

  function onRedirect() {
    const text = instruction.trim();
    if (!text) return;
    void steer("redirect", { instruction: text });
  }

  const isTask = run.kind === "task";

  return (
    <div className="rounded-lg border bg-card px-2.5 py-1.5">
      <div className="flex items-center gap-2">
        <div className="min-w-0 flex-1">
          <p className="truncate text-xs font-medium">{run.title}</p>
          <p className="truncate text-2xs text-muted-foreground">
            {run.kind === "delegation" ? "Delegation" : "Task"} · {run.agentId}
          </p>
        </div>

        {pending !== null ? (
          <span className="shrink-0 rounded-full bg-muted px-2 py-0.5 text-3xs font-medium text-muted-foreground">
            {PENDING_LABEL[pending] ?? "steering…"}
          </span>
        ) : (
          <div className="flex shrink-0 items-center gap-1">
            {busy && <Loader2 className="size-3.5 animate-spin text-muted-foreground" />}
            {isTask && (
              <>
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 px-2 text-xs"
                  disabled={disabled}
                  onClick={() => void steer("pause")}
                >
                  <Pause className="mr-1 size-3.5" />
                  Pause
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 px-2 text-xs"
                  disabled={disabled}
                  aria-pressed={redirecting}
                  onClick={() => setRedirecting((r) => !r)}
                >
                  <CornerUpRight className="mr-1 size-3.5" />
                  Redirect
                </Button>
              </>
            )}
            <Button
              variant="ghost"
              size="sm"
              className="h-7 px-2 text-xs text-destructive hover:text-destructive"
              disabled={disabled}
              onClick={onCancel}
            >
              <X className="mr-1 size-3.5" />
              Cancel
            </Button>
          </div>
        )}
      </div>

      {isTask && redirecting && pending === null && (
        <div className="mt-1.5 flex items-center gap-1.5">
          <Input
            value={instruction}
            onChange={(e) => setInstruction(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                onRedirect();
              }
            }}
            placeholder="New instruction for this task…"
            aria-label={`New instruction for ${run.title}`}
            className="h-7 flex-1 text-xs"
            autoFocus
          />
          <Button
            size="icon"
            className="size-7 shrink-0"
            disabled={disabled || !instruction.trim()}
            onClick={onRedirect}
            aria-label="Send redirect"
          >
            <Send className="size-3.5" />
          </Button>
        </div>
      )}
    </div>
  );
}
