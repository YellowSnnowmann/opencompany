import { useState } from "react";
import { Loader2 } from "lucide-react";
import { toast } from "sonner";

import { clearPaypal, savePaypal, type PaypalStatus } from "@/api/billing";
import type { OpenCompanyClient } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

interface Props {
  client: OpenCompanyClient;
  company: string | null;
  status: PaypalStatus;
  onStatus: (status: PaypalStatus) => void;
}

/**
 * The PayPal credential form.
 *
 * A form, not a "Connect PayPal" button: these tools read the company's **own**
 * wallet, so there is no third party for an OAuth popup to ask permission of.
 *
 * The environment is stored rather than inferred, because sandbox and live
 * credentials are indistinguishable from the credential itself — a sandbox
 * client id against the live host fails with "invalid client", which reads as a
 * typo rather than as pointing at the wrong world.
 */
export function PaypalForm({ client, company, status, onStatus }: Props) {
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  const [environment, setEnvironment] = useState(status.environment || "sandbox");
  const [busy, setBusy] = useState(false);

  async function onSave() {
    const body: Record<string, string> = {};
    if (clientId.trim()) body.clientId = clientId.trim();
    if (clientSecret.trim()) body.clientSecret = clientSecret.trim();
    if (environment !== (status.environment ?? "sandbox")) body.environment = environment;

    if (Object.keys(body).length === 0) {
      toast.info("Nothing to save — fill in a field first.");
      return;
    }
    setBusy(true);
    try {
      onStatus(await savePaypal(client, company, body));
      setClientId("");
      setClientSecret("");
      toast.success("PayPal settings saved.");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Could not save PayPal settings.");
    } finally {
      setBusy(false);
    }
  }

  async function onClear() {
    setBusy(true);
    try {
      const next = await clearPaypal(client, company);
      onStatus(next);
      setClientId("");
      setClientSecret("");
      setEnvironment(next.environment || "sandbox");
      toast.success("PayPal disconnected.");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Could not disconnect PayPal.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="space-y-4" data-testid="paypal-form">
      <div className="grid gap-4 sm:grid-cols-2">
        <div className="space-y-2">
          <Label htmlFor="pp-id">Client ID</Label>
          <Input
            id="pp-id"
            data-testid="paypal-client-id"
            type="password"
            autoComplete="off"
            placeholder={
              status.clientIdConfigured
                ? "Configured — type to replace"
                : "From developer.paypal.com → Apps & Credentials"
            }
            value={clientId}
            onChange={(e) => setClientId(e.target.value)}
          />
        </div>
        <div className="space-y-2">
          <Label htmlFor="pp-secret">Client secret</Label>
          <Input
            id="pp-secret"
            data-testid="paypal-client-secret"
            type="password"
            autoComplete="off"
            placeholder={status.clientSecretConfigured ? "Configured — type to replace" : "Secret"}
            value={clientSecret}
            onChange={(e) => setClientSecret(e.target.value)}
          />
        </div>
      </div>

      <div className="space-y-2">
        <Label htmlFor="pp-env">Environment</Label>
        <select
          id="pp-env"
          data-testid="paypal-environment"
          className="h-9 w-full rounded-md border border-input bg-transparent px-3 text-sm"
          value={environment}
          onChange={(e) => setEnvironment(e.target.value)}
        >
          <option value="sandbox">Sandbox — developer.paypal.com test accounts</option>
          <option value="live">Live — real money</option>
        </select>
        <p className="text-xs text-muted-foreground">
          Sandbox and live credentials are not interchangeable. Picking the wrong one fails with
          &ldquo;invalid client&rdquo;, which reads like a typo.
        </p>
      </div>

      <div className="flex gap-2">
        <Button onClick={onSave} disabled={busy} data-testid="paypal-save">
          {busy ? <Loader2 className="mr-2 size-4 animate-spin" /> : null}
          Save
        </Button>
        {status.clientIdConfigured || status.clientSecretConfigured ? (
          <Button variant="ghost" onClick={onClear} disabled={busy} data-testid="paypal-clear">
            Disconnect
          </Button>
        ) : null}
      </div>
    </div>
  );
}
