#!/usr/bin/env bash
#
# Fixture tests for the tenant landed-state gate in
# `.github/workflows/deploy-staging.yml`.
#
# That gate is the last thing standing between a bad tenant image and staging,
# and it is the one part of the deploy nothing else can exercise: it runs over
# ssh on the staging node, against a live fleet, only on a push to main. A
# false positive there blocks every deploy; a false negative ships a fleet that
# never came up. Both have happened.
#
# The gate's shell lives inside a quoted heredoc, so it is literal data to
# YAML, to `actionlint`, and to `shellcheck` — nothing lints it and nothing
# runs it. This script extracts the three functions by name, de-indents them,
# sources them with a stub `kubectl` on PATH, and drives them over fixtures.
#
# The stub models the one kubectl behaviour the gate depends on: `-l k=v`
# filters by the label on the object being listed. That is what makes the
# no-selector pod lookup testable — reinstate the selector and F1 goes red.
#
# The fixtures are hand-written kubectl JSON. They encode what we believe the
# API returns, not a capture from the cluster; keep them minimal so a shape
# that turns out to be wrong is cheap to correct.
set -euo pipefail

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH='' cd -- "${SCRIPT_DIR}/../.." && pwd)
WORKFLOW="${REPO_ROOT}/.github/workflows/deploy-staging.yml"

TMP_DIR=$(mktemp -d)
trap 'rm -rf "${TMP_DIR}"' EXIT HUP INT TERM

failures=0

fail() {
    echo "tenant-landed-gate: $*" >&2
    failures=$((failures + 1))
}

# Pull one shell function out of the workflow heredoc. Anchored on the
# definition line and closed by the first `}` at that same indentation, which
# is why the gate's functions are defined at the heredoc's own margin.
extract_function() {
    local name=$1 out=$2
    awk -v name="${name}" '
        !f && $0 ~ "^ *" name "\\(\\) \\{$" {
            f = 1
            match($0, /^ */)
            indent = substr($0, 1, RLENGTH)
            print substr($0, length(indent) + 1)
            next
        }
        f {
            print substr($0, length(indent) + 1)
            if ($0 == indent "}") exit
        }
    ' "${WORKFLOW}" >"${out}"
    [ -s "${out}" ] || { echo "tenant-landed-gate: could not extract ${name}()" >&2; exit 1; }
    bash -n "${out}" || { echo "tenant-landed-gate: extracted ${name}() does not parse" >&2; exit 1; }
}

GATE="${TMP_DIR}/gate.sh"
: >"${GATE}"
for fn in enumerate_tenant_statefulsets refresh_tenant_images report_tenant_landed_state; do
    extract_function "${fn}" "${TMP_DIR}/${fn}.sh"
    cat "${TMP_DIR}/${fn}.sh" >>"${GATE}"
    printf '\n' >>"${GATE}"
done

if command -v shellcheck >/dev/null 2>&1; then
    shellcheck -s bash -e SC2154 "${GATE}" \
        || fail "shellcheck rejected the extracted gate functions"
else
    echo "tenant-landed-gate: shellcheck not installed, skipping lint of the extracted gate" >&2
fi

mkdir -p "${TMP_DIR}/bin"
cat >"${TMP_DIR}/bin/kubectl" <<'STUB'
#!/usr/bin/env bash
set -u
printf '%s\n' "$*" >>"${KUBECTL_LOG}"

ns=
selector=
rest=()
while [ "$#" -gt 0 ]; do
    case "$1" in
        -n) ns=$2; shift 2 ;;
        -l) selector=$2; shift 2 ;;
        *) rest+=("$1"); shift ;;
    esac
done

case "${rest[0]:-}:${rest[1]:-}" in
    get:statefulset) src="${FIXTURE_DIR}/statefulsets.json" ;;
    get:pod) src="${FIXTURE_DIR}/pods-${ns}.json" ;;
    rollout:status)
        target=${rest[2]#statefulset/}
        if [ -f "${FIXTURE_DIR}/rollout-${ns}-${target}.fail" ]; then
            cat "${FIXTURE_DIR}/rollout-${ns}-${target}.fail" >&2
            exit 1
        fi
        echo "statefulset rolling update complete"
        exit 0
        ;;
    set:image) exit 0 ;;
    *) echo "stub kubectl: unhandled ${rest[*]:-<none>}" >&2; exit 64 ;;
esac

if [ ! -f "${src}" ]; then
    echo "Error from server (NotFound): no fixture at ${src}" >&2
    exit 1
fi

if [ -n "${selector}" ]; then
    jq --arg k "${selector%%=*}" --arg v "${selector#*=}" \
        '.items |= map(select(.metadata.labels[$k]? == $v))' "${src}"
else
    cat "${src}"
fi
STUB
chmod +x "${TMP_DIR}/bin/kubectl"

statefulset() {
    local ns=$1 name=$2 replicas=$3 container=$4
    jq -n --arg ns "${ns}" --arg name "${name}" \
        --argjson replicas "${replicas}" --arg container "${container}" '{
        metadata: {namespace: $ns, name: $name, labels: {app: "opencompany"}},
        spec: {
            replicas: $replicas,
            template: {spec: {containers: [{name: $container}]}}
        }
    }'
}

