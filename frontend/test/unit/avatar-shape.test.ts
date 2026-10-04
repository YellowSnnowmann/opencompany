// @vitest-environment jsdom

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it } from "vitest";

import {
  AVATAR_SHAPE_STORAGE_KEY,
  applyStoredAvatarShape,
  readStoredAvatarShape,
  setAvatarShape,
} from "@/lib/avatar-shape";

/**
 * The avatar shape is a per-browser appearance preference: round by default,
 * the rounded square on request, applied as `data-avatar-shape` on `<html>`
 * and persisted beside the accent preset.
 */

const read = (rel: string) =>
  readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), "../../src", rel), "utf8");

afterEach(() => {
  localStorage.clear();
  delete document.documentElement.dataset.avatarShape;
});

describe("the avatar shape preference", () => {
  it("is round by default, with no attribute on the page", () => {
    expect(readStoredAvatarShape()).toBe("round");
    applyStoredAvatarShape();
    expect(document.documentElement.dataset.avatarShape).toBeUndefined();
  });

  it("applies and persists the rounded square, and clears both for round", () => {
    setAvatarShape("rounded");
    expect(document.documentElement.dataset.avatarShape).toBe("rounded");
    expect(localStorage.getItem(AVATAR_SHAPE_STORAGE_KEY)).toBe("rounded");
    setAvatarShape("round");
    expect(document.documentElement.dataset.avatarShape).toBeUndefined();
    expect(localStorage.getItem(AVATAR_SHAPE_STORAGE_KEY)).toBeNull();
  });

  it("ignores a stored value it does not know", () => {
    localStorage.setItem(AVATAR_SHAPE_STORAGE_KEY, "hexagon");
    expect(readStoredAvatarShape()).toBe("round");
  });

  it("drives every teammate face through --avatar-radius, round unless overridden", () => {
    const css = read("index.css");
    expect(css).toMatch(/:root\s*\{\s*--avatar-radius:\s*9999px;/);
    expect(css).toContain(':root[data-avatar-shape="rounded"]');
    const avatar = read("components/teammate-avatar.tsx");
    expect(avatar).not.toContain("rounded-md");
    expect(avatar.match(/rounded-\(--avatar-radius\)/g)?.length).toBe(3);
  });

  it("is applied before first paint", () => {
    expect(read("main.tsx")).toContain("applyStoredAvatarShape();");
  });
});
