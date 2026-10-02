#!/bin/sh
# Selects which example company this container runs, from $OPENCOMPANY_COMPANY.
# The value may be an example directory name (e.g. venture_capital) or a
# friendly alias (e.g. fund). This is the "which module spins up" switch.
#
# A BLANK or absent $OPENCOMPANY_COMPANY means blank — `serve` runs with **no**
# `--company`, so an empty `/data` boots an empty registry. That is what puts
# the console into the first-run setup wizard: `AppSpec.setup_complete` is false
# with no company registered (`app/types.rs`), so `ConnectionConsole` shows
# `SetupWizard.tsx`, and the wizard's `POST /api/v1/setup` seeds the company the
# user picks. The platform launches an unconfigured instance and lets the owner
# build it in the wizard, rather than pre-picking a template here (WS-A design:
# opencompany-sso-onboarding-design.md, Part 1). A named value still selects a
# baked company exactly as before, so a local `--company`-style boot is
# unchanged.
set -eu

COMPANY="${OPENCOMPANY_COMPANY:-}"

BIND="${OPENCOMPANY_BIND:-0.0.0.0:8080}"
HOME_DIR="${OPENCOMPANY_DATA_DIR:-/data}"

DISCOVER=""
if [ "${OPENCOMPANY_DISCOVERABLE:-false}" = "true" ]; then
  DISCOVER="--discoverable"
fi

if [ -z "${COMPANY}" ]; then
  # Unconfigured launch: no company, empty registry, first-run wizard.
  echo "opencompany: launching unconfigured (setup wizard) on ${BIND}"
  # shellcheck disable=SC2086
  exec opencompany serve \
    --bind "${BIND}" \
    --home "${HOME_DIR}" \
    ${DISCOVER}
fi

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

echo "opencompany: launching '${COMPANY}' on ${BIND}"
# shellcheck disable=SC2086
exec opencompany serve \
  --company "${DIR}" \
  --bind "${BIND}" \
  --home "${HOME_DIR}" \
  ${DISCOVER}
