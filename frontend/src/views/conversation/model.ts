// The Conversation view's pure layer: who a line is from, how consecutive lines
// group, and how a `ChatMessage` crosses into assistant-ui's message shape.
//
// Kept out of the components deliberately. assistant-ui renders one message at
// a time — `ThreadPrimitive.Messages` hands the render function a single
// message, never the array — so every decision that depends on a line's
// *neighbours* (does it start a new group, does a day separator go above it)
// has to be made before the list is handed over. Making it here means it is
// made once per snapshot and can be asserted by a test with no DOM at all.

import type { AppendMessage, ThreadMessageLike } from "@assistant-ui/react";

import type { ChatMessage } from "@/lib/chat";
import type { ThreadContact } from "@/lib/threads";

/** Consecutive messages from one sender within this window group together. */
export const GROUP_WINDOW_MS = 5 * 60 * 1000;

/** Who a line is from, once the thread's contact identity is applied. */
export interface Sender {
  /** Groups consecutive lines: same key + inside the window = one group. */
  key: string;
  name: string;
  kind: "you" | "company" | "agent" | "system";
  tone?: string;
}

/**
 * One transcript line, with everything the renderer needs that it could not
 * work out from the line alone.
 *
 * This is what rides in `metadata.custom` across the assistant-ui boundary —
 * see {@link toThreadMessageLike}.
 */
export interface ConversationLine {
  message: ChatMessage;
  sender: Sender;
  /** A day separator is drawn above this line. */
  showDay: boolean;
  /** This line opens a group, so it carries the avatar and the sender name. */
  groupHead: boolean;
  /** This line closes a group, so its bubble gets the tail corner. */
  groupTail: boolean;
}

const COMPANY_VOICE = new Set(["operator", "console", "chat", "owner", ""]);

/**
 * Resolve a message's sender within a thread: the company side wears the
 * thread's contact identity unless the reply names a distinct channel.
 */
export function senderOf(m: ChatMessage, contact: ThreadContact): Sender {
  if (m.from === "you") return { key: "you", name: "You", kind: "you" };
  if (m.from === "system") return { key: "system", name: "System", kind: "system" };
  const channel = m.channel?.trim().toLowerCase() ?? "";
  if (channel && !COMPANY_VOICE.has(channel)) {
    return { key: `agent:${channel}`, name: titleize(m.channel!), kind: "agent", tone: channel };
  }
  return { key: `contact:${contact.name}`, name: contact.name, kind: contact.kind, tone: contact.tone };
}

/**
 * Decorate a transcript: one {@link ConversationLine} per message, in order.
 *
 * The output is index-aligned with the input, which is what lets
 * `convertMessage` — which assistant-ui calls with `(message, index)` — look a
 * line up by index rather than re-deriving it per message.
 *
 * A group runs while the sender key is unchanged, the gap stays under
 * {@link GROUP_WINDOW_MS}, and the day does not turn over. `groupTail` is only
 * knowable once the *next* line is seen, so it is stamped in a second pass.
 */
export function decorate(messages: readonly ChatMessage[], contact: ThreadContact): ConversationLine[] {
  const lines: ConversationLine[] = [];
  // The head of the open group, and the timestamp of its most recent line —
  // grouping compares against the last line in, not the first, so a slow
  // back-and-forth inside the window stays one group.
  let openHead: ConversationLine | null = null;
  let openAt = 0;

  for (const message of messages) {
    const sender = senderOf(message, contact);
    const continues =
      openHead !== null &&
      openHead.sender.key === sender.key &&
      message.at - openAt < GROUP_WINDOW_MS &&
      sameDay(openAt, message.at);

    const line: ConversationLine = {
      message,
      sender,
      showDay: !continues && (openHead === null || !sameDay(openAt, message.at)),
      groupHead: !continues,
      // Provisional: overwritten below for every line that turns out to have a
      // successor in its own group.
      groupTail: true,
    };
    if (continues) {
      lines[lines.length - 1]!.groupTail = false;
    } else {
      openHead = line;
    }
    openAt = message.at;
    lines.push(line);
  }
  return lines;
}

/**
 * Decorate one message as a group of its own.
 *
 * The fallback for a `convertMessage` call whose index does not land on the
 * line it was meant for. assistant-ui holds the message array and the converter
 * as two separate inputs, so a render in between two snapshots can pair a new
 * converter with an older array; indexing blind would throw on a transcript
 * that had shrunk. Degrading to "this line starts its own group" costs one
 * avatar for one frame and cannot crash the transcript.
 */
export function soloLine(message: ChatMessage, contact: ThreadContact): ConversationLine {
  return {
    message,
    sender: senderOf(message, contact),
    showDay: false,
    groupHead: true,
    groupTail: true,
  };
}

