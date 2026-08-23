// The Conversation view's decoration layer — the half of the assistant-ui
// migration that a browser should never have to prove.
//
// assistant-ui renders one message at a time, so grouping and day separators
// are decided *before* the list is handed over (`views/conversation/model.ts`).
// That makes them assertable here, in milliseconds, instead of by reading
// bubble corners off a screenshot.

import { describe, expect, it } from "vitest";

import { makeMessage, type ChatMessage } from "@/lib/chat";
import type { ThreadContact } from "@/lib/threads";
import {
  GROUP_WINDOW_MS,
  decorate,
  lineOf,
  senderOf,
  soloLine,
  textOf,
  toThreadMessageLike,
} from "@/views/conversation/model";

const CONTACT: ThreadContact = { name: "Your company", kind: "company" };
const T0 = Date.UTC(2026, 0, 2, 12, 0, 0);

function line(from: ChatMessage["from"], text: string, at: number, extra: Partial<ChatMessage> = {}) {
  return { ...makeMessage(from, text, { at }), ...extra };
}

describe("senderOf", () => {
  it("gives your own lines and system notices their fixed identities", () => {
    expect(senderOf(line("you", "hi", T0), CONTACT)).toMatchObject({ key: "you", kind: "you" });
    expect(senderOf(line("system", "(no reply)", T0), CONTACT)).toMatchObject({
      key: "system",
      kind: "system",
    });
  });

  it("wears the thread's contact identity when the reply names no distinct channel", () => {
    for (const channel of [undefined, "", "operator", "console", "chat", "owner"]) {
      const sender = senderOf(line("company", "ok", T0, { channel }), CONTACT);
      expect(sender).toMatchObject({ key: "contact:Your company", name: "Your company" });
    }
  });

  it("threads a named channel as its own agent, titleized", () => {
    expect(senderOf(line("company", "ok", T0, { channel: "growth_desk" }), CONTACT)).toMatchObject({
      key: "agent:growth_desk",
      name: "Growth Desk",
      kind: "agent",
      tone: "growth_desk",
    });
  });
});

describe("decorate", () => {
  it("opens a group on the first line and draws a day separator above it", () => {
    const [first] = decorate([line("you", "hi", T0)], CONTACT);
    expect(first).toMatchObject({ showDay: true, groupHead: true, groupTail: true });
  });

  it("keeps consecutive lines from one sender inside the window in a single group", () => {
    const lines = decorate(
      [line("you", "one", T0), line("you", "two", T0 + 1000), line("you", "three", T0 + 2000)],
      CONTACT,
    );
    expect(lines.map((l) => l.groupHead)).toEqual([true, false, false]);
    // Only the last bubble of a group wears the tail corner.
    expect(lines.map((l) => l.groupTail)).toEqual([false, false, true]);
    // …and a separator is drawn once, above the group, not per line.
    expect(lines.map((l) => l.showDay)).toEqual([true, false, false]);
  });

  it("breaks the group when the sender changes", () => {
    const lines = decorate([line("you", "ask", T0), line("company", "answer", T0 + 1000)], CONTACT);
    expect(lines.map((l) => l.groupHead)).toEqual([true, true]);
    expect(lines.map((l) => l.groupTail)).toEqual([true, true]);
  });

  it("breaks the group once the gap reaches the window, measured from the last line in", () => {
    // Each step is inside the window, so a run of slow replies stays one group
    // even though the first and last are further apart than the window itself.
    const slow = decorate(
      [
        line("you", "one", T0),
        line("you", "two", T0 + GROUP_WINDOW_MS - 1),
        line("you", "three", T0 + 2 * (GROUP_WINDOW_MS - 1)),
      ],
      CONTACT,
    );
    expect(slow.map((l) => l.groupHead)).toEqual([true, false, false]);

    const gapped = decorate([line("you", "one", T0), line("you", "two", T0 + GROUP_WINDOW_MS)], CONTACT);
    expect(gapped.map((l) => l.groupHead)).toEqual([true, true]);
    // A new group on the same day gets no second separator.
    expect(gapped.map((l) => l.showDay)).toEqual([true, false]);
  });

  it("draws a separator whenever the day turns over, whoever is speaking", () => {
    const nextDay = T0 + 24 * 60 * 60 * 1000;
    const lines = decorate([line("you", "one", T0), line("you", "two", nextDay)], CONTACT);
    expect(lines.map((l) => l.showDay)).toEqual([true, true]);
    expect(lines.map((l) => l.groupHead)).toEqual([true, true]);
  });

  it("stays index-aligned with its input, which is how convertMessage looks a line up", () => {
    const messages = [line("you", "a", T0), line("company", "b", T0 + 1), line("system", "c", T0 + 2)];
    const lines = decorate(messages, CONTACT);
    expect(lines).toHaveLength(messages.length);
    expect(lines.map((l) => l.message.text)).toEqual(["a", "b", "c"]);
  });
});

