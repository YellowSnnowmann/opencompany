// @vitest-environment jsdom

import { act, createElement } from "react";

import { menuItemEnabled, openAttachItem } from "./support/composer-menu";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ApiError } from "@/api/types";
import { MessageComposer } from "@/views/room/MessageComposer";

/**
 * `uploadChatAttachment` (`api/chat.ts`) enforces no size cap of its own —
 * the limit, if any, is the host's on `POST …/chat/upload`. `chat-composer-
 * attach.test.ts` pins the happy path; nothing pinned what a refusal from
 * that route (an oversized file, most plausibly a `413`) does to the
 * composer. It must say so and leave no phantom chip staged for a node the
 * host never created — the same "silent success" shape 's other
 * findings were about.
 */

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
  vi.restoreAllMocks();
});

async function render(upload: ReturnType<typeof vi.fn>) {
  await act(async () => {
    root.render(
      createElement(MessageComposer, {
        placeholder: "Message engineering",
        onSend: vi.fn(),
        uploadAttachment: upload,
        deleteAttachment: vi.fn(),
      }),
    );
  });
}

async function pick(file: File) {
  const input = container.querySelector('input[type="file"]') as HTMLInputElement;
  Object.defineProperty(input, "files", { value: [file], configurable: true });
  await act(async () => {
    input.dispatchEvent(new Event("change", { bubbles: true }));
  });
}

const bigFile = () => new File([new Uint8Array(16)], "recording.mov", { type: "video/quicktime" });

describe("a file the host refuses as too large", () => {
  it("names the host's own refusal rather than a silent failure", async () => {
    const upload = vi.fn(async () => {
      throw new ApiError(413, "payload_too_large", "That file is larger than this host allows.");
    });
    await render(upload);
    await pick(bigFile());

    expect(container.querySelector('[role="alert"]')?.textContent).toBe(
      "That file is larger than this host allows.",
    );
  });

  it("stages no chip for the refused upload — nothing to send", async () => {
    const upload = vi.fn(async () => {
      throw new ApiError(413, "payload_too_large", "too big");
    });
    await render(upload);
    await pick(bigFile());

    expect(container.textContent).not.toContain("recording.mov");
    const send = container.querySelector('[aria-label="Send"]') as HTMLButtonElement;
    // No text typed either, so Send stays disabled either way — the chip is
    // the thing under test, not the button's overall enabled state.
    expect(send.disabled).toBe(true);
  });

  it("clears the paperclip's busy state so a retry is offered, not stuck spinning", async () => {
    const upload = vi.fn(async () => {
      throw new ApiError(413, "payload_too_large", "too big");
    });
    await render(upload);
    await pick(bigFile());

    const paperclip = openAttachItem(container)!;
    expect(menuItemEnabled(paperclip)).toBe(true);
  });
});