pod() {
    local ns=$1 name=$2 owner=$3 app_label=$4 phase=$5 container=$6 image=$7 reason=${8:-}
    jq -n --arg ns "${ns}" --arg name "${name}" --arg owner "${owner}" \
        --arg app "${app_label}" --arg phase "${phase}" \
        --arg container "${container}" --arg image "${image}" --arg reason "${reason}" '{
        metadata: {
            namespace: $ns,
            name: $name,
            labels: {app: $app},
            ownerReferences: [{kind: "StatefulSet", name: $owner}]
        },
        status: {
            phase: $phase,
            containerStatuses: [
                {name: $container, image: $image}
                + (if $reason == "" then {} else {state: {waiting: {reason: $reason}}} end)
            ]
        }
    }'
}

as_list() {
    jq -s '{apiVersion: "v1", kind: "List", items: .}'
}

# Run the gate over a fixture directory. Echoes the gate's own stdout; the
# caller asserts on that and on the exit status.
run_gate() {
    local fixture_dir=$1 refresh=$2
    (
        set +e
        PATH="${TMP_DIR}/bin:${PATH}" \
        FIXTURE_DIR="${fixture_dir}" \
        KUBECTL_LOG="${fixture_dir}/kubectl.log" \
        IMAGE="${IMAGE_UNDER_TEST}" \
        REFRESH_ALL="${refresh}" \
            bash -c '
                set -euo pipefail
                . "$1"
                enumerate_tenant_statefulsets
                if [ "${REFRESH_ALL}" = "true" ]; then
                    refresh_tenant_images
                fi
                report_tenant_landed_state
            ' _ "${GATE}" 2>&1
        echo "GATE_EXIT=$?"
    )
}

gate_exit() {
    printf '%s\n' "$1" | sed -n 's/^GATE_EXIT=//p' | tail -1
}

expect_exit() {
    local label=$1 want=$2 output=$3
    local got
    got=$(gate_exit "${output}")
    [ "${got}" = "${want}" ] || {
        fail "${label}: expected exit ${want}, got ${got}"
        printf '%s\n' "${output}" >&2
    }
}

expect_contains() {
    local label=$1 needle=$2 output=$3
    printf '%s\n' "${output}" | grep -qF -- "${needle}" || {
        fail "${label}: expected output to contain '${needle}'"
        printf '%s\n' "${output}" >&2
    }
}

expect_absent() {
    local label=$1 needle=$2 output=$3
    if printf '%s\n' "${output}" | grep -qF -- "${needle}"; then
        fail "${label}: output unexpectedly contains '${needle}'"
        printf '%s\n' "${output}" >&2
    fi
}

IMAGE_UNDER_TEST='registry.example/opencompany-tenant:staging-abc123'
OLD_IMAGE='registry.example/opencompany-tenant:staging-000000'
SIDECAR_IMAGE='registry.example/tinycortex:1.2.3'

new_fixture() {
    local name=$1
    local dir="${TMP_DIR}/${name}"
    mkdir -p "${dir}"
    printf '%s' "${dir}"
}

# F1 — the reported regression. Two labelled StatefulSets in one namespace: the
# idle app, and the manager's cortexdb sidecar whose POD is labelled
# `app=cortexdb`. Attribution is by ownerReferences, so the sidecar's pod is
# found and the fleet is healthy.
f1=$(new_fixture f1)
{
    statefulset demo opencompany 0 opencompany
    statefulset demo cortexdb 1 tinycortex
} | as_list >"${f1}/statefulsets.json"
pod demo cortexdb-0 cortexdb cortexdb Running tinycortex "${SIDECAR_IMAGE}" \
    | as_list >"${f1}/pods-demo.json"

out=$(run_gate "${f1}" false)
expect_exit "F1 mixed fleet" 0 "${out}"
expect_contains "F1 mixed fleet" "scaled to zero" "${out}"
expect_contains "F1 mixed fleet" "pod cortexdb-0 is Running" "${out}"
expect_absent "F1 mixed fleet" "no pod belongs to this StatefulSet" "${out}"

# F2 — replicas, but nothing owns a pod. The sentinel the gate exists for.
f2=$(new_fixture f2)
statefulset demo opencompany 1 opencompany | as_list >"${f2}/statefulsets.json"
pod demo unrelated-0 something-else opencompany Running opencompany "${IMAGE_UNDER_TEST}" \
    | as_list >"${f2}/pods-demo.json"

out=$(run_gate "${f2}" false)
expect_exit "F2 no owned pod" 1 "${out}"
expect_contains "F2 no owned pod" "no pod belongs to this StatefulSet" "${out}"

# F3 — the pod exists but never started.
f3=$(new_fixture f3)
statefulset demo opencompany 1 opencompany | as_list >"${f3}/statefulsets.json"
pod demo opencompany-0 opencompany opencompany Pending opencompany "${IMAGE_UNDER_TEST}" ImagePullBackOff \
    | as_list >"${f3}/pods-demo.json"

