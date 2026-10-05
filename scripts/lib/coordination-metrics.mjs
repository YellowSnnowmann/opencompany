// Coordination metrics over a company's `/events` stream, as pure functions.
//
// `scripts/measure-coordination.mjs` tails the SSE feed of a running host and
// folds every frame through `foldFrame`; `summarize` turns the fold into the
// numbers the run prints, and `evaluate` turns those into a verdict. All three
// are here, with no I/O, so `node --test scripts/lib/coordination-metrics.test.mjs`
// can state each rule in a few lines.
//
// The frames folded are the company hive's (OC-2), named in
// `docs/spec/runtime/events.md`: `turn_started` / `turn_settled` (with their
// `hive` ref), `agent_reply` (its `hive.episodeId`), `hive_accepted` (the
// starter route), `hive_message` (a direct or private line between agents),
// `hive_episode_settled` and `hive_turn_interrupted`. This is the twin of
// `opencompany measure` (`crates/opencompany-core/src/hive/measure.rs`), which
// folds the same rows from the store: the two are kept in step by hand, and
// the thresholds are the same numbers.

/** A fresh, empty ledger. */
export function createLedger() {
  return {
    turns: {
      /** Open turns, by turn id. @type {Map<string, {agentId?: string, startedAt: number, episodeId?: string}>} */
      open: new Map(),
      /** @type {{agentId?: string, startedAt: number, settledAt: number, episodeId?: string}[]} */
      closed: [],
      peak: 0,
      overlaps: 0,
      sameAgentOverlaps: 0,
    },
    /** @type {Map<string, {id: string, chatId?: string, openedAt: number, settledAt?: number, status: "open"|"settled"|"failed", turns: number, failure?: string}>} */
    episodes: new Map(),
    /** @type {{from: string, to: string[], via: "direct"|"private", at: number, episodeId?: string}[]} */
    contacts: [],
    /** @type {Record<string, number>} from `hive_accepted.route`. */
    starterRoutes: {},
    interruptedTurns: 0,
    frames: 0,
  };
}

function episodeOf(ledger, id, chatId, at) {
  let episode = ledger.episodes.get(id);
  if (!episode) {
    episode = { id, chatId, openedAt: at, status: "open", turns: 0 };
    ledger.episodes.set(id, episode);
  }
  if (!episode.chatId && chatId) episode.chatId = chatId;
  return episode;
}

function turnKey(frame) {
  return frame.turnId ?? `${frame.agentId ?? "?"}:${frame.chatId ?? "?"}`;
}

/**
 * Folds one frame into the ledger. Mutates and returns it.
 *
 * @param {ReturnType<typeof createLedger>} ledger
 * @param {any} frame a parsed `/events` frame
 */
export function foldFrame(ledger, frame) {
  if (!frame || typeof frame.type !== "string") return ledger;
  ledger.frames += 1;
  const at = typeof frame.atMillis === "number" ? frame.atMillis : Date.now();
  switch (frame.type) {
    case "turn_started": {
      const { turns } = ledger;
      const episodeId = frame.hive?.episodeId;
      if (episodeId) episodeOf(ledger, episodeId, frame.hive?.hiveId, at).turns += 1;
      if (turns.open.size > 0) turns.overlaps += 1;
      if (frame.agentId && [...turns.open.values()].some((turn) => turn.agentId === frame.agentId)) {
        turns.sameAgentOverlaps += 1;
      }
      turns.open.set(turnKey(frame), { agentId: frame.agentId, startedAt: at, episodeId });
      turns.peak = Math.max(turns.peak, turns.open.size);
      break;
    }
    case "turn_settled": {
      const { turns } = ledger;
      const key = turnKey(frame);
      const turn = turns.open.get(key);
      if (!turn) break;
      turns.open.delete(key);
      turns.closed.push({ ...turn, settledAt: at });
      break;
    }
    case "agent_reply": {
      const episodeId = frame.hive?.episodeId;
      if (episodeId) episodeOf(ledger, episodeId, frame.chatId, at);
      break;
    }
    case "hive_message": {
      const destination = frame.destination ?? {};
      if (frame.episodeId) {
        episodeOf(ledger, frame.episodeId, destination.type === "hive" ? destination.id : undefined, at);
      }
      if (destination.type === "agent") {
        const to = destination.id === frame.sender ? [] : [destination.id];
        ledger.contacts.push({ from: frame.sender, to, via: "direct", at, episodeId: frame.episodeId });
      } else {
        const to = (frame.onlyFor ?? []).filter((id) => id !== frame.sender);
        ledger.contacts.push({ from: frame.sender, to, via: "private", at, episodeId: frame.episodeId });
      }
      break;
    }
    case "hive_accepted": {
      if (frame.route) ledger.starterRoutes[frame.route] = (ledger.starterRoutes[frame.route] ?? 0) + 1;
      break;
    }
    case "hive_episode_settled": {
      const episode = episodeOf(ledger, frame.episodeId, frame.chatId, at);
      if (episode.status === "open") {
        episode.status = frame.failure ? "failed" : "settled";
        episode.failure = frame.failure;
        episode.settledAt = at;
      }
      break;
    }
    case "hive_turn_interrupted":
      ledger.interruptedTurns += 1;
      break;
    default:
      break;
  }
  return ledger;
}

/** Whether every episode the ledger saw has settled (or failed). At least one must have opened. */
export function allSettled(ledger) {
  if (ledger.episodes.size === 0) return false;
  for (const episode of ledger.episodes.values()) if (episode.status === "open") return false;
  return true;
}

/**
 * The most attempts open at once, from `GET /runs` rows — the cross-check for
 * the SSE bracket count. A row with no start is not an attempt that ran.
 *
 * @param {{startedAtMillis?: number, finishedAtMillis?: number, agentId?: string}[]} runs
 * @param {number} nowMillis
 */
