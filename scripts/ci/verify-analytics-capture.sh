#!/usr/bin/env bash
#
# Read a product-analytics event BACK from OpenPanel, so "the collector answered
# 2xx" is never mistaken for "the event was stored".
#
# usage: verify-analytics-capture.sh <profileId>     read the event back
#        verify-analytics-capture.sh --selftest      run the parsing/polling logic
#                                                    against canned fixtures, no network
#
# `opencompany analytics-test` prints the throwaway `s_<128 bits>` profile it
# reported under and exits 0 only when the collector accepted the event. That is
# the same blind spot the crash-reporting gate closes for Sentry: an ingest can
# answer 2xx while discarding everything. This script asks OpenPanel's MCP
# endpoint for the profile's events and passes only when the profile is there.
#
# Environment:
#   OPENPANEL_MCP_BEARER        base64(readClientId:readSecret) of a READ-mode
#                               client. Environment only: it is masked in
#                               Actions, never echoed, and handed to curl on
#                               stdin (`-K -`) so it is in neither argv nor a file.
#   VERIFY_ANALYTICS_REQUIRED   "1" makes a missing bearer a failure. Anything
#                               else makes it a loud warning and exit 0, so a
#                               gate can ship before the secret exists.
#   VERIFY_ANALYTICS_ATTEMPTS   polls before giving up (default 18)
#   VERIFY_ANALYTICS_INTERVAL   seconds between polls (default 10)
#   OPENPANEL_MCP_URL           endpoint (default https://panel.tinyhumans.ai/api/mcp)
#
# Exit: 0 stored (or skipped, see above), 1 not stored / refused / bad input.
set -euo pipefail

MCP_URL="${OPENPANEL_MCP_URL:-https://panel.tinyhumans.ai/api/mcp}"
FIXTURES="$(cd "$(dirname "$0")" && pwd)/fixtures/analytics-capture"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Pass a JSON-RPC body to the endpoint. Prints "<http-code>" on the first line
# and leaves the body in $WORK/body and response headers in $WORK/headers.
# With VERIFY_ANALYTICS_FAKE_DIR set, replays the next canned response instead of
# using the network (the --selftest transport).
http_post() {
  local body="$1" session="${2:-}"
  if [ -n "${VERIFY_ANALYTICS_FAKE_DIR:-}" ]; then
    local n=0
    [ -f "$VERIFY_ANALYTICS_FAKE_DIR/.n" ] && n="$(cat "$VERIFY_ANALYTICS_FAKE_DIR/.n")"
    n=$((n + 1))
    echo "$n" > "$VERIFY_ANALYTICS_FAKE_DIR/.n"
    # Past the end of the script, keep replaying the last response.
    while [ "$n" -gt 1 ] && [ ! -f "$VERIFY_ANALYTICS_FAKE_DIR/$n.code" ]; do n=$((n - 1)); done
    cat "$VERIFY_ANALYTICS_FAKE_DIR/$n.code"
    cp "$VERIFY_ANALYTICS_FAKE_DIR/$n.body" "$WORK/body"
    if [ -f "$VERIFY_ANALYTICS_FAKE_DIR/$n.headers" ]; then
      cp "$VERIFY_ANALYTICS_FAKE_DIR/$n.headers" "$WORK/headers"
    else
      : > "$WORK/headers"
    fi
    return 0
  fi
  # curl config on stdin: the token and body never appear in argv or on disk.
  local esc_body="${body//\\/\\\\}"
  esc_body="${esc_body//\"/\\\"}"
  local cfg
  cfg="header = \"Authorization: Bearer ${OPENPANEL_MCP_BEARER}\"
header = \"Content-Type: application/json\"
header = \"Accept: application/json, text/event-stream\"
data = \"${esc_body}\""
  if [ -n "$session" ]; then
    cfg="${cfg}
header = \"mcp-session-id: ${session}\""
  fi
  curl -sS --max-time 30 -o "$WORK/body" -D "$WORK/headers" -w '%{http_code}\n' \
    -K - "$MCP_URL" <<<"$cfg" || echo 000
}

