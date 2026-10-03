// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { TeamMember } from "@/lib/team";
import { ChannelInfoPanel, ChannelPill } from "@/views/room/ChannelInfo";
import type { Channel } from "@/views/room/model";

/**
 * Who a conversation is with, without a header bar: a pill over the transcript
 * that toggles a details panel listing the members.
 */

const ADA = { id: "ada", name: "Ada", role: "Backend Engineer" } as TeamMember;
const BO = { id: "bo", name: "QA Engineer", role: "QA Engineer" } as TeamMember;
const CY = { id: "cy", name: "Cy", role: "Designer" } as TeamMember;
const DESK: Channel = {
  id: "engineering",
  name: "engineering",
  kind: "channel",
  purpose: "Build the product.",
  memberIds: ["ada", "bo"],
};

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("the channel pill", () => {
  it("names the conversation and toggles the panel", () => {
    const onToggle = vi.fn();
    act(() =>
      root.render(createElement(ChannelPill, { channel: DESK, members: [ADA, BO], open: false, onToggle })),
    );
    const pill = container.querySelector<HTMLButtonElement>('[data-testid="channel-pill"]')!;
    expect(pill.textContent).toContain("#engineering");
    expect(pill.getAttribute("aria-expanded")).toBe("false");
    act(() => pill.click());
    expect(onToggle).toHaveBeenCalledOnce();
  });
});

describe("the channel info panel", () => {
  function renderPanel(onMessage = vi.fn(), onAddExisting: ((id: string) => void) | undefined = vi.fn()) {
    act(() =>
      root.render(
        createElement(ChannelInfoPanel, {
          channel: DESK,
          members: [ADA, BO],
          channelMembers: [ADA, BO],
          others: [CY],
          people: [{ id: "u1", label: "Sam" }],
          leadId: "ada",
          onAddExisting,
          onClose: () => {},
          onMessage,
        }),
      ),
    );
    return onMessage;
  }

  it("shows the purpose, a link to the desk, and every member with the lead marked", () => {
    renderPanel();
    expect(container.textContent).toContain("Build the product.");
    expect(container.querySelector('a[href="#/company/engineering"]')).not.toBeNull();
    expect(container.textContent).toContain("In this channel");
    expect(container.querySelector('[data-testid="channel-info-summary"]')?.textContent).toBe(
      "2 in this channel · 3 in the company",
    );
    const rows = [...container.querySelectorAll("li")];
    expect(rows[0].textContent).toContain("Lead");
    expect(rows[1].textContent).not.toContain("Lead");
  });

  it("drops a role that only repeats the name", () => {
    renderPanel();
    const rows = [...container.querySelectorAll("li")];
    expect(rows[0].textContent).toContain("Backend Engineer");
    expect(rows[1].textContent?.match(/QA Engineer/g)).toHaveLength(1);
  });

  it("opens a DM when a member is pressed", () => {
    const onMessage = renderPanel();
    act(() => container.querySelector<HTMLButtonElement>('li button[title="Message Ada"]')!.click());
    expect(onMessage).toHaveBeenCalledWith(expect.stringContaining("ada"));
  });

  it("offers raw turns as a toggle on a DM, pressed while it is on", () => {
    const onToggle = vi.fn();
    const dm: Channel = { id: "dm:ada", name: "Ada", kind: "dm", purpose: "", member: ADA };
    const draw = (on: boolean) =>
      act(() =>
        root.render(
          createElement(ChannelInfoPanel, {
            channel: dm,
            members: [ADA],
            channelMembers: null,
            onClose: () => {},
            onMessage: () => {},
            raw: { on, onToggle },
          }),
        ),
      );
    const toggle = () =>
      container.querySelector<HTMLButtonElement>('[data-testid="channel-info-raw-toggle"]')!;
    draw(false);
    expect(toggle().getAttribute("aria-pressed")).toBe("false");
    act(() => toggle().click());
    expect(onToggle).toHaveBeenCalledOnce();
    draw(true);
    expect(toggle().getAttribute("aria-pressed")).toBe("true");
  });

  it("offers no raw toggle on a desk", () => {
    renderPanel();
    expect(container.querySelector('[data-testid="channel-info-raw-toggle"]')).toBeNull();
  });

  it("lists everyone else with a + that adds them to the desk", () => {
    const onAdd = vi.fn();
    renderPanel(vi.fn(), onAdd);
    expect(container.textContent).toContain("Everyone else");
    act(() => container.querySelector<HTMLButtonElement>('[aria-label="Add Cy to this channel"]')!.click());
    expect(onAdd).toHaveBeenCalledWith("cy");
  });

  it("offers no + where the desk's membership cannot change", () => {
    renderPanel(vi.fn(), undefined);
    expect(container.querySelector('[aria-label="Add Cy to this channel"]')).toBeNull();
  });

  it("lists the company's people and a control to copy the name", () => {
    renderPanel();
    expect(container.querySelector('[data-testid="person-row"]')?.textContent).toContain("Sam");
    expect(container.querySelector('[aria-label="Copy channel name: #engineering"]')).not.toBeNull();
  });
});