describe("soloLine", () => {
  it("stands a message up as its own group, drawing no separator", () => {
    // The fallback when a converter and a message array disagree about length.
    // It must never claim to open a day or close a group it cannot see.
    const m = line("company", "orphan", T0);
    expect(soloLine(m, CONTACT)).toMatchObject({
      message: m,
      showDay: false,
      groupHead: true,
      groupTail: true,
    });
    expect(soloLine(m, CONTACT).sender).toEqual(senderOf(m, CONTACT));
  });
});

describe("toThreadMessageLike", () => {
  const [you, company, system] = decorate(
    [line("you", "ask", T0), line("company", "answer", T0 + 1), line("system", "(no reply)", T0 + 2)],
    CONTACT,
  );

  it("maps the console's three origins onto assistant-ui's three roles", () => {
    expect(toThreadMessageLike(you!).role).toBe("user");
    expect(toThreadMessageLike(company!).role).toBe("assistant");
    // A system notice stays a system message rather than being smuggled in as
    // an assistant turn, so it keeps its place in document order.
    expect(toThreadMessageLike(system!).role).toBe("system");
  });

  it("carries the text as the one part, and the id and time unchanged", () => {
    const converted = toThreadMessageLike(company!);
    expect(converted.content).toEqual([{ type: "text", text: "answer" }]);
    expect(converted.id).toBe(company!.message.id);
    expect(converted.createdAt?.getTime()).toBe(T0 + 1);
  });

  it("round-trips the decorated line through metadata, losing nothing", () => {
    const converted = toThreadMessageLike(company!);
    expect(lineOf(converted.metadata as { custom?: Record<string, unknown> })).toBe(company);
  });
});

describe("lineOf", () => {
  it("refuses a message the console did not put there", () => {
    // The empty assistant placeholder the external-store runtime inserts while
    // a run is open. Reading `null` here is what keeps the transcript from
    // drawing a second working indicator under its own.
    expect(lineOf(undefined)).toBeNull();
    expect(lineOf({ custom: {} })).toBeNull();
    expect(lineOf({ custom: { line: "not a line" } })).toBeNull();
  });
});

describe("textOf", () => {
  /** An `AppendMessage`, narrowed to the two fields this helper reads. */
  const appended = (content: { type: string; text?: string }[]) =>
    ({ content }) as unknown as Parameters<typeof textOf>[0];

  it("joins the text parts and trims, which is the whole message on this surface", () => {
    expect(textOf(appended([{ type: "text", text: "  ship it  " }]))).toBe("ship it");
    expect(textOf(appended([{ type: "text", text: "one" }, { type: "text", text: "two" }]))).toBe(
      "one\ntwo",
    );
  });

  it("ignores parts this console never offered a way to add", () => {
    // Attachments and tool calls ride the same channel as prose in
    // assistant-ui's model; the composer here sends neither, so anything that
    // is not text cannot be part of what the operator typed.
    expect(textOf(appended([{ type: "image" }, { type: "text", text: "hi" }]))).toBe("hi");
    expect(textOf(appended([{ type: "image" }]))).toBe("");
  });

  it("reports an all-whitespace submission as empty, so the send is refused", () => {
    expect(textOf(appended([{ type: "text", text: "   \n  " }]))).toBe("");
  });
});
