import {
  request,
  type APIRequestContext,
  type APIResponse,
  type FullConfig,
} from "@playwright/test";

import {
  EXPECTED_INSTANCE_ID,
  MANAGED_HOST_HOME,
  SPEC_PATH,
  identityFailure,
  readHomeInstanceId,
} from "./host-identity";
import { LIVE_BRAIN, MOCK_BRAIN_BIND } from "./capabilities";

const ADMIN_EMAIL = "harness-e2e@tinyhumans.ai";
const REQUEST_PATH = "/api/v1/company/auth/request";
const VERIFY_PATH = "/api/v1/company/auth/verify";

/**
 * Identifies the server Playwright adopted, then authenticates once and shares
 * the session with every spec through Playwright storage state, so the suite
 * signs in a single time.
 *
 * Every failure here aborts the whole run before a single spec executes, so the
 * message has to carry enough to diagnose it without a second run: the endpoint,
 * the status, and the body the host actually returned. `auth/request` answers
 * `202 {"sent": true}` for *every* outcome by design — it refuses to say whether
 * an address is a member — so a missing `dev_code` is otherwise indistinguishable
 * from a broken host, which is exactly what issue #271 sent people chasing.
 *
 * The identity check comes first, and ahead of the storage-state early return,
 * because this hook runs after `webServer` has resolved and is therefore the
 * only place that sees which server was actually adopted rather than which one
 * was configured — issue #1773, and `host-identity.ts` for the whole story. A
 * run with no sign-in still gets it: what is on the port is worth knowing even
 * when nothing is about to log in to it.
 */
export default async function globalSetup(config: FullConfig) {
  const baseURL = config.projects[0]?.use.baseURL as string | undefined;
  if (!baseURL) {
    throw new Error(
      "[e2e global-setup] no baseURL is configured. Set PW_BASE_URL to the " +
        "running OpenCompany host, e.g. PW_BASE_URL=http://127.0.0.1:8080.",
    );
  }

  const identity = await request.newContext({ baseURL });
  try {
    await identifyServer(identity, baseURL);
  } finally {
    await identity.dispose();
  }

  // Read the RESOLVED path off the config, not `process.env.PW_STORAGE_STATE`.
  // The config now defaults it when it is the one bringing the host up (issue
  // #406), and reading the raw variable meant this returned early in exactly
  // that case — writing no session, and leaving every spec to fail on a
  // storage-state file nobody had created. The env var still wins where it is
  // set: the config honours it first.
  const storageState = config.projects[0]?.use.storageState as
    string | undefined;
  if (!storageState) return;

  const context = await request.newContext({ baseURL });
  try {
    const requested = await post(context, baseURL, REQUEST_PATH, {
      email: ADMIN_EMAIL,
    });
    if (!requested.response.ok()) {
      throw new Error(
        `[e2e global-setup] ${describe(requested)}\n` +
          "The host is reachable but rejected the sign-in request.",
      );
    }

    const devCode = readDevCode(requested);
    if (!devCode) {
      throw new Error(
        `[e2e global-setup] ${describe(requested)}\n` +
          `No dev_code came back, so the suite cannot sign in as ${ADMIN_EMAIL}.\n` +
          "The host answers this route identically whatever happened, so check, " +
          "in order:\n" +
          `  1. The host serves a company whose [users] admins lists ${ADMIN_EMAIL} ` +
          "(companies/e2e_harness). An address the host does not recognise gets " +
          'this exact {"sent": true} answer.\n' +
          `  2. The host binds loopback (127.0.0.1 / localhost) and has no ` +
          "OPENCOMPANY_PUBLIC_URL set. A host that looks routable never echoes a " +
          "login code, even with no mail configured.\n" +
          "  3. No OPENCOMPANY_MAIL_* transport is configured. With one wired the " +
          "code is mailed instead of echoed, and this bootstrap cannot read it " +
          "at all — a throttled resend is not the difference, the mailing is.\n" +
          "Re-running the suite inside 60s is NOT a cause: the resend throttle no " +
          "longer applies where the code is echoed rather than mailed (issue #271, " +
          "see RESEND_INTERVAL_MILLIS in src/server/users/routes.rs). If the host " +
          "predates that fix, a second run within the minute does fail here.",
      );
    }

    const verified = await post(context, baseURL, VERIFY_PATH, {
      code: devCode,
    });
    if (!verified.response.ok()) {
      throw new Error(
        `[e2e global-setup] ${describe(verified)}\n` +
          "The dev_code from auth/request was refused, so no session was minted. " +
          "A login code is single-use and expires 15 minutes after it is minted; " +
          "a 401 here means it was already spent or is stale.",
      );
    }
    await context.storageState({ path: storageState });
    // Not gated on `MANAGED_HOST_HOME !== undefined` (this run bringing the
    // host up itself): the caller-managed mode (`PW_BASE_URL` + `PW_LIVE_BRAIN=1`,
    // `playwright.config.ts`'s documented "against a host you brought
    // yourself, the flag still enables the four specs but the fixtures are
    // yours to start too") is on the same shared, mutable `e2e_harness`
    // company and hits the identical X1/X14 poisoning this guards against —
    // a caller who started `mock-brain.mjs` on the documented default address
    // is exactly who this is for. See `connectAnchorProvider`'s own doc
    // comment for the mechanism.
    if (LIVE_BRAIN) {
      await connectAnchorProvider(context);
    }
  } finally {
    await context.dispose();
  }
}

