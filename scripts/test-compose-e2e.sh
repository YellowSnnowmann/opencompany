#!/bin/sh
# Boot the development Compose stack and verify both published services and
# the console-to-host proxy. This is intentionally separate from the fast,
# Docker-free launcher test because it builds images and starts containers.
set -eu

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH='' cd -- "${SCRIPT_DIR}/.." && pwd)
RUN_ID=${E2E_RUN_ID:-$(date +%s)-$$}
RUN_ID=$(printf '%s' "$RUN_ID" | tr -cd 'a-zA-Z0-9_-')
PROJECT="opencompany-compose-e2e-${RUN_ID}"
PORT_SEED=$(printf '%s' "$RUN_ID" | cksum | awk '{print $1}')
OPENCOMPANY_PORT=${E2E_API_PORT:-$((20000 + (PORT_SEED % 10000)))}
CONSOLE_PORT=${E2E_CONSOLE_PORT:-$((30000 + (PORT_SEED % 10000)))}
OPENCOMPANY_COMPANY=${E2E_COMPANY:-marketing_agency}
OPENCOMPANY_ADMIN_EMAIL=${E2E_ADMIN_EMAIL:-e2e-admin@example.com}
export OPENCOMPANY_PORT CONSOLE_PORT OPENCOMPANY_COMPANY OPENCOMPANY_ADMIN_EMAIL

compose() {
    docker compose \
        --project-directory "${REPO_ROOT}/deploy" \
        --project-name "$PROJECT" \
        --file "${REPO_ROOT}/deploy/docker-compose.yml" \
        --file "${REPO_ROOT}/deploy/docker-compose.dev.yml" \
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
    until curl --connect-timeout 2 --max-time 10 \
        --fail --silent --output /dev/null "$url"; do
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
printf '%s\n%s\n' "${E2E_ADMIN_PASSWORD:-e2e-password}" \
    "${E2E_ADMIN_PASSWORD:-e2e-password}" \
    | OPENCOMPANY_PROJECT_NAME="$PROJECT" \
        "${SCRIPT_DIR}/init-demo-admin.sh" "$OPENCOMPANY_COMPANY" "$OPENCOMPANY_ADMIN_EMAIL"
compose up --build --detach

api_url="http://localhost:${OPENCOMPANY_PORT}"
console_url="http://localhost:${CONSOLE_PORT}"
wait_for_url "API" "${api_url}/healthz"
wait_for_url "console" "${console_url}/"

api_health=$(curl --connect-timeout 2 --max-time 10 \
    --fail --silent --show-error "${api_url}/healthz")
proxied_health=$(curl --connect-timeout 2 --max-time 10 \
    --fail --silent --show-error "${console_url}/healthz")
if [ "$api_health" != "$proxied_health" ]; then
    echo "compose e2e: console /healthz did not return the API response" >&2
    exit 1
fi

if ! curl --connect-timeout 2 --max-time 10 \
    --fail --silent --show-error "${console_url}/" \
    | grep -F '<title>OpenCompany Console</title>' >/dev/null; then
    echo "compose e2e: console did not serve the Vite application" >&2
    exit 1
fi

echo "compose e2e passed: API ${api_url}, console ${console_url}, proxy connected"
