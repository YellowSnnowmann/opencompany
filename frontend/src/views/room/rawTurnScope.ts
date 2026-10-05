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
 * This DM's raw turns: the rows on its own channel, under either spelling.
 *
 * Raw turns is scoped to **this conversation**, not to the teammate's whole
 * session: flipping a DM into a stream that also carried `#general` would
 * change what the thing is rather than how it is drawn, and the cross-channel
 * view has its own address. A teammate's direct lines with other teammates
 * (`hivemind_send_agent`) are not chat rows at all since OC-2; they are read
 * from `GET {scope}/agents/{id}/messages` instead.
 */
export function dmRawTurns(
  rows: AgentSessionMessageDto[],
  agentId: string,
): AgentSessionMessageDto[] {
  return rows.filter((row) => inDmWith(row, agentId));
}