/** This bootstrap's own slug and model — read back by name, so both the
 * duplicate-detection and the default-repair paths agree on what they are
 * looking for with whatever `mock-brain.mjs` is listening on for THIS run. */
const ANCHOR_SLUG = "e2e-anchor-default";
const ANCHOR_MODEL = "e2e-anchor-model";
/** The anchor's stored credential — a placeholder the mock brain never checks. */
const ANCHOR_KEY = "pw-e2e-anchor";

/**
 * Connects one permanent, reachable provider before any spec runs, on the
 * live-brain lane only, and makes sure it is (still) the company default.
 *
 * Decision D-first-default (X1, keys rework issue #2306,
 * `server/ops/inference/providers.rs`): the *first* provider a company ever
 * connects becomes its default automatically, with no opt-out on the write
 * path — `POST …/inference/providers` claims an `Unset` default whatever
 * `make_default` says. Decision D-never-clear-default (X14): deleting,
 * disabling, or clearing the key of *that* provider later never rewrites the
 * default marker — `resolve_for_turn` then fails every later turn closed,
 * "The company default uses …, which is removed."
 *
 * `companies/e2e_harness` starts with an `Unset` default (no `[inference]`
 * section), so whichever spec happens to run first and connects a provider
 * claims it. On the live-brain lane that used to be
 * `agent-detail.spec.ts`'s pin test, which points its own provider at the
 * discard port and deletes it in an `afterEach` — after which every later
 * spec in the run, and not just that file's, failed every real agent turn on
 * the stale default. `frontend/test/e2e/shared-inference.ts` documents the
 * dead end from the test side: there is no route today that clears or
 * re-points an already-set default, only ones that refuse to.
 *
 * The fix here does not touch that Rust invariant — X14 has its own test
 * (`a_delete_disable_or_key_clear_never_rewrites_the_stored_default_marker`)
 * and stays exactly as strict. It wins the race instead: this runs before
 * `webServer`'s first spec, so it is unconditionally the *first* provider
 * this company ever connects, and it never disconnects — no spec's cleanup
 * list names its slug. It points at `mock-brain.mjs`, already up by the time
 * this runs (`playwright.config.ts`'s `webServer` array starts the fixtures
 * ahead of the host), so it stays healthy for the rest of the run and every
 * later spec's default-routed turn reaches a real, scripted answer instead of
 * a closed door.
 *
 * Default-feature `Console E2E` (no `LIVE_BRAIN`) does not need this: without
 * `--features openhuman` the harness that calls `resolve_for_turn` for a real
 * turn is not compiled in, so a stale default there has no later spec to
 * poison — confirmed by CI, where that lane's only failures were the
 * provider-page specs themselves, never a downstream one.
 *
 * ## Three things a reused host (`reuseExistingServer`, a repeat local run
 * against `target/e2e/data`) needs that a fresh one does not, all from
 * Codex/CodeRabbit review on #2310
 *
 * 1. **The anchor might already be the default, but a stale one might not
 *    be.** An earlier run's spec could have connected and later deleted a
 *    throwaway provider *before* this bootstrap existed, or `X14` could have
 *    left the default pointed at a row nothing here created. Reading the
 *    status and explicitly `POST`ing `…/default` whether or not the anchor
 *    row is new closes both: an already-current default is a no-op write,
 *    and a stale one is repointed.
 * 2. **A reused anchor row can carry a stale `baseUrl`.** `PW_MOCK_BRAIN_BIND`
 *    can differ between two local runs (this file's own isolation story:
 *    several `PW_*` port variables exist precisely so concurrent runs on one
 *    box do not collide). A leftover row still named `ANCHOR_SLUG` but
 *    pointed at a `mock-brain.mjs` from a *different* run's port would make
 *    every turn in *this* run fail to connect. Compared and repaired via
 *    `PUT` before trusting it.
 * 3. **Read state, don't parse an error message for it.** The previous
 *    version detected "already exists" from `add_provider`'s `400` body
 *    text. That still works, but a `GET` first is the same idempotency check
 *    without depending on error-message wording, and it is what supplies the
 *    stored `baseUrl` finding (2) needs anyway.
 */
