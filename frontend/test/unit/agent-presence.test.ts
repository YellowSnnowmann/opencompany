import { describe, expect, it } from "vitest";

import {
  RUN_STATUS_CAP,
  STALE_TURN_MS,
  approvalAgentCounts,
  clearedOnThread,
  derivePresence,
  dropTurnMeta,
  inflightAgentCounts,
  presenceChatKey,
  presenceIn,
  presenceOf,
  recordRunStatus,
  sameCounts,
  settledInChat,
  staleTurnMeta,
  strongerPresence,
  type PresenceInputs,
} from "@/lib/agent-presence";

/**
 * `lib/agent-presence.ts`: the six-state answer to "what is this agent doing".
 * Pure, so every rule is asserted on plain objects: the precedence matrix, an
 * approval outliving its turn, queued, the DM-thread fallback, and the age-out.
 */

const NOW = 1_000_000_000;
const inputs = (over: Partial<PresenceInputs> = {}): PresenceInputs => ({
  openTurns: {},
  liveStepsByThread: {},
  liveStepsByMessage: {},
  liveAgentByTurn: {},
  turnMeta: {},
  ledgerTurns: [],
  approvalAgents: {},
  inflightAgents: {},
  runStatuses: {},
  threadAgents: {},
  now: NOW,
  ...over,
});
const meta = (chatId: string, over: { replying?: boolean; lastFrameAt?: number } = {}) => ({
  chatId,
  replying: false,
  lastFrameAt: NOW - 1000,
  ...over,
});

describe("strongerPresence", () => {
  it("orders approval > working > typing > thinking > queued > inactive", () => {
    const order = ["approval", "working", "typing", "thinking", "queued", "inactive"] as const;
    for (let i = 0; i < order.length; i++) {
      for (let j = i + 1; j < order.length; j++) {
        expect(strongerPresence(order[j], order[i])).toBe(order[i]);
        expect(strongerPresence(order[i], order[j])).toBe(order[i]);
      }
    }
  });
});

