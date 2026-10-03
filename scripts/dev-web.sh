#!/usr/bin/env bash
# Run the operator console in a real browser, signed in, for a CDP client.
#
# Modelled on OpenHuman's `scripts/run-dev-web.sh`. The desktop shell
# (`crates/opencompany-app`) renders through Tauri's default Wry webview —
# WKWebView on macOS, WebView2 on Windows, WebKitGTK on Linux — and none of
# those speak the Chrome DevTools Protocol, so no DevTools client (including
# the `chrome-devtools` MCP in `.mcp.json`) can attach to the desktop window.
# The console is the same SPA either way, so this runs it where CDP works:
#
#   opencompany serve (127.0.0.1:<host>) <- Vite proxy -- Vite (:<vite>) in Chrome
#
# and prints one URL that lands the browser in the console already signed in.
#
# ## How the sign-in works, and why it needs no dev-only route
#
# OpenHuman needs a `/dev/connect` route on its core to hand a bearer to the
# browser. OpenCompany does not: a host bound to loopback with no mail
# transport already echoes a magic-link code from `POST …/auth/request` as
# `dev_code` (`server/users/routes.rs`, gated on `is_local_only`). The host is
# started with `OPENCOMPANY_ADMIN_EMAIL` naming a dev address, which makes that
# address a standing admin (docs/spec/runtime/users.md), so this script asks
# for a code for it and prints `<vite>/?code=<code>` — the console's ordinary
# magic-link landing (`readMagicLinkFrom` in `frontend/src/App.tsx`), which
# redeems it and sets the session cookie through the Vite proxy.
#
# A code is single-use and expires 15 minutes after it is minted. Once
# redeemed, the session lives in the browser profile; a fresh profile (the MCP
# runs `--isolated`) needs a fresh link, which `--link` prints against the
# running stack without restarting anything.
#
# Usage:
#   scripts/dev-web.sh                       # build + host + vite, open the browser
#   scripts/dev-web.sh --no-browser          # same, just print the URL (for agents)
#   scripts/dev-web.sh --company marketing_agency
#   scripts/dev-web.sh --fresh               # wipe this company's dev data root first
#   scripts/dev-web.sh --host-url http://127.0.0.1:8080
#                                            # vite + sign-in against a host you run
#   scripts/dev-web.sh --link                # print a fresh sign-in URL for the
#                                            # stack the last run started
#
# Env:
#   OC_DEV_PORT          preferred Vite port (default 5180; 5173 belongs to
#                        scripts/desktop-dev.sh)
#   OC_DEV_HOST_PORT     preferred host port (default 8090)
#   OC_DEV_EMAIL         the address signed in (default dev@opencompany.localhost).
#                        With --host-url it must already be eligible on that host.
#   OC_DEV_FEATURES      cargo features for the host build (default: none)
#   OC_DEV_SKIP_BUILD=1  run the existing binary instead of rebuilding
#   OPENCOMPANY_DATA_DIR data root (default target/dev-web/<company>)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
FRONTEND="$REPO_ROOT/frontend"
SCRATCH="$REPO_ROOT/target/dev-web"
# What the last run started, so `--link` knows where to ask. Written once the
# stack is up and removed on exit, so a stale file never points at a dead host.
STATE_FILE="$SCRATCH/current.env"

log() { echo "[dev:web] $*" >&2; }
die() { log "ERROR: $*"; exit 1; }

open_browser=1
fresh=0
link_only=0
company_arg="e2e_harness"
host_url=""
while (( $# )); do
  case "$1" in
    --no-browser) open_browser=0 ;;
    --fresh) fresh=1 ;;
    --link) link_only=1 ;;
    --company) shift; company_arg="${1:?--company needs a value}" ;;
    --host-url) shift; host_url="${1:?--host-url needs a value}" ;;
    -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" ;;
  esac
  shift
done

email="${OC_DEV_EMAIL:-dev@opencompany.localhost}"

port_is_free() {
  ! nc -z 127.0.0.1 "$1" >/dev/null 2>&1
}

# First free port at or after $1 (within 20), else fail. The result is passed
# on explicitly, so a moved port is never a URL nobody is pointed at.
next_free_port() {
  local start="$1" label="$2" port="$1"
  while ! port_is_free "$port"; do
    port=$(( port + 1 ))
    (( port <= start + 20 )) || die "no free $label port near $start"
  done
  [[ "$port" == "$start" ]] || log "port $start busy; $label will use $port"
  echo "$port"
}

