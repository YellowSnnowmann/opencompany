// The memory API module's wire shapes over the OpenHuman memory v2 contract:
// which route each call hits and what body it sends. The view tests drive the
// page; this pins the URLs the Rust side serves, so a renamed route fails here
// rather than as a blank Brain.
import { describe, expect, it, vi } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import {
  brainSources,
  createMemory,
  deleteMemory,
  forgetAgentMemory,
  forgetDocument,
  listMemory,
  memoryAgents,
  memoryStatus,
  recallMemory,
} from "@/api/memory";

function client() {
  const get = vi.fn(() => Promise.resolve({}));
  const post = vi.fn(() => Promise.resolve({}));
  const del = vi.fn(() => Promise.resolve({}));
  const c = { scopeFor: () => "/api/v1/company/acme", get, post, del };
  return { c: c as unknown as OpenCompanyClient, get, post, del };
}

describe("memory api routes", () => {
  it("lists with no query string when unfiltered", async () => {
    const { c, get } = client();
    await listMemory(c, "acme");
    expect(get).toHaveBeenCalledWith("/api/v1/company/acme/memory");
  });

  it("lists with every filter and the cursor", async () => {
    const { c, get } = client();
    await listMemory(c, "acme", {
      query: "friday",
      kind: "conversation",
      agent: "ceo",
      cursor: "abc",
      limit: 50,
    });
    expect(get).toHaveBeenCalledWith(
      "/api/v1/company/acme/memory?query=friday&kind=conversation&agent=ceo&cursor=abc&limit=50",
    );
  });

  it("creates a learning with its text and kind", async () => {
    const { c, post } = client();
    await createMemory(c, "acme", { text: "Ship on Fridays.", kind: "procedure" });
    expect(post).toHaveBeenCalledWith("/api/v1/company/acme/memory", {
      text: "Ship on Fridays.",
      kind: "procedure",
    });
  });

  it("deletes an item by its encoded id", async () => {
    const { c, del } = client();
    await deleteMemory(c, "acme", "a/b");
    expect(del).toHaveBeenCalledWith("/api/v1/company/acme/memory/a%2Fb");
  });

  it("reads status, agents and brain sources", async () => {
    const { c, get } = client();
    await memoryStatus(c, "acme");
    await memoryAgents(c, "acme");
    await brainSources(c, "acme");
    expect(get.mock.calls.map((call) => (call as unknown[])[0])).toEqual([
      "/api/v1/company/acme/memory/status",
      "/api/v1/company/acme/memory/agents",
      "/api/v1/company/acme/memory/brain",
    ]);
  });

  it("forgets one agent's memory and one document source", async () => {
    const { c, del } = client();
    await forgetAgentMemory(c, "acme", "ceo");
    await forgetDocument(c, "acme", "markdown");
    expect(del.mock.calls.map((call) => (call as unknown[])[0])).toEqual([
      "/api/v1/company/acme/memory/agents/ceo",
      "/api/v1/company/acme/memory/document/markdown",
    ]);
  });

  it("recalls with the agent only when one is given", async () => {
    const { c, post } = client();
    await recallMemory(c, "acme", "when do we ship?");
    await recallMemory(c, "acme", "when do we ship?", "ceo");
    expect(post.mock.calls).toEqual([
      ["/api/v1/company/acme/memory/recall", { question: "when do we ship?" }],
      ["/api/v1/company/acme/memory/recall", { question: "when do we ship?", agent: "ceo" }],
    ]);
  });
});
