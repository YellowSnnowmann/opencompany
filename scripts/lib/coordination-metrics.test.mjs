// `node --test scripts/lib/coordination-metrics.test.mjs`
//
// The rules `measure-coordination.mjs` reports against, stated on canned
// company-hive frames (OC-2): the bracket peak and the same-agent overlap
// count, the contacts a direct or private `hive_message` implies, the
// starter-route histogram, the settlement gate, and the thresholds.

import assert from "node:assert/strict";
import { test } from "node:test";

import {
  allSettled,
  createLedger,
  createSseSplitter,
  evaluate,
  foldFrame,
  parseSseBlock,
  peakFromRuns,
  summarize,
} from "./coordination-metrics.mjs";

const fold = (frames) => frames.reduce(foldFrame, createLedger());

const bracket = (type, agentId, seq, extra = {}) => ({ type, seq, atMillis: seq * 10, chatId: "engineering", agentId, ...extra });

test("the bracket peak counts turns open at once, and same-agent overlaps stay zero", () => {
  const ledger = fold([
    bracket("turn_started", "engineer", 1, { turnId: "a" }),
    bracket("turn_started", "writer", 2, { turnId: "b" }),
    bracket("turn_settled", "engineer", 3, { turnId: "a" }),
    bracket("turn_started", "ceo", 4, { turnId: "c" }),
    bracket("turn_settled", "writer", 5, { turnId: "b" }),
    bracket("turn_settled", "ceo", 6, { turnId: "c" }),
  ]);
  assert.equal(ledger.turns.peak, 2);
  assert.equal(ledger.turns.overlaps, 2);
  assert.equal(ledger.turns.open.size, 0);
  assert.equal(ledger.turns.sameAgentOverlaps, 0);
  assert.equal(ledger.turns.closed.length, 3);
});

test("a second start for an agent still running is the overlap the Coordinator forbids", () => {
  const ledger = fold([
    bracket("turn_started", "ceo", 1, { turnId: "a" }),
    bracket("turn_started", "ceo", 2, { turnId: "b" }),
  ]);
  assert.equal(ledger.turns.sameAgentOverlaps, 1);
  assert.equal(ledger.turns.peak, 2);
});

test("a stray settle for a turn never seen is ignored", () => {
  const ledger = fold([bracket("turn_settled", "writer", 3, { turnId: "z" })]);
  assert.equal(ledger.turns.closed.length, 0);
});

const RUN = [
  { type: "hive_accepted", seq: 1, atMillis: 100, chatId: "engineering", messageId: "op:1", sequence: 0, starters: ["engineer"], route: "mention" },
  bracket("turn_started", "engineer", 2, { turnId: "t1", hive: { hiveId: "engineering", episodeId: "ep-1" } }),
  { type: "hive_message", seq: 3, atMillis: 30, sequence: 2, sender: "engineer", destination: { type: "agent", id: "writer" }, text: "copy?", episodeId: "ep-1" },
  bracket("turn_started", "writer", 4, { turnId: "t2", hive: {} }),
  { type: "hive_message", seq: 5, atMillis: 50, sequence: 3, sender: "writer", destination: { type: "agent", id: "engineer" }, text: "here" },
  bracket("turn_settled", "writer", 6, { turnId: "t2" }),
  { type: "agent_reply", seq: 7, atMillis: 70, chatId: "engineering", agentId: "engineer", hive: { sequence: 4, episodeId: "ep-1" } },
  bracket("turn_settled", "engineer", 8, { turnId: "t1" }),
  { type: "hive_episode_settled", seq: 9, atMillis: 90, chatId: "engineering", episodeId: "ep-1", openedAt: 0 },
];

test("a run's contacts, routes, turns and settlement fold into the summary", () => {
  const summary = summarize(fold(RUN));
  assert.equal(summary.episodesOpened, 1);
  assert.equal(summary.episodesSettled, 1);
  assert.equal(summary.episodesFailed, 0);
  assert.deepEqual(summary.episodesOpen, []);
  assert.deepEqual(summary.turnsPerEpisode, { "ep-1": 1 });
  assert.equal(summary.directMessages, 2);
  assert.equal(summary.privateLines, 0);
  assert.deepEqual(summary.distinctPairs, ["engineer→writer", "writer→engineer"]);
  assert.deepEqual(summary.starterRoutes, { mention: 1 });
  assert.equal(summary.maxConcurrentTurns, 2);
  assert.deepEqual(summary.timeToSettleMillis, { "ep-1": 70 });
});

