import { useEffect, useState } from "react";
import { ShieldCheck } from "lucide-react";

import { analyticsPreference, setAnalyticsPreference } from "@/api/transport/desktop";
import { isDesktopRuntime } from "@/api/transport";
import { Button } from "@/components/ui/button";
import { DISCLOSURE } from "@/lib/analytics-copy";

/**
 * The one-time notice that the desktop app reports anonymous usage data.
 *
 * # Opt-out, said out loud
 *
 * The desktop reports by default (`docs/spec/runtime/analytics-desktop.md`), so
 * this is the moment a person is told, with the way out in the same card.
 * Non-blocking by design — it floats over the console rather than gating it —
 * and prominent: it stays until the person answers.
 *
 * # Remembered without a key of its own
 *
 * "Shown once" is the shell's own preference. `source` is `default` until the
 * person has made a choice, and **either button makes one**: "Turn off" saves
 * `false`, "Got it" saves `true` (the same thing the default already was, now
 * written down as acknowledged). After that `source` is `setting` and this
 * never renders again, so there is no second flag to keep in step with the
 * first. An operator's `OPENCOMPANY_ANALYTICS` (`source: "env"`) is a decision
 * already made on the person's behalf, so nothing is asked.
 *
 * Renders nothing outside the desktop shell, and calls nothing there either.
 *
 * All wording is `lib/analytics-copy.ts`.
 */
export function AnalyticsDisclosure() {
  const [visible, setVisible] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    if (!isDesktopRuntime()) return;
    let live = true;
    void analyticsPreference().then((pref) => {
      if (live && pref && pref.source === "default" && pref.enabled) setVisible(true);
    });
    return () => {
      live = false;
    };
  }, []);

  if (!visible) return null;

  const answer = async (enabled: boolean) => {
    setBusy(true);
    setFailed(false);
    try {
      await setAnalyticsPreference(enabled);
      setVisible(false);
    } catch (error) {
      console.error("[desktop] could not save the analytics choice", error);
      setFailed(true);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div
      role="region"
      aria-label={DISCLOSURE.title}
      className="fixed inset-x-4 bottom-4 z-50 mx-auto w-auto max-w-xl rounded-2xl border border-border bg-popover p-4 text-popover-foreground shadow-lg"
      data-testid="analytics-disclosure"
    >
      <div className="flex items-start gap-3">
        <span className="mt-0.5 inline-flex size-7 shrink-0 items-center justify-center rounded-full bg-primary/10 text-primary">
          <ShieldCheck className="size-3.5" />
        </span>
        <div className="min-w-0 flex-1">
          <p className="text-sm font-medium leading-tight">{DISCLOSURE.title}</p>
          <p className="mt-1 text-xs leading-relaxed text-muted-foreground">{DISCLOSURE.body}</p>
          <p className="mt-1 text-xs leading-relaxed text-muted-foreground">{DISCLOSURE.where}</p>
          {failed && (
            <p role="alert" className="mt-1 text-xs leading-relaxed text-destructive">
              {DISCLOSURE.failed}
            </p>
          )}
          <div className="mt-3 flex items-center gap-2">
            <Button
              size="sm"
              disabled={busy}
              onClick={() => void answer(true)}
              data-testid="analytics-disclosure-got-it"
            >
              {DISCLOSURE.gotIt}
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => void answer(false)}
              data-testid="analytics-disclosure-turn-off"
            >
              {DISCLOSURE.turnOff}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