describe("derivePresence", () => {
  it("reads inactive when nothing is open", () => {
    const index = derivePresence(inputs());
    expect(presenceOf(index, "rae")).toBe("inactive");
    expect(presenceIn(index, "rae", "rae")).toBe("inactive");
  });

  it("reads thinking for an accepted turn on a DM before any frame, via the thread's teammate", () => {
    const index = derivePresence(
      inputs({
        openTurns: { rae: [{ queued: false, chatId: "rae" }] },
        threadAgents: { rae: "rae" },
      }),
    );
    expect(presenceIn(index, "rae", "rae")).toBe("thinking");
    expect(presenceOf(index, "rae")).toBe("thinking");
  });

  it("joins the dm:<id> thread of a teammate named like a general channel", () => {
    const index = derivePresence(
      inputs({
        openTurns: { "dm:general": [{ queued: false, chatId: "dm:general" }] },
        threadAgents: { "dm:general": "general" },
      }),
    );
    expect(presenceIn(index, "general", "dm:general")).toBe("thinking");
  });

  it("lights a bare-id DM from a hive seat's dm:<id> bracket (one key per DM)", () => {
    // The real shape: the console addresses Rae's DM as `rae`, the hive seat
    // that answers it brackets its turn under `dm:rae`, and streams no frames.
    const index = derivePresence(
      inputs({
        ledgerTurns: [{ agentId: "rae", chatId: "dm:rae", startedAtMillis: NOW - 5000 }],
        threadAgents: { rae: "rae" },
      }),
    );
    expect(presenceIn(index, "rae", "rae")).toBe("working");
    expect(presenceIn(index, "rae", "dm:rae")).toBe("working");
    // A queued chat-route turn on the bare id and the seat's bracket are one
    // conversation, so the stronger state wins rather than two keys splitting it.
    const both = derivePresence(
      inputs({
        openTurns: { rae: [{ queued: true, chatId: "rae" }] },
        ledgerTurns: [{ agentId: "rae", chatId: "dm:rae", startedAtMillis: NOW - 5000 }],
        threadAgents: { rae: "rae" },
      }),
    );
    expect(presenceIn(both, "rae", "rae")).toBe("working");
  });

  it("presenceChatKey folds a DM's two spellings and leaves desks, pairs and #general alone", () => {
    const threads = { rae: "rae", "dm:general": "general" };
    expect(presenceChatKey("rae", threads)).toBe("dm:rae");
    expect(presenceChatKey("dm:rae", threads)).toBe("dm:rae");
    expect(presenceChatKey("dm:general", threads)).toBe("dm:general");
    expect(presenceChatKey("general", threads)).toBe("general");
    expect(presenceChatKey("engineering", threads)).toBe("engineering");
    expect(presenceChatKey("dm:ada+rae", threads)).toBe("dm:ada+rae");
  });

  it("does not light a teammate's DM from the #general channel", () => {
    const index = derivePresence(
      inputs({
        ledgerTurns: [{ agentId: "general", chatId: "general", startedAtMillis: NOW - 5000 }],
        threadAgents: { "dm:general": "general" },
      }),
    );
    expect(presenceIn(index, "general", "dm:general")).toBe("inactive");
    expect(presenceIn(index, "general", "general")).toBe("working");
  });

  it("reads queued only when every open turn is queued, and thinking beats it", () => {
    const queued = { queued: true, chatId: "rae" };
    const running = { queued: false, chatId: "rae" };
    const all = derivePresence(inputs({ openTurns: { rae: [queued, queued] }, threadAgents: { rae: "rae" } }));
    expect(presenceOf(all, "rae")).toBe("queued");
    const mixed = derivePresence(inputs({ openTurns: { rae: [running, queued] }, threadAgents: { rae: "rae" } }));
    expect(presenceOf(mixed, "rae")).toBe("thinking");
  });

  it("prefers the agent the frames named over the one the host started the turn on", () => {
    const index = derivePresence(
      inputs({
        openTurns: { desk: [{ queued: false, chatId: "desk", agentId: "ceo" }] },
        liveAgentByTurn: { desk: "engineer" },
        turnMeta: { desk: meta("desk") },
      }),
    );
    expect(presenceOf(index, "engineer")).toBe("thinking");
    expect(presenceOf(index, "ceo")).toBe("inactive");
  });

  it("reads working while a tool row runs, and thinking again once it finishes", () => {
    const running = derivePresence(
      inputs({
        liveAgentByTurn: { rae: "rae" },
        turnMeta: { rae: meta("rae") },
        liveStepsByThread: { rae: [{ status: "running" }] },
      }),
    );
    expect(presenceOf(running, "rae")).toBe("working");
    const done = derivePresence(
      inputs({
        liveAgentByTurn: { rae: "rae" },
        turnMeta: { rae: meta("rae") },
        liveStepsByThread: { rae: [{ status: "ok" }] },
      }),
    );
    expect(presenceOf(done, "rae")).toBe("thinking");
  });

  it("reads typing after a replying frame, and working wins if a tool is still running", () => {
    const typing = derivePresence(
      inputs({ liveAgentByTurn: { "m:1": "rae" }, turnMeta: { "m:1": meta("rae", { replying: true }) } }),
    );
    expect(presenceOf(typing, "rae")).toBe("typing");
    const both = derivePresence(
      inputs({
        liveAgentByTurn: { "m:1": "rae" },
        turnMeta: { "m:1": meta("rae", { replying: true }) },
        liveStepsByMessage: { "m:1": [{ status: "running" }] },
      }),
    );
    expect(presenceOf(both, "rae")).toBe("working");
  });

  it("has no timer on typing: a long reply stays typing until a frame or settle resets it", () => {
    const index = derivePresence(
      inputs({
        liveAgentByTurn: { rae: "rae" },
        turnMeta: { rae: meta("rae", { replying: true, lastFrameAt: NOW - 60_000 }) },
      }),
    );
    expect(presenceOf(index, "rae")).toBe("typing");
  });

  it("scopes a chat lookup to that chat, but not an approval", () => {
    const index = derivePresence(
      inputs({ liveAgentByTurn: { desk: "rae" }, turnMeta: { desk: meta("desk") } }),
    );
    expect(presenceIn(index, "rae", "desk")).toBe("thinking");
    expect(presenceIn(index, "rae", "rae")).toBe("inactive");
    const approving = derivePresence(inputs({ approvalAgents: { rae: 1 } }));
    expect(presenceIn(approving, "rae", "rae")).toBe("approval");
  });

  it("keeps approval after the turn has settled and counts several as one state", () => {
    const index = derivePresence(inputs({ approvalAgents: { rae: 3 } }));
    expect(presenceOf(index, "rae")).toBe("approval");
    expect(presenceOf(derivePresence(inputs({ approvalAgents: { rae: 0 } })), "rae")).toBe("inactive");
  });

  it("wins for approval over a running tool, and reads a parked live row as approval", () => {
    const parked = derivePresence(
      inputs({
        liveAgentByTurn: { rae: "rae" },
        turnMeta: { rae: meta("rae") },
        liveStepsByThread: { rae: [{ status: "awaiting_approval" }, { status: "running" }] },
      }),
    );
    expect(presenceOf(parked, "rae")).toBe("approval");
    const listed = derivePresence(
      inputs({
        liveAgentByTurn: { rae: "rae" },
        turnMeta: { rae: meta("rae") },
        liveStepsByThread: { rae: [{ status: "running" }] },
        approvalAgents: { rae: 1 },
      }),
    );
    expect(presenceOf(listed, "rae")).toBe("approval");
  });

  it("reads a ledger-only seat as working, and defers to the frames when they speak", () => {
    const seat = derivePresence(
      inputs({ ledgerTurns: [{ agentId: "seat", chatId: "desk", startedAtMillis: NOW - 5000 }] }),
    );
    expect(presenceOf(seat, "seat")).toBe("working");
    const framed = derivePresence(
      inputs({
        ledgerTurns: [{ agentId: "rae", chatId: "desk", startedAtMillis: NOW - 5000 }],
        liveAgentByTurn: { desk: "rae" },
        turnMeta: { desk: meta("desk", { replying: true }) },
      }),
    );
    expect(presenceOf(framed, "rae")).toBe("typing");
  });

  it("ignores a chat-route bracket (no agent) on a desk, where the thread names nobody", () => {
    const index = derivePresence(inputs({ ledgerTurns: [{ chatId: "desk", startedAtMillis: NOW }] }));
    expect(index.byAgent.size).toBe(0);
  });

  it("reads a DM turn another console sent as queued, then thinking once its run is running", () => {
    // The chat route's bracket names no agent, and the run id is its turn id.
    const bracket = { key: "run-1", chatId: "rae", startedAtMillis: NOW - 1000 };
    const at = (runStatuses: Record<string, string>) =>
      derivePresence(inputs({ ledgerTurns: [bracket], threadAgents: { rae: "rae" }, runStatuses }));
    expect(presenceIn(at({}), "rae", "rae")).toBe("queued");
    expect(presenceIn(at({ "run-1": "pending" }), "rae", "rae")).toBe("queued");
    expect(presenceIn(at({ "run-1": "running" }), "rae", "rae")).toBe("thinking");
    expect(presenceOf(at({ "run-1": "running" }), "rae")).toBe("thinking");
    // Ended, but the settle never arrived: nothing, rather than a stuck dot.
    expect(presenceIn(at({ "run-1": "completed" }), "rae", "rae")).toBe("inactive");
    // Its frames, once they come, speak for it.
    const framed = derivePresence(
      inputs({
        ledgerTurns: [bracket],
        threadAgents: { rae: "rae" },
        liveAgentByTurn: { rae: "rae" },
        turnMeta: { rae: meta("rae", { replying: true }) },
      }),
    );
    expect(presenceIn(framed, "rae", "rae")).toBe("typing");
  });

  it("does not count this console's own turn twice through its bracket", () => {
    const index = derivePresence(
      inputs({
        openTurns: { rae: [{ queued: false, chatId: "rae", turnId: "run-1" }] },
        ledgerTurns: [{ key: "run-1", chatId: "rae", startedAtMillis: NOW - 1000 }],
        threadAgents: { rae: "rae" },
      }),
    );
    // The open turn says it holds the lock; the bracket alone would say queued.
    expect(presenceIn(index, "rae", "rae")).toBe("thinking");
    const queuedOwn = derivePresence(
      inputs({
        openTurns: { rae: [{ queued: true, chatId: "rae", turnId: "run-1" }] },
        ledgerTurns: [{ key: "run-1", chatId: "rae", startedAtMillis: NOW - 1000 }],
        threadAgents: { rae: "rae" },
        runStatuses: { "run-1": "running" },
      }),
    );
    expect(presenceIn(queuedOwn, "rae", "rae")).toBe("queued");
  });

  it("skips a seat bracket only where the frames describe that agent, not everywhere", () => {
    // Rae is framed on the engineering desk while her seat turn runs in her DM.
    const index = derivePresence(
      inputs({
        ledgerTurns: [{ key: "seat-1", agentId: "rae", chatId: "dm:rae", startedAtMillis: NOW - 1000 }],
        liveAgentByTurn: { engineering: "rae" },
        turnMeta: { engineering: meta("engineering") },
        threadAgents: { rae: "rae" },
      }),
    );
    expect(presenceIn(index, "rae", "rae")).toBe("working");
    expect(presenceIn(index, "rae", "engineering")).toBe("thinking");
    // In the chat the frames describe, the frames win.
    const same = derivePresence(
      inputs({
        ledgerTurns: [{ key: "seat-1", agentId: "rae", chatId: "engineering", startedAtMillis: NOW - 1000 }],
        liveAgentByTurn: { engineering: "rae" },
        turnMeta: { engineering: meta("engineering") },
      }),
    );
    expect(presenceIn(same, "rae", "engineering")).toBe("thinking");
  });

  it("an inflight card run reads working agent-wide, not on the DM row", () => {
    const index = derivePresence(inputs({ inflightAgents: { rae: 1 }, threadAgents: { rae: "rae" } }));
    expect(presenceOf(index, "rae")).toBe("working");
    expect(presenceIn(index, "rae", "rae")).toBe("inactive");
    expect(presenceOf(derivePresence(inputs({ inflightAgents: { rae: 0 } })), "rae")).toBe("inactive");
    // Approval still wins over it.
    const both = derivePresence(inputs({ inflightAgents: { rae: 1 }, approvalAgents: { rae: 1 } }));
    expect(presenceOf(both, "rae")).toBe("approval");
  });

  it("ages out a turn nothing has heard from, in the ledger and in the frames", () => {
    const old = NOW - STALE_TURN_MS - 1;
    const index = derivePresence(
      inputs({
        ledgerTurns: [{ agentId: "seat", startedAtMillis: old }],
        liveAgentByTurn: { desk: "rae" },
        turnMeta: { desk: meta("desk", { lastFrameAt: old }) },
      }),
    );
    expect(presenceOf(index, "seat")).toBe("inactive");
    expect(presenceOf(index, "rae")).toBe("inactive");
  });

  it("lets a recent frame keep a long-running ledger turn alive", () => {
    const index = derivePresence(
      inputs({
        ledgerTurns: [{ agentId: "rae", startedAtMillis: NOW - STALE_TURN_MS - 1 }],
        liveAgentByTurn: { other: "rae" },
        turnMeta: { other: meta("other") },
      }),
    );
    expect(presenceOf(index, "rae")).toBe("thinking");
  });
});