test("a private desk line counts its readers as contacts", () => {
  const summary = summarize(fold([
    { type: "hive_message", seq: 1, atMillis: 1, sequence: 1, sender: "ceo", destination: { type: "hive", id: "engineering" }, onlyFor: ["engineer", "ceo"], episodeId: "ep-9" },
  ]));
  assert.equal(summary.privateLines, 1);
  assert.deepEqual(summary.distinctPairs, ["ceo→engineer"]);
  assert.equal(summary.episodesOpened, 1);
});

test("a failed episode is settled for the gate but counted apart", () => {
  const ledger = fold([
    { type: "agent_reply", seq: 1, atMillis: 1, chatId: "content", hive: { sequence: 1, episodeId: "ep-2" } },
    { type: "hive_episode_settled", seq: 2, atMillis: 5, chatId: "content", episodeId: "ep-2", failure: "turn wall" },
    { type: "hive_turn_interrupted", seq: 3, atMillis: 6, agentId: "writer", reason: "restart" },
  ]);
  assert.equal(allSettled(ledger), true);
  const summary = summarize(ledger);
  assert.equal(summary.episodesFailed, 1);
  assert.deepEqual(summary.failures, { "ep-2": "turn wall" });
  assert.equal(summary.interruptedTurns, 1);
});

test("settlement is gated on every episode seen, and on at least one", () => {
  assert.equal(allSettled(createLedger()), false);
  const ledger = fold(RUN.slice(0, 7));
  assert.equal(allSettled(ledger), false);
  foldFrame(ledger, RUN.at(-1));
  assert.equal(allSettled(ledger), true);
});

test("the thresholds name each shortfall, and an empty list is a pass", () => {
  const good = summarize(fold(RUN));
  assert.deepEqual(evaluate(good), []);
  assert.deepEqual(evaluate(good, {}, { runsPeak: 2 }), []);
  assert.deepEqual(evaluate(good, {}, { runsPeak: 1 }), ["GET /runs cross-check: peak 1 < 2"]);

  const empty = summarize(createLedger());
  assert.deepEqual(evaluate(empty), [
    "max concurrent turns 0 < 2",
    "agent→agent contacts 0 < 1",
    "distinct pairs 0 < 2",
    "no episode opened",
  ]);

  const stuck = summarize(fold([...RUN.slice(0, 7), bracket("turn_started", "ceo", 30, { turnId: "x" }), bracket("turn_started", "ceo", 31, { turnId: "y" })]));
  const failures = evaluate(stuck);
  assert.ok(failures.includes("same-agent overlaps 1 (must be 0)"), failures.join("; "));
  assert.ok(failures.some((f) => f.startsWith("1 episode(s) never settled: engineering/ep-1")), failures.join("; "));
});

test("the /runs cross-check counts overlapping attempts, treating a hand-off as no overlap", () => {
  assert.equal(
    peakFromRuns([
      { startedAtMillis: 0, finishedAtMillis: 10 },
      { startedAtMillis: 10, finishedAtMillis: 20 },
      { startedAtMillis: 5, finishedAtMillis: 8 },
      { createdAtMillis: 1 },
    ]),
    2,
  );
  assert.equal(peakFromRuns([{ startedAtMillis: 0 }, { startedAtMillis: 1 }], 5), 2);
});

test("SSE blocks split on blank lines and parse their data lines", () => {
  const splitter = createSseSplitter();
  assert.deepEqual(splitter.push("id: 1\ndata: {\"type\":\"a\"}\n\nid: 2\ndata: {\"ty"), ["id: 1\ndata: {\"type\":\"a\"}"]);
  assert.deepEqual(splitter.push("pe\":\"b\"}\r\n\r\n: keepalive\n\n"), ["id: 2\ndata: {\"type\":\"b\"}", ": keepalive"]);
  assert.deepEqual(parseSseBlock("id: 2\ndata: {\"type\":\"b\"}"), { type: "b" });
  assert.equal(parseSseBlock(": keepalive"), null);
  assert.equal(parseSseBlock("data: not json"), null);
});
