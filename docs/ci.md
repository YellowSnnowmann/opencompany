# CI

OpenCompany's CI uses the design OpenHuman arrived at after a long run of
speed-up PRs (its #6537, #6551, #6579). It has three parts:

- **One required check.** `PR CI Gate` is posted by `ci-gate.yml`.
- **One lane plan, run in two places.** For org members it runs on the Hetzner
  EX63 microVMs. For everyone else, and for every commit that is not a PR, it
  runs on GitHub-hosted runners.
- **Each Rust graph is compiled once per lane, not once per job.** The old
  monolithic `ci.yml` compiled the dependency graph on six separate runners
  for every PR.

## The flows

| Workflow | Trigger | Runs |
| --- | --- | --- |
| `ci-fast.yml` | `pull_request_target` | A `route` job decides whether the PR author **and** the pusher are tinyhumansai members. If so, it calls `ci-lanes.yml` with `profile: ex63`. |
| `ci-fast-hosted.yml` | `pull_request`, `merge_group`, `push` to main/release, `workflow_dispatch` | `scripts/ci/ci-fast-route.sh` follows ci-fast's decision. Outsiders, merge-queue commits, pushes and dispatches call `ci-lanes.yml` with `profile: hosted`. |
| `ci-lanes.yml` | `workflow_call` | The shared body: `plan`, the lane jobs, the two service-backed suites, and `Gate`. |
| `ci-gate.yml` | `workflow_run` of both flows | `scripts/ci/ci-gate.mjs` posts the `PR CI Gate` commit status. |

`PR CI Gate` passes as soon as either flow's `Lanes / Gate` job passes. It is
pending while either flow is still running, and it fails once both have
finished without a pass. **A skipped Gate is never a pass.** GitHub treats a
skipped required check as green, and that is how OpenHuman's gate failed open
twice (#5474, #5616). When the gate passes, it cancels the hosted run still
working on that commit. It never cancels an EX63 run.

The check keeps the name the old `ci.yml` aggregate job had, so the branch
ruleset needed no edit. It is also posted on merge-queue commits.

### Why `pull_request_target`, and why it is safe here

A `pull_request_target` run always uses the base branch's copy of the
workflow, so a PR cannot edit the routing. The EX63 runner group admits only
`ci-fast.yml` and `ci-lanes.yml` at `refs/heads/main`. A fork's own
`pull_request` run (ref `refs/pull/N/merge`) therefore cannot claim those
runners, even if it names their label.

PR code runs only in the lane jobs, and only for members. Those jobs get a
read-only token and no secrets. Every EX63 job runs in a throwaway microVM.
The host supervisor re-checks membership on each job, and on a mismatch it
kills the VM and wipes the caches (`tinyhumansai/gh-hosted-runner`, "Security
model").

Rust caches are never saved from a `pull_request_target` run
(`save-if: github.event_name != 'pull_request_target'`), because that event
runs in the base branch's cache scope.

`workflow_run` and `pull_request_target` only fire once their workflow file
is on the default branch. The PR that introduces them therefore runs only the
hosted flow, and its own `PR CI Gate` status is never posted.

### Switches and secrets

- **`vars.CI_EX63`** (repository variable). The EX63 is used only while this
  is `enabled`. When it is unset or set to anything else, every PR runs
  hosted, so a down or unprovisioned box never leaves PRs waiting on a runner
  that will not come.
- **`secrets.CI_MEMBERSHIP_TOKEN`** (optional). This is a fine-grained token
  with only Organization → Members: Read. Without it, a private member is seen
  only through the PR's author association and may run hosted.

## Lanes

`scripts/ci/lanes/lanes-plan.mjs` is pure data. Each lane is a list of named
checks, and each check carries an area condition and an ordinary shell
command. `scripts/ci/lanes/lanes.mjs` runs the plan: lanes in parallel, the
checks in each lane in order.

Rules every check follows:

- **An area turns a whole suite on or off.** Nothing is narrowed to the
  changed files. OpenHuman tried changed-file selection and backed it out
  (#4486 → #6349). The self-test asserts that no command interpolates the
  diff.
- **A failed check never stops the checks after it.** It does block the checks
  that `needs` it. `after` orders checks without requiring success.
- **Every lane fails if any of its gating checks failed or was blocked.** A
  `reportOnly` check, such as the openhuman drift notice, never gates.

| Lane | What runs | Areas |
| --- | --- | --- |
| `static` | Pin and wiring guards, Markdown/Rust layout caps, version sync, the CI scripts' own tests. With a Rust change it adds the toolchain pin, vendored deps, lockfile, feature-lanes and `cargo fmt`. | always |
| `gated` | The gated binary (`openhuman,mcp,composio`) first. Then the `openhuman` build, clippy, `--tests`, every scoped suite (acp/runner/tinymemory, mcp/media, chargebee/paypal/composio, …), integration targets, offline e2e, `--all-features`. | rust (binary: rust or frontend) |
| `core` | The default host binary first. Then clippy, `cargo test`, sqlite and the default-graph scoped suites, and the TUI. | rust (binary: rust or frontend) |
| `console` | `npm ci`, the three typechecks, vitest, both builds, the frontend policy scripts, release-script tests, pnpm lockfiles. | frontend |
| `e2e` | Playwright `e2e`, `e2e:analytics` and first-run, run against `ci-out/bin/opencompany`. | rust or frontend |
| `e2e-live` | Playwright `e2e:live`, run against `ci-out/bin/opencompany-gated`. | rust or frontend |
| `desktop` | The Tauri shell's fmt, clippy and test with the release features, then `tauri build --debug --no-bundle` from both directories. | rust, frontend or desktop |

The MongoDB suite and the Stalwart mail round trip are separate GitHub-hosted
jobs in `ci-lanes.yml`, on both profiles. They need a job-level `services:`
container, and the microVM has no Docker. `Console (current Node, advisory)`
is also a separate job, and it never gates.

The areas come from `.github/ci-paths-filter.yml`: `rust`, `frontend` and
`desktop`. Every area also matches the CI wiring itself, so a CI change runs
everything. Merge-queue commits, `release` pushes and dispatches force every
area on (#793). `scripts/ci/assert-merge-group-workflow.sh` keeps that wiring
intact.

### Profiles

| | `ex63` | `hosted` |
| --- | --- | --- |
| Where | One job on one microVM: 10 vCPU, 28 GiB | Four GitHub-hosted jobs: `checks` (static+console), `core` (core+e2e), `gated` (gated+e2e-live), `desktop` |
| Target dirs | One per Rust lane on the per-job scratch disk, so lanes never queue on cargo's build lock | cargo's own, one per job |
| Rust cache | The host's shared, capped sccache store (`RUSTC_WRAPPER=sccache`) | `Swatinem/rust-cache`, one `shared-key` per group, written by main pushes. No sccache: OpenHuman measured 0% extra hits under a warm rust-cache (#4721). |
| Heavy compiles | A priority semaphore, about one Rust graph per 9 GiB (gated, then core, then desktop), plus swap. OpenHuman's VMs were OOM-killed without it (#6550). | One graph per job |
| Frontend | One `npm ci` and one console build in `console`, shared by e2e and desktop. npm cache and Chromium live on the persistent cache disk. | Each group installs its own |
| No sudo | `unshare --user --map-root-user --net` for the offline lane. Chromium's libraries are baked into the guest (`GUEST_EXTRA_PACKAGES`). | `sudo unshare --net`, `playwright install --with-deps` |

On the EX63 the two e2e suites run one after the other. They share one
checkout, and each suite's managed host binds that checkout's port.
`e2e-live` therefore waits for `e2e` to finish (`after`), but does not need it
to pass.

## Adding a check

1. Add it to the right lane in `lanes-plan.mjs`, with a `when:` area and a
   static command. If it needs a feature set, write it as a `cargo test …
   --features X` or `scripts/ci/run-scoped-suite.sh` line.
   `scripts/ci/assert-feature-lanes.sh` reads the plan for exactly those
   lines.
2. If it needs a new tool on the EX63, the job user has no sudo. Add an apt
   package to `GUEST_EXTRA_PACKAGES` in `tinyhumansai/gh-hosted-runner`, or
   install the tool into the workspace.
3. Run `node --test scripts/ci/lanes/lanes.test.mjs scripts/ci/ci-gate.test.mjs`.
4. Try the plan locally:

```bash
CI_AREA_RUST=true CI_AREA_FRONTEND=true node scripts/ci/lanes/lanes.mjs --profile hosted --print-matrix
CI_AREA_RUST=true node scripts/ci/lanes/lanes.mjs --profile hosted --lanes static --dry-run
CI_AREA_RUST=true node scripts/ci/lanes/lanes.mjs --profile hosted --lanes static   # really runs it
```

A new lane also needs a `Lane:` step in both the `ex63` job and the `lanes`
job of `ci-lanes.yml`. Add it to a group in `HOSTED_GROUPS` as well. The
self-test fails until both are done.

## Reading a run

Each lane is its own step. Inside it, each check is a folded group, followed
by a table of outcome, time and peak RSS. The `ci-out-<group>` artifact (or
`ci-out-ex63`) contains:

- `logs/<lane>.log`
- `ci-timings.json`, with each check's outcome, timings and peak RSS, target
  dir sizes, sccache hit rate and the VM's lowest free memory
- Playwright's `test-results/` when a suite failed

`gh run view <id> --log` shows the folded groups in plain text.
`CI_GATE_DRY_RUN=1 GH_TOKEN=… REPO=tinyhumansai/opencompany HEAD_SHA=<sha> node scripts/ci/ci-gate.mjs`
prints the gate's verdict for a commit without posting anything.

## Known costs

- **The host binary rebuilds part of the graph.** Building the host binary
  after `cargo test` in the same target dir recompiles about 45 crates,
  including tokio, hyper, axum and `opencompany-core`. The cause is
  test-only features in `[dev-dependencies]`: tokio `test-util`, tinyflows
  `mock`, wiremock. Aligning them would change the shipped binary, so the
  `core` lane pays it once.
- **nextest is not adopted yet.** `run-scoped-suite.sh`,
  `assert-integration-targets-run.sh` and the MongoDB count all parse
  libtest's `test result:` line. Moving them is its own change.
