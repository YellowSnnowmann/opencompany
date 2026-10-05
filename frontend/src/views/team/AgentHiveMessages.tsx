// One teammate's direct lines with other teammates in the company hive (OC-2).
//
// Teammates reach each other with `hivemind_send_agent`, and the answer comes
// back on a later turn. Those lines are between two agents, so they never land
// on a desk and the session stream above does not carry them; the host serves
// them from `GET {scope}/agents/{agentId}/messages`, oldest first, which is all
// this reads. Read once per open, like the session itself — a reader who wants
// the newest presses the tab again.

import { useEffect, useState } from "react";
import { ArrowRight } from "lucide-react";

import type { OpenCompanyClient } from "@/api/client";
import type { AgentHiveMessageDto } from "@/api/types";

/** How many direct lines one read asks for. */
const HIVE_MESSAGES_PAGE = 200;

/** A manifest id as a person reads it: the roster's name, else the id. */
function nameOf(id: string, agentNames?: Readonly<Record<string, string>>): string {
  return agentNames?.[id] ?? id;
}

export function AgentHiveMessages({
  client,
  company,
  agentId,
  agentNames,
}: {
  client: OpenCompanyClient;
  company: string | null;
  agentId: string;
  agentNames?: Readonly<Record<string, string>>;
}) {
  const [rows, setRows] = useState<AgentHiveMessageDto[] | null>(null);

  useEffect(() => {
    let live = true;
    setRows(null);
    client
      .listAgentHiveMessages(agentId, { limit: HIVE_MESSAGES_PAGE }, company)
      .then((answer) => {
        if (live) setRows(answer);
      })
      // A host without the route, or a failed read, shows nothing rather than
      // a red box: the session above is the primary record.
      .catch(() => {
        if (live) setRows([]);
      });
    return () => {
      live = false;
    };
  }, [client, company, agentId]);

  if (!rows || rows.length === 0) return null;
  return (
    <section className="space-y-2" data-testid="agent-hive-messages">
      <h3 className="text-xs font-semibold text-muted-foreground">
        With teammates · {rows.length} direct message{rows.length === 1 ? "" : "s"}
      </h3>
      <ol className="space-y-2">
        {rows.map((row) => (
          <li key={row.seq} className="text-sm" data-testid="agent-hive-message">
            <div className="flex items-center gap-1 text-2xs text-muted-foreground">
              <span className="font-medium text-foreground">{nameOf(row.sender, agentNames)}</span>
              <ArrowRight className="size-3" aria-hidden />
              <span className="font-medium text-foreground">{nameOf(row.recipient, agentNames)}</span>
              <span>· {new Date(row.atMillis).toLocaleString()}</span>
            </div>
            <p className="leading-relaxed whitespace-pre-wrap">{row.text}</p>
          </li>
        ))}
      </ol>
    </section>
  );
}
