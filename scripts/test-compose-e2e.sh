#!/bin/sh
# Boot the development Compose stack and verify both published services and
# the console-to-host proxy. This is intentionally separate from the fast,
# Docker-free launcher test because it builds images and starts containers.
set -eu

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH='' cd -- "${SCRIPT_DIR}/.." && pwd)
PROJECT="opencompany-compose-e2e"
OPENCOMPANY_PORT=${E2E_API_PORT:-$((20000 + ($$ % 10000)))}
CONSOLE_PORT=${E2E_CONSOLE_PORT:-$((30000 + ($$ % 10000)))}
OPENCOMPANY_COMPANY=${E2E_COMPANY:-marketing_agency}
export OPENCOMPANY_PORT CONSOLE_PORT OPENCOMPANY_COMPANY

compose() {
    docker compose \
        --project-directory "$REPO_ROOT" \
        --project-name "$PROJECT" \
        --file "${REPO_ROOT}/docker-compose.yml" \
        --file "${REPO_ROOT}/docker-compose.dev.yml" \
        "$@"
}

cleanup() {
    status=$?
    trap - 0 HUP INT TERM
    if [ "$status" -ne 0 ]; then
        compose ps >&2 || true
        compose logs --no-color >&2 || true
    fi
    # Keep the named Cargo/npm caches: the normal launcher does the same, and
    # a second smoke run should not rebuild the world. `down -v` remains the
    # explicit way to remove them.
    compose down --remove-orphans >/dev/null 2>&1 || true
    exit "$status"
}
trap cleanup 0 HUP INT TERM

wait_for_url() {
    label=$1
    url=$2
    attempts=0
    until curl --fail --silent --output /dev/null "$url"; do
        attempts=$((attempts + 1))
        if [ "$attempts" -ge 120 ]; then
            echo "compose e2e: ${label} did not become ready at ${url}" >&2
            return 1
        fi
        sleep 1
    done
}

command -v docker >/dev/null 2>&1 || {
    echo "compose e2e: docker is required" >&2
    exit 127
}
command -v curl >/dev/null 2>&1 || {
    echo "compose e2e: curl is required" >&2
    exit 127
}

echo "compose e2e: starting API on :${OPENCOMPANY_PORT} and console on :${CONSOLE_PORT}"
compose up --build --detach

api_url="http://localhost:${OPENCOMPANY_PORT}"
console_url="http://localhost:${CONSOLE_PORT}"
wait_for_url "API" "${api_url}/healthz"
wait_for_url "console" "${console_url}/"

api_health=$(curl --fail --silent --show-error "${api_url}/healthz")
proxied_health=$(curl --fail --silent --show-error "${console_url}/healthz")
if [ "$api_health" != "$proxied_health" ]; then
    echo "compose e2e: console /healthz did not return the API response" >&2
    exit 1
fi

if ! curl --fail --silent --show-error "${console_url}/" \
    | grep -F '<title>OpenCompany Console</title>' >/dev/null; then
    echo "compose e2e: console did not serve the Vite application" >&2
    exit 1
fi

echo "compose e2e passed: API ${api_url}, console ${console_url}, proxy connected"
