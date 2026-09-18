import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { removalWarnings } from "./removal";
import type { ProviderIntent, RemovalImpact } from "./removal";

/**
 * Which of the four is being confirmed.
 *
 * `disable` and `enable` join the two removals because **every toggle
 * confirms now** (keys rework, issue #2306's confirmation contract) — both
 * are fully reversible, so they get the same machinery and deliberately
 * lighter language. See `removalWarnings`.
 */
export type RemovalIntent = ProviderIntent;

/**
 * Confirming a removal, with what it costs said out loud.
 *
 * ## Why this is a dialog and not a second click
 *
 * Both of these actions are one menu item away from each other, both read as
 * "remove", and they are not the same thing: one is recoverable by retyping a
 * credential, the other deletes a record and its endpoint. An operator who
 * means the first and gets the second finds out when a turn stops resolving.
 *
 * ## Why it names facts rather than asking "are you sure?"
 *
 * A confirmation that only asks for confidence is a speed bump. It tells the
 * operator nothing they did not already know, and the only thing it reliably
 * teaches is to click through. [`removalWarnings`](./removal.ts) names what is
 * actually about to change — whether it is the company default (which stays
 * pointed here and fails rather than moving, decision X14), which agents are
 * pinned to it, the fact that this was the last provider switched on — one
 * sentence per fact, from the same `usedBy` the host refuses the write against,
 * so this dialog cannot disagree with what a `409 in_use` would say.
 *
 * ## The softer option is offered, not implied
 *
 * Most of the time somebody removing a provider wants it to stop being used, and
 * **switching it off does that** while keeping the endpoint and the credential.
 * It is offered here, on the screen where the destructive choice is being made,
 * because that is the moment it is useful — a hint somewhere else on the page
 * is a hint nobody reads at the point of the decision.
 */
export function RemoveProviderDialog({
  intent,
  label,
  impact,
  busy,
  serverError,
  onCancel,
  onDisable,
  onConfirm,
}: {
  /** `null` when nothing is being confirmed — the dialog is closed. */
  intent: RemovalIntent | null;
  label: string;
  impact: RemovalImpact;
  busy: boolean;
  /**
   * The host's own message from a `409 in_use` this same request already hit
   * once (keys rework, issue #2306's confirmation contract) — shown verbatim,
   * never "Something went wrong". `impact.usedBy` is refreshed from the same
   * refusal, so the sentences above already reflect it; this is the host's
   * own words on top, for whatever it phrased differently.
   */
  serverError?: string | null;
  onCancel: () => void;
  /** Offered only where it is a real alternative — see the component doc. */
  onDisable?: () => void;
  onConfirm: () => void;
}) {
  if (!intent) return null;
  const lines = removalWarnings(intent, label, impact);
  // **Reversible, and the buttons say so.** Switching a provider on or off
  // keeps its endpoint and its key; the host never scrubs anything on a
  // toggle, precisely so that switching it back is a switch rather than a
  // re-configuration. Destructive styling here would teach an operator that a
  // toggle and a delete are the same act, which is the confusion this dialog
  // exists to clear up.
  const reversible = intent === "disable" || intent === "enable";

  return (
    // Live lane 1, KR-L1-05: Escape and an overlay click are ignored while
    // `busy` — a confirmed write is in flight, the same guard
    // `ProviderConnectDialog` has (round-2 review, P0-2). Without it, Escape
    // could dismiss this dialog while its own request was still pending, and
    // — because `busy` is shared with the connect flow — a later dialog
    // opened before that pending request settled could read a stale `busy`
    // it never set and never expected to clear.
    <Dialog open onOpenChange={(next) => !next && !busy && onCancel()}>
      <DialogContent className="sm:max-w-md" data-testid="inference-remove-dialog">
        <DialogHeader>
          <DialogTitle>
            {intent === "disable"
              ? `Turn off ${label}?`
              : intent === "enable"
                ? `Turn on ${label}?`
                : intent === "key"
                  ? `Remove ${label}'s key?`
                  : `Remove ${label}?`}
          </DialogTitle>
          {/* The first line is always the distinction between the two, because
              it is the one an operator is most likely to have got wrong. */}
          <DialogDescription>{lines[0]}</DialogDescription>
        </DialogHeader>

        {lines.length > 1 && (
          <ul className="grid gap-1.5" data-testid="inference-remove-impact">
            {lines.slice(1).map((line) => (
              <li
                key={line}
                className={cn("text-xs", reversible ? "text-muted-foreground" : "text-status-blocked-text")}
              >
                {line}
              </li>
            ))}
          </ul>
        )}

        {serverError && (
          <p className="text-sm text-status-blocked-text" data-testid="inference-remove-server-error">
            {serverError}
          </p>
        )}

        <DialogFooter>
          <Button type="button" variant="outline" disabled={busy} onClick={onCancel}>
            Cancel
          </Button>
          {onDisable && (
            <Button
              type="button"
              variant="outline"
              disabled={busy}
              data-testid="inference-remove-disable-instead"
              onClick={onDisable}
            >
              Turn it off instead
            </Button>
          )}
          <Button
            type="button"
            variant={reversible ? "default" : "destructive"}
            disabled={busy}
            data-testid="inference-remove-confirm"
            onClick={onConfirm}
          >
            {intent === "disable" || intent === "enable"
              ? "Continue"
              : intent === "key"
                ? "Remove key"
                : "Remove provider"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
