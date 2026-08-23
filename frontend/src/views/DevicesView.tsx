import { useCallback, useEffect, useState } from "react";
import { Copy, Laptop, Loader2, TriangleAlert } from "lucide-react";
import { toast } from "sonner";

import type { OpenCompanyClient } from "@/api/client";
import { listDevices, revokeDevice, startPairing, type Device } from "@/api/devices";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";

interface Props {
  client: OpenCompanyClient;
  company: string | null;
}

/** The message out of a rejected request, whatever it was rejected with. */
function reason(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** A timestamp as a person reads it. */
function when(millis: number): string {
  return new Date(millis).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

/** How long is left, as `m:ss`, or null once there is none. */
function remaining(expiresAtMillis: number, now: number): string | null {
  const left = Math.floor((expiresAtMillis - now) / 1000);
  if (left <= 0) return null;
  return `${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}`;
}

/**
 * Settings → Devices: the machines paired to your account, and the codes that
 * enrol them.
 *
 * # Why this page exists
 *
 * The host has served `GET/POST …/devices` since pairing landed, and the
 * desktop app told people to come here for a code — but the page was never
 * built, so the instruction pointed at nothing and a first-run desktop had no
 * way forward at all (issue #1476). The backend flow is unchanged; this is the
 * missing half of it.
 *
 * # What is on screen, and what deliberately is not
 *
 * A pairing code is shown **once**, in the sitting it was minted, and expires
 * in five minutes — so it is rendered with the time left beside it rather than
 * as a value that sits there looking durable. Nothing re-reads it: only its
 * hash is stored, and a lost code is replaced by minting another.
 *
 * The session token a code is redeemed for never appears here at all. That
 * exchange happens on the machine being enrolled, over `…/devices/claim`, and
 * the token goes from the host into that machine's keychain. A console that
 * displayed it would turn a design where the webview *cannot* hold the
 * credential into one where it merely *should not*.
 *
 * # Revoking
 *
 * A paired device *is* a session record, so revoking one signs that machine out
 * immediately rather than at some later expiry. The row for the credential
 * making the request — only ever the case when this console is itself running
 * inside a paired desktop — says so, because revoking it signs out the window
 * you are reading.
 */
export function DevicesView({ client, company }: Props) {
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [code, setCode] = useState<{ code: string; expiresAtMillis: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const [now, setNow] = useState(() => Date.now());

  const load = useCallback(async () => {
    try {
      setDevices(await listDevices(client, company));
      setLoadError(null);
    } catch (err) {
      setLoadError(reason(err));
    }
  }, [client, company]);

  useEffect(() => {
    void load();
  }, [load]);

  // Only while a code is on screen. A countdown that keeps ticking after the
  // code is gone re-renders the whole page once a second for nothing.
  useEffect(() => {
    if (!code) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [code]);

  async function onPair() {
    setBusy(true);
    try {
      const minted = await startPairing(client, company);
      setNow(Date.now());
      setCode(minted);
    } catch (err) {
      // Passed through rather than reworded: the refusal a paired device gets
      // names the remedy ("sign in on the web console"), and a generic "could
      // not create a code" would drop exactly the part that helps.
      toast.error(reason(err));
    } finally {
      setBusy(false);
    }
  }

  async function onRevoke(device: Device) {
    setBusy(true);
    try {
      await revokeDevice(client, company, device.id);
      toast.success(`${device.label ?? "That device"} is signed out.`);
      await load();
    } catch (err) {
      toast.error(reason(err));
    } finally {
      setBusy(false);
    }
  }

  if (loadError) {
    return (
      <div className="mx-auto w-full max-w-5xl px-4 py-6">
        <Alert variant="destructive" data-testid="devices-load-error">
          <TriangleAlert className="size-4" />
          <AlertDescription>Could not load your devices: {loadError}</AlertDescription>
        </Alert>
      </div>
    );
  }

  if (!devices) {
    return (
      <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
        <Loader2 className="mr-2 size-4 animate-spin" /> Loading devices…
      </div>
    );
  }

  const left = code ? remaining(code.expiresAtMillis, now) : null;

  return (
    <div className="flex-1 overflow-y-auto" data-testid="devices-view">
      <div className="mx-auto w-full max-w-5xl space-y-6 px-4 py-6">
        <div>
          <h1 className="flex items-center gap-2 text-lg font-medium">
            <Laptop className="size-5" /> Devices
          </h1>
          <p className="text-sm text-muted-foreground">
            The desktop app cannot use this browser&rsquo;s session, so it enrols
            as a device of its own. Create a code here and paste it into the app
            on the machine you want to sign in.
          </p>
        </div>

        <Card>
          <CardContent className="space-y-4 pt-6">
            {code ? (
              <div className="space-y-2">
                <p className="text-sm font-medium">Paste this into the desktop app</p>
                <div className="flex flex-wrap items-center gap-2">
                  <code
                    data-testid="pairing-code"
                    className="select-all break-all rounded-md bg-muted px-3 py-2 font-mono text-sm"
                  >
                    {code.code}
                  </code>
                  <Button
                    variant="outline"
                    size="sm"
                    data-testid="pairing-code-copy"
                    onClick={() => {
                      void navigator.clipboard
                        .writeText(code.code)
                        .then(() => toast.success("Pairing code copied."))
                        .catch(() => toast.error("Could not copy — select the code instead."));
                    }}
                  >
                    <Copy className="mr-1.5 size-3.5" /> Copy
                  </Button>
                </div>
                <p className="text-xs text-muted-foreground" data-testid="pairing-code-expiry">
                  {left
                    ? `Expires in ${left}. It is shown once and works once — create another if you lose it.`
                    : "This code has expired. Create another."}
                </p>
              </div>
            ) : null}

            <Button onClick={() => void onPair()} disabled={busy} data-testid="devices-pair">
              {busy ? <Loader2 className="mr-2 size-4 animate-spin" /> : null}
              {code ? "Create another code" : "Create a pairing code"}
            </Button>
          </CardContent>
        </Card>

        <div className="space-y-2">
          <h2 className="text-sm font-medium">Paired devices</h2>
          {devices.length === 0 ? (
            <Card>
              <CardContent className="pt-6">
                <p className="text-sm text-muted-foreground" data-testid="devices-empty">
                  No machines are paired to your account yet.
                </p>
              </CardContent>
            </Card>
          ) : (
            <Card>
              <CardContent className="space-y-0 divide-y pt-2">
                {devices.map((device) => (
                  <div
                    key={device.id}
                    data-testid="device-row"
                    className="flex flex-wrap items-center justify-between gap-3 py-3"
                  >
                    <div className="min-w-0 space-y-1">
                      <p className="flex items-center gap-2 text-sm font-medium">
                        {device.label ?? "Unnamed device"}
                        {device.current ? (
                          <Badge variant="secondary" data-testid="device-current">
                            This device
                          </Badge>
                        ) : null}
                      </p>
                      <p className="text-xs text-muted-foreground">
                        Paired {when(device.createdAtMillis)} · signs out{" "}
                        {when(device.expiresAtMillis)}
                      </p>
                    </div>
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={busy}
                      data-testid="device-revoke"
                      onClick={() => void onRevoke(device)}
                    >
                      {device.current ? "Sign out this device" : "Revoke"}
                    </Button>
                  </div>
                ))}
              </CardContent>
            </Card>
          )}
        </div>

        <p className="text-xs text-muted-foreground">
          A paired device acts as you, with everything you can do and nothing
          more. Revoking one signs that machine out at once — as does changing
          your password, or an admin suspending the account.
        </p>
      </div>
    </div>
  );
}
