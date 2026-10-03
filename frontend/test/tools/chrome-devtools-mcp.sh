#!/usr/bin/env bash
#
# Launch `chrome-devtools-mcp` against the browser Playwright already manages.
#
# The MCP server's own default is to look for an installed Google Chrome, which
# this repository does not require anybody to have — the console's e2e suite
# runs on the Chromium that `npx playwright install chromium` puts in
# `~/.cache/ms-playwright`. Left to its default the server either fails to find
# a browser or, on a box where the only Chromium is the snap build, launches one
# whose sandbox refuses the temporary profile directory `--isolated` hands it.
#
# So resolve the executable from `@playwright/test` at launch rather than naming
# a path. The revision directory is versioned (`chromium-1234/`) and changes
# whenever the Playwright pin in `package.json` moves; a literal path in
# `.mcp.json` would break silently at that bump, and the symptom — an MCP server
# that connects and then cannot open a page — reads as a broken tool rather than
# a stale path.
#
# Invoked from `.mcp.json` at the repository root. Arguments are forwarded, so
# `--headless=false` on the command line still wins over the default below.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
frontend="$(cd "$here/../.." && pwd)"

if [[ ! -d "$frontend/node_modules/@playwright/test" ]]; then
  echo "chrome-devtools-mcp: $frontend/node_modules is missing — run 'pnpm install' in frontend/ first." >&2
  exit 1
fi

executable="$(
  cd "$frontend" &&
    node -e 'import("@playwright/test").then((m) => console.log(m.chromium.executablePath()))'
)"

if [[ ! -x "$executable" ]]; then
  echo "chrome-devtools-mcp: Playwright reports '$executable', which is not executable — run 'npx playwright install chromium' in frontend/." >&2
  exit 1
fi

# Chromium's namespace sandbox needs unprivileged user namespaces, and Ubuntu
# 24.04+ denies those to any binary without a matching AppArmor profile. The
# distribution ships profiles for packaged browsers; the build Playwright
# downloads into `~/.cache/ms-playwright` has none, so the browser process dies
# during startup and every tool call after it reports `Protocol error
# (Target.setDiscoverTargets): Target closed` — a message that says nothing
# about sandboxes and sends you looking at the MCP server instead.
#
# Dropping the sandbox is the fix that does not need root. It is scoped to a
# browser that this repository launches, against pages this repository serves,
# in a throwaway profile — not to the browser anybody browses with. On a host
# where the sandbox does work, the flag is not added, so we do not weaken a box
# that had no problem.
#
# The alternative is `sudo` and a system-wide AppArmor profile for the
# Playwright binary, which would have to be reapplied at every browser revision
# bump. If you would rather do that, drop the block below.
sandbox_args=()
if [[ "$(cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns 2>/dev/null || echo 0)" == "1" ]]; then
  sandbox_args=(--chromeArg=--no-sandbox)
fi

# The version is pinned rather than `@latest`, the same way `.mcp.json` pins
# `@playwright/mcp`: `npx` resolves the tag on every launch, so a fresh release
# would be downloaded and executed on a checkout that never changed — a release
# that broke the CLI, or a compromised one, would become the repository's MCP
# integration without any deliberate move. `@1.7.0` is the version these flags
# were verified against; bump it on purpose, and re-verify.
#
# `${arr[@]+"${arr[@]}"}`, not `"${arr[@]}"`: macOS still ships bash 3.2, where
# `set -u` reports an EMPTY array as unbound. On every Mac (where the AppArmor
# block above never fires) the plain form killed this script before the server
# started, and the MCP client reported only "Connection closed".
exec npx -y chrome-devtools-mcp@1.7.0 \
  --executablePath "$executable" \
  --headless=true \
  --isolated=true \
  ${sandbox_args[@]+"${sandbox_args[@]}"} \
  "$@"
