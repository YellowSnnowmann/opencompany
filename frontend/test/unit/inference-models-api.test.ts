import { describe, expect, it } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import { listInferenceModels } from "@/api/inference";

describe("the inference model catalog client", () => {
  it("gets the addressed company's model-list route", async () => {
    const calls: string[] = [];
    const client = {
      scopeFor: (company: string | null) =>
        company ? `/api/v1/companies/${company}` : "/api/v1/company",
      get: async (path: string) => {
        calls.push(path);
        return {
          baseUrl: "https://provider.example/v1",
          models: [{ id: "provider/model", name: "Model" }],
        };
      },
    } as unknown as OpenCompanyClient;

    // The catalog travels whole: the route answers with the endpoint that was
    // read as well as the ids, not just a bare list — the console needs
    // `baseUrl` to say *whose* list it is showing.
    await expect(listInferenceModels(client, "acme")).resolves.toEqual({
      baseUrl: "https://provider.example/v1",
      models: [{ id: "provider/model", name: "Model" }],
    });
    expect(calls).toEqual(["/api/v1/companies/acme/inference/models"]);
  });

  it("passes an unreadable catalog's explanation through rather than an empty list alone", async () => {
    // An unreadable catalog is a 200 carrying `error`, so the console can name
    // the endpoint that did not answer instead of rendering an empty picker
    // that reads as "this provider publishes no models".
    const client = {
      scopeFor: () => "/api/v1/company",
      get: async () => ({
        baseUrl: "http://localhost:11434/v1",
        models: [],
        error: "Could not list models from http://localhost:11434/v1: connection refused.",
      }),
    } as unknown as OpenCompanyClient;

    const catalog = await listInferenceModels(client, null);
    expect(catalog.models).toEqual([]);
    expect(catalog.error).toContain("http://localhost:11434/v1");
  });
});
