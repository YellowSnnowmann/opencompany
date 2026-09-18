import { Button } from "@/components/ui/button";

/**
 * "Use the same key for LLM/Composio?" — offered when the company's
 * TinyHumans account key exists but a page's own managed slot has nothing of
 * its own (keys rework, issue #2306, slice 4c;
 * `docs/key-reworks/phase-4c-reuse-banner.md` §4 step 10).
 *
 * Shared by both pages that can offer this (round-3b review, item 6): the LLM
 * page's copy is a single-slot fill exactly like Composio's, with its own
 * follow-up model step handled entirely by the caller (a separate `Dialog`
 * next to this banner, the same shape `AccountKeyDialog`'s step two already
 * uses) — not by this component, which stays the same plain "Yes / Not now"
 * question over a caption either way. Originally written Composio-only
 * (`@/composio/ReuseAccountKeyBanner`, keys-rework slice 1a/4c) on the
 * reasoning that the LLM variant would need a shape neither caller could give
 * an honest default for; the plan doc settled on giving the model step to the
 * caller instead, which is what makes one component enough.
 *
 * Deliberately dumb: no fetch, no dismissal state, no visibility decision.
 * `ComposioSection.tsx` and (once wired) `ProvidersTab.tsx` decide whether to
 * render this at all (`@/inference/reuse-banner`'s `showsComposioReuseBanner`
 * / `showsInferenceReuseBanner`) and own every callback's effect.
 */
export function ReuseAccountKeyBanner({
  testId,
  text,
  busy,
  onYes,
  onNotNow,
}: {
  /** Root test id. The two buttons append `-yes` / `-not-now`. */
  testId: string;
  /** The question, in full — this component adds no copy of its own. */
  text: string;
  /** Disables "Yes" while the copy is in flight. "Not now" stays clickable. */
  busy: boolean;
  onYes: () => void;
  onNotNow: () => void;
}) {
  return (
    <div
      role="status"
      data-testid={testId}
      className="flex flex-wrap items-center gap-3 rounded-md border border-border bg-muted/40 p-3 text-xs"
    >
      <span className="min-w-0 flex-1 text-foreground">{text}</span>
      <div className="flex shrink-0 items-center gap-2">
        <Button
          size="sm"
          disabled={busy}
          data-testid={`${testId}-yes`}
          onClick={onYes}
        >
          Yes
        </Button>
        <Button
          size="sm"
          variant="ghost"
          data-testid={`${testId}-not-now`}
          onClick={onNotNow}
        >
          Not now
        </Button>
      </div>
    </div>
  );
}
