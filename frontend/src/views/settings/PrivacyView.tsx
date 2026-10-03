// Privacy: the one switch that decides whether this desktop app reports
// anonymous usage data, and the plain-language account of what that means.
//
// Desktop only. The setting lives in the Tauri shell (`preferences.json` beside
// the app's data), because the shell's Rust host is what sends — the webview
// never does and its CSP does not allow it. A browser console has no such
// switch, so `SettingsSection` does not list or render this page there and
// nothing below runs: the first call is made from this component, which is
// mounted only when `isDesktopRuntime()`.
//
// What the page promises is `lib/analytics-copy.ts`, in one file so that the
// first-launch notice and this page cannot say different things.

import { useCallback, useEffect, useState } from "react";

import {
  type AnalyticsPreference,
  type AnalyticsStatus,
  analyticsPreference,
  setAnalyticsPreference,
} from "@/api/transport/desktop";
import { PageHeader } from "@/components/page-header";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Switch } from "@/components/ui/switch";
import {
  ANALYTICS_COLLECTED,
  ANALYTICS_DOCS_URL,
  ANALYTICS_IP_NOTE,
  ANALYTICS_NEVER_COLLECTED,
  LAST_SEND_LABEL,
  PRIVACY,
} from "@/lib/analytics-copy";

/** `last_at` is RFC-3339 UTC; show it in the reader's own zone, or as-is if unparseable. */
function formatWhen(iso: string | null): string {
  if (!iso) return "never";
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

function LastSend({ status, enabled }: { status: AnalyticsStatus | null; enabled: boolean }) {
  if (!enabled) {
    return <p className="text-sm text-muted-foreground">{PRIVACY.statusOff}</p>;
  }
  if (!status) {
    return <p className="text-sm text-muted-foreground">{PRIVACY.statusUnavailable}</p>;
  }
  // A closed vocabulary, but a newer shell may add a slug this console has not
  // heard of; show the slug rather than nothing.
  const label = LAST_SEND_LABEL[status.last_send] ?? status.last_send;
  return (
    <dl
      className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1 text-sm"
      data-testid="analytics-last-send"
      data-last-send={status.last_send}
    >
      <dt className="text-muted-foreground">Result</dt>
      <dd className="min-w-0 break-words font-medium">
        {label}
        {status.last_status !== null && (
          <span className="font-normal text-muted-foreground"> (HTTP {status.last_status})</span>
        )}
      </dd>
      <dt className="text-muted-foreground">When</dt>
      <dd className="min-w-0 break-words">{formatWhen(status.last_at)}</dd>
      <dt className="text-muted-foreground">Accepted</dt>
      <dd>{status.accepted}</dd>
      <dt className="text-muted-foreground">Dropped</dt>
      <dd>{status.dropped}</dd>
      {status.endpoint && (
        <>
          <dt className="text-muted-foreground">Collector</dt>
          <dd className="min-w-0 break-all">{status.endpoint}</dd>
        </>
      )}
    </dl>
  );
}

export function PrivacyView() {
  const [pref, setPref] = useState<AnalyticsPreference | null>(null);
  const [loadError, setLoadError] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const refresh = useCallback(async () => {
    const next = await analyticsPreference();
    if (next) {
      setPref(next);
      setLoadError(false);
    } else {
      setLoadError(true);
    }
  }, []);

  useEffect(() => {
    // Read on mount, and again on a slow timer: the last-send line is the point
    // of the status, and the first send lands a few seconds after launch.
    void refresh();
    const timer = window.setInterval(() => void refresh(), 10_000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  const toggle = async (enabled: boolean) => {
    setSaving(true);
    setSaveError(null);
    try {
      setPref(await setAnalyticsPreference(enabled));
    } catch (error) {
      setSaveError(error instanceof Error ? error.message : String(error));
      // An opt-out may already have taken effect even though the write failed.
      await refresh();
    } finally {
      setSaving(false);
    }
  };

  const fromEnv = pref?.source === "env";

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <PageHeader title={PRIVACY.title} width="full" />
      <div className="min-h-0 w-full flex-1 space-y-6 overflow-y-auto px-4 py-6">
        <Card>
          <CardHeader>
            <CardTitle className="text-base">{PRIVACY.toggleTitle}</CardTitle>
            <CardDescription>{PRIVACY.toggleDescription}</CardDescription>
            <CardAction>
              <Switch
                aria-label={PRIVACY.toggleLabel}
                data-testid="analytics-toggle"
                checked={pref?.enabled ?? false}
                disabled={pref === null || fromEnv || saving}
                onCheckedChange={(checked) => void toggle(checked)}
              />
            </CardAction>
          </CardHeader>
          <CardContent className="space-y-2 text-sm">
            {loadError && (
              <p role="alert" className="text-destructive">
                {PRIVACY.loadFailed}
              </p>
            )}
            {saveError && (
              <p role="alert" className="break-words text-destructive">
                {PRIVACY.saveFailed}: {saveError}
              </p>
            )}
            {fromEnv && (
              <p className="text-muted-foreground" data-testid="analytics-env-note">
                {PRIVACY.envNote}
              </p>
            )}
            {pref?.restart_required && (
              <p className="text-muted-foreground" data-testid="analytics-restart-note">
                {PRIVACY.restartNote}
              </p>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="text-base">{PRIVACY.statusTitle}</CardTitle>
          </CardHeader>
          <CardContent>
            <LastSend status={pref?.status ?? null} enabled={pref?.enabled ?? false} />
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="text-base">{PRIVACY.collectedTitle}</CardTitle>
            <CardDescription>{PRIVACY.collectedIntro}</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3 text-sm">
            <ul className="list-disc space-y-1.5 pl-5" data-testid="analytics-collected">
              {ANALYTICS_COLLECTED.map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
            <p className="text-muted-foreground">{ANALYTICS_IP_NOTE}</p>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="text-base">{PRIVACY.neverTitle}</CardTitle>
          </CardHeader>
          <CardContent className="text-sm">
            <ul className="list-disc space-y-1.5 pl-5" data-testid="analytics-never">
              {ANALYTICS_NEVER_COLLECTED.map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
          </CardContent>
        </Card>

        <p className="text-sm">
          <a
            href={ANALYTICS_DOCS_URL}
            target="_blank"
            rel="noreferrer"
            className="text-primary underline underline-offset-4"
          >
            {PRIVACY.docsLink}
          </a>
        </p>
      </div>
    </div>
  );
}
