import { describe, expect, it } from "vitest";

import type { FsNode } from "@/api/workspace";
import { rosterNameMap } from "@/lib/roster-names";
import {
  childrenOf,
  folderPathLabel,
  isDerivedNode,
  sortedFolders,
  subtreeIds,
} from "@/lib/workspace";

/**
 * The two derivations the Move dialog was missing (issue #1381).
 *
 * It listed every folder by bare `name` — so two `Drafts` under different
 * parents were identical rows and a roster folder was a raw ULID — in the host's
 * own unspecified `tree()` order, beside a tree that was sorted.
 */

function node(over: {
  id: string;
  name: string;
  kind: "folder" | "file";
  parentId?: string | null;
}): FsNode {
  return {
    parentId: null,
    updatedAt: 1,
    createdBy: { kind: "operator" },
    updatedBy: { kind: "operator" },
    ...over,
  } as FsNode;
}

const TREE: FsNode[] = [
  node({ id: "product", name: "Product", kind: "folder" }),
  node({ id: "p-drafts", name: "Drafts", kind: "folder", parentId: "product" }),
  node({ id: "agents", name: "Agents", kind: "folder" }),
  node({
    id: "roster",
    name: "01JQZY8T7K",
    kind: "folder",
    parentId: "agents",
  }),
  node({ id: "artifacts", name: "Artifacts", kind: "folder" }),
  node({ id: "derived", name: "derived", kind: "folder" }),
  node({ id: "d-child", name: "Goals", kind: "folder", parentId: "derived" }),
  node({ id: "standards", name: "Standards", kind: "folder" }),
  node({
    id: "s-drafts",
    name: "Drafts",
    kind: "folder",
    parentId: "standards",
  }),
  node({ id: "note", name: "Plan.md", kind: "file", parentId: "product" }),
];

const NAMES = rosterNameMap([{ id: "01JQZY8T7K", name: "Nadia" }]);

describe("folderPathLabel", () => {
  it("tells two same-named folders apart by their path", () => {
    expect(folderPathLabel(TREE, "p-drafts", NAMES)).toBe("Product / Drafts");
    expect(folderPathLabel(TREE, "s-drafts", NAMES)).toBe("Standards / Drafts");
    // The defect: both rows read "Drafts".
    expect(folderPathLabel(TREE, "p-drafts", NAMES)).not.toBe(
      folderPathLabel(TREE, "s-drafts", NAMES),
    );
  });

  it("resolves a roster id the way the tree does", () => {
    expect(folderPathLabel(TREE, "roster", NAMES)).toBe("Agents / Nadia");
  });

  it("falls back to the id when the roster has not loaded", () => {
    expect(folderPathLabel(TREE, "roster", new Map())).toBe(
      "Agents / 01JQZY8T7K",
    );
  });

  it("names a root folder as itself", () => {
    expect(folderPathLabel(TREE, "product", NAMES)).toBe("Product");
  });
});

describe("sortedFolders", () => {
  it("walks the tree in the order the explorer draws it", () => {
    const order = sortedFolders(TREE, new Set()).map((f) => f.id);

    // Depth-first through `childrenOf`, which is the tree's own sort.
    // `derived` sorts last among folders (issue #1382), and the destination
    // list follows the explorer rather than second-guessing it.
    expect(order).toEqual([
      "agents",
      "roster",
      "artifacts",
      "product",
      "p-drafts",
      "standards",
      "s-drafts",
      "derived",
      "d-child",
    ]);
    // And it agrees with the tree at the top level.
    expect(
      order.filter((id) => TREE.find((n) => n.id === id)?.parentId === null),
    ).toEqual(
      childrenOf(TREE, null)
        .filter((n) => n.kind === "folder")
        .map((n) => n.id),
    );
  });

  it("returns no files, only destinations", () => {
    expect(sortedFolders(TREE, new Set()).some((f) => f.kind === "file")).toBe(
      false,
    );
  });

  it("drops a blocked subtree wholesale, not just its root", () => {
    const derivedIds = TREE.filter((n) => isDerivedNode(TREE, n.id)).map(
      (n) => n.id,
    );
    const out = sortedFolders(TREE, new Set(derivedIds)).map((f) => f.id);

    expect(out).not.toContain("derived");
    // The child would otherwise still be offered — and the host refuses writes
    // anywhere under `derived/`.
    expect(out).not.toContain("d-child");
  });

  it("drops the moving node's own descendants, which would be a cycle", () => {
    const out = sortedFolders(TREE, subtreeIds(TREE, "product")).map(
      (f) => f.id,
    );
    expect(out).not.toContain("product");
    expect(out).not.toContain("p-drafts");
    expect(out).toContain("standards");
  });
});