async function connectAnchorProvider(
  context: APIRequestContext,
): Promise<void> {
  const anchorUrl = `http://${MOCK_BRAIN_BIND}/v1`;
  const existing = await findAnchorProvider(context);
  if (existing) {
    if (existing.baseUrl !== anchorUrl) {
      // The key goes with the move. A stale anchor from another run's
      // `PW_MOCK_BRAIN_BIND` is almost always a different *origin* (another
      // port), and `edit_provider` refuses to carry a stored credential across
      // origins unless the request re-enters it — a blank key field means
      // "unchanged", which is exactly what must not happen when the host
      // changes. The anchor's key is this file's own placeholder, so resending
      // it is free and turns a 400 into the repoint this branch exists for.
      const edited = await context.put(
        `/api/v1/company/inference/providers/${ANCHOR_SLUG}`,
        {
          data: { baseUrl: anchorUrl, key: ANCHOR_KEY },
        },
      );
      if (!edited.ok()) {
        throw await setupError(
          "PUT",
          `/api/v1/company/inference/providers/${ANCHOR_SLUG}`,
          edited,
          "the anchor exists from an earlier run but points at a stale mock-brain address, and " +
            "repointing it failed",
        );
      }
    }
  } else {
    const created = await context.post("/api/v1/company/inference/providers", {
      data: {
        kind: "custom",
        label: "E2E Anchor Default",
        baseUrl: anchorUrl,
        key: ANCHOR_KEY,
        model: ANCHOR_MODEL,
      },
    });
    if (!created.ok()) {
      throw await setupError(
        "POST",
        "/api/v1/company/inference/providers",
        created,
        "connecting the permanent anchor default failed",
      );
    }
  }
  // Whether the row above is new or was already there: make it the default
  // unconditionally. `add_provider` only auto-selects a *new* row into an
  // `Unset` default, so a reused host whose default was left pointed at
  // something else (or at nothing, per X14) needs this explicit write
  // regardless — a POST that finds it already the default is a no-op.
  const defaulted = await context.post(
    `/api/v1/company/inference/providers/${ANCHOR_SLUG}/default`,
    { data: { model: ANCHOR_MODEL } },
  );
  if (!defaulted.ok()) {
    throw await setupError(
      "POST",
      `/api/v1/company/inference/providers/${ANCHOR_SLUG}/default`,
      defaulted,
      "the anchor row exists and is reachable but could not be made the company default",
    );
  }
}

