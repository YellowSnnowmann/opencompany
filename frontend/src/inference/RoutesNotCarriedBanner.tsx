import { Info } from "lucide-react";

import type { InferenceStatus } from "@/api/inference";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { routesNotCarriedCopy } from "./routes-not-carried";

/**
 * The one-release bridge for a company whose stored routing table named
 * something the boot-time carry (phase 5a) could not fold into a single
 * default — a mixed table, a table naming Managed, or one missing a tier.
 * Nothing here reads or writes routing itself; it is a status field the host
 * computed once, at boot, from a table nothing else reads any more (phase 5b).
 */
export function RoutesNotCarriedBanner({
  rows,
  canManage,
  onChooseDefault,
}: {
  rows: InferenceStatus["routesNotCarried"];
  canManage: boolean;
  /** Opens the add-provider or set-default flow. Absent: no button. */
  onChooseDefault?: () => void;
}) {
  const copy = routesNotCarriedCopy(rows);
  if (!copy) return null;
  return (
    <Alert variant="warning" data-testid="inference-routes-not-carried-banner">
      <Info className="size-4" />
      <AlertDescription>
        <p>{copy}</p>
        {canManage && onChooseDefault && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="mt-2"
            data-testid="inference-routes-not-carried-choose"
            onClick={onChooseDefault}
          >
            Choose a default
          </Button>
        )}
      </AlertDescription>
    </Alert>
  );
}
