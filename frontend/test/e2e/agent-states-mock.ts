import type { Page } from "@playwright/test";

/**
 * A fully mocked company for the agent-state and DM-order specs.
 *
 * Both specs need what a real host cannot do on demand: fifteen teammates (the
 * rail's real cap, not demo data), and a frame at a chosen instant followed by
 * deliberate silence. So every `/api/v1/**` call is answered here, in the idiom
 * of `chat-presence.spec.ts`, and the SSE stream is a gate the test opens.
 *
 * The gate, not a fixed body: a fulfilled `text/event-stream` response ends, and
 * the console's `EventSource` reconnects (`retry: 50` makes that quick). Each
 * request waits until the test pushes frames, gets exactly those, and the next
 * reconnect waits again, so frames arrive when the test says and never replay.
 */

export const COMPANY = "acme";

/** Fifteen names, some long enough to force truncation in the 15rem rail. */
export const NAMES = [
  "Rae Ceo",
  "Ada Lovelace",
  "Grace Hopper",
  "Alan Turing",
  "Katherine Johnson",
  "Margaret Hamilton",
  "Barbara Liskov",
  "Donald Knuth",
  "Edsger Dijkstra",
  "Tim Berners-Lee",
  "Radia Perlman",
  "Guido van Rossum",
  "Hedy Lamarr",
  "Linus Torvalds",
  "Annie Easley-Montgomery",
];

export const ROSTER = NAMES.map((name, i) => ({
  id: `agent-${i + 1}`,
  name,
  role: i === 0 ? "Chief Executive" : "Engineer",
}));

/** A controllable server-sent-event stream. */
export interface Sse {
  /** Delivers these frames to the console the next time it is listening. */
  push: (...frames: unknown[]) => void;
}

export interface MockOptions {
  /** Pending approvals, read on every `GET …/approvals`. */
  approvals?: () => unknown[];
  /** Open runs, for the reload re-arm (`GET …/runs`). */
  runs?: () => unknown[];
  /** `chat/history` rows for one desk. */
  history?: (desk: string) => unknown[];
}

export async function mockCompany(page: Page, opts: MockOptions = {}): Promise<Sse> {
  await page.addInitScript(() => {
    const real = Storage.prototype.getItem;
    Storage.prototype.getItem = function getItem(key: string) {
      return key.startsWith("oc-tour:") ? '{"skipped":true}' : real.call(this, key);
    };
  });

  const buffered: string[] = [];
  let waiter: ((frames: string[]) => void) | null = null;
  const sse: Sse = {
    push: (...frames) => {
      const wire = frames.map((f) => `data: ${JSON.stringify(f)}\n\n`);
      if (waiter) {
        const w = waiter;
        waiter = null;
        w(wire);
      } else {
        buffered.push(...wire);
      }
    },
  };

  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const status = {
      id: COMPANY,
      name: "Acme",
      lifecycle: "running",
      pending_approvals: opts.approvals?.().length ?? 0,
    };

    if (path === "/api/v1/companies") return json([status]);
    if (path === `/api/v1/companies/${COMPANY}`) return json(status);
    if (path.endsWith("/desks")) return json([]);
    if (path.endsWith("/team")) return json(ROSTER);
    if (path.endsWith("/chat/mentionables")) {
      return json({
        agents: ROSTER,
        people: [],
        desks: [],
        everyone: { label: "everyone", aliases: ["everyone"] },
      });
    }
    if (path.endsWith("/chat/read-state")) return json({ markers: [] });
    if (path.endsWith("/chat/history")) {
      return json(opts.history?.(url.searchParams.get("desk") ?? "") ?? []);
    }
    if (path.endsWith("/approvals")) return json(opts.approvals?.() ?? []);
    if (path.includes("/runs/")) {
      const id = decodeURIComponent(path.split("/runs/")[1]);
      const run = (opts.runs?.() ?? []).find((r) => (r as { id: string }).id === id);
      return run
        ? json({ run: { phase: "active", ...(run as object) }, steps: [] })
        : json({ error: "missing" }, 404);
    }
    if (path.endsWith("/runs")) return json(opts.runs?.() ?? []);
    if (path.endsWith("/presence")) {
      return route.request().method() === "GET"
        ? json({ people: [] })
        : route.fulfill({ status: 204, body: "" });
    }
    if (path.endsWith("/chat/typing")) return route.fulfill({ status: 204, body: "" });
    if (path.endsWith("/events")) {
      const frames = buffered.length
        ? buffered.splice(0)
        : await new Promise<string[]>((resolve) => {
            waiter = resolve;
          });
      return route.fulfill({
        status: 200,
        headers: { "content-type": "text/event-stream", "cache-control": "no-cache" },
        body: `retry: 50\n\n${frames.join("")}`,
      });
    }
    if (path.endsWith("/me")) return json({ id: "op", email: "op@example.com", role: "member" });
    return json([]);
  });
  return sse;
}

/** The rail's row buttons, in on-screen order. */
export function railRows(page: Page) {
  return page.getByTestId("room-rail-slot").locator("li button");
}

/** A DM row by its teammate's name. */
export function dmRow(page: Page, name: string) {
  return railRows(page).filter({ hasText: name });
}
