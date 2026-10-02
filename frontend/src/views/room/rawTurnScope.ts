/**
 * **Which of a teammate's session rows belong to one DM.**
 *
 * `GET .../agents/{id}/session` answers the agent's merged, cross-channel
 * stream, so a DM's raw turns are a filter over it. Kept separate from the view
 * because the filter is the whole claim — "what the agent did for this" — and
 * a claim worth making is worth testing without mounting a screen.
 */
import type { AgentSessionMessageDto } from "../../api/types";

/**
 * Whether a session row belongs to the DM with `agentId`.
 *
 * Both spellings, because the host lists both: `chat_history::agent_channels`
 * registers a teammate's DM under its **bare** id (what `dmThreadId` posts to,
 * after issue #364 re-keyed DMs) *and* under `dm:<id>` (the console's channel
 * key and a documented route key). Matching one would silently drop every line
 * keyed the other way — including, depending on which wrote it, the whole of
 * the operator's own side of the conversation.
 */
export function inDmWith(
  row: AgentSessionMessageDto,
  agentId: string,
): boolean {
  return (
    row.sessionChannelId === agentId || row.sessionChannelId === `dm:${agentId}`
  );
}

/**
 * Whether a row sits in the pair channel this teammate shares with one other
 * seat — `dm:<a>+<b>`, which `hive::referral::pair_conversation` names by
 * sorting the two ids.
 *
 * Deliberately blind to *which* other seat: a teammate may ask anyone, and the
 * caller scopes by episode rather than by partner.
 */
export function inPairConversationWith(
  row: AgentSessionMessageDto,
  agentId: string,
): boolean {
  const channel = row.sessionChannelId;
  if (!channel?.startsWith("dm:") || !channel.includes("+")) return false;
  return channel.slice("dm:".length).split("+").includes(agentId);
}

/**
 * This DM's raw turns: its own channel, **and the conversations it opened**.
 *
 * # Why the pair channels belong here
 *
 * An `ask` does not run on the DM's channel. It opens a conversation of its
 * own, `dm:<asker>+<askee>`, and the asking, the answer and the conclusion all
 * live there. Filtering to the DM's own two spellings therefore hides the part
 * a reader actually wants. Measured on one live run: the teammate's session
 * held eight rows, the DM's own spellings matched **two**, and the three it
 * dropped were the whole of the work — the question, "Yes, I'll take it end to
 * end", and the conclusion.
 *
 * # Why episode and not partner
 *
 * `pair_conversation` is deterministic, so the same two seats reuse one channel
 * across every episode they ever speak in. Matching the channel alone would
 * pull an unrelated exchange from last week into this transcript. The episode
 * is what makes a conversation *this* DM's: rows opened under it carry its
 * `episode.id`, and rows from another episode — including the same teammate's
 * work on a desk — do not.
 *
 * That is also what keeps the toggle honest. Raw turns is scoped to **this
 * conversation**, not to the teammate's whole session: flipping a DM into a
 * stream that also carried `#general` would change what the thing is rather
 * than how it is drawn, and the cross-channel view has its own address. Episode
 * scoping excludes other desks for free.
 */
export function dmRawTurns(
  rows: AgentSessionMessageDto[],
  agentId: string,
): AgentSessionMessageDto[] {
  const episodes = new Set(
    rows
      .filter((row) => inDmWith(row, agentId))
      .map((row) => row.episode?.id)
      .filter((id): id is string => Boolean(id)),
  );
  return rows.filter(
    (row) =>
      inDmWith(row, agentId) ||
      (inPairConversationWith(row, agentId) &&
        Boolean(row.episode?.id) &&
        episodes.has(row.episode!.id)),
  );
}
