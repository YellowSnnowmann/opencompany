#!/usr/bin/env node
//
// The inference fixture for the **live-LLM** lane: a real model, reached
// through the same seam `mock-brain.mjs` occupies.
//
// `mock-brain.mjs` proves the chain runs by scripting every choice in it. That
// is the right default and it is also its limit: what it cannot show is that a
// model, handed a goal and this company's real tool descriptions, *decides* to
// break the goal up, hand the pieces to the right teammates, and close them out
// afterwards. Nothing in the scripted lane would notice if the orchestrator's
// prompt stopped describing its own tools, because the scripted lane never
// reads them.
//
// So this lane points the host at a real chat-completions endpoint, and this
// process sits in between for the two things the host needs that a plain
// upstream will not do:
//
//   1. **`/embeddings`.** The routers this is pointed at (a local llm-ladder,
//      an OpenRouter-style gateway) serve chat completions and answer `404` for
//      embeddings. The host's embeddings client shares one base URL with its
//      chat client and validates the returned width, so an unanswered
//      `/embeddings` fails a memory write in the middle of a turn. Served from
//      `./embedding.mjs`, exactly as the mock does.
//   2. **The model name.** The lane names the rung once, here, rather than
//      through the host's own `OPENCOMPANY_INFERENCE_MODEL` — so pointing a run
//      at a different rung is one variable and never a question of which of two
//      names won.
//
// It also writes one line per turn to stderr naming the tool calls the model
// chose, because "the model answered but called nothing" and "the model was
// never asked" are the two failures of this lane and they are otherwise the
// same silence.
//
// Nothing here scripts, filters or retries a decision. Whatever the model says
// is what the host gets — the point of the lane is that the decisions are not
// ours.
//
// Usage:
//   node live-brain-proxy.mjs [--bind HOST:PORT]
// Env:
//   PW_LIVE_LLM_BIND      same as --bind        (default 127.0.0.1:8096)
//   PW_LIVE_LLM_URL       upstream base URL     (default http://127.0.0.1:6969/v1)
//   PW_LIVE_LLM_MODEL     the rung to ask for   (default flash)
//   PW_LIVE_LLM_KEY       upstream bearer       (default $LADDER_API_KEY)
//
// A `:0` port binds an ephemeral one; the chosen address is always printed to
// stderr as `[live brain] listening on http://HOST:PORT`.
//

import { createServer } from "node:http";

import { embeddings } from "./embedding.mjs";

const bindArg = process.argv.indexOf("--bind");
const BIND = (bindArg >= 0 ? process.argv[bindArg + 1] : undefined) ||
  process.env.PW_LIVE_LLM_BIND ||
  "127.0.0.1:8096";

/** Where the real model lives. */
const UPSTREAM = (process.env.PW_LIVE_LLM_URL || "http://127.0.0.1:6969/v1").replace(/\/+$/, "");

/** The rung asked for, whatever the host asked for. */
const MODEL = process.env.PW_LIVE_LLM_MODEL || "flash";

/** The upstream credential. Absent is allowed — some routers want none. */
const KEY = process.env.PW_LIVE_LLM_KEY || process.env.LADDER_API_KEY || "";

/**
 * How long one upstream turn may take.
 *
 * Generous on purpose: a reasoning model deciding a fan-out over a company's
 * whole toolbelt is tens of seconds, and a proxy that gave up at the usual
 * `fetch` default would surface as "the agent never replied" — a failure that
 * reads as the product's and is not.
 */
const UPSTREAM_TIMEOUT_MS = Number.parseInt(process.env.PW_LIVE_LLM_TIMEOUT_MS || "180000", 10);

/**
 * Reads a whole request body.
 *
 * @param {import("node:http").IncomingMessage} request
 * @returns {Promise<string>}
 */
function readBody(request) {
  return new Promise((resolve, reject) => {
    /** @type {Buffer[]} */
    const chunks = [];
    request.on("data", (chunk) => chunks.push(chunk));
    request.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    request.on("error", reject);
  });
}

/**
 * @param {import("node:http").ServerResponse} response
 * @param {number} status
 * @param {any} payload
 */
