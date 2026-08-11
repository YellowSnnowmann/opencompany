# Testing epic #565 end to end

This branch (`integration/565-mcp-skills-epic`) carries **all four** findings of
[#565](https://github.com/tinyhumansai/opencompany/issues/565) at once, so the console↔harness
disagreement can be checked as one story rather than four PRs.

| Finding | Issue | Where it comes from |
|---|---|---|
| 1 · console tells operators to restart | #566 | already in `main` (PR #598, merged) |
| 3 · nothing shows which agents reach a server | #568 | already in `main` (PR #631, merged) |
| 2 · management routes not feature-gated | #567 | PR #641, merged into this branch |
| 4 · skills readable but never runnable | #569 | PR #640, merged into this branch |

All three merges were clean — no conflict resolution is hiding in here.

**The one thing to understand before starting:** #567 is a statement about *the build you are running*,
so half of these checks only mean anything when you run the host **twice** — once on the default
feature set (no MCP bridge) and once with `--features openhuman,tinycortex,mcp`. Scenario A and
Scenario B below are the same console against those two binaries, and several rows are expected to
differ. That difference *is* the feature.

---

## 0 · One-time setup

```bash
cd <this worktree>
git submodule update --init --recursive          # vendored openhuman + tinyagents

cd frontend && npm ci && cd ..                   # console deps
```

Build **both** binaries up front — the second one takes a while, so start it before you need it.
Point cargo at the main checkout's warm target directory rather than growing a second 30 GB tree
(`host.sh` takes an absolute binary path, so where it was built does not matter):

```bash
export CARGO_TARGET_DIR=$PWD/../opencompany/target
mkdir -p target/debug

# Scenario A binary: default features, no MCP bridge, no harness
cargo build --locked --bin opencompany
cp "$CARGO_TARGET_DIR/debug/opencompany" target/debug/opencompany-default

# Scenario B binary: the bridge and the harness compiled in
cargo build --locked --features openhuman,tinycortex,mcp --bin opencompany
cp "$CARGO_TARGET_DIR/debug/opencompany" target/debug/opencompany-mcp
```

Both were pre-built when this branch was prepared, so if `target/debug/opencompany-default` and
`target/debug/opencompany-mcp` already exist, skip straight to §1.

---

## 1 · Booting a host

`frontend/test/e2e/host.sh` is the reproducible recipe the e2e suite uses; it rebuilds the console
bundle, wipes `target/e2e/data`, and serves `companies/e2e_harness` on `127.0.0.1:8080`. Use it for
manual testing too:

```bash
PW_HOST_BINARY=$PWD/target/debug/opencompany-default frontend/test/e2e/host.sh
```

Swap `PW_HOST_BINARY` for `opencompany-mcp` in Scenario B. Other knobs: `PW_HOST_COMPANY`,
`PW_HOST_BIND`, `PW_HOST_DATA_DIR`, `PW_SKIP_CONSOLE_BUILD=1`.

**Do not set `OPENCOMPANY_PUBLIC_URL` or any `OPENCOMPANY_MAIL_*`.** Either one stops the host
echoing the magic-link code, and sign-in below depends on that echo.

### Sign in

Browser: open <http://127.0.0.1:8080>, enter **`harness-e2e@tinyhumans.ai`** (the standing admin in
the harness manifest). The host echoes the login code and the console offers to continue with it.

API, for the `curl` checks below:

```bash
CODE=$(curl -s -X POST http://127.0.0.1:8080/api/v1/company/auth/request \
  -H 'content-type: application/json' \
  -d '{"email":"harness-e2e@tinyhumans.ai"}' | jq -r .dev_code)

curl -s -c /tmp/oc-jar.txt -X POST http://127.0.0.1:8080/api/v1/company/auth/verify \
  -H 'content-type: application/json' -d "{\"code\":\"$CODE\"}" | jq .
```

Then pass `-b /tmp/oc-jar.txt` on every request. A code is single-use and expires after 15 minutes.

---

## 2 · Scenario A — the default build (no `mcp` feature)

This is the deployment the epic is about: the MCP tab works, and nothing an agent does can involve
an MCP server.

### A1 · #567 — the host admits the bridge is missing

```bash
curl -s -b /tmp/oc-jar.txt http://127.0.0.1:8080/api/v1/company/capabilities | jq '.mcpInBuild'
```

