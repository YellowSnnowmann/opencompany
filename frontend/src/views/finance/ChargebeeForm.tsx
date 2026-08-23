import { useState } from "react";
import { Copy, Loader2 } from "lucide-react";
import { toast } from "sonner";

import {
  clearBilling,
  saveBilling,
  type BillingStatus,
} from "@/api/billing";
import type { OpenCompanyClient } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

interface Props {
  client: OpenCompanyClient;
  company: string | null;
  status: BillingStatus;
  /** Hands the fresh status back so the panel's verdict updates in place. */
  onStatus: (status: BillingStatus) => void;
}

/**
 * The Chargebee credential form — the site, the API key and the webhook
 * credential.
 *
 * Lifted out of `BillingView` unchanged in behaviour, because the behaviour is
 * the careful part:
 *
 * **Credentials are write-only.** The host never returns the API key or the
 * webhook credential, so neither is ever rendered. A stored key shows as
 * "Configured" with an empty input whose placeholder says typing replaces it —
 * an input pre-filled with dots invites an operator to "correct" a value they
 * cannot see, and submitting the dots would store the dots.
 *
 * **Save is a patch.** Only non-empty fields are sent, so correcting the site
 * never means re-typing a key the operator cannot read back.
 */
export function ChargebeeForm({ client, company, status, onStatus }: Props) {
  const [apiKey, setApiKey] = useState("");
  const [site, setSite] = useState(status.site ?? "");
  const [webhookSecret, setWebhookSecret] = useState("");
  const [busy, setBusy] = useState(false);

  async function onSave() {
    const body: Record<string, string> = {};
    if (apiKey.trim()) body.apiKey = apiKey.trim();
    if (site.trim() && site.trim() !== (status.site ?? "")) body.site = site.trim();
    if (webhookSecret.trim()) body.webhookSecret = webhookSecret.trim();

    if (Object.keys(body).length === 0) {
      toast.info("Nothing to save — fill in a field first.");
      return;
    }

    setBusy(true);
    try {
      onStatus(await saveBilling(client, company, body));
      // Clear the secret inputs on success: leaving a key sitting in a form
      // field after it has been stored is one stray screen-share from a leak.
      setApiKey("");
      setWebhookSecret("");
      toast.success("Chargebee settings saved.");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Could not save Chargebee settings.");
    } finally {
      setBusy(false);
    }
  }

  async function onClear() {
    setBusy(true);
    try {
      const next = await clearBilling(client, company);
      onStatus(next);
      setApiKey("");
      setWebhookSecret("");
      setSite("");
      toast.success("Chargebee credentials cleared.");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Could not clear the credentials.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="space-y-4" data-testid="chargebee-form">
      <div className="grid gap-4 sm:grid-cols-2">
        <div className="space-y-2">
          <Label htmlFor="cb-site">Site</Label>
          <Input
            id="cb-site"
            data-testid="billing-site"
            placeholder="acme-test"
            value={site}
            onChange={(e) => setSite(e.target.value)}
          />
          <p className="text-xs text-muted-foreground">
            The part before <code>.chargebee.com</code>. Pasting the full URL is fine.
          </p>
        </div>

        <div className="space-y-2">
          <Label htmlFor="cb-key">API key</Label>
          <Input
            id="cb-key"
            data-testid="billing-api-key"
            type="password"
            autoComplete="off"
            placeholder={
              status.apiKeyConfigured ? "Configured — type to replace" : "From Chargebee → API Keys"
            }
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
          />
          <p className="text-xs text-muted-foreground">
            {status.apiKeyConfigured
              ? "Stored. It is never shown again."
              : "Stored write-only; it is never shown again."}
          </p>
        </div>
      </div>

      <div className="space-y-2 border-t pt-4">
        <Label>Payment notifications</Label>
        <p className="text-xs text-muted-foreground">
          Optional. Without this, invoicing still works — nobody is just told when a customer pays.
        </p>
        {status.webhookUrl ? (
          <div className="flex gap-2">
            <Input readOnly value={status.webhookUrl} data-testid="billing-webhook-url" />
            <Button
              variant="outline"
              size="icon"
              onClick={async () => {
                // Await before claiming success: clipboard writes are refused in
                // some browsers and over plain http, and a toast saying "copied"
                // over an empty clipboard sends an operator to paste nothing
                // into Chargebee.
                try {
                  await navigator.clipboard.writeText(status.webhookUrl ?? "");
                  toast.success("Webhook URL copied.");
                } catch {
                  toast.error("Could not copy — select the URL and copy it manually.");
                }
              }}
            >
              <Copy className="size-4" />
            </Button>
          </div>
        ) : (
          // Deliberately not a disabled box showing a loopback address:
          // Chargebee cannot deliver to one, and showing it would send an
          // operator to configure a webhook that silently never arrives.
          <p className="text-xs text-muted-foreground" data-testid="billing-no-webhook-url">
            This host has no public URL, so Chargebee cannot reach it. Set{" "}
            <code>OPENCOMPANY_PUBLIC_URL</code> to an https address to get a webhook URL.
          </p>
        )}
        <Input
          id="cb-hook"
          data-testid="billing-webhook-secret"
          type="password"
          autoComplete="off"
          placeholder={status.webhookConfigured ? "Configured — type to replace" : "username:password"}
          value={webhookSecret}
          onChange={(e) => setWebhookSecret(e.target.value)}
        />
        <p className="text-xs text-muted-foreground">
          Invent a username and password, save them here as <code>username:password</code>, then set
          the same pair in Chargebee under{" "}
          <em>Protect webhook URL with basic authentication</em>. Subscribe to{" "}
          <code>payment_succeeded</code> and <code>payment_failed</code>.
        </p>
      </div>

      <div className="flex gap-2">
        <Button onClick={onSave} disabled={busy} data-testid="billing-save">
          {busy ? <Loader2 className="mr-2 size-4 animate-spin" /> : null}
          Save
        </Button>
        {status.apiKeyConfigured || status.webhookConfigured ? (
          <Button variant="ghost" onClick={onClear} disabled={busy} data-testid="billing-clear">
            Disconnect
          </Button>
        ) : null}
      </div>
    </div>
  );
}
