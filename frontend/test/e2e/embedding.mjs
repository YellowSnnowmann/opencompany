//
// The deterministic `/embeddings` the two inference fixtures both answer with.
//
// It lives on its own because there are now two servers in front of the host —
// `mock-brain.mjs`, which scripts every answer, and `live-brain-proxy.mjs`,
// which forwards the thinking to a real model — and only one of those two
// things is the model's job. Embeddings are not: the routers this suite is
// pointed at serve chat completions and answer `404` for `/v1/embeddings`, and
// a host whose memory writes 404 fails for a reason that has nothing to do with
// the behaviour under test.
//
// So both fixtures serve the same vectors from here. Deterministic ones, never
// random: two runs of the suite must not disagree about what a note means, and
// a proxy run and a mock run must not disagree with each other either.
//

/**
 * Width of every vector returned. `HostedEmbeddings` compares this against its
 * declared dimensionality and errors on a mismatch rather than truncating, and
 * its default is 1024 (`embedding-v1`'s only allowed size).
 */
export const EMBEDDING_DIM = 1024;

/**
 * A deterministic unit-ish vector for one input.
 *
 * @param {string} input
 * @returns {number[]}
 */
export function embedding(input) {
  let seed = 2166136261;
  for (let i = 0; i < input.length; i += 1) {
    seed = Math.imul(seed ^ input.charCodeAt(i), 16777619) >>> 0;
  }
  const vector = new Array(EMBEDDING_DIM);
  for (let i = 0; i < EMBEDDING_DIM; i += 1) {
    seed = (Math.imul(seed, 1103515245) + 12345) >>> 0;
    vector[i] = seed / 4294967295 - 0.5;
  }
  return vector;
}

/**
 * The embeddings reply for one request, in input order.
 *
 * @param {any} body
 * @returns {any}
 */
export function embeddings(body) {
  const raw = body?.input;
  const inputs = Array.isArray(raw) ? raw : [raw ?? ""];
  return {
    object: "list",
    model: typeof body?.model === "string" ? body.model : "mock-embedding",
    data: inputs.map((input, index) => ({
      object: "embedding",
      index,
      embedding: embedding(String(input)),
    })),
    usage: { prompt_tokens: 0, total_tokens: 0 },
  };
}
