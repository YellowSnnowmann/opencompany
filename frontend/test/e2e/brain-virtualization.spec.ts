import { expect, test, type Page } from "@playwright/test";

/**
 * The Brain memory list windows a large set instead of mounting every row.
 *
 * The pure window math is proven in the unit suite; what only a browser can
 * show is that a real 1000-item payload renders as a bounded handful of cards
 * and that scrolling the page moves that window. The payload is staged with
 * `page.route` rather than seeded through the host: a thousand real `POST
 * /memory` writes would be slow and prove nothing the fabricated list does not.
 */

const COMPANY = "acme";
const COUNT = 1000;

interface WireEntry {
  id: string;
  kind: string;
  learningKind: string;
  title: string;
  body: string;
  namespace: string;
  tags: string[];
  updatedAt: number;
  editable: boolean;
}

function entries(count: number): WireEntry[] {
  return Array.from({ length: count }, (_, i) => ({
    id: `m-${i}`,
    kind: "learning",
    learningKind: "fact",
    title: `memory ${i}`,
    body: "",
    namespace: `team:${COMPANY}`,
    tags: [],
    updatedAt: 0,
    editable: true,
  }));
}

async function mockApi(page: Page) {
  await page.addInitScript(() => {
    const real = Storage.prototype.getItem;
    Storage.prototype.getItem = function getItem(key: string) {
      return key.startsWith("oc-tour:") ? '{"skipped":true}' : real.call(this, key);
    };
  });

  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const status = { id: COMPANY, name: "Acme", lifecycle: "running", pending_approvals: 0 };

    if (path === "/api/v1/companies") return json([status]);
    if (path === `/api/v1/companies/${COMPANY}`) return json(status);

    if (path.endsWith("/memory/status")) {
      return json({ root: `team:${COMPANY}`, on: true, engine: "local" });
    }
    if (path.endsWith("/memory/agents")) return json({ root: `team:${COMPANY}`, agents: [] });
    if (path.endsWith("/memory/brain")) {
      return json({ root: `team:${COMPANY}`, sources: [], unfiled: 0 });
    }
    if (path.endsWith("/memory")) return json({ items: entries(COUNT) });

    if (path.endsWith("/me")) return json({ id: "op", email: "op@example.com", role: "member" });
    return json([]);
  });
}

/** The overflow-y-auto ancestor the list windows against. */
async function scrollListBy(page: Page, delta: number) {
  await page.evaluate((by) => {
    const list = document.querySelector('[data-testid="memory-list"]');
    let el = list?.parentElement ?? null;
    while (el) {
      const style = getComputedStyle(el);
      if (/(auto|scroll)/.test(style.overflowY) && el.scrollHeight > el.clientHeight) {
        el.scrollTop += by;
        el.dispatchEvent(new Event("scroll"));
        return;
      }
      el = el.parentElement;
    }
  }, delta);
}

test("Brain windows a 1000-item memory set and moves the window on scroll", async ({ page }) => {
  await mockApi(page);

  await page.goto("/#/settings/brain");
  await expect(page.getByRole("heading", { name: "Brain", level: 1 })).toBeVisible();

  const cards = page.getByTestId("memory-card");
  await expect(cards.first()).toBeVisible({ timeout: 30_000 });

  // The property under test: a windowed list mounts only the rows the viewport
  // can show plus overscan — never the full 1000 the pre-fix view mounted.
  const initialCount = await cards.count();
  expect(initialCount).toBeGreaterThan(0);
  expect(initialCount).toBeLessThan(80);

  const before = await cards.allInnerTexts();

  // Scroll the page well past the first window and prove the window followed:
  // new rows mounted and the leading row is no longer row 0.
  await scrollListBy(page, 6000);
  await expect
    .poll(async () => (await cards.allInnerTexts())[0], { timeout: 10_000 })
    .not.toBe(before[0]);

  const after = await cards.allInnerTexts();
  expect(after.length).toBeGreaterThan(0);
  expect(after.length).toBeLessThan(80);
  // The window moved rather than grew: rows that were on screen are gone, and
  // rows that were not are now mounted.
  expect(after.some((t) => !before.includes(t))).toBe(true);
  expect(before.some((t) => !after.includes(t))).toBe(true);
});
