import { useState, type ReactNode } from "react";
import { Check, ChevronDown, ChevronRight, Loader2, TriangleAlert } from "lucide-react";
import { toast } from "sonner";

import { ApiError } from "@/api/types";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { cn } from "@/lib/utils";
import type { Health } from "@/views/finance/health";

interface Props {
  /** `Chargebee` or `PayPal` — the provider's own name, shown as the title. */
  title: string;
  /** The verdict driving the collapsed line and the badge. */
  health: Health;
  /** Whether the panel is open. Owned by the parent so it survives a reload. */
  expanded: boolean;
  onExpandedChange: (expanded: boolean) => void;
  /** Runs the provider's `POST …/test`. Absent while unconfigured. */
  onTest?: () => Promise<{ detail: string }>;
  /** Test-id prefix, e.g. `chargebee`. */
  testId: string;
  /** The credential form. Rendered only while expanded. */
  children: ReactNode;
}

/**
 * The collapsible connection panel that tops both provider pages.
 *
 * # Why it collapses
 *
 * An operator configures a payment provider once and reads its data daily. A
 * permanently expanded credential form taxes every one of those visits for a
 * task performed once — which is what made Settings → Billing the wrong home
 * for this in the first place. Collapsed, it is one line that says whether the
 * integration is working and against which account; expanded, it is the form it
 * always was.
 *
 * # Why the badge is not just "Connected"
 *
 * It renders `health.state`, and `health` picks the **worst** of four possible
 * problems rather than the most flattering. See `health.ts`.
 */
export function ConnectionPanel({
  title,
  health,
  expanded,
  onExpandedChange,
  onTest,
  testId,
  children,
}: Props) {
  const [testing, setTesting] = useState(false);

  async function runTest() {
    if (!onTest) return;
    setTesting(true);
    try {
      const result = await onTest();
      // The detail names the site or environment that answered. "OK" alone
      // would leave "connected to the wrong account" looking like success —
      // the confusion this whole status surface exists to remove.
      toast.success(result.detail);
    } catch (err) {
      // The provider's own message, not a generic failure: `provider_error`
      // carries the token that tells the operator which setting is wrong, and
      // `not_configured` means they have not filled the form in yet.
      toast.error(
        err instanceof ApiError
          ? err.message
          : err instanceof Error
            ? err.message
            : `Could not reach ${title}.`,
      );
    } finally {
      setTesting(false);
    }
  }

  const ok = health.state === "connected";
  const Chevron = expanded ? ChevronDown : ChevronRight;

  return (
    <Card data-testid={`${testId}-panel`}>
      <CardContent className="space-y-4 py-4">
        <div className="flex flex-wrap items-center gap-3">
          <button
            type="button"
            onClick={() => onExpandedChange(!expanded)}
            aria-expanded={expanded}
            className="flex min-w-0 flex-1 items-center gap-2 text-left"
            data-testid={`${testId}-toggle`}
          >
            <Chevron className="size-4 shrink-0 text-muted-foreground" />
            <span className="text-sm font-medium">{title}</span>
            <span
              className="truncate text-xs text-muted-foreground"
              data-testid={`${testId}-summary`}
            >
              {health.label}
            </span>
          </button>

          <Badge
            variant={ok ? "secondary" : "outline"}
            data-testid={`${testId}-state`}
            className={cn(
              "shrink-0",
              health.state === "not_granted" && "text-status-blocked-text",
              health.state === "not_in_build" && "text-muted-foreground",
            )}
          >
            {ok ? <Check className="mr-1 size-3" /> : null}
            {ok ? "Connected" : health.state === "not_configured" ? "Set up" : "Attention"}
          </Badge>

          {onTest ? (
            <Button
              variant="outline"
              size="sm"
              onClick={runTest}
              disabled={testing}
              data-testid={`${testId}-test`}
            >
              {testing ? <Loader2 className="mr-2 size-3.5 animate-spin" /> : null}
              Test
            </Button>
          ) : null}

          <Button
            variant="ghost"
            size="sm"
            onClick={() => onExpandedChange(!expanded)}
            data-testid={`${testId}-manage`}
          >
            {expanded ? "Hide" : "Manage"}
          </Button>
        </div>

        {/* The remedy stays visible while collapsed. A problem folded away
            behind a chevron is a problem nobody fixes — and two of the four
            states cannot be fixed by opening this panel at all, so hiding the
            text that says where the fix *is* would be the worst of both. */}
        {health.remedy ? (
          <Alert
            variant={health.state === "connected" ? "default" : "destructive"}
            data-testid={`${testId}-remedy`}
          >
            <TriangleAlert className="size-4" />
            <AlertDescription>{health.remedy}</AlertDescription>
          </Alert>
        ) : null}

        {expanded ? <div className="space-y-4 border-t pt-4">{children}</div> : null}
      </CardContent>
    </Card>
  );
}
