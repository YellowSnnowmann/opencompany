import { useState } from "react";

import type { OpenCompanyClient } from "@/api/client";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  checkModelId,
  defaultModelPrefill,
  modelIdErrorCopy,
  replacesDifferentDefault as computeReplacesDifferentDefault,
} from "./connect";
import { ModelField } from "./ModelField";
import type { DefaultChoice, Provider } from "./types";

/**
 * "Set as default" — and, when a full default already exists, the confirm
 * step that names the old and new pair before it is replaced.
 *
 * Decision X1/X4 (orchestrator, 2026-09-15): there is no separate "clear the
 * default" action. Changing the default always goes through this dialog, and
 * it always requires a model (D-model, Q2) — a default is never a provider
 * alone. Setting the **first** default (nothing stored, or a bare-slug default
 * with no model to replace) saves directly; replacing a full `{provider,
 * model}` pair with a different one shows a confirm panel naming both first.
 */
export function DefaultModelDialog({
  client,
  company,
  provider,
  providers,
  defaultChoice,
  busy,
  error,
  onCancel,
  onSubmit,
}: {
  client: OpenCompanyClient;
  company: string | null;
  /** The row being set as default, or `null` when the dialog is closed. */
  provider: Provider | null;
  /** Every provider this company has, so the confirm step can name the old pair by its label rather than its slug (X7). */
  providers: readonly Pick<Provider, "slug" | "label">[];
  /** The company's current stored default, so a replace can be confirmed against it. */
  defaultChoice: DefaultChoice | null | undefined;
  busy: boolean;
  error: string | null;
  onCancel: () => void;
  onSubmit: (model: string) => void;
}) {
  const open = provider !== null;
  // Seeded once per open (keyed by the caller on `provider?.slug`), same
  // pattern as `ProviderConnectDialog`'s fields: an effect here would race the
  // dialog's own paint.
  const [model, setModel] = useState(() => (provider ? defaultModelPrefill(provider, defaultChoice) : ""));
  const [confirming, setConfirming] = useState(false);
  const modelError = model.trim() ? checkModelId(model) : "empty";
  const ready = modelError === null;

  if (!provider) return null;

  const wasLabel = defaultChoice
    ? (providers.find((p) => p.slug === defaultChoice.provider)?.label ?? defaultChoice.provider)
    : "";
  const replacesDifferentDefault = computeReplacesDifferentDefault(defaultChoice, provider.slug, model);

  const proceed = () => {
    if (!ready) return;
    if (replacesDifferentDefault && !confirming) {
      setConfirming(true);
      return;
    }
    onSubmit(model.trim());
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        // Live lane 1, KR-L1-05: ignored while `busy`, the same guard the
        // other dialogs in this rework use (round-2 review, P0-2) — a save
        // is in flight, and closing out from under it is exactly the
        // moment a stuck-looking control gets reported against a dialog
        // that already moved on.
        if (!next && !busy) {
          setConfirming(false);
          onCancel();
        }
      }}
    >
      <DialogContent className="sm:max-w-md" data-testid="inference-default-model-step">
        <DialogHeader>
          <DialogTitle>
            {confirming ? "Change the company default?" : `Set ${provider.label} as the default`}
          </DialogTitle>
          {confirming && (
            <DialogDescription>
              Every agent with no pinned pair of its own moves to this default the moment you
              confirm.
            </DialogDescription>
          )}
        </DialogHeader>

        {confirming ? (
          <div className="grid gap-2 text-sm" data-testid="inference-default-model-confirm">
            <p>
              <span className="text-muted-foreground">Was: </span>
              {wasLabel} · {defaultChoice?.model ?? "no model"}
            </p>
            <p>
              <span className="text-muted-foreground">Now: </span>
              {provider.label} · {model.trim()}
            </p>
          </div>
        ) : (
          <div className="grid gap-1.5">
            <ModelField
              client={client}
              company={company}
              slug={provider.slug}
              id="inference-default-model"
              value={model}
              disabled={busy}
              onChange={setModel}
            />
            {model.trim() && modelError && (
              <p className="text-xs text-status-blocked-text" data-testid="inference-model-id-error">
                {modelIdErrorCopy(modelError)}
              </p>
            )}
          </div>
        )}

        <p
          aria-live="polite"
          className="text-sm text-status-blocked-text empty:hidden"
          data-testid="inference-default-model-error"
        >
          {error ?? ""}
        </p>

        <DialogFooter>
          {confirming ? (
            <Button type="button" variant="outline" disabled={busy} onClick={() => setConfirming(false)}>
              Back
            </Button>
          ) : (
            <Button type="button" variant="outline" disabled={busy} onClick={onCancel}>
              Cancel
            </Button>
          )}
          <Button
            type="button"
            disabled={busy || !ready}
            data-testid="inference-default-model-submit"
            onClick={proceed}
          >
            {busy ? "Saving…" : confirming ? "Confirm" : replacesDifferentDefault ? "Continue" : "Make default"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