# A fresh single-use sign-in URL for $email against the host at $1, landing on
# the console at $2. Fails loudly with the reason a code was not echoed rather
# than printing a link that silently lands on the sign-in form.
sign_in_url() {
  local api="$1" app="$2" body code
  body="$(curl -sS -m 10 -X POST "$api/api/v1/company/auth/request" \
    -H 'Content-Type: application/json' \
    -d "{\"email\":\"$email\"}")" || die "could not reach $api to request a sign-in code"
  code="$(printf '%s' "$body" | node -e '
    let s = ""; process.stdin.on("data", (d) => (s += d)).on("end", () => {
      try { process.stdout.write(JSON.parse(s).dev_code ?? ""); } catch { }
    });')"
  if [[ -z "$code" ]]; then
    log "the host did not echo a login code for $email (response: $body)."
    log "It only does so on a loopback bind with no OPENCOMPANY_PUBLIC_URL and no"
    log "OPENCOMPANY_MAIL_* transport, for an address it accepts (a manifest"
    log "[users].admins entry or OPENCOMPANY_ADMIN_EMAIL). With --host-url, set"
    log "OC_DEV_EMAIL to such an address."
    exit 1
  fi
  echo "$app/?code=$code"
}

if (( link_only )); then
  [[ -f "$STATE_FILE" ]] || die "no running dev:web stack ($STATE_FILE is missing)"
  # shellcheck source=/dev/null
  source "$STATE_FILE"
  curl -sf -m 2 -o /dev/null "$DEV_WEB_HOST/healthz" || die "host $DEV_WEB_HOST is not answering"
  email="$DEV_WEB_EMAIL"
  sign_in_url "$DEV_WEB_HOST" "$DEV_WEB_APP"
  exit 0
fi

[[ -x "$FRONTEND/node_modules/.bin/vite" ]] ||
  die "$FRONTEND/node_modules is missing — run 'pnpm install' (or 'npm install') in frontend/"

