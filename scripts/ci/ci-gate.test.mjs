// Tests for scripts/ci/ci-gate.mjs — the PR CI Gate decision. Pure: no API.
// Run: node --test scripts/ci/ci-gate.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";

import { CONTEXT, SOURCES, decide, latestPrRun } from "./ci-gate.mjs";

const [EX63, HOSTED] = SOURCES;

function entry(source, id, status, conclusion, job) {
  return {
    source,
    run: { id, status, conclusion, html_url: `https://runs/${id}` },
    job: job === undefined ? null : { ...job, html_url: `https://jobs/${id}` },
  };
}

const done = (conclusion) => ({ status: "completed", conclusion });

test("the status keeps the ruleset's existing required-check name", () => {
  assert.equal(CONTEXT, "PR CI Gate");
});

test("each flow is decided by its in-workflow Gate job", () => {
  assert.deepEqual(
    SOURCES.map((s) => [s.workflow, s.job, s.hosted]),
    [
      ["ci-fast.yml", "Lanes / Gate", false],
      ["ci-fast-hosted.yml", "Lanes / Gate", true],
    ],
  );
});

test("the EX63 flow passing wins and cancels a hosted run still going", () => {
  const verdict = decide([
    entry(EX63, 1, "completed", "success", done("success")),
    entry(HOSTED, 2, "in_progress", null, null),
  ]);
  assert.equal(verdict.state, "success");
  assert.equal(verdict.description, "CI Fast passed");
  assert.equal(verdict.targetUrl, "https://jobs/1");
  assert.deepEqual(verdict.cancel, [{ id: 2, name: "CI Fast (hosted)" }]);
});

test("the hosted flow passing never cancels the EX63 run", () => {
  const verdict = decide([
    entry(EX63, 1, "in_progress", null, { status: "in_progress", conclusion: null }),
    entry(HOSTED, 2, "completed", "success", done("success")),
  ]);
  assert.equal(verdict.state, "success");
  assert.deepEqual(verdict.cancel, []);
});

test("a member's hosted run skipping its Gate is not a pass: pending on the EX63", () => {
  const verdict = decide([
    entry(EX63, 1, "in_progress", null, { status: "queued", conclusion: null }),
    entry(HOSTED, 2, "completed", "success", done("skipped")),
  ]);
  assert.equal(verdict.state, "pending");
  assert.match(verdict.description, /CI Fast/);
});

test("an outsider's EX63 flow skipping its Gate is not a pass either", () => {
  const verdict = decide([
    entry(EX63, 1, "completed", "success", done("skipped")),
    entry(HOSTED, 2, "in_progress", null, { status: "in_progress", conclusion: null }),
  ]);
  assert.equal(verdict.state, "pending");
});

test("every flow finished with only skips: failure, never success", () => {
  const verdict = decide([
    entry(EX63, 1, "completed", "success", done("skipped")),
    entry(HOSTED, 2, "completed", "success", done("skipped")),
  ]);
  assert.equal(verdict.state, "failure");
  assert.equal(verdict.description, "No CI flow ran its checks");
});

test("a flow whose Gate job never materialised is not a pass", () => {
  const verdict = decide([entry(HOSTED, 2, "completed", "failure", null)]);
  assert.equal(verdict.state, "failure");
});

test("a failed Gate with nothing else running fails and links the failure", () => {
  const verdict = decide([
    entry(EX63, 1, "completed", "success", done("skipped")),
    entry(HOSTED, 2, "completed", "failure", done("failure")),
  ]);
  assert.equal(verdict.state, "failure");
  assert.equal(verdict.description, "CI Fast (hosted): failure");
  assert.equal(verdict.targetUrl, "https://jobs/2");
});

test("no run yet: pending, waiting for CI to start", () => {
  const verdict = decide([]);
  assert.equal(verdict.state, "pending");
  assert.equal(verdict.description, "Waiting for CI to start");
});

test("merge-queue runs gate their commit; pushes do not", () => {
  const runs = [
    { id: 1, event: "push", created_at: "2026-10-03T10:00:00Z" },
    { id: 2, event: "merge_group", created_at: "2026-10-03T09:00:00Z" },
  ];
  assert.equal(latestPrRun(runs).id, 2);
  assert.equal(latestPrRun([runs[0]]), null);
});

test("the latest gated run is chosen by creation time", () => {
  const runs = [
    { id: 1, event: "pull_request", created_at: "2026-10-03T09:00:00Z" },
    { id: 3, event: "pull_request", created_at: "2026-10-03T11:00:00Z" },
    { id: 2, event: "pull_request_target", created_at: "2026-10-03T10:00:00Z" },
  ];
  assert.equal(latestPrRun(runs).id, 3);
});
