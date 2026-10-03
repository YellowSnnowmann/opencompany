// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { MessageComposer } from "@/views/room/MessageComposer";
import type { MessageIntent } from "@/api/tasks";
import type { Channel } from "@/views/room/model";

/**
 * First-run chat has one job: get an operator to make a request.
 *
 * A staffed but empty company used to lead with roster administration. These
 * checks exercise the two ends of the invitation: the card asks the parent to
 * start a brief, and the composer honours that request by replacing and
 * focusing its draft. Keeping them as rendered controls catches a regression
 * where the copy survives but the affordance no longer does anything.
 */

let container: HTMLDivElement;
let root: Root;

function renderComposer(prefill?: { text: string; revision: number }) {
  act(() => {
    root.render(
      createElement(MessageComposer, {
        placeholder: "Message #general",
        onSend: () => {},
        deliverableChoice: true,
        prefill,
      }),
    );
  });
}

function renderComposerForSend(
  onSend: (text: string, intent?: MessageIntent) => void,
) {
  act(() => {
    root.render(
      createElement(MessageComposer, {
        placeholder: "Message #general",
        onSend,
        deliverableChoice: true,
      }),
    );
  });
}

beforeEach(() => {
  (
    globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("the empty-channel first brief", () => {
  it("uses every new prefill revision", () => {
    renderComposer({ text: "Plan our first week.", revision: 1 });
    const textarea = container.querySelector("textarea");
    expect(textarea?.value).toBe("Plan our first week.");
    expect(document.activeElement).toBe(textarea);

    renderComposer({ text: "Plan our first month.", revision: 2 });
    expect(textarea?.value).toBe("Plan our first month.");

    // The three mode chips explained themselves through their `title`s. They
    // are behind `COMPOSER_INTENT_HIDDEN` now, so there is nothing to explain
    // and the prefill revision above is the whole of what this test covers.
    for (const intent of ["chat", "once", "workflow"]) {
      expect(
        container.querySelector(`[data-testid="composer-deliverable-${intent}"]`),
      ).toBeNull();
    }
  });

  it("sends the brief as a one-off task, whatever the draft held before", () => {
    const onSend = vi.fn();
    renderComposerForSend(onSend);

    // This used to pick "Just chatting" first and assert the brief cleared it:
    // a stale mode would have withheld the brief's own request. The chips are
    // behind `COMPOSER_INTENT_HIDDEN`, so there is no way to set a mode and no
    // stale one to clear — the half that survives is the outcome, which is that
    // the brief goes out as `once` on its own account rather than by default.
    //
    // Kept rather than retired with the chips: `once` is what makes the brief a
    // task the team acts on, and nothing else in the suite pins it.
    act(() => {
      root.render(
        createElement(MessageComposer, {
          placeholder: "Message #general",
          onSend,
          deliverableChoice: true,
          prefill: { text: "Help us get started.", revision: 1 },
        }),
      );
    });
    expect(container.querySelector("textarea")?.value).toBe(
      "Help us get started.",
    );

    // The brief is sent as a one-off task, not under the stale "chat" intent —
    // otherwise its request would be withheld. No mention directory is loaded
    // here, so the mentions arg is absent (undefined) rather than an empty list.
    act(() => {
      [...container.querySelectorAll("button")]
        .find((button) => button.getAttribute("aria-label") === "Send")!
        .click();
    });
    // The composer always passes third (attachments, issue #1682) and fourth
    // (mentions) arguments now — undefined here since this test never gives it
    // `uploadAttachment` or a mention directory.
    expect(onSend).toHaveBeenCalledWith("Help us get started.", "once", undefined, undefined);
  });
});
