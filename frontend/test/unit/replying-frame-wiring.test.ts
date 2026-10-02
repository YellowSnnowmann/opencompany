import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

/**
 * The host's `replying` frame (an agent started writing its reply) is wired
 * through three files that only meet at runtime, so this pins the seams in the
 * source-contract idiom of `chat-rail-focus.test.ts`:
 *
 *  - `use-events` routes it to `onTurnEvent`, not the `default` arm that warns;
 *  - the shell's `onTurnEvent` accepts it and folds it through `foldTurnFrame`,
 *    which never makes it a row (behaviour in `live-frame-replying.test.ts`);
 *  - the shell gives it no timer (a person's `typing` frame expires after 8s, an
 *    agent's reply can stream longer).
 */

const here = dirname(fileURLToPath(import.meta.url));
const read = (rel: string) => readFileSync(resolve(here, "../../src", rel), "utf8");

describe("the replying frame", () => {
  const events = read("hooks/use-events.ts");
  const shell = read("components/app-shell.tsx");

  it("is a member of the frame union and routed with the other turn frames", () => {
    expect(events).toContain('type: "replying";');
    expect(events).toMatch(/case "thinking":\s*case "replying":\s*onTurnEvent\?\.\(event\);/);
  });

  it("is accepted by the shell and folded through the rule that keeps it out of the rows", () => {
    // The behaviour itself is pinned in `live-frame-replying.test.ts`; this pins
    // that the shell runs that rule rather than a copy of it.
    expect(shell).toContain('event.type !== "replying"');
    expect(shell).toContain("foldTurnFrame(prev[rowKey] ?? [], event)");
    expect(shell).not.toContain("foldLiveFrame(");
    expect(shell).toContain("frameTurnMeta(threadId, event.type, Date.now())");
  });

  it("has no expiry of its own", () => {
    const start = shell.indexOf("frameTurnMeta(threadId");
    const body = shell.slice(start - 400, start + 200);
    expect(body).not.toMatch(/setTimeout|expire|TTL/i);
  });
});
