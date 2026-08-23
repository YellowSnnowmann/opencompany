// Enrolling this machine as a person, from the desktop application.
//
// The console cannot hold a session cookie when it runs in a webview:
// `SameSite=Lax` means a browser never sends one cross-site, and a webview is
// cross-site with every host it talks to. Pairing is how the desktop gets a
// credential of its own — the same session, in the header carrier, with a
// longer life and a name on it.
//
// This surface is deliberately small. It renders only in the desktop build,
// because in a browser the cookie already works and a pairing prompt would be
// an invitation to a step nobody needs.
//
// What it never touches: the token. `pairDevice` returns which company, which
// device and how long it lasts — the secret goes from the host into the OS
// keychain without passing through here. That is the difference between a
// design where the webview *should not* hold the credential and one where it
// *cannot*, and it only holds if this component stays uninvolved.

import { useState } from "react";
import { KeyRound, Loader2 } from "lucide-react";

import { isDesktopRuntime, mayCarryACredential } from "@/api/transport";
import { forgetDevice, pairDevice } from "@/api/transport/desktop";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { getConnection, useConnection } from "@/connections/registry";
import { pairedConnection, unpairConnection } from "@/connections/registry";
import { useLocalScope } from "@/connections/ConnectionContext";
import { settingsPageLabel } from "@/views/settings-pages";

// Where a code comes from, read off the sub-page table rather than written out
// here. This sentence named a page that did not exist for a whole release
// (issue #1476); read from the table, it cannot name one again — an id that is
// not a real page does not compile, and renaming the page rewrites the
// sentence.
const CODE_SOURCE = `Settings → ${settingsPageLabel("devices")}`;

export function DevicePairing() {
  const scope = useLocalScope();
  const connection = useConnection(scope.connection);
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // A browser has a cookie; there is nothing to pair.
  if (!isDesktopRuntime() || !connection) return null;

  const paired = connection.credential.kind === "device";
  // A host this machine must not hand a session to. The core refuses the claim
  // too — that is the check that counts, since it also covers anything invoking
  // `oc_pair_device` directly — but a form that cannot succeed should say so
  // before someone fetches a code from another screen to type into it.
  const encrypted = mayCarryACredential(connection.baseUrl);

  const submit = async () => {
    const trimmed = code.trim();
    if (!trimmed || busy) return;
    setBusy(true);
    setError(null);
    try {
      const host = getConnection(scope.connection);
      if (!host) throw new Error("this connection is no longer registered");
      const device = await pairDevice(
        scope.connection,
        host.baseUrl,
        trimmed,
        deviceLabel(),
      );
      pairedConnection(scope.connection, device.deviceId);
      setCode("");
    } catch (err) {
      // The host answers one indistinguishable message for every way a claim
      // can fail — unknown code, expired, already redeemed, suspended user. It
      // is passed through rather than re-interpreted here, because narrowing it
      // would leak which of those it was.
      setError(err instanceof Error ? err.message : "pairing failed");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-base">This device</CardTitle>
        <CardDescription>
          {paired
            ? "This machine is paired with this host and signs in as you."
            : encrypted
              ? `Pair this machine so it acts as you rather than anonymously. Ask this host's web console for a pairing code under ${CODE_SOURCE}.`
              : "This host is reached over an unencrypted connection, so pairing is unavailable — a session sent to it could be read by anyone on the network path."}
        </CardDescription>
      </CardHeader>
      <CardContent>
        {paired ? (
          <div className="flex items-center justify-between gap-3">
            <span className="inline-flex items-center gap-1.5 text-sm text-muted-foreground">
              <KeyRound className="size-3.5" />
              Paired
            </span>
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                setError(null);
                void forgetDevice(scope.connection)
                  .then(() => unpairConnection(scope.connection))
                  // Without this the rejection is unhandled, `unpairConnection`
                  // never runs, and the button appears to do nothing — the row
                  // still says "Paired" with no reason given.
                  .catch((err: unknown) =>
                    setError(
                      err instanceof Error ? err.message : "could not forget this device",
                    ),
                  );
              }}
            >
              Forget on this machine
            </Button>
          </div>
        ) : null}
        {paired && error ? (
          // The paired branch renders no form, so without this a failed forget
          // sets state nobody can see: the row goes on saying "Paired" and the
          // button appears to do nothing, which is the symptom the catch was
          // added to prevent.
          <p role="alert" className="mt-2 text-xs text-destructive">
            {error}
          </p>
        ) : null}
        {!paired && !encrypted ? (
          // No form at all, rather than a disabled one. A greyed-out field
          // invites a person to hunt for what would enable it, and the answer
          // is not on this screen — it is the host's address, which they
          // change by re-adding it over https.
          <p role="alert" data-testid="pairing-insecure-host" className="text-xs text-destructive">
            {connection.baseUrl} is not encrypted. Pair over https, or from a host running on
            this machine.
          </p>
        ) : null}
        {!paired && encrypted ? (
          <div className="space-y-2">
            <div className="flex items-center gap-2">
              <Input
                value={code}
                onChange={(e) => setCode(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") void submit();
                }}
                placeholder="Pairing code"
                autoComplete="off"
                spellCheck={false}
                className="font-mono text-xs"
                aria-label="Pairing code"
                disabled={busy}
              />
              <Button size="sm" onClick={() => void submit()} disabled={busy || !code.trim()}>
                {busy ? <Loader2 className="size-3.5 animate-spin" /> : "Pair"}
              </Button>
            </div>
            {error ? (
              <p role="alert" className="text-xs text-destructive">
                {error}
              </p>
            ) : null}
          </div>
        ) : null}
      </CardContent>
    </Card>
  );
}

/**
 * A name for this machine, shown in the host's device list.
 *
 * Best-effort and display-only — the host truncates it and treats it as
 * untrusted. A person revoking a device needs to recognise which one it is, and
 * "Mac" beats a bare id.
 */
function deviceLabel(): string | undefined {
  if (typeof navigator === "undefined") return undefined;
  const platform = navigator.platform || navigator.userAgent;
  return platform ? `OpenCompany desktop (${platform})` : undefined;
}