**Expect `false`.**

### A2 · #567 — the console says so, before you touch anything

Settings → **MCP Servers**. Above the list:

> **No agent can use tool servers in this deployment** — The MCP bridge isn't compiled into this
> build, so servers added here are stored and can be probed, but no teammate ever receives their
> tools…

Then open **Connections** — the same section is rendered inline there, and the banner must appear in
both places (one component, two mount points).

### A3 · #567 — the success path agrees with the banner

Still on Settings → MCP Servers, add a server:

- Name `scratch`, endpoint `https://mcp.example.test/scratch`, **Add**.

**Expect the toast:** *"Added scratch. It is stored, but no agent can call it until this deployment
is rebuilt with the MCP bridge."*

This is the check worth doing slowly. Before the fix the same click said *"Agents pick it up on the
next rebuild"* — a promise fired at the exact moment the operator acts, a few pixels under a banner
saying the opposite. A banner alone does not make the screen honest.

### A4 · #568 — reachability is on the wire and on the screen

```bash
curl -s -b /tmp/oc-jar.txt http://127.0.0.1:8080/api/v1/company/mcp/servers \
  | jq '.[] | {name, enabled, reachableBy}'
```

The harness company grants every agent `mcp:*`, so **expect `deepwiki` to report
`["ceo","engineer","writer"]`**, and the row in the console to read *"Reachable by: ceo, engineer,
writer"*.

### A5 · #568 — a disabled server reaches nobody, and is not scolded for it

Toggle `deepwiki` **off** in the console. Then:

```bash
curl -s -b /tmp/oc-jar.txt http://127.0.0.1:8080/api/v1/company/mcp/servers \
  | jq '.[] | select(.name=="deepwiki") | {enabled, reachableBy}'
```

**Expect `enabled: false`, `reachableBy: []`** — the harness excludes disabled servers from every
agent's registry, so listing agents here would be the console disagreeing with the harness.

**And in the console: no red "no agent can reach this server" warning on that row.** An off server
reaching nobody is intent, not misconfiguration; the loud state is scoped to enabled servers. Toggle
it back on and the warning stays away, because now it is reachable again.

### A6 · #566 — no restart is demanded

Any mutating MCP call returns the host's note:

```bash
curl -s -b /tmp/oc-jar.txt -X PUT \
  http://127.0.0.1:8080/api/v1/company/mcp/servers/deepwiki \
  -H 'content-type: application/json' -d '{"enabled":true}' | jq -r '.note'
```

**Expect:** *"Agents pick up this change on their next turn — no restart needed."* No "rebuild", no
"restart the company", anywhere in the response or the console.

### A7 · #569 — the Skills tab does not imply execution

Check all five places the claim lives:

1. **Settings rail**, before you open anything: the Skills entry reads *"Playbooks your teammates
   read"* (it used to say *"What this company knows how to do"*).
2. **Skills subtitle**: *"Playbooks your teammates read. Enable, install from the registry, or add
   your own."*
3. **The standing note** above the tabs: skills are reference material teammates read — playbooks
   they follow, not buttons they press — and executing a saved workflow stays **the orchestrator's**
   job.
4. **Per installed card**: *"Teammates can read this"*, and *"Hidden from teammates"* after you
   toggle one off. Not "can use".
5. **Add skill → dialog description**: *"Describe a playbook your teammates should follow — what to
   do, and when."* (it used to say *"Describe a capability your company should have"*).

The note must be visible on the **Registry** tab too — that is the page an operator browses from,
and install/enable is the vocabulary that creates the wrong expectation.

---

## 3 · Scenario B — the build with the bridge

Restart the host against the other binary:

```bash
PW_HOST_BINARY=$PWD/target/debug/opencompany-mcp frontend/test/e2e/host.sh
```

Sign in again (fresh data dir), then:

| Check | Expect |
|---|---|
| `curl … /capabilities \| jq .mcpInBuild` | **`true`** |
| Settings → MCP Servers | **No** bridge banner, in either mount point |
| Add a server | Toast: *"Added <name>. Agents pick it up on their next turn."* |
| `GET …/mcp/servers` | `reachableBy` still populated; **Tools** and **Test** now do real work |
| Skills tab | Identical to Scenario A — #569 is not build-dependent |

