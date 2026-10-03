import { describe, expect, it } from "vitest";

import type { ChatMessage } from "@/lib/chat";
import type { TeamMember } from "@/lib/team";
import type { Channel } from "@/views/room/model";
import { channelPreview, railTime } from "@/views/room/railPreview";

/**
 * The second line of a sidebar conversation row: the last top-level line said
 * there, prefixed with who said it where a channel has many voices.
 */

const ADA = { id: "ada", name: "Ada Lovelace" } as TeamMember;
const DM: Channel = { id: "dm:ada", name: "Ada Lovelace", kind: "dm", purpose: "", member: ADA };
const DESK: Channel = { id: "ops", name: "ops", kind: "channel", purpose: "Ops", memberIds: ["ada"] };

const msg = (over: Partial<ChatMessage>): ChatMessage => ({
  id: Math.random().toString(36),
  from: "company",
  text: "hello",
  at: 1,
  ...over,
});

describe("channelPreview", () => {
  it("is null with no transcript or only empty lines", () => {
    expect(channelPreview(DM, undefined, [ADA])).toBeNull();
    expect(channelPreview(DM, [msg({ text: "   " })], [ADA])).toBeNull();
  });

  it("takes the last top-level line, skipping thread replies", () => {
    const messages = [
      msg({ id: "a", text: "first", at: 1 }),
      msg({ id: "b", text: "second", at: 2 }),
      msg({ id: "c", text: "in a thread", at: 3, parentId: "a" }),
    ];
    expect(channelPreview(DM, messages, [ADA])).toEqual({ text: "second", at: 2 });
  });

  it("prefixes your own line with You, and a DM's agent line with nothing", () => {
    expect(channelPreview(DM, [msg({ from: "you", text: "hi" })], [ADA])?.text).toBe("You: hi");
    expect(channelPreview(DM, [msg({ text: "hey" })], [ADA])?.text).toBe("hey");
  });

  it("prefixes a channel line with the speaker's first name", () => {
    const line = msg({ text: "booked the venue", channel: "ada" });
    expect(channelPreview(DESK, [line], [ADA])?.text).toBe("Ada: booked the venue");
  });

  it("flattens markdown to one plain line", () => {
    const line = msg({ text: "**Done.**\n\nSee [the doc](https://x.y) and `code`" });
    expect(channelPreview(DM, [line], [ADA])?.text).toBe("Done. See the doc and code");
  });
});

describe("railTime", () => {
  const now = new Date(2026, 9, 3, 15, 0).getTime();

  it("prints Yesterday for the previous calendar day", () => {
    expect(railTime(new Date(2026, 9, 2, 23, 59).getTime(), now)).toBe("Yesterday");
  });

  it("prints a time of day for today", () => {
    expect(railTime(new Date(2026, 9, 3, 9, 5).getTime(), now)).toMatch(/9:05/);
  });

  it("prints a weekday within the week and a date past it", () => {
    expect(railTime(new Date(2026, 8, 29, 12).getTime(), now)).toMatch(/Tuesday/);
    expect(railTime(new Date(2026, 7, 1, 12).getTime(), now)).toMatch(/Aug/);
  });
});
