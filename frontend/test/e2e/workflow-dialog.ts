import { expect, type APIRequestContext, type Locator, type Page, type Route } from "@playwright/test";

import { openWorkflow } from "./workflows";

/**
 * How a spec reaches each of the New-workflow dialog's two surfaces.
 *
 * The dialog used to be one thing: open it and the graph form was there, in
 * create mode, with every control on screen. It is now two, and which one you
 * get is decided by `createSurface` (`src/lib/workflow-create-surface.ts`):
 *
 * - **the one box** — a sentence and Create, on every create, on every company
 *   and every build. No Name, no Workflow ID, no Nodes, no Connections.
 * - **the graph form** — byte-for-byte the dialog that always existed, reached
 *   by an **edit**, and by a create the host **refused**.
 *
 * So "open the creator and fill in the fields" is no longer a route to
 * anything, and every spec that opened one used to reach the other. This module
 * is where the two routes are stated once rather than re-derived per spec — the
 * same reason `workflows.ts` exists for "how do I reach a workflow".
 *
 * ## Why some of these stub the host
 *
 * The suite runs in two lanes against two different hosts (`playwright.config.ts`):
 * a default-feature host, whose companies think with the offline `echo` brain,
 * and a feature-gated one thinking with `mock-brain.mjs`. The one box behaves
 * differently on each — one drafts, the other falls back to the operator's
 * sentence — and both are behaviours worth pinning, so the *cognition read* is
 * stubbed to name which one is under test rather than left to the lane.
 *
 * What is never stubbed is the **write**. Every helper here that creates a
 * workflow lets the real host store it, because "the console posted a graph the
 * host accepts" is the half a stub cannot tell you.
 */

export const COMPANY_SCOPE = "/api/v1/company";

/** Matches a company-scoped path under either scope shape the client can emit. */
function scoped(url: URL, suffix: string): boolean {
  return new RegExp(`/api/v1/(company|companies/[^/]+)${suffix}$`).test(url.pathname);
}

/**
 * Clears the first-run welcome tour, and waits until it is gone.
 *
 * Scoped to its own dialog rather than to the button's name: the overlay
 * swallows pointer events, so the final assertion is the one that matters —
 * nothing is left to swallow the next click.
 */
export async function dismissTour(page: Page) {
  const welcome = page
    .getByRole("dialog")
    .filter({ hasText: "Welcome to your company" });
  const tourSkip = welcome.getByRole("button", { name: "Skip for now" });
  if (await visibleSoon(tourSkip, 10_000)) {
    await tourSkip.click();
    await expect(welcome).toHaveCount(0);
  }

  await expect(
    page.getByRole("button", { name: "Skip for now" }),
    "a first-run overlay is still on screen, swallowing clicks",
  ).toHaveCount(0);
}

/** Whether `locator` becomes visible within `timeout`, without failing if not. */
async function visibleSoon(locator: Locator, timeout: number): Promise<boolean> {
  try {
    await locator.first().waitFor({ state: "visible", timeout });
    return true;
  } catch {
    return false;
  }
}

/** A minimal graph the host accepts: one trigger and nothing else. */
export function starterGraph(id: string, name: string) {
  return {
    id,
    name,
    description: "Created by the automation-dialog e2e specs.",
    nodes: [{ id: "start", kind: "trigger", name: "Start" }],
    edges: [],
  };
}

/** Creates a workflow over HTTP so a spec has one to edit. */
export async function createGraph(
  request: APIRequestContext,
  id: string,
  name: string,
): Promise<void> {
  const res = await request.post(`${COMPANY_SCOPE}/workflows`, {
    data: starterGraph(id, name),
  });
  expect(res.ok(), `create ${id}: ${res.status()} ${await res.text()}`).toBeTruthy();
}

/**
 * Best-effort teardown so a failed spec does not poison the next run.
 * `expectedVersion` is required (issue #1013), so this reads the current token
 * first — a spec's own copy may be stale by teardown time.
 */
export async function removeWorkflow(request: APIRequestContext, id: string) {
  const version = await request
    .get(`${COMPANY_SCOPE}/workflows/${id}`)
    .then(async (res) => (res.ok() ? ((await res.json()).version as string | null) : null))
    .catch(() => null);
  const query = version ? `?expectedVersion=${encodeURIComponent(version)}` : "";
  await request.delete(`${COMPANY_SCOPE}/workflows/${id}${query}`).catch(() => undefined);
}

/**
 * **Route one to the graph form: an edit.**
 *
 * Creates a one-trigger workflow over HTTP, opens it, and opens its edit dialog.
 * The returned dialog is the same form a create used to open on — the fields,
 * the node rows, the blur validation and the host pre-flight are all identical,
 * because there is exactly one implementation of them and `createSurface`
 * chooses when to render it rather than what it is.
 *
 * The starter is a lone trigger on purpose: the create form opened on one, so
 * every ported assertion that counts rows (`.nth(1)` is "the row I just added")
 * keeps its original meaning.
 */
