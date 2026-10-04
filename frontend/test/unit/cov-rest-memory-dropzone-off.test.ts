// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import { DropZone } from "@/views/memory/DropZone";

/**
 * With memory off the host refuses every ingest (`409 not_configured`), but
 * the console's own lever is `off`, and `DropZone` is where it has to hold: a
 * raw drag-and-drop bypasses the two buttons' `disabled` prop entirely, so the
 * guard inside `onDrop` is the only thing standing between an operator's drop
 * and a request that can only fail.
 */

function dataTransferWith(files: File[]): DataTransfer {
  return {
    items: files.map((file) => ({
      kind: "file",
      webkitGetAsEntry: () => ({
        isFile: true,
        isDirectory: false,
        name: file.name,
        file: (cb: (f: File) => void) => cb(file),
      }),
    })),
    getData: () => "",
    files,
  } as unknown as DataTransfer;
}

let container: HTMLDivElement;
let root: Root;

async function show(client: OpenCompanyClient, off: boolean) {
  await act(async () => {
    root.render(
      createElement(DropZone, {
        client,
        company: "acme",
        off,
        onIngested: () => {},
      }),
    );
  });
}

function dropzone(): HTMLElement {
  return container.querySelector('[data-testid="memory-dropzone"]') as HTMLElement;
}

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

describe("a drop while memory is off ingests nothing", () => {
  it("refuses a raw drop, which the two disabled buttons cannot stop", async () => {
    const postForm = vi.fn(() => Promise.resolve({ items: [] }));
    const client = {
      scopeFor: () => "/api/v1/company/acme",
      postForm,
      post: vi.fn(),
    } as unknown as OpenCompanyClient;
    await show(client, true);

    const file = new File(["hello"], "note.txt", { type: "text/plain" });
    await act(async () => {
      const event = new Event("drop", { bubbles: true, cancelable: true }) as unknown as Event & {
        dataTransfer: DataTransfer;
        preventDefault: () => void;
      };
      Object.defineProperty(event, "dataTransfer", { value: dataTransferWith([file]) });
      dropzone().dispatchEvent(event);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(postForm).not.toHaveBeenCalled();
  });

  it("ingests normally once memory is on", async () => {
    const postForm = vi.fn(() => Promise.resolve({ items: [{ source: "note.txt", status: "stored" }] }));
    const client = {
      scopeFor: () => "/api/v1/company/acme",
      postForm,
      post: vi.fn(),
    } as unknown as OpenCompanyClient;
    await show(client, false);

    const file = new File(["hello"], "note.txt", { type: "text/plain" });
    await act(async () => {
      const event = new Event("drop", { bubbles: true, cancelable: true }) as unknown as Event & {
        dataTransfer: DataTransfer;
      };
      Object.defineProperty(event, "dataTransfer", { value: dataTransferWith([file]) });
      dropzone().dispatchEvent(event);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(postForm).toHaveBeenCalled();
  });
});