out=$(run_gate "${f3}" false)
expect_exit "F3 ImagePullBackOff" 1 "${out}"
expect_contains "F3 ImagePullBackOff" "phase Pending" "${out}"
expect_contains "F3 ImagePullBackOff" "ImagePullBackOff" "${out}"

# F4 — a refresh that did not take. The whole point of refresh_all_tenants.
f4=$(new_fixture f4)
statefulset demo opencompany 1 opencompany | as_list >"${f4}/statefulsets.json"
pod demo opencompany-0 opencompany opencompany Running opencompany "${OLD_IMAGE}" \
    | as_list >"${f4}/pods-demo.json"

out=$(run_gate "${f4}" true)
expect_exit "F4 stale app image" 1 "${out}"
expect_contains "F4 stale app image" "pod did not land on the requested image" "${out}"

# F5 — the same mismatch on a sidecar is not a failure: this pipeline neither
# builds nor pushes the image that row runs.
f5=$(new_fixture f5)
{
    statefulset demo opencompany 1 opencompany
    statefulset demo cortexdb 1 tinycortex
} | as_list >"${f5}/statefulsets.json"
{
    pod demo opencompany-0 opencompany opencompany Running opencompany "${IMAGE_UNDER_TEST}"
    pod demo cortexdb-0 cortexdb cortexdb Running tinycortex "${SIDECAR_IMAGE}"
} | as_list >"${f5}/pods-demo.json"

out=$(run_gate "${f5}" true)
expect_exit "F5 sidecar image ignored" 0 "${out}"
expect_absent "F5 sidecar image ignored" "pod did not land on the requested image" "${out}"

# F6 — the anti-disarm guard. A fleet with rows but no app container means the
# refresh patches nothing and the image check asserts nothing.
f6=$(new_fixture f6)
statefulset demo cortexdb 1 tinycortex | as_list >"${f6}/statefulsets.json"
pod demo cortexdb-0 cortexdb cortexdb Running tinycortex "${SIDECAR_IMAGE}" \
    | as_list >"${f6}/pods-demo.json"

out=$(run_gate "${f6}" true)
expect_exit "F6 disarmed refresh" 1 "${out}"
expect_contains "F6 disarmed refresh" \
    "no tenant StatefulSet has a container named opencompany" "${out}"

# F7 — an empty fleet is the known-good state, not a failure.
f7=$(new_fixture f7)
jq -n '{apiVersion: "v1", kind: "List", items: []}' >"${f7}/statefulsets.json"

out=$(run_gate "${f7}" false)
expect_exit "F7 empty fleet" 0 "${out}"
expect_contains "F7 empty fleet" "Tenant landed-state report" "${out}"
expect_absent "F7 empty fleet" "failed" "${out}"

# F8 — what the gate actually asked kubectl to do. `set image` names a
# container the sidecar does not have, so firing it there would abort the
# refresh for the whole fleet under `set -e`.
f8=$(new_fixture f8)
{
    statefulset demo opencompany 1 opencompany
    statefulset demo cortexdb 1 tinycortex
} | as_list >"${f8}/statefulsets.json"
{
    pod demo opencompany-0 opencompany opencompany Running opencompany "${IMAGE_UNDER_TEST}"
    pod demo cortexdb-0 cortexdb cortexdb Running tinycortex "${SIDECAR_IMAGE}"
} | as_list >"${f8}/pods-demo.json"

out=$(run_gate "${f8}" true)
expect_exit "F8 patch targets" 0 "${out}"
log=$(cat "${f8}/kubectl.log")
expect_contains "F8 patch targets" \
    "set image statefulset/opencompany opencompany=${IMAGE_UNDER_TEST}" "${log}"
expect_absent "F8 patch targets" "set image statefulset/cortexdb" "${log}"
expect_absent "F8 patch targets" "get pod -l" "${log}"

# F9 — a sidecar (no opencompany container) whose pod stays phase Running
# while its own container crash-loops. The jq record's image field is empty
# for this row (only the opencompany container's image is captured), which is
# the case that exposed a tab-collapsing IFS read: a $'\t'-joined record with
# an empty middle field parses one column short and the reason lands in the
# image slot, leaving the reason check nothing to see.
f9=$(new_fixture f9)
statefulset demo cortexdb 1 tinycortex | as_list >"${f9}/statefulsets.json"
pod demo cortexdb-0 cortexdb cortexdb Running tinycortex "${SIDECAR_IMAGE}" CrashLoopBackOff \
    | as_list >"${f9}/pods-demo.json"

out=$(run_gate "${f9}" false)
expect_exit "F9 sidecar crash loop" 1 "${out}"
expected_row=$(printf '%-24s %-24s %-8s %-90s %s' \
    demo cortexdb failed - "pod cortexdb-0: CrashLoopBackOff")
expect_contains "F9 sidecar crash loop" "${expected_row}" "${out}"

if [ "${failures}" -ne 0 ]; then
    echo "tenant-landed-gate: ${failures} check(s) failed" >&2
    exit 1
fi

echo "tenant-landed-gate tests passed"
