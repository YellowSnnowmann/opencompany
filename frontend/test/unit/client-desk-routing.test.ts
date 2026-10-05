import { describe, expect, it } from "vitest";

import { OpenCompanyClient } from "@/api/client";
import type { Transport, TransportRequest, TransportResponse } from "@/api/transport";

/**
 * The routing and hive reads on the client hit the routes the host serves
 * (`/desks/{id}/routing`, `/agents/{id}/messages`) with the verbs the contract
 * names — a PUT that carried the block on the wrong route would 404 on every
 * save.
 */

function harness(payload: unknown = {}) {
  const sent: { method: string; url: string; body: unknown }[] = [];
  const transport: Transport = {
    request: async ({ method, url, body }: TransportRequest): Promise<TransportResponse> => {
      sent.push({ method, url, body: body ? JSON.parse(body) : undefined });
      return { status: 200, statusText: "", url, text: JSON.stringify(payload), header: () => null };
    },
    subscribe: () => () => {},
    cancelsInFlight: true,
  };
  const client = new OpenCompanyClient(
    { baseUrl: "", company: null, operatorToken: null, sessionHeader: null },
    transport,
  );
  return { client, sent };
}

describe("desk routing on the client", () => {
  it("reads, installs and resets a desk's block on its own route", async () => {
    const { client, sent } = harness({ deskId: "eng desk", source: "overlay", declared: {}, effective: {}, candidates: [] });
    await client.getDeskRouting("eng desk", "acme");
    await client.putDeskRouting("eng desk", { round_width: 2 }, "acme");
    await client.resetDeskRouting("eng desk", "acme");
    expect(sent.map((r) => `${r.method} ${r.url}`)).toEqual([
      "GET /api/v1/companies/acme/desks/eng%20desk/routing",
      "PUT /api/v1/companies/acme/desks/eng%20desk/routing",
      "DELETE /api/v1/companies/acme/desks/eng%20desk/routing",
    ]);
    expect(sent[1].body).toEqual({ round_width: 2 });
  });

  it("reads an agent's direct hive messages with only the cursor the caller gave", async () => {
    const { client, sent } = harness([]);
    await client.listAgentHiveMessages("eng lead", {}, "acme");
    await client.listAgentHiveMessages("eng lead", { after: 41, limit: 5 }, "acme");
    expect(sent.map((r) => `${r.method} ${r.url}`)).toEqual([
      "GET /api/v1/companies/acme/agents/eng%20lead/messages",
      "GET /api/v1/companies/acme/agents/eng%20lead/messages?after=41&limit=5",
    ]);
  });

  it("no longer knows the retired episodes route", () => {
    const { client } = harness();
    expect((client as unknown as Record<string, unknown>).listEpisodes).toBeUndefined();
  });

  it("no longer knows the retired hive route", () => {
    const { client } = harness();
    expect((client as unknown as Record<string, unknown>).getDeskHive).toBeUndefined();
  });
});
