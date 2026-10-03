#!/usr/bin/env bash
#
# Prove a BUILT binary's analytics leave it the way the spec says, with no
# network beyond loopback.
#
# usage: assert-desktop-analytics.sh <binary | path/to/OpenCompany.app>
#
# Runs `<binary> analytics-test` against `scripts/ci/analytics-capture-server.py`
# on a free loopback port (OPENCOMPANY_ANALYTICS_ENDPOINT=http://127.0.0.1:<port>/track)
# and asserts: exit 0 (the collector accepted), an `s_<32 hex>` profile id on
# stdout, exactly the `openpanel-client-id` and `openpanel-sdk-name: opencompany`
# headers, and none of Origin / Authorization / Cookie.
#
# The desktop binary needs nothing else: it reports by default, and `HOME` is
# pointed at a scratch directory so no saved opt-out from a developer's machine
# can turn the run silent. For a server binary set
# ASSERT_ANALYTICS_DEPLOYMENT=hosted-tenant (it reports only as a hosted tenant).
#
# docs/spec/runtime/analytics-smoke.md.
set -euo pipefail

target="${1:-}"
if [ -z "$target" ]; then
  echo "usage: $0 <binary | OpenCompany.app>" >&2
  exit 2
fi
if [ -d "$target" ]; then
  target="$target/Contents/MacOS/opencompany-desktop"
fi
if [ ! -x "$target" ]; then
  echo "::error::no executable at ${target}" >&2
  exit 1
fi

here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
server_pid=""
cleanup() {
  if [ -n "$server_pid" ]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

python3 "$here/analytics-capture-server.py" --port 0 --log "$work/log.jsonl" --port-file "$work/port" &
server_pid=$!
for _ in $(seq 1 50); do
  [ -s "$work/port" ] && break
  sleep 0.1
done
[ -s "$work/port" ] || { echo "::error::capture server did not start" >&2; exit 1; }
port="$(cat "$work/port")"

extra=()
[ -z "${ASSERT_ANALYTICS_DEPLOYMENT:-}" ] || extra+=("OPENCOMPANY_DEPLOYMENT=${ASSERT_ANALYTICS_DEPLOYMENT}")

rc=0
env -i PATH="$PATH" HOME="$work/home" \
  OPENCOMPANY_ANALYTICS_ENDPOINT="http://127.0.0.1:${port}/track" \
  ${extra[@]+"${extra[@]}"} \
  "$target" analytics-test >"$work/stdout" 2>"$work/stderr" || rc=$?

fail() { echo "::error::$1" >&2; echo "--- stdout" >&2; cat "$work/stdout" >&2; echo "--- stderr" >&2; cat "$work/stderr" >&2; exit 1; }

[ "$rc" -eq 0 ] || fail "analytics-test exited ${rc} (0 = accepted, 2 = silent, 1 = refused/unreachable)"
profile="$(tr -d '[:space:]' < "$work/stdout")"
[[ "$profile" =~ ^s_[0-9a-f]{32}$ ]] || fail "stdout is not an s_<128-bit hex> profile id: '${profile}'"
[ -s "$work/log.jsonl" ] || fail "the capture server received nothing"

python3 - "$work/log.jsonl" "$profile" <<'PY' || fail "captured request violates the wire contract"
import json, sys
entries = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
profile = sys.argv[2]
problems = []
posts = [e for e in entries if e["method"] == "POST" and e["path"].split("?")[0] == "/track"]
if not posts:
    problems.append("no POST /track arrived: " + ", ".join(f'{e["method"]} {e["path"]}' for e in entries))
for e in posts:
    h = e["headers"]
    if not h.get("openpanel-client-id"):
        problems.append("missing openpanel-client-id")
    if h.get("openpanel-sdk-name") != "opencompany":
        problems.append(f'openpanel-sdk-name is {h.get("openpanel-sdk-name")!r}, want "opencompany"')
    for forbidden in ("origin", "authorization", "cookie"):
        if forbidden in h:
            problems.append(f"forbidden header present: {forbidden}")
if not any(profile in e["body"] for e in posts):
    problems.append("the posted body does not carry the printed profile id")
if not any("analytics_self_test" in e["body"] for e in posts):
    problems.append("the posted body does not carry analytics_self_test")
if problems:
    print("\n".join(problems), file=sys.stderr)
    sys.exit(1)
PY

echo "desktop analytics wire contract verified: ${profile} captured on 127.0.0.1:${port}"
[ -z "${GITHUB_STEP_SUMMARY:-}" ] || echo "desktop analytics wire contract verified (loopback capture): \`${profile}\`" >> "$GITHUB_STEP_SUMMARY"
