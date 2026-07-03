#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
scenario=""
router_root=""
client_count="3"
progress_threshold_ms="750"

usage() {
  cat <<'USAGE'
Usage:
  tests/smoke/proxy_db_runtime_isolation.sh \
    --scenario websocket-sqlite-pressure \
    --router-root "$PWD/tmp/dev-state/codex-router" \
    --client-count 3 \
    --progress-threshold-ms 750

Required environment:
  CODEX_HOME must point under repo tmp/dev-state/codex
  HOME must point under repo tmp/dev-state/sentinel-home

The harness refuses production/default DB roots and writes its receipt under
repo-local tmp/smoke.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --scenario)
      [[ $# -ge 2 ]] || {
        echo "--scenario requires a value" >&2
        exit 2
      }
      scenario="$2"
      shift 2
      ;;
    --router-root)
      [[ $# -ge 2 ]] || {
        echo "--router-root requires a value" >&2
        exit 2
      }
      router_root="$2"
      shift 2
      ;;
    --client-count)
      [[ $# -ge 2 ]] || {
        echo "--client-count requires a value" >&2
        exit 2
      }
      client_count="$2"
      shift 2
      ;;
    --progress-threshold-ms)
      [[ $# -ge 2 ]] || {
        echo "--progress-threshold-ms requires a value" >&2
        exit 2
      }
      progress_threshold_ms="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

canonical_path() {
  python3 - "$1" <<'PY'
import os
import sys

print(os.path.realpath(sys.argv[1]))
PY
}

require_under_dev_state() {
  local label="$1"
  local raw_path="$2"
  local resolved_path
  resolved_path="$(canonical_path "$raw_path")"
  case "$resolved_path" in
    "$dev_state_root"/*) printf '%s\n' "$resolved_path" ;;
    *)
      echo "refusing ${label} outside repo tmp/dev-state: ${raw_path} -> ${resolved_path}" >&2
      exit 2
      ;;
  esac
}

receipt_path_value() {
  local raw_path="$1"
  python3 - "${repo_root}" "$raw_path" <<'PY'
import os
import sys

repo_root = os.path.realpath(sys.argv[1])
resolved_path = os.path.realpath(sys.argv[2])
try:
    relative = os.path.relpath(resolved_path, repo_root)
except ValueError:
    print("<external>")
    raise SystemExit(0)
if relative == ".":
    print("<repo>")
elif relative.startswith(".."):
    print("<external>")
else:
    print(f"<repo>/{relative}")
PY
}

if [[ "${scenario}" != "websocket-sqlite-pressure" ]]; then
  echo "--scenario must be websocket-sqlite-pressure" >&2
  exit 2
fi

if ! [[ "${client_count}" =~ ^[0-9]+$ ]] || [[ "${client_count}" -lt 3 ]]; then
  echo "--client-count must be an integer >= 3" >&2
  exit 2
fi

if [[ "${client_count}" -ne 3 ]]; then
  echo "websocket-sqlite-pressure currently supports exactly 3 clients" >&2
  exit 2
fi

if ! [[ "${progress_threshold_ms}" =~ ^[0-9]+$ ]] || [[ "${progress_threshold_ms}" -le 0 ]]; then
  echo "--progress-threshold-ms must be a positive integer" >&2
  exit 2
fi

if [[ -z "${router_root}" ]]; then
  echo "--router-root is required" >&2
  exit 2
fi

if [[ -z "${CODEX_HOME:-}" ]]; then
  echo "CODEX_HOME must be set to repo tmp/dev-state/codex" >&2
  exit 2
fi

if [[ -z "${HOME:-}" ]]; then
  echo "HOME must be set to repo tmp/dev-state/sentinel-home" >&2
  exit 2
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required for path validation" >&2
  exit 127
fi

if [[ -L "${repo_root}/tmp" ]]; then
  echo "refusing smoke under symlinked repo tmp/: ${repo_root}/tmp" >&2
  exit 2
fi

tmp_root="$(canonical_path "${repo_root}/tmp")"
dev_state_root="${tmp_root}/dev-state"
router_root_resolved="$(require_under_dev_state "router root" "${router_root}")"
codex_home_resolved="$(require_under_dev_state "CODEX_HOME" "${CODEX_HOME}")"
home_resolved="$(require_under_dev_state "HOME" "${HOME}")"

if [[ "${router_root_resolved}" != "${dev_state_root}/codex-router" ]]; then
  echo "router root must be ${dev_state_root}/codex-router, got ${router_root_resolved}" >&2
  exit 2
fi

if [[ "${codex_home_resolved}" != "${dev_state_root}/codex" ]]; then
  echo "CODEX_HOME must be ${dev_state_root}/codex, got ${codex_home_resolved}" >&2
  exit 2
fi

if [[ "${home_resolved}" != "${dev_state_root}/sentinel-home" ]]; then
  echo "HOME must be ${dev_state_root}/sentinel-home, got ${home_resolved}" >&2
  exit 2
fi

router_db="${router_root_resolved}/state.sqlite"
codex_db="${codex_home_resolved}/state_5.sqlite"
router_root_receipt="$(receipt_path_value "${router_root_resolved}")"
router_db_receipt="$(receipt_path_value "${router_db}")"
codex_home_receipt="$(receipt_path_value "${codex_home_resolved}")"
codex_db_receipt="$(receipt_path_value "${codex_db}")"
home_receipt="$(receipt_path_value "${home_resolved}")"
if [[ ! -f "${router_db}" ]]; then
  echo "copied router DB is missing: ${router_db}" >&2
  exit 2
fi
if [[ ! -f "${codex_db}" ]]; then
  echo "copied Codex DB is missing: ${codex_db}" >&2
  exit 2
fi

if [[ -e "${home_resolved}/.codex-router/state.sqlite" || -e "${home_resolved}/.codex/state_5.sqlite" ]]; then
  echo "sentinel HOME already contains default production-style DB paths" >&2
  exit 2
fi

mkdir -p "${repo_root}/tmp/smoke"
receipt_path="$(
  mktemp "${repo_root}/tmp/smoke/proxy-db-runtime-isolation-websocket-sqlite-pressure.receipt.XXXXXX"
)"
artifact_pointer="${repo_root}/tmp/smoke/installed-codex-s8-overlap-quota-artifact.txt"
s8_run_id="proxy-db-runtime-isolation-$RANDOM-$(date -u +%Y%m%dT%H%M%SZ)"
rm -f "${artifact_pointer}"

soak_seconds=$(( (progress_threshold_ms + 999) / 1000 ))
if [[ "${soak_seconds}" -lt 1 ]]; then
  soak_seconds=1
fi

started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
CODEX_ROUTER_S8_RUN_ID="${s8_run_id}" \
CODEX_ROUTER_SOAK_SECONDS="${CODEX_ROUTER_SOAK_SECONDS:-${soak_seconds}}" \
  "${repo_root}/tests/smoke/installed_codex_mock.sh" \
    --transport websocket \
    --scenario s8-overlap-quota \
    --runtime-root-mode copied-dev-state \
    --router-root "${router_root_resolved}" \
    --codex-home "${codex_home_resolved}" \
    --process-home "${home_resolved}"
finished_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

if [[ ! -s "${artifact_pointer}" ]]; then
  echo "S8 overlap quota artifact pointer missing: ${artifact_pointer}" >&2
  exit 1
fi
artifact_path="$(cat "${artifact_pointer}")"
case "${artifact_path}" in
  /*) ;;
  *) artifact_path="${repo_root}/${artifact_path}" ;;
esac
artifact_validation_path="$(
  mktemp "${repo_root}/tmp/smoke/proxy-db-runtime-isolation-artifact-validation.XXXXXX"
)"
set +e
python3 "${repo_root}/scripts/validate-proxy-db-runtime-isolation-artifact.py" \
  "${artifact_path}" \
  "${repo_root}" \
  "${router_root_resolved}" \
  "${codex_home_resolved}" \
  "${home_resolved}" \
  "${progress_threshold_ms}" > "${artifact_validation_path}"
artifact_validation_status=$?
set -e
if [[ "${artifact_validation_status}" -ne 0 ]]; then
  {
    printf 'status=BLOCKED\n'
    printf 'scenario=%s\n' "${scenario}"
    printf 'started_at=%s\n' "${started_at}"
    printf 'finished_at=%s\n' "${finished_at}"
    printf 's8_run_id=%s\n' "${s8_run_id}"
    printf 'router_root=%s\n' "${router_root_receipt}"
    printf 'router_db=%s\n' "${router_db_receipt}"
    printf 'codex_home=%s\n' "${codex_home_receipt}"
    printf 'codex_db=%s\n' "${codex_db_receipt}"
    printf 'sentinel_home=%s\n' "${home_receipt}"
    printf 'client_count=%s\n' "${client_count}"
    printf 'progress_threshold_ms=%s\n' "${progress_threshold_ms}"
    cat "${artifact_validation_path}"
    printf 'scrubbed_signal_log_path=%s\n' "$(receipt_path_value "${artifact_path}")"
    printf 's8_overlap_quota_artifact=%s\n' "$(receipt_path_value "${artifact_path}")"
  } > "${receipt_path}"
  printf 'proxy DB runtime isolation smoke blocked: %s\n' "${receipt_path}" >&2
  exit 1
fi

if [[ -e "${home_resolved}/.codex-router/state.sqlite" || -e "${home_resolved}/.codex/state_5.sqlite" ]]; then
  echo "smoke created production-style DB paths under sentinel HOME" >&2
  exit 1
fi

{
  printf 'scenario=%s\n' "${scenario}"
  printf 'started_at=%s\n' "${started_at}"
  printf 'finished_at=%s\n' "${finished_at}"
  printf 's8_run_id=%s\n' "${s8_run_id}"
  printf 'router_root=%s\n' "${router_root_receipt}"
  printf 'router_db=%s\n' "${router_db_receipt}"
  printf 'codex_home=%s\n' "${codex_home_receipt}"
  printf 'codex_db=%s\n' "${codex_db_receipt}"
  printf 'sentinel_home=%s\n' "${home_receipt}"
  printf 'client_count=%s\n' "${client_count}"
  printf 'progress_threshold_ms=%s\n' "${progress_threshold_ms}"
  cat "${artifact_validation_path}"
  printf 'scrubbed_signal_log_path=%s\n' "$(receipt_path_value "${artifact_path}")"
  printf 's8_overlap_quota_artifact=%s\n' "$(receipt_path_value "${artifact_path}")"
  printf 'artifact=%s\n' "$(receipt_path_value "${artifact_path}")"
} > "${receipt_path}"

printf 'proxy DB runtime isolation smoke ok: %s\n' "${receipt_path}"