# Reads a response body on stdin (plain JSON or SSE `data:` frames) and prints
# the number of events whose profile id is "$1". Prints "error" if any frame is
# a JSON-RPC error or a tool result flagged isError, "none" when no frame parsed.
count_matches() {
  python3 -c '
import json, sys
want = sys.argv[1]
raw = sys.stdin.read()
frames = []
if any(l.startswith("data:") for l in raw.splitlines()):
    for l in raw.splitlines():
        if l.startswith("data:"):
            frames.append(l[5:].strip())
else:
    frames.append(raw.strip())
docs = []
for f in frames:
    try:
        docs.append(json.loads(f))
    except ValueError:
        pass
if not docs:
    print("none"); sys.exit(0)
hits = 0
err = False
def walk(node):
    global hits, err
    if isinstance(node, dict):
        if node.get("isError") is True or "error" in node and "jsonrpc" in node:
            err = True
        for k in ("profile_id", "profileId"):
            if node.get(k) == want:
                hits += 1
                break
        for v in node.values():
            walk(v)
    elif isinstance(node, list):
        for v in node:
            walk(v)
    elif isinstance(node, str) and node[:1] in "[{":
        try:
            walk(json.loads(node))
        except ValueError:
            pass
for d in docs:
    walk(d)
print(hits if hits else ("error" if err else 0))
' "$1"
}

# The session id from the last response headers, empty when the server is
# stateless.
session_id() {
  awk 'BEGIN{IGNORECASE=1} /^mcp-session-id:/ {gsub(/\r/,""); print $2; exit}' "$WORK/headers" 2>/dev/null || true
}

utc_day() { python3 -c 'import datetime as d,sys; print((d.datetime.now(d.timezone.utc).date()+d.timedelta(days=int(sys.argv[1]))).isoformat())' "$1"; }

# One full handshake + both queries. Prints "<verdict>" where verdict is one of
# stored | missing | denied | httperror:<code>.
check_once() {
  local profile="$1" code session
  code="$(http_post '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"opencompany-verify-analytics","version":"1"}}}')"
  case "$code" in 401 | 403) echo denied; return 0 ;; esac
  [[ "$code" =~ ^2 ]] || { echo "httperror:${code}"; return 0; }
  session="$(session_id)"
  http_post '{"jsonrpc":"2.0","method":"notifications/initialized"}' "$session" >/dev/null
  local start end args matches
  start="$(utc_day -1)"
  end="$(utc_day 1)"
  # The eventNames filter has returned [] for names that exist, so ask with it
  # first and then by profile alone; the match is on the profile id either way.
  for args in \
    "{\"profileId\":\"${profile}\",\"eventNames\":[\"analytics_self_test\"],\"startDate\":\"${start}\",\"endDate\":\"${end}\",\"limit\":5}" \
    "{\"profileId\":\"${profile}\",\"startDate\":\"${start}\",\"endDate\":\"${end}\",\"limit\":20}"; do
    code="$(http_post "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"query_events\",\"arguments\":${args}}}" "$session")"
    case "$code" in 401 | 403) echo denied; return 0 ;; esac
    [[ "$code" =~ ^2 ]] || { echo "httperror:${code}"; return 0; }
    matches="$(count_matches "$profile" < "$WORK/body")"
    if [[ "$matches" =~ ^[0-9]+$ ]] && [ "$matches" -gt 0 ]; then
      echo stored
      return 0
    fi
  done
  echo missing
}

summary() { [ -n "${GITHUB_STEP_SUMMARY:-}" ] && echo "$1" >> "$GITHUB_STEP_SUMMARY"; return 0; }

verify() {
  local profile="$1"
  if ! [[ "$profile" =~ ^s_[0-9a-f]{32}$ || "$profile" =~ ^[A-Za-z0-9_.-]{4,128}$ ]]; then
    echo "::error::not a plausible profile id: '${profile}'" >&2
    return 1
  fi
  local attempt verdict=missing
  local ATTEMPTS="${VERIFY_ANALYTICS_ATTEMPTS:-18}" INTERVAL="${VERIFY_ANALYTICS_INTERVAL:-10}"
  for attempt in $(seq 1 "$ATTEMPTS"); do
    verdict="$(check_once "$profile")"
    case "$verdict" in
      stored)
        echo "analytics capture verified: events for ${profile} are stored (attempt ${attempt})"
        summary "analytics capture verified: events for \`${profile}\` are stored (attempt ${attempt})"
        return 0
        ;;
      denied)
        echo "::error::OpenPanel read token lacks access (HTTP 401/403 from ${MCP_URL}). OPENPANEL_MCP_BEARER must be base64(clientId:secret) of a READ-mode client. The event (${profile}) was sent and may well have arrived." >&2
        summary "analytics read-back FAILED: read token lacks access"
        return 1
        ;;
    esac
    [ "$attempt" -lt "$ATTEMPTS" ] && sleep "$INTERVAL"
  done
  echo "::error::collector accepted (2xx) but the event was never stored: no event for ${profile} after ${ATTEMPTS} polls (last verdict: ${verdict}). Reporting is NOT working: the collector answers while events are discarded. Check the OpenPanel project, not this repository." >&2
  summary "analytics read-back FAILED: ${profile} never stored (${verdict})"
  return 1
}