export function peakFromRuns(runs, nowMillis = Date.now()) {
  const events = [];
  for (const run of runs) {
    if (typeof run.startedAtMillis !== "number") continue;
    events.push({ at: run.startedAtMillis, delta: 1 });
    events.push({ at: run.finishedAtMillis ?? nowMillis, delta: -1 });
  }
  // Ends before starts at the same instant: a turn that ends as another begins
  // is a hand-off, not an overlap.
  events.sort((a, b) => a.at - b.at || a.delta - b.delta);
  let open = 0;
  let peak = 0;
  for (const event of events) {
    open += event.delta;
    peak = Math.max(peak, open);
  }
  return peak;
}

/**
 * The numbers a run prints — the same fields `opencompany measure` reports.
 *
 * @param {ReturnType<typeof createLedger>} ledger
 */
export function summarize(ledger) {
  const pairs = new Set();
  let directMessages = 0;
  let privateLines = 0;
  for (const contact of ledger.contacts) {
    if (contact.via === "direct") directMessages += 1;
    else privateLines += 1;
    for (const to of contact.to) pairs.add(`${contact.from}→${to}`);
  }
  const episodes = [...ledger.episodes.values()];
  const turnsPerEpisode = {};
  const timeToSettle = {};
  const failures = {};
  for (const episode of episodes) {
    turnsPerEpisode[episode.id] = episode.turns;
    if (episode.settledAt !== undefined) timeToSettle[episode.id] = episode.settledAt - episode.openedAt;
    if (episode.failure) failures[episode.id] = episode.failure;
  }
  return {
    frames: ledger.frames,
    maxConcurrentTurns: ledger.turns.peak,
    overlaps: ledger.turns.overlaps,
    openTurns: ledger.turns.open.size,
    sameAgentOverlaps: ledger.turns.sameAgentOverlaps,
    interruptedTurns: ledger.interruptedTurns,
    episodesOpened: episodes.length,
    episodesSettled: episodes.filter((episode) => episode.status === "settled").length,
    episodesFailed: episodes.filter((episode) => episode.status === "failed").length,
    episodesOpen: episodes.filter((episode) => episode.status === "open").map((episode) => `${episode.chatId ?? "?"}/${episode.id}`),
    turnsPerEpisode,
    directMessages,
    privateLines,
    distinctPairs: [...pairs].sort(),
    starterRoutes: ledger.starterRoutes,
    timeToSettleMillis: timeToSettle,
    failures,
  };
}

/** The thresholds a run must clear: `Thresholds::default()` in `hive/measure.rs`. */
export const DEFAULT_THRESHOLDS = Object.freeze({
  maxConcurrentTurns: 2,
  agentContacts: 1,
  distinctPairs: 2,
});

/**
 * The failures a summary carries against the thresholds — an empty list is a
 * pass, and the length is the script's exit code.
 *
 * @param {ReturnType<typeof summarize>} summary
 * @param {Partial<typeof DEFAULT_THRESHOLDS>} [thresholds]
 * @param {{runsPeak?: number}} [crossCheck]
 * @returns {string[]}
 */
export function evaluate(summary, thresholds = {}, crossCheck = {}) {
  const t = { ...DEFAULT_THRESHOLDS, ...thresholds };
  const failures = [];
  if (summary.maxConcurrentTurns < t.maxConcurrentTurns) {
    failures.push(`max concurrent turns ${summary.maxConcurrentTurns} < ${t.maxConcurrentTurns}`);
  }
  if (summary.sameAgentOverlaps > 0) {
    failures.push(`same-agent overlaps ${summary.sameAgentOverlaps} (must be 0)`);
  }
  const contacts = summary.directMessages + summary.privateLines;
  if (contacts < t.agentContacts) {
    failures.push(`agent→agent contacts ${contacts} < ${t.agentContacts}`);
  }
  if (summary.distinctPairs.length < t.distinctPairs) {
    failures.push(`distinct pairs ${summary.distinctPairs.length} < ${t.distinctPairs}`);
  }
  if (summary.episodesOpened === 0) {
    failures.push("no episode opened");
  } else if (summary.episodesOpen.length > 0) {
    failures.push(`${summary.episodesOpen.length} episode(s) never settled: ${summary.episodesOpen.join(", ")}`);
  }
  if (typeof crossCheck.runsPeak === "number" && crossCheck.runsPeak < t.maxConcurrentTurns) {
    failures.push(`GET /runs cross-check: peak ${crossCheck.runsPeak} < ${t.maxConcurrentTurns}`);
  }
  return failures;
}

/**
 * Parses one SSE event block into its JSON `data`, or null. A block is the
 * text between two blank lines; `data:` lines are joined with newlines.
 *
 * @param {string} block
 */
export function parseSseBlock(block) {
  const data = block
    .split(/\r?\n/)
    .filter((line) => line.startsWith("data:"))
    .map((line) => line.slice(5).replace(/^ /, ""))
    .join("\n");
  if (!data) return null;
  try {
    return JSON.parse(data);
  } catch {
    return null;
  }
}

/**
 * A stateful line splitter for an SSE body: feed it chunks, get whole blocks.
 */
export function createSseSplitter() {
  let buffer = "";
  return {
    /** @param {string} chunk @returns {string[]} the blocks completed by this chunk */
    push(chunk) {
      buffer += chunk.replace(/\r\n/g, "\n");
      const blocks = [];
      let at;
      while ((at = buffer.indexOf("\n\n")) !== -1) {
        const block = buffer.slice(0, at);
        buffer = buffer.slice(at + 2);
        if (block.trim()) blocks.push(block);
      }
      return blocks;
    },
  };
}