If the banner shows here, `mcpInBuild` is lying or the console is ignoring it — that is a bug, not a
build difference.

---

## 4 · Scenario C — a company where reachability is interesting

The harness company grants `mcp:*` to everyone, so it can only ever show the *happy* case. To see
the flagged zero-state, serve a company with narrowed grants. Put it under `target/` so it stays out
of git:

```bash
mkdir -p target/manual && cp -R companies/e2e_harness target/manual/reach_demo
```

Edit `target/manual/reach_demo/company.toml`:

- `[tools] allow` → `["mcp:deepwiki", "workspace", "workspace.*"]` (drops the `mcp:*` wildcard)
- the `ceo` agent's `tools` → `["mcp:deepwiki", "workspace.read"]`
- the `engineer` agent's `tools` → `["workspace.read"]` (reaches no MCP server at all)
- add a second server nothing grants:

```toml
[[mcp_server]]
name = "linear"
endpoint = "https://linear.example/mcp"
```

Serve it:

```bash
PW_HOST_COMPANY=$PWD/target/manual/reach_demo \
PW_HOST_BINARY=$PWD/target/debug/opencompany-default \
frontend/test/e2e/host.sh
```

Expect, in the console and in `GET …/mcp/servers`:

- `deepwiki` → `reachableBy: ["ceo", "writer"]` — the writer still holds the company wildcard it
  inherits; the engineer narrowed itself out.
- `linear` → **`reachableBy: []`**, and the row carries the loud red warning: *"No agent can reach
  this server — no teammate's tool grants cover `mcp:linear`."*

That red state on `linear` while `deepwiki` reads normally, on the same screen, is finding #3 in one
screenshot.

---

## 5 · The automated suites

From the repo root:

```bash
# Rust — the console routes, both feature configurations
cargo test --locked --lib
cargo test --locked --features mcp --lib server::ops::capabilities
cargo test --locked --features openhuman,tinycortex --lib

# Console
cd frontend
npm run typecheck && npm run typecheck:unit && npm run typecheck:e2e
npm test                 # 300+ unit tests incl. mcp-bridge + skills-read-only
npm run build            # production bundle

# End-to-end, default host (Scenario A assertions, in a real browser)
npx playwright install chromium
PW_HOST_BINARY=$PWD/../target/debug/opencompany-default npm run e2e

# End-to-end, gated host (asserts the banner is ABSENT with the bridge in)
PW_HOST_BINARY=$PWD/../target/debug/opencompany-mcp npm run e2e:live
```

The two e2e lanes are the ones that prove #567 from both sides: `mcp.spec.ts` asserts the banner is
**present** on the default host and **absent** under `PW_LIVE_BRAIN`.

---

## 6 · Acceptance criteria → the step that proves it

| Issue | Criterion | Proven by |
|---|---|---|
| #566 | mutating responses promise next-turn pickup, no restart | A6 |
| #568 | each server lists the agents whose grants reach it | A4, C |
| #568 | a server no agent can reach is flagged | C (`linear`) |
| #568 | a disabled server reaches nobody, without a false alarm | A5 |
| #567 | on a build without `mcp`, the console states tool servers are unavailable | A1, A2, A3 |
| #567 | on a build with `mcp`, no change in behaviour | B |
| #567 | a test covers the without-feature path | §5 (Rust default lane + `mcp-bridge` unit + `mcp.spec.ts`) |
| #569 | the console no longer implies a desk agent will execute a skill | A7 |
| #569 | execution, if wired, goes through metering and the pin is updated deliberately | **does not fire** — nothing is wired; `dispatched_belt_excludes_every_deferred_family` is untouched (see PR #640's decision comment) |

---

## 7 · Known gaps, so they are not mistaken for bugs

- **`docs/modules/mcp.md`'s "Pool-staleness caveat"** still says mid-session MCP edits need
  "practically, a company restart" — the claim #566 removed from the UI. The doc is stale, the
  behaviour is not. Untracked follow-up.
- **PR #554** (install-wide default MCP servers, #527) is not in this branch. It also edits
  `McpServerDto`, `list_servers` and the frontend `McpServer` type, and changes `resolve_effective`
  from 3 to 4 args. Expect a rebase there, not here.
- Scenario A's host runs the **offline echo brain**. Agents answer, but nothing exercises a real MCP
  call — which is the point of Scenario A, not a limitation of it.