describe("helpers", () => {
  it("dropTurnMeta returns the same object when nothing matches and prunes when it does", () => {
    const all = { a: meta("x"), b: meta("y") };
    expect(dropTurnMeta(all, () => false)).toBe(all);
    expect(Object.keys(dropTurnMeta(all, (_k, m) => m.chatId === "x"))).toEqual(["b"]);
  });

  it("settledInChat folds a DM's spellings, so a dm:<id> settle clears bare-id frames", () => {
    const all = { rae: meta("rae"), q1: meta("rae"), desk: meta("engineering") };
    const drop = settledInChat("dm:rae", { rae: "rae" });
    expect(Object.keys(dropTurnMeta(all, drop))).toEqual(["desk"]);
    expect(Object.keys(dropTurnMeta(all, settledInChat("engineering", { rae: "rae" })))).toEqual([
      "rae",
      "q1",
    ]);
  });

  it("clearedOnThread leaves another question's per-query state when a send starts", () => {
    const all = { rae: meta("rae"), "h:41": meta("rae"), other: meta("x") };
    expect(Object.keys(dropTurnMeta(all, clearedOnThread("rae", { queries: false })))).toEqual([
      "h:41",
      "other",
    ]);
    expect(Object.keys(dropTurnMeta(all, clearedOnThread("rae", { queries: true })))).toEqual(["other"]);
  });

  it("staleTurnMeta collects only what the age-out already ignores", () => {
    const all = { old: meta("a", { lastFrameAt: NOW - STALE_TURN_MS }), fresh: meta("b") };
    expect(Object.keys(dropTurnMeta(all, staleTurnMeta(NOW)))).toEqual(["fresh"]);
  });

  it("counts an episode seat's approval when it names no agent, and skips resolved ones", () => {
    const approvals = [
      { id: "a1", agent: null, episode: { seat: "rae" } },
      { id: "a2", agent: "ada" },
      { id: "a3", agent: "ada" },
    ];
    expect(approvalAgentCounts(approvals)).toEqual({ rae: 1, ada: 2 });
    // An `approval_resolved` (an expiry, `automatic: true`, included) lands
    // before the feed re-read drops the row: the dot goes out on the frame.
    expect(approvalAgentCounts(approvals, { a1: {}, a2: {} })).toEqual({ ada: 1 });
  });

  it("counts in-flight runs per agent and records run statuses newest-last, bounded", () => {
    expect(inflightAgentCounts([{ agentId: "rae" }, { agentId: "rae" }, { agentId: "" }])).toEqual({ rae: 2 });
    const one = recordRunStatus({}, "r1", "pending");
    expect(recordRunStatus(one, "r1", "pending")).toBe(one);
    expect(recordRunStatus(one, "r1", "running")).toEqual({ r1: "running" });
    let many: Record<string, string> = {};
    for (let i = 0; i <= RUN_STATUS_CAP; i++) many = recordRunStatus(many, `run-${i}`, "pending");
    expect(Object.keys(many)).toHaveLength(RUN_STATUS_CAP);
    expect(many["run-0"]).toBeUndefined();
    expect(many[`run-${RUN_STATUS_CAP}`]).toBe("pending");
  });

  it("counts approvals per asker and skips the ones no agent raised", () => {
    const counts = approvalAgentCounts([{ agent: "a" }, { agent: "a" }, { agent: null }, {}]);
    expect(counts).toEqual({ a: 2 });
    expect(sameCounts(counts, { a: 2 })).toBe(true);
    expect(sameCounts(counts, { a: 1 })).toBe(false);
    expect(sameCounts(counts, {})).toBe(false);
  });
});