/** The one field `connectAnchorProvider` needs off each row `GET …/inference` reports. */
async function findAnchorProvider(
  context: APIRequestContext,
): Promise<{ baseUrl: string } | undefined> {
  const response = await context.get("/api/v1/company/inference");
  if (!response.ok()) {
    throw await setupError(
      "GET",
      "/api/v1/company/inference",
      response,
      "could not read this company's inference status to check for an existing anchor",
    );
  }
  const status = (await response.json()) as {
    providers?: { slug: string; baseUrl: string }[];
  };
  return status.providers?.find((p) => p.slug === ANCHOR_SLUG);
}

/** One consistently-shaped error for every `connectAnchorProvider` request that fails. */
async function setupError(
  method: string,
  path: string,
  response: APIResponse,
  why: string,
): Promise<Error> {
  const body = await response.text().catch(() => "<body could not be read>");
  return new Error(
    `[e2e global-setup] ${method} ${path} → ${response.status()} ${response.statusText()}; ` +
      `body: ${body || "<empty>"}\n` +
      `${why}, so every later spec's agent turn would resolve through whatever the first ` +
      "test-created provider leaves behind instead — see connectAnchorProvider's doc comment.",
  );
}

/**
 * Asks `/spec` who answered, and throws unless it is this run's host.
 *
 * The `instance-id` read happens **after** the request and not before: the id
 * is minted lazily on first use, so answering us is what creates that file
 * under the responder's own data root. Read in this order, a root of ours with
 * no file is proof the responder does not serve it. See `host-identity.ts`.
 */
async function identifyServer(
  context: APIRequestContext,
  baseURL: string,
): Promise<void> {
  const url = `${baseURL.replace(/\/$/, "")}${SPEC_PATH}`;

  let response: APIResponse;
  try {
    response = await context.get(SPEC_PATH);
  } catch (error) {
    throw new Error(
      `[e2e global-setup] GET ${url} did not answer: ${String(error)}\n` +
        "Either nothing is serving this address, or something is holding the " +
        "connection open without ever replying — a wedged process still owns " +
        "the port. Either way the suite has no host to drive.",
    );
  }

  const failure = identityFailure({
    url,
    status: response.status(),
    contentType: response.headers()["content-type"] ?? null,
    // Text, not JSON: a dev server's HTML fallback is exactly the body worth
    // quoting back, and `.json()` would throw over it before it could be shown.
    body: await response.text().catch(() => "<body could not be read>"),
    expectedInstanceId: EXPECTED_INSTANCE_ID,
    home: MANAGED_HOST_HOME,
    homeInstanceId: MANAGED_HOST_HOME
      ? readHomeInstanceId(MANAGED_HOST_HOME)
      : undefined,
  });

  if (failure) throw new Error(`[e2e global-setup] ${failure}`);
}

/** A response paired with the request that produced it, for reporting. */
type Attempt = {
  method: string;
  url: string;
  response: APIResponse;
  body: string;
};

async function post(
  context: APIRequestContext,
  baseURL: string,
  path: string,
  data: Record<string, unknown>,
): Promise<Attempt> {
  const response = await context.post(path, { data });
  return {
    method: "POST",
    url: `${baseURL.replace(/\/$/, "")}${path}`,
    response,
    // Read as text, not JSON: an HTML error page or an empty body is exactly
    // the case worth reporting verbatim, and `.json()` would throw over it.
    body: await response.text().catch(() => "<body could not be read>"),
  };
}

/** One line naming the request, its status, and what came back. */
function describe(attempt: Attempt): string {
  const { method, url, response, body } = attempt;
  const shown = body.length > 500 ? `${body.slice(0, 500)}…` : body;
  return `${method} ${url} → ${response.status()} ${response.statusText()}; body: ${shown || "<empty>"}`;
}

/** The echoed login code, if the body is JSON and carries one. */
function readDevCode(attempt: Attempt): string | undefined {
  try {
    const parsed = JSON.parse(attempt.body) as { dev_code?: unknown };
    return typeof parsed.dev_code === "string" && parsed.dev_code
      ? parsed.dev_code
      : undefined;
  } catch {
    return undefined;
  }
}