/**
 * A decorated line as assistant-ui sees it.
 *
 * The text becomes the message's one content part — the shape a text part is
 * required to have — and everything the console renders *around* that text
 * (the sender, the grouping flags, the tool steps, the board-card id) rides in
 * `metadata.custom` as the {@link ConversationLine} itself.
 *
 * That is the whole integration seam, and it is deliberately one-directional:
 * assistant-ui owns the composer, the viewport and the run state; the console
 * keeps owning what a message *means*. Nothing here is lossy, so a renderer can
 * always reach the original `ChatMessage` — which is what the board-card
 * actions and the markdown/plain-text split need.
 */
export function toThreadMessageLike(line: ConversationLine): ThreadMessageLike {
  const { message } = line;
  return {
    // `system` is a real assistant-ui role, so the console's own notices ("(no
    // reply)", "Couldn't send — …") stay in the transcript in document order
    // rather than being smuggled in as assistant turns.
    role: message.from === "you" ? "user" : message.from === "system" ? "system" : "assistant",
    id: message.id,
    createdAt: new Date(message.at),
    content: [{ type: "text", text: message.text }],
    metadata: { custom: { line } },
  };
}

/**
 * Read a line back out of an assistant-ui message.
 *
 * Returns `null` for a message the console did not put there — specifically
 * the empty assistant placeholder the external-store runtime inserts while a
 * run is open, which carries `isOptimistic` and no custom payload. The
 * renderer draws nothing for it; this surface keeps its own working row, so a
 * placeholder bubble would be a second one.
 */
export function lineOf(metadata: { custom?: Record<string, unknown> } | undefined): ConversationLine | null {
  const line = metadata?.custom?.["line"];
  return isLine(line) ? line : null;
}

function isLine(value: unknown): value is ConversationLine {
  return typeof value === "object" && value !== null && "message" in value && "sender" in value;
}

/**
 * The text a composer submission carries.
 *
 * `AppendMessage.content` is a part array because assistant-ui models
 * attachments and tool calls in the same channel as prose. This surface sends
 * neither, so the text parts joined back together *are* the message — and
 * anything else in there would be something this console never offered a way
 * to add.
 */
export function textOf(message: AppendMessage): string {
  return message.content
    .filter((part): part is { type: "text"; text: string } => part.type === "text")
    .map((part) => part.text)
    .join("\n")
    .trim();
}

/* ---- formatting ---- */

export function titleize(s: string): string {
  return s.replace(/[._-]+/g, " ").replace(/\w\S*/g, (w) => w.charAt(0).toUpperCase() + w.slice(1));
}

export function previewOf(m: ChatMessage): string {
  const prefix = m.from === "you" ? "You: " : "";
  return `${prefix}${m.text}`;
}

export function formatTime(at: number): string {
  return new Date(at).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
}

export function sameDay(a: number, b: number): boolean {
  return new Date(a).toDateString() === new Date(b).toDateString();
}

export function formatDay(at: number): string {
  const d = new Date(at);
  const today = new Date();
  const yesterday = new Date(today);
  yesterday.setDate(today.getDate() - 1);
  if (d.toDateString() === today.toDateString()) return "Today";
  if (d.toDateString() === yesterday.toDateString()) return "Yesterday";
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}

export function formatElapsed(ms: number): string {
  return ms < 1000 ? `${ms}ms` : `${(ms / 1000).toFixed(1)}s`;
}

export function initials(name: string): string {
  const parts = name.trim().split(/\s+/).slice(0, 2);
  return parts.map((p) => p.charAt(0).toUpperCase()).join("") || "?";
}

/**
 * Speaker tints — identity, not state. Keys are legacy slot names; see
 * `TEAM_TONES` in `@/lib/team`, which this mirrors and which explains why
 * the palette stays clear of the status hues.
 */
const TONES: Record<string, string> = {
  sky: "bg-tone-2/15 text-tone-2-text",
  violet: "bg-tone-1/15 text-tone-1-text",
  amber: "bg-tone-5/15 text-tone-5-text",
  emerald: "bg-tone-3/15 text-tone-3-text",
  rose: "bg-tone-4/15 text-tone-4-text",
  cyan: "bg-tone-2/15 text-tone-2-text",
};
const TONE_KEYS = Object.keys(TONES);

export function toneClass(tone?: string): string {
  if (tone && TONES[tone]) return TONES[tone];
  const key = tone ?? "";
  let hash = 0;
  for (let i = 0; i < key.length; i++) hash = (hash * 31 + key.charCodeAt(i)) | 0;
  return TONES[TONE_KEYS[Math.abs(hash) % TONE_KEYS.length]];
}
