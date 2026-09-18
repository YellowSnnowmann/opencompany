import type { OpenCompanyClient } from "@/api/client";
import { AdminOnlyNotice } from "@/components/admin-only-notice";
import { PageHeader } from "@/components/page-header";
import { useCanManage } from "@/hooks/use-can-manage";
import { ProvidersTab } from "@/inference/ProvidersTab";
import { useInference } from "@/inference/use-inference";

interface Props {
  client: OpenCompanyClient;
  company: string | null;
}

/**
 * What this company can reach a model through.
 *
 * It had a second tab, Routing, until the keys rework removed per-workload
 * routing (issue #2306, phase 5b). A company now has one default provider and
 * model, and an agent can pin its own (Team → the agent → Harness & model).
 * An old `?tab=routing` address is ignored and lands here, which is the page
 * the Routing tab's work moved to.
 */
export function InferenceView({ client, company }: Props) {
  // Changing the model or the key changes what every teammate's turn costs, so
  // it is an admin's.
  const canManage = useCanManage(client, company);
  const inference = useInference(client, company);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <PageHeader
        title="LLM"
        width="full"
        description="Configure AI providers, local models, and the agent chat tools."
      />
      <div className="min-h-0 w-full flex-1 space-y-6 overflow-y-auto px-4 py-6">
        {!canManage && (
          <AdminOnlyNotice
            testId="inference-read-only"
            title="Only an admin can change this company's model"
          >
            The model and its key decide what every agent&apos;s turn costs, so an admin sets
            them. You can see what is configured.
          </AdminOnlyNotice>
        )}
        <ProvidersTab client={client} company={company} state={inference} actions={inference} canManage={canManage} />
      </div>
    </div>
  );
}