# ---- --selftest: parsing and polling against canned fixtures, no network -------
selftest() {
  local fail=0 id="s_00112233445566778899aabbccddeeff"
  expect() { # name want got
    if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: want '$2' got '$3'"; fail=1; fi
  }
  expect "json match" 1 "$(count_matches "$id" < "$FIXTURES/events-hit.json")"
  expect "sse match" 1 "$(count_matches "$id" < "$FIXTURES/events-hit.sse")"
  expect "empty list" 0 "$(count_matches "$id" < "$FIXTURES/events-empty.json")"
  expect "other profile" 0 "$(count_matches "s_ffffffffffffffffffffffffffffffff" < "$FIXTURES/events-hit.json")"
  expect "tool error" error "$(count_matches "$id" < "$FIXTURES/tool-error.json")"
  expect "garbage" none "$(printf 'not json' | count_matches "$id")"

  # Scripted transports: <n>.code/<n>.body, replayed in order.
  run_script() { # name want-exit want-text; remaining args: "code:body-fixture" ...
    local name="$1" want="$2" text="$3"; shift 3
    local dir n=0 out rc=0 item
    dir="$(mktemp -d)"
    for item in "$@"; do
      n=$((n + 1))
      printf '%s' "${item%%:*}" > "$dir/$n.code"
      cp "$FIXTURES/${item#*:}" "$dir/$n.body"
    done
    out="$(VERIFY_ANALYTICS_FAKE_DIR="$dir" VERIFY_ANALYTICS_ATTEMPTS=3 VERIFY_ANALYTICS_INTERVAL=0 \
      OPENPANEL_MCP_BEARER=fake-not-a-real-token verify "$id" 2>&1)" || rc=$?
    rm -rf "$dir"
    expect "$name exit" "$want" "$rc"
    case "$out" in *"$text"*) echo "ok   $name text" ;; *) echo "FAIL $name text: wanted '$text' in: $out"; fail=1 ;; esac
  }
  # Per attempt: initialize, initialized, query(eventNames), query(profile only).
  run_script "stored via eventNames" 0 "verified" 200:init.json 202:empty.txt 200:events-hit.json
  run_script "stored via profile-only fallback (sse)" 0 "verified" \
    200:init.json 202:empty.txt 200:events-empty.json 200:events-hit.sse
  run_script "never stored" 1 "never stored" 200:init.json 202:empty.txt 200:events-empty.json 200:events-empty.json
  run_script "401 fails immediately" 1 "lacks access" 401:empty.txt
  run_script "403 on query" 1 "lacks access" 200:init.json 202:empty.txt 403:empty.txt
  run_script "5xx never stored" 1 "never stored" 502:empty.txt

  local rc=0 out
  out="$(env -u OPENPANEL_MCP_BEARER -u VERIFY_ANALYTICS_REQUIRED "$0" "$id" 2>&1)" || rc=$?
  expect "missing bearer warns, exit 0" 0 "$rc"
  case "$out" in *"::warning::analytics read-back skipped: OPENPANEL_MCP_BEARER not set"*) echo "ok   warning text" ;; *) echo "FAIL warning text: $out"; fail=1 ;; esac
  rc=0
  VERIFY_ANALYTICS_REQUIRED=1 env -u OPENPANEL_MCP_BEARER "$0" "$id" >/dev/null 2>&1 || rc=$?
  expect "missing bearer required, exit 1" 1 "$rc"
  return "$fail"
}

main() {
  if [ "${1:-}" = "--selftest" ]; then selftest; return; fi
  if [ $# -ne 1 ] || [ -z "$1" ]; then
    echo "usage: $0 <profileId> | --selftest" >&2
    return 1
  fi
  if [ -z "${OPENPANEL_MCP_BEARER:-}" ]; then
    if [ "${VERIFY_ANALYTICS_REQUIRED:-}" = "1" ]; then
      echo "::error::OPENPANEL_MCP_BEARER is not set, so this build cannot be shown to be captured. See docs/spec/runtime/analytics-smoke.md." >&2
      summary "analytics read-back FAILED: OPENPANEL_MCP_BEARER not set"
      return 1
    fi
    echo "::warning::analytics read-back skipped: OPENPANEL_MCP_BEARER not set (see docs/spec/runtime/analytics-smoke.md)" >&2
    summary "analytics read-back skipped: OPENPANEL_MCP_BEARER not set"
    return 0
  fi
  [ -z "${GITHUB_ACTIONS:-}" ] || echo "::add-mask::${OPENPANEL_MCP_BEARER}"
  verify "$1"
}

main "$@"
