#!/usr/bin/env node
// PR CI Gate — the one commit status branch protection requires. It passes as
// soon as ANY CI flow for the commit passes, and then cancels the
// GitHub-hosted runs still working on it. Ported from OpenHuman's
// scripts/ci/ci-gate.mjs (#6579).
//
// Run by .github/workflows/ci-gate.yml on every `workflow_run` (requested and
// completed) of the flows below. That workflow runs from the default branch
// and never checks out PR code; this script only reads run and job state
// through the API and writes a commit status.
//
//   success  a flow's decisive job passed; the other GitHub-hosted runs for
//            the commit are cancelled (the EX63 run never is)
//   pending  nothing has passed yet and at least one flow is still running
//   failure  every flow finished and none passed
//
// A skipped decisive job is never a pass. GitHub treats a skipped required
// check as passing, which is how OpenHuman's gate failed open (#5474, #5616);
// here a skip only means "this flow did not handle the commit".
//
// The context keeps the name the old monolithic ci.yml's aggregate job had,
// `PR CI Gate`, so the branch ruleset that requires it needs no edit. It is
// posted on merge-queue commits too: the queue requires the same check.
//
// Env: GH_TOKEN, REPO (owner/name), HEAD_SHA. Optional: GITHUB_API_URL,
// CI_GATE_DRY_RUN=1 (decide and log, but post nothing and cancel nothing).

import { pathToFileURL } from "node:url";

export const CONTEXT = "PR CI Gate";

// Order is preference: when several passed, the first names the status.
// Both flows call ci-lanes.yml from a job named "Lanes"; its in-workflow
// "Gate" job aggregates every lane job of that flow, so it is the decisive one.
export const SOURCES = [
  {
    workflow: "ci-fast.yml",
    name: "CI Fast",
    job: "Lanes / Gate",
    hosted: false,
  },
  {
    workflow: "ci-fast-hosted.yml",
    name: "CI Fast (hosted)",
    job: "Lanes / Gate",
    hosted: true,
  },
];

/** The events whose runs gate a commit. Pushes to main gate nothing. */
export const GATED_EVENTS = new Set([
  "pull_request",
  "pull_request_target",
  "merge_group",
]);

function log(msg) {
  console.log(`[ci-gate] ${msg}`);
}

/**
 * The latest gated-event run from a workflow's runs for one commit. A re-run
 * keeps its run id (the API reports the newest attempt), so "latest" is by
 * creation.
 */
export function latestPrRun(runs) {
  return (
    runs
      .filter((run) => GATED_EVENTS.has(run.event))
      .sort((a, b) =>
        String(b.created_at).localeCompare(String(a.created_at)),
      )[0] ?? null
  );
}

/**
 * Pure decision over the flows that have a run for this commit.
 * @param {{source: typeof SOURCES[number], run: {id:number,status:string,conclusion:string|null,html_url?:string}, job: {status:string,conclusion:string|null,html_url?:string}|null}[]} entries
 */
export function decide(entries) {
  const winner = entries.find((e) => e.job?.conclusion === "success");
  if (winner) {
    const cancel = entries
      .filter(
        (e) => e !== winner && e.source.hosted && e.run.status !== "completed",
      )
      .map((e) => ({ id: e.run.id, name: e.source.name }));
    return {
      state: "success",
      description: `${winner.source.name} passed`,
      targetUrl: winner.job.html_url ?? winner.run.html_url ?? null,
      cancel,
    };
  }
  const running = entries.filter((e) => e.run.status !== "completed");
  if (running.length > 0 || entries.length === 0) {
    const names = running.map((e) => e.source.name).join(", ");
    return {
      state: "pending",
      description:
        entries.length === 0
          ? "Waiting for CI to start"
          : `Waiting on ${names}`,
      targetUrl: running[0]?.run.html_url ?? null,
      cancel: [],
    };
  }
  const ran = entries.filter(
    (e) => e.job && e.job.conclusion && e.job.conclusion !== "skipped",
  );
  const failed = ran.find((e) => e.job.conclusion !== "success");
  return {
    state: "failure",
    description:
      ran.length === 0
        ? "No CI flow ran its checks"
        : ran.map((e) => `${e.source.name}: ${e.job.conclusion}`).join("; "),
    targetUrl:
      failed?.job.html_url ??
      failed?.run.html_url ??
      entries[0].run.html_url ??
      null,
    cancel: [],
  };
}

async function api(path, { method = "GET", body } = {}) {
  const base = process.env.GITHUB_API_URL || "https://api.github.com";
  const res = await fetch(`${base}${path}`, {
    method,
    headers: {
      Accept: "application/vnd.github+json",
      Authorization: `Bearer ${process.env.GH_TOKEN}`,
      "X-GitHub-Api-Version": "2022-11-28",
      ...(body ? { "Content-Type": "application/json" } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  return { status: res.status, data: text ? JSON.parse(text) : null };
}

async function collect(repo, sha) {
  const entries = [];
  for (const source of SOURCES) {
    const runs = await api(
      `/repos/${repo}/actions/workflows/${source.workflow}/runs?head_sha=${sha}&per_page=20`,
    );
    if (runs.status !== 200)
      throw new Error(`list ${source.workflow} runs: HTTP ${runs.status}`);
    const run = latestPrRun(runs.data.workflow_runs ?? []);
    if (!run) {
      log(`${source.name}: no gated run for ${sha.slice(0, 10)}`);
      continue;
    }
    const jobs = await api(
      `/repos/${repo}/actions/runs/${run.id}/jobs?filter=latest&per_page=100`,
    );
    if (jobs.status !== 200)
      throw new Error(`list jobs of run ${run.id}: HTTP ${jobs.status}`);
    const job =
      (jobs.data.jobs ?? []).find((j) => j.name === source.job) ?? null;
    log(
      `${source.name}: run=${run.id} status=${run.status} conclusion=${run.conclusion} ` +
        `job="${source.job}" ${job ? `${job.status}/${job.conclusion}` : "absent"}`,
    );
    entries.push({ source, run, job });
  }
  return entries;
}

async function main() {
  const { REPO: repo, HEAD_SHA: sha, GH_TOKEN: token } = process.env;
  if (!repo || !sha || !token)
    throw new Error("REPO, HEAD_SHA and GH_TOKEN are required");
  if (!/^[0-9a-f]{40}$/.test(sha))
    throw new Error(`HEAD_SHA is not a commit sha: ${sha}`);
  const dry = process.env.CI_GATE_DRY_RUN === "1";

  const verdict = decide(await collect(repo, sha));
  log(`verdict state=${verdict.state} description="${verdict.description}"`);
  if (dry) return;

  const status = await api(`/repos/${repo}/statuses/${sha}`, {
    method: "POST",
    body: {
      state: verdict.state,
      context: CONTEXT,
      description: verdict.description.slice(0, 140),
      ...(verdict.targetUrl ? { target_url: verdict.targetUrl } : {}),
    },
  });
  if (status.status !== 201)
    throw new Error(`post status: HTTP ${status.status}`);

  for (const { id, name } of verdict.cancel) {
    // 409: it finished between the listing and now. Nothing to do.
    const res = await api(`/repos/${repo}/actions/runs/${id}/cancel`, {
      method: "POST",
    });
    log(`cancel ${name} run=${id}: HTTP ${res.status}`);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((err) => {
    console.error(`::error::[ci-gate] ${err.message}`);
    process.exit(1);
  });
}