function sendJson(response, status, payload) {
  const body = JSON.stringify(payload);
  response.writeHead(status, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(body),
  });
  response.end(body);
}

/** One readable line about what the model just decided. */
function describe(reply) {
  const message = reply?.choices?.[0]?.message;
  const calls = Array.isArray(message?.tool_calls) ? message.tool_calls : [];
  if (calls.length > 0) {
    return `tool calls: ${calls.map((call) => call?.function?.name ?? "?").join(" + ")}`;
  }
  const text = typeof message?.content === "string" ? message.content : "";
  return `text reply (${text.length} chars)`;
}

/**
 * Forwards one chat-completions request upstream, with the lane's model.
 *
 * @param {any} body the parsed request
 * @returns {Promise<{status: number, payload: any}>}
 */
async function forward(body) {
  const asked = { ...body, model: MODEL };
  // `stream` is refused rather than forwarded: the host does not ask for it,
  // and a streamed body would arrive here as something this proxy does not
  // reassemble — better a loud 400 than a silently empty reply.
  if (asked.stream) {
    return {
      status: 400,
      payload: { error: { message: "live-brain-proxy does not forward streaming requests" } },
    };
  }

  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), UPSTREAM_TIMEOUT_MS);
  try {
    const response = await fetch(`${UPSTREAM}/chat/completions`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        ...(KEY ? { authorization: `Bearer ${KEY}` } : {}),
      },
      body: JSON.stringify(asked),
      signal: controller.signal,
    });
    const text = await response.text();
    let payload;
    try {
      payload = JSON.parse(text);
    } catch {
      // An upstream that answered with prose (a gateway error page, usually) is
      // reported as itself. Swallowing it into a fabricated completion would
      // put words in the model's mouth.
      return {
        status: 502,
        payload: { error: { message: `upstream sent a non-JSON body: ${text.slice(0, 500)}` } },
      };
    }
    return { status: response.status, payload };
  } catch (error) {
    return {
      status: 504,
      payload: { error: { message: `upstream did not answer: ${String(error)}` } },
    };
  } finally {
    clearTimeout(timer);
  }
}

const server = createServer((request, response) => {
  const path = new URL(request.url ?? "/", "http://localhost").pathname;

  if (path === "/healthz") {
    sendJson(response, 200, { ok: true, upstream: UPSTREAM, model: MODEL });
    return;
  }

  if (request.method !== "POST") {
    sendJson(response, 405, { error: { message: "POST only" } });
    return;
  }

  void (async () => {
    let body;
    try {
      body = JSON.parse((await readBody(request)) || "{}");
    } catch {
      sendJson(response, 400, { error: { message: "body was not JSON" } });
      return;
    }

    // Matching on the suffix, as the mock does: a base URL with or without a
    // `/v1` both work, which is one fewer way for the lane's configuration and
    // this server to disagree.
    if (path.endsWith("/chat/completions")) {
      const started = Date.now();
      const { status, payload } = await forward(body);
      const turns = Array.isArray(body?.messages) ? body.messages.length : 0;
      const belt = Array.isArray(body?.tools) ? body.tools.length : 0;
      process.stderr.write(
        `[live brain] ${status} in ${Date.now() - started}ms ` +
          `(${turns} messages, ${belt} tools) — ${describe(payload)}\n`,
      );
      sendJson(response, status, payload);
      return;
    }

    if (path.endsWith("/embeddings")) {
      sendJson(response, 200, embeddings(body));
      return;
    }

    sendJson(response, 404, { error: { message: `no route for ${path}` } });
  })();
});

const [host, port] = BIND.split(":");
server.listen(Number(port), host, () => {
  const chosen = server.address();
  const shown = typeof chosen === "object" && chosen ? `${chosen.address}:${chosen.port}` : BIND;
  process.stderr.write(`[live brain] listening on http://${shown}\n`);
  process.stderr.write(`[live brain] forwarding to ${UPSTREAM} as model "${MODEL}"\n`);
  if (!KEY) process.stderr.write("[live brain] no upstream credential set\n");
});
