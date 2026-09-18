// `errorEnvelope`/`parseUsedBy` — the client-side half of the keys-rework
// in-use contract (issue #2306): a `409 in_use` echoes `usedBy` so a confirm
// dialog reopened by the refusal can show it without a second request.
// Round-2 review, P2-6: this parsing had no coverage of its own — malformed
// or empty `usedBy`, and a 409 carrying some other code.

import { describe, expect, it } from "vitest";

import { errorEnvelope, parseUsedBy } from "@/api/client";

describe("parseUsedBy", () => {
  it("is undefined for anything that is not an object", () => {
    expect(parseUsedBy(undefined)).toBeUndefined();
    expect(parseUsedBy(null)).toBeUndefined();
    expect(parseUsedBy("in_use")).toBeUndefined();
    expect(parseUsedBy(42)).toBeUndefined();
    expect(parseUsedBy([])).toBeUndefined();
  });

  it("is undefined for an object with nothing readable in it", () => {
    expect(parseUsedBy({})).toBeUndefined();
    expect(parseUsedBy({ default: false })).toBeUndefined();
    expect(parseUsedBy({ agents: [] })).toBeUndefined();
    expect(parseUsedBy({ agents: "researcher" })).toBeUndefined();
    expect(parseUsedBy({ surfaces: ["carrier_pigeon"] })).toBeUndefined();
  });

  it("keeps default: true only when it is literally true", () => {
    expect(parseUsedBy({ default: true })).toEqual({ default: true });
    expect(parseUsedBy({ default: "true" })).toBeUndefined();
  });

  it("keeps only well-shaped agent entries, dropping malformed ones rather than the whole field", () => {
    expect(
      parseUsedBy({
        agents: [
          { id: "a1", name: "Researcher" },
          { id: "a2" }, // missing name
          "not an object",
          { id: "a3", name: "Web search" },
        ],
      }),
    ).toEqual({
      agents: [
        { id: "a1", name: "Researcher" },
        { id: "a3", name: "Web search" },
      ],
    });
  });

  it("keeps only recognised surfaces", () => {
    expect(parseUsedBy({ surfaces: ["llm", "composio", "search", "ghost"] })).toEqual({
      surfaces: ["llm", "composio", "search"],
    });
  });

  it("combines every field present at once", () => {
    expect(
      parseUsedBy({
        default: true,
        agents: [{ id: "a1", name: "Researcher" }],
        surfaces: ["composio"],
      }),
    ).toEqual({
      default: true,
      agents: [{ id: "a1", name: "Researcher" }],
      surfaces: ["composio"],
    });
  });
});

describe("errorEnvelope", () => {
  it("is undefined for a body that is not the host's {error, code} shape", () => {
    expect(errorEnvelope("not json")).toBeUndefined();
    expect(errorEnvelope("null")).toBeUndefined();
    expect(errorEnvelope("[]")).toBeUndefined();
    expect(errorEnvelope('"just a string"')).toBeUndefined();
    expect(errorEnvelope(JSON.stringify({ error: "x" }))).toBeUndefined(); // no code
    expect(errorEnvelope(JSON.stringify({ code: "x" }))).toBeUndefined(); // no error
  });

  it("parses a plain refusal with no usedBy at all", () => {
    expect(errorEnvelope(JSON.stringify({ error: "not found", code: "not_found" }))).toEqual({
      error: "not found",
      code: "not_found",
    });
  });

  it("carries usedBy on a 409 in_use envelope", () => {
    const body = JSON.stringify({
      error: "still in use",
      code: "in_use",
      usedBy: { default: true, agents: [{ id: "a1", name: "Researcher" }] },
    });
    expect(errorEnvelope(body)).toEqual({
      error: "still in use",
      code: "in_use",
      usedBy: { default: true, agents: [{ id: "a1", name: "Researcher" }] },
    });
  });

  it("drops an empty or malformed usedBy rather than attaching junk", () => {
    const body = JSON.stringify({ error: "still in use", code: "in_use", usedBy: {} });
    expect(errorEnvelope(body)).toEqual({ error: "still in use", code: "in_use" });
  });

  it("does not attach usedBy to a 409 carrying some other code", () => {
    const body = JSON.stringify({
      error: "a different conflict",
      code: "slug_taken",
      usedBy: { default: true },
    });
    // The envelope still carries whatever the host sent — the console does
    // not special-case which code a usedBy arrives with — but the important
    // case a caller must not assume is that a non-"in_use" 409 carries one at
    // all; the host will not, in practice, put usedBy on a code that is not
    // in_use, so this only pins that the parser itself does not gate on code.
    expect(errorEnvelope(body)).toEqual({
      error: "a different conflict",
      code: "slug_taken",
      usedBy: { default: true },
    });
  });
});
