#!/bin/sh
# Selects which example company this container runs, from $OPENCOMPANY_COMPANY.
# The value may be an example directory name (e.g. venture_capital) or a
# friendly alias (e.g. fund). This is the "which module spins up" switch.
set -eu

COMPANY="${OPENCOMPANY_COMPANY:-marketing_agency}"

# Friendly aliases → example directory names.
case "$COMPANY" in
  fund | vc | venture-capital)     COMPANY="venture_capital" ;;
  marketing | agency)              COMPANY="marketing_agency" ;;
  software | saas | dev)           COMPANY="software_company" ;;
  studio | venture-studio)         COMPANY="venture_studio" ;;
  accelerator)                     COMPANY="startup_accelerator" ;;
  law | legal)                     COMPANY="law_firm" ;;
  accounting | finance)            COMPANY="accounting_firm" ;;
  support)                         COMPANY="customer_support" ;;
  signals | opportunity)           COMPANY="signals_opportunity_studio" ;;
esac

DIR="companies/${COMPANY}"
if [ ! -f "${DIR}/company.toml" ] && [ ! -f "${DIR}/agents.toml" ]; then
  echo "opencompany: unknown company '${OPENCOMPANY_COMPANY}' (no manifest at ${DIR})" >&2
  echo "available companies:" >&2
  ls companies | sed 's/^/  - /' >&2
  exit 1
fi

DISCOVER=""
if [ "${OPENCOMPANY_DISCOVERABLE:-false}" = "true" ]; then
  DISCOVER="--discoverable"
fi

echo "opencompany: launching '${COMPANY}' on ${OPENCOMPANY_BIND:-0.0.0.0:8080}"
# shellcheck disable=SC2086
exec opencompany serve \
  --company "${DIR}" \
  --bind "${OPENCOMPANY_BIND:-0.0.0.0:8080}" \
  --home "${OPENCOMPANY_DATA_DIR:-/data}" \
  ${DISCOVER}
