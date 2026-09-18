import type { APIRequestContext } from "@playwright/test";

/**
 * Disconnects the providers a spec connected to the shared E2E company, after
 * that spec — including after one that timed out.
 *
 * ## Why a spec that adds a provider owes the rest of the run this
 *
 * `playwright.config.ts` brings up **one** host serving **one** company
 * (`companies/e2e_harness`) for the whole run — every spec file that is not
 * first-run / Euler / live-LLM / visual drives that same company. A provider
 * connected by one spec is therefore visible to every spec that runs after it,
 * and two keys-rework decisions (issue #2306, `docs/key-reworks/README.md`)
 * make that far more than a stray row in a list:
 *
 * - **D-first-default (X1):** the first provider a company ever connects
 *   becomes its default automatically, with no opt-out. `resolve_for_turn`
 *   resolves every turn through a full default *before* it looks at routes.
 * - **D-never-clear-default (X14):** deleting or disabling that provider never
 *   clears `inference/default`. A turn then fails closed — "The company
 *   default uses …, which is removed" — and no route unsets the default or
 *   points it back at the platform brain, which is a credential chain rather
 *   than a row (`DELETE …/inference`, the console's "Reset to default", clears
 *   only the legacy single-slot config and its key).
 *
 * So a spec that connects a row pointing at the discard port and leaves it
 * sends every later agent turn in the run to `127.0.0.1:9/v1/chat/completions`
 * (thirty-odd unrelated specs went red that way on the live-brain lane), and
 * with the row deleted every later turn fails closed on the stale default
 * instead. The second is the honest failure — it names the cause — but it is
 * still a failure: the shared company cannot be handed back to the platform
 * brain through the API as it stands. Until it can (a reset that also clears
 * the default, or a lane of their own for the specs that connect providers),
 * a spec that connects a provider to the shared company breaks every turn
 * after it, and this helper only makes that failure say so.
 *
 * ## What this does
 *
 * Deletes each slug in `slugs`, confirming the in-use guard: a delete of the
 * default, or of a pinned provider, is refused with `409 in_use` otherwise —
 * which is why a plain `DELETE` in a `finally` never cleared these rows even
 * when it ran. A slug already gone answers 404 and is ignored.
 *
 * Call it from a `test.afterEach` hook, **not** from a `finally` inside the
 * test: when a test hits its timeout Playwright abandons the test function
 * outright and an in-body `finally` never runs, which is how the leak
 * happened in the first place. A hook runs after a timed-out test, with a
 * request context that still works.
 *
 * A `404` (slug already gone — a test that deleted it itself, or one that
 * never got as far as creating it) is the one response this treats as
 * success. Every other non-OK response throws instead of being swallowed
 * (CodeRabbit review on #2310): a `.catch(() => {})` on the whole request
 * hid a failed DELETE exactly as quietly as a *successful* one, so a real
 * cleanup failure — the row still connected, still enabled, still the
 * company default — reported nothing and left the same poisoned-default
 * failure this file's own header describes for every spec after it, with
 * no error pointing back at the cleanup that should have caught it.
 */
export async function disconnectSharedProviders(
  request: APIRequestContext,
  slugs: string[],
) {
  // Every slug gets its DELETE before anything is reported. Throwing on the
  // first failure would skip the rest, and a spec that connected two (the
  // `e2e-one`/`e2e-two` case in `inference.spec.ts`) would then leave the
  // second one behind — the very leak this helper exists to close. The first
  // failure is what gets thrown, after the loop.
  let firstFailure: Error | null = null;
  for (const slug of slugs) {
    const path = `/api/v1/company/inference/providers/${slug}?confirmInUse=true`;
    const response = await request.delete(path);
    if (response.ok() || response.status() === 404) continue;
    const body = await response.text().catch(() => "<body could not be read>");
    firstFailure ??= new Error(
      `[shared-inference] DELETE ${path} → ${response.status()} ${response.statusText()}; ` +
        `body: ${body || "<empty>"}\n` +
        `Failed to disconnect "${slug}" from the shared E2E company — left connected, it (or ` +
        "the default it may have become) can poison every later spec's agent turn. See this " +
        "function's own doc comment.",
    );
  }
  if (firstFailure) throw firstFailure;
}
