// The second line of a conversation row in the sidebar: the last thing said
// there, and when.
//
// Pure, so the rail stays a renderer and the rules — which line counts, how it
// is prefixed, how its time is printed — are testable without a DOM.

import type { ChatMessage } from "@/lib/chat";
import type { TeamMember } from "@/lib/team";
import type { Channel } from "./channels";
import { senderOf } from "./timeline";

/** What a conversation row prints under its name. */
export interface RailPreview {
  /** One line, whitespace collapsed and markdown punctuation stripped. */
  text: string;
  /** Wall-clock of the line, for the timestamp on the name row. */
  at: number;
}

/**
 * The last top-level line of a transcript, as a one-line preview, or `null`
 * when nothing has been said yet.
 *
 * Thread replies are skipped: the rail names the conversation, and a reply
 * inside a thread is not what the channel itself last said. A channel line is
 * prefixed with who said it ("Ada: …") because a channel has many voices; a DM
 * line is not, except your own ("You: …"), since the row already names the
 * one teammate on the other end.
 */
export function channelPreview(
  channel: Channel,
  messages: ChatMessage[] | undefined,
  members: TeamMember[],
): RailPreview | null {
  if (!messages) return null;
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if (m.parentId) continue;
    const text = flatten(m.text);
    if (!text) continue;
    if (m.from === "system") return { text, at: m.at };
    const sender = senderOf(m, channel, members);
    const prefix =
      m.from === "you" ? "You" : channel.kind === "dm" ? null : firstName(sender.name);
    return { text: prefix ? `${prefix}: ${text}` : text, at: m.at };
  }
  return null;
}

/**
 * The timestamp on a row: a time today, "Yesterday", a weekday inside the last
 * week, and a short date past that — the convention every messaging client
 * uses, so a list can be scanned for recency without reading it.
 */
export function railTime(at: number, now: number = Date.now()): string {
  const then = new Date(at);
  const today = new Date(now);
  const startOfToday = new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime();
  const day = 86_400_000;
  if (at >= startOfToday) {
    return then.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  }
  if (at >= startOfToday - day) return "Yesterday";
  if (at >= startOfToday - 6 * day) {
    return then.toLocaleDateString(undefined, { weekday: "long" });
  }
  return then.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function firstName(name: string): string {
  return name.trim().split(/\s+/)[0] ?? name;
}

/** Markdown to a single plain line: enough to read, not a renderer. */
function flatten(text: string): string {
  return text
    .replace(/```[\s\S]*?```/g, " ")
    .replace(/!\[[^\]]*\]\([^)]*\)/g, " ")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/[*_`~>#]+/g, "")
    .replace(/\s+/g, " ")
    .trim();
}