host_pid=""
vite_pid=""
cleanup() {
  trap - EXIT INT TERM
  rm -f "$STATE_FILE"
  # Vite runs in its own process group (see below); signal the whole group so
  # node goes too, not just the subshell.
  if [[ -n "$vite_pid" ]]; then
    kill -- "-$vite_pid" 2>/dev/null || kill "$vite_pid" 2>/dev/null || true
  fi
  [[ -n "$host_pid" ]] && kill "$host_pid" 2>/dev/null || true
  wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

if [[ -n "$host_url" ]]; then
  host_url="${host_url%/}"
  curl -sf -m 3 -o /dev/null "$host_url/healthz" || die "no OpenCompany host answering at $host_url/healthz"
  log "using the host at $host_url"
else
  case "$company_arg" in
    */*) company_dir="$(cd "$company_arg" && pwd)" ;;
    *) company_dir="$REPO_ROOT/companies/$company_arg" ;;
  esac
  [[ -f "$company_dir/company.toml" || -f "$company_dir/agents.toml" ]] ||
    die "no company manifest in $company_dir"
  company_slug="$(basename "$company_dir")"

  data_dir="${OPENCOMPANY_DATA_DIR:-$SCRATCH/$company_slug}"
  mkdir -p "$data_dir" "$SCRATCH"
  data_dir="$(cd "$data_dir" && pwd -P)"
  if (( fresh )); then
    # Delete only inside this script's own scratch area, canonicalised, so an
    # inherited OPENCOMPANY_DATA_DIR can never take somebody's real data with it.
    scratch_real="$(cd "$SCRATCH" && pwd -P)"
    [[ "$data_dir" == "$scratch_real"/?* ]] ||
      die "--fresh only wipes data roots under target/dev-web; $data_dir is not one"
    rm -rf -- "$data_dir"
    mkdir -p "$data_dir"
    log "wiped $data_dir"
  fi

  if [[ "${OC_DEV_SKIP_BUILD:-}" != "1" ]]; then
    # Always run the incremental build: a stale host against a current console
    # is misleading to debug against, and a warm build is seconds.
    log "building opencompany…"
    (cd "$REPO_ROOT" && cargo build -p opencompany-core --bin opencompany \
      ${OC_DEV_FEATURES:+--features "$OC_DEV_FEATURES"})
  fi
  # Ask cargo where the binary went rather than assuming `target/`: worktrees
  # may share a target dir through `.cargo/config.toml`.
  target_dir="$(cd "$REPO_ROOT" && cargo metadata --format-version 1 --no-deps |
    node -e 'let s="";process.stdin.on("data",(d)=>(s+=d)).on("end",()=>process.stdout.write(JSON.parse(s).target_directory))')"
  binary="$target_dir/debug/opencompany"
  [[ -x "$binary" ]] || die "no binary at $binary"

  host_port="$(next_free_port "${OC_DEV_HOST_PORT:-8090}" host)"
  host_url="http://127.0.0.1:$host_port"

  # Inherit the caller's environment (inference keys arrive that way) minus the
  # variables that would stop the code echo the sign-in depends on, or move the
  # host onto a different backend: see `frontend/test/e2e/host.sh` for the
  # longer version of this argument.
  unset_args=(-u OPENCOMPANY_PUBLIC_URL -u OPENCOMPANY_BIND -u OPENCOMPANY_STORAGE
    -u OPENCOMPANY_TENANT_ID -u OPENCOMPANY_MONGODB_URI -u OPENCOMPANY_MONGODB_DB)
  for name in $(env | sed -n 's/^\(OPENCOMPANY_MAIL_[A-Za-z0-9_]*\)=.*/\1/p'); do
    unset_args+=(-u "$name")
  done

  log "serving $company_dir on :$host_port (data: $data_dir)"
  (cd "$REPO_ROOT" && exec env "${unset_args[@]}" \
    OPENCOMPANY_DATA_DIR="$data_dir" \
    OPENCOMPANY_ADMIN_EMAIL="$email" \
    OPENCOMPANY_SKIP_ACTIVATION_GATE=1 \
    "$binary" serve --bind "127.0.0.1:$host_port" --company "$company_dir") &
  host_pid=$!

  for _ in $(seq 1 120); do
    curl -sf -m 2 -o /dev/null "$host_url/healthz" && break
    kill -0 "$host_pid" 2>/dev/null || die "host exited during startup"
    sleep 1
  done
  curl -sf -m 2 -o /dev/null "$host_url/healthz" || die "host did not become healthy on :$host_port"
fi

vite_port="$(next_free_port "${OC_DEV_PORT:-5180}" vite)"
app_url="http://localhost:$vite_port"

log "starting vite on :$vite_port"
# Job control gives the background job its own process group (pgid == pid),
# which is what lets cleanup reach the node process.
set -m
(cd "$FRONTEND" && OC_API_TARGET="$host_url" exec node_modules/.bin/vite \
  --port "$vite_port" --strictPort --clearScreen false) &
vite_pid=$!
set +m

for _ in $(seq 1 60); do
  curl -sf -m 2 -o /dev/null "$app_url/" && break
  kill -0 "$vite_pid" 2>/dev/null || die "vite exited during startup"
  sleep 1
done

# The console proxies `/healthz` to the host; checking through Vite proves the
# proxy target is right, not merely that both processes are up.
curl -sf -m 5 -o /dev/null "$app_url/healthz" || die "vite is up but its proxy cannot reach $host_url"

connect_url="$(sign_in_url "$host_url" "$app_url")"

cat > "$STATE_FILE" <<EOF
DEV_WEB_HOST='$host_url'
DEV_WEB_APP='$app_url'
DEV_WEB_EMAIL='$email'
EOF

echo
log "ready"
log "  host : $host_url"
log "  app  : $app_url"
log "  open : $connect_url"
log "  signed in as $email (admin); the link is single-use, 15 minutes."
log "  another link for a fresh browser profile: scripts/dev-web.sh --link"
echo

if (( open_browser )); then
  if command -v open >/dev/null 2>&1; then
    open "$connect_url"
  elif command -v xdg-open >/dev/null 2>&1; then
    xdg-open "$connect_url"
  else
    log "no opener found; visit the URL above."
  fi
else
  log "--no-browser: point your CDP client at the URL above."
fi

wait "$vite_pid"