export async function openEditForm(
  page: Page,
  request: APIRequestContext,
  id: string,
  name: string,
): Promise<Locator> {
  await createGraph(request, id, name);
  await page.goto("/#/workflows");
  await dismissTour(page);
  await openWorkflow(page, name);
  const edit = page.getByTestId("workflow-edit");
  await expect(edit, "a console-created automation must be editable").toBeEnabled();
  await edit.click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText(`Edit “${name}”`, { exact: true })).toBeVisible();
  return dialog;
}

/**
 * Answers the dialog's cognition read (`GET …/inference`) with a fixed path, so
 * a spec states which one box it is driving instead of inheriting the lane's.
 *
 * Only `cognition` is read by the dialog; the rest of `InferenceStatus` is
 * absent rather than invented, so a spec that starts depending on another field
 * fails here rather than passing against a fiction.
 */
export async function stubCognition(page: Page, cognition: "echo" | "hosted") {
  await page.route(
    (url) => scoped(url, "/inference"),
    async (route: Route) => {
      if (route.request().method() !== "GET") return route.fallback();
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ cognition }),
      });
    },
  );
}

/**
 * A drafted graph a stubbed copilot hands back — trigger → output, wired.
 *
 * `output` rather than `agent` on purpose: an `agent` node names a teammate the
 * host resolves against the company's roster, so a graph built here with an
 * invented one is refused by the write — and these specs are about the write
 * succeeding, not about roster validation. An output routed nowhere needs
 * nothing the harness company has to supply.
 */
export function draftedGraph(id: string, name: string) {
  return {
    id,
    name,
    description: "Drafted by the stubbed copilot.",
    nodes: [
      { id: "start", kind: "trigger", name: "Start", schedule: "0 9 * * 1" },
      { id: "report", kind: "output", name: "Report" },
    ],
    edges: [{ from: "start", to: "report" }],
  };
}

/**
 * Answers `POST …/workflows/draft-from-description` with a fixed body.
 *
 * The copilot's answer is a model's, so it is the one thing in this flow that
 * cannot be asserted against without pinning it. Counting the calls is part of
 * the point: "the console did not ask a host that cannot answer" is a claim
 * only the counter can make.
 */
export async function stubDraft(page: Page, body: unknown, calls: { count: number }) {
  await page.route(
    (url) => scoped(url, "/workflows/draft-from-description"),
    async (route: Route) => {
      if (route.request().method() !== "POST") return route.fallback();
      calls.count += 1;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(body),
      });
    },
  );
}

/**
 * Fails `POST …/workflows` with a chosen status and envelope, leaving every
 * other verb on that path — the list read the view lives on — alone.
 *
 * The status is the subject, not scenery: a `409` is an instruction the operator
 * can obey and a `500` is not, and the dialog has to tell them apart.
 */
export async function stubCreateFailure(
  page: Page,
  status: number,
  envelope: { error: string; code: string },
) {
  await page.route(
    (url) => scoped(url, "/workflows"),
    async (route: Route) => {
      if (route.request().method() !== "POST") return route.fallback();
      await route.fulfill({
        status,
        contentType: "application/json",
        body: JSON.stringify(envelope),
      });
    },
  );
}

/** Opens Workflows → New workflow and returns the one-box dialog. */
export async function openOneBox(page: Page): Promise<Locator> {
  await page.goto("/#/workflows");
  await dismissTour(page);
  await page.getByRole("button", { name: "New automation" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByTestId("workflow-describe-box")).toBeVisible();
  return dialog;
}

/**
 * **Route two to the graph form: a create the host refused.**
 *
 * Stubs a drafted graph and a `409` on the write, then drives the one box until
 * the fields come back. This is the route the redesign actually ships — the
 * refusal names an id, and the one box has no field to obey it with — so a spec
 * that needs the *create-mode* form (an editable id, the created-paused notice,
 * `validate()`'s complaints) both reaches it and pins that it is reachable.
 *
 * Returns the dialog, now showing the form loaded with the refused graph.
 */
export async function openRefusedCreateForm(
  page: Page,
  graph: ReturnType<typeof draftedGraph>,
): Promise<Locator> {
  await stubCognition(page, "hosted");
  await stubDraft(page, { automatable: true, summary: "a digest", workflow: graph }, {
    count: 0,
  });
  await stubCreateFailure(page, 409, {
    error: `A workflow with id \`${graph.id}\` already exists. Pick a different id.`,
    code: "conflict",
  });

  const dialog = await openOneBox(page);
  await dialog.getByTestId("workflow-describe-box").fill("Every Monday, draft the digest.");
  await dialog.getByTestId("workflow-dialog-submit").click();

  await expect(
    dialog.getByLabel("Automation ID", { exact: true }),
    "a refusal naming an id must hand over the field that obeys it",
  ).toBeVisible({ timeout: 30_000 });
  return dialog;
}
