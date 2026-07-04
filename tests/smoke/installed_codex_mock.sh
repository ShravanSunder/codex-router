#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
transport="all"
scenario="serial"
smoke_target_model="gpt-5.4-mini"
smoke_prompt_contract="bounded explicit exact-reply prompt"
runtime_root_mode="isolated-temp"
router_root=""
codex_home=""
process_home=""

require_copied_root_under_dev_state() {
  local label="$1"
  local candidate="$2"
  python3 -c '
import os
import sys

repo_root, label, candidate = sys.argv[1:4]
dev_state = os.path.realpath(os.path.join(repo_root, "tmp", "dev-state"))
if os.path.islink(candidate):
    print(
        f"copied-dev-state {label} must not be a symlink; got {candidate}",
        file=sys.stderr,
    )
    raise SystemExit(2)
resolved = os.path.realpath(candidate)
try:
    under_dev_state = os.path.commonpath([resolved, dev_state]) == dev_state
except ValueError:
    under_dev_state = False
if not under_dev_state:
    print(
        f"copied-dev-state {label} must be under repo-local tmp/dev-state; got {resolved}",
        file=sys.stderr,
    )
    raise SystemExit(2)
' "$repo_root" "$label" "$candidate"
}

repo_relative_path() {
  local candidate="$1"
  python3 -c '
import os
import sys

repo_root, candidate = sys.argv[1:3]
resolved = os.path.realpath(candidate)
repo = os.path.realpath(repo_root)
try:
    relative = os.path.relpath(resolved, repo)
except ValueError:
    print(resolved)
    raise SystemExit(0)
if relative.startswith(".."):
    print(resolved)
else:
    print(relative)
' "$repo_root" "$candidate"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --transport)
      if [[ $# -lt 2 ]]; then
        echo "--transport requires one of: http-sse, websocket, all" >&2
        exit 2
      fi
      transport="$2"
      shift 2
      ;;
    --scenario)
      if [[ $# -lt 2 ]]; then
        echo "--scenario requires one of: serial, concurrent, quota-reconnect, s8-overlap-quota, soak, all" >&2
        exit 2
      fi
      scenario="$2"
      shift 2
      ;;
    --runtime-root-mode)
      if [[ $# -lt 2 ]]; then
        echo "--runtime-root-mode requires one of: isolated-temp, copied-dev-state" >&2
        exit 2
      fi
      runtime_root_mode="$2"
      shift 2
      ;;
    --router-root)
      if [[ $# -lt 2 ]]; then
        echo "--router-root requires a value" >&2
        exit 2
      fi
      router_root="$2"
      shift 2
      ;;
    --codex-home)
      if [[ $# -lt 2 ]]; then
        echo "--codex-home requires a value" >&2
        exit 2
      fi
      codex_home="$2"
      shift 2
      ;;
    --process-home)
      if [[ $# -lt 2 ]]; then
        echo "--process-home requires a value" >&2
        exit 2
      fi
      process_home="$2"
      shift 2
      ;;
    --help|-h)
      cat <<'USAGE'
Usage: tests/smoke/installed_codex_mock.sh [--transport http-sse|websocket|all] [--scenario serial|concurrent|quota-reconnect|s8-overlap-quota|soak|all]
       [--runtime-root-mode isolated-temp|copied-dev-state]
       [--router-root PATH --codex-home PATH --process-home PATH]

Installed Codex smoke contract:
  - uses the existing codex CLI from PATH; it does not install Codex
  - targets the cheap mini model gpt-5.4-mini
  - concurrent and soak scenarios run three Codex client jobs at once
  - quota-reconnect proves provider quota exhaustion reconnects onto another account
  - s8-overlap-quota proves quota reconnect during a three-client overlap
  - prompts are bounded exact-reply instructions, with harness markers only
USAGE
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

case "${transport}" in
  http-sse)
    test_filter="installed_codex_http_sse_"
    ;;
  websocket)
    test_filter="installed_codex_websocket_"
    ;;
  all)
    test_filter="installed_codex_"
    ;;
  *)
    echo "--transport must be one of: http-sse, websocket, all" >&2
    exit 2
    ;;
esac

case "${scenario}" in
  serial|concurrent|quota-reconnect|s8-overlap-quota|soak|all)
    ;;
  *)
    echo "--scenario must be one of: serial, concurrent, quota-reconnect, s8-overlap-quota, soak, all" >&2
    exit 2
    ;;
esac

case "${runtime_root_mode}" in
  isolated-temp)
    ;;
  copied-dev-state)
    if [[ -z "${router_root}" || -z "${codex_home}" || -z "${process_home}" ]]; then
      echo "--runtime-root-mode copied-dev-state requires --router-root, --codex-home, and --process-home" >&2
      exit 2
    fi
    require_copied_root_under_dev_state "router root" "${router_root}"
    require_copied_root_under_dev_state "Codex home" "${codex_home}"
    require_copied_root_under_dev_state "process HOME" "${process_home}"
    ;;
  *)
    echo "--runtime-root-mode must be one of: isolated-temp, copied-dev-state" >&2
    exit 2
    ;;
esac

if [[ "${scenario}" =~ ^(concurrent|quota-reconnect|s8-overlap-quota|soak)$ && "${transport}" != "websocket" ]]; then
  echo "--scenario ${scenario} requires --transport websocket" >&2
  exit 2
fi

export CODEX_ROUTER_INSTALLED_SMOKE_RUNTIME_ROOT_MODE="${runtime_root_mode}"
export CODEX_ROUTER_S8_RUN_ID="${CODEX_ROUTER_S8_RUN_ID:-}"
if [[ "${runtime_root_mode}" == "copied-dev-state" ]]; then
  export CODEX_ROUTER_INSTALLED_SMOKE_ROUTER_ROOT="${router_root}"
  export CODEX_ROUTER_INSTALLED_SMOKE_CODEX_HOME="${codex_home}"
  export CODEX_ROUTER_INSTALLED_SMOKE_PROCESS_HOME="${process_home}"
else
  unset CODEX_ROUTER_INSTALLED_SMOKE_ROUTER_ROOT
  unset CODEX_ROUTER_INSTALLED_SMOKE_CODEX_HOME
  unset CODEX_ROUTER_INSTALLED_SMOKE_PROCESS_HOME
fi

export PATH="${HOME}/.cargo/bin:${PATH}"

cd "${repo_root}"
three_websocket_soak_artifact_pointer="${repo_root}/tmp/smoke/installed-codex-three-websocket-soak-artifact.txt"
quota_reconnect_artifact_pointer="${repo_root}/tmp/smoke/installed-codex-quota-reconnect-artifact.txt"
s8_overlap_quota_artifact_pointer="${repo_root}/tmp/smoke/installed-codex-s8-overlap-quota-artifact.txt"
case "${scenario}" in
  concurrent|soak)
    smoke_client_summary="3 concurrent clients"
    ;;
  all)
    smoke_client_summary="serial + 3 concurrent clients + quota reconnect + soak"
    ;;
  quota-reconnect)
    smoke_client_summary="1 client with quota reconnect"
    ;;
  s8-overlap-quota)
    smoke_client_summary="3 concurrent clients with quota reconnect"
    ;;
  *)
    smoke_client_summary="1 client"
    ;;
esac

printf 'installed Codex smoke contract: model=%s clients=%s prompt=%s\n' \
  "${smoke_target_model}" \
  "${smoke_client_summary}" \
  "${smoke_prompt_contract}" >&2

if command -v cargo >/dev/null 2>&1 && cargo --version >/dev/null 2>&1; then
  cargo_command=(cargo)
elif command -v rustup >/dev/null 2>&1 && rustup run 1.95.0 cargo --version >/dev/null 2>&1; then
  cargo_command=(rustup run 1.95.0 cargo)
elif [[ -x "${HOME}/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin/cargo" ]]; then
  cargo_command=("${HOME}/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin/cargo")
else
  echo "cargo or rustup with toolchain 1.95.0 is required" >&2
  exit 127
fi

"${cargo_command[@]}" build -p codex-router-cli --bin codex-router

run_test_filter() {
  local filter="$1"
  "${cargo_command[@]}" test \
    -p codex-router-test-support \
    "${filter}" \
    -- \
    --ignored \
    --nocapture \
    --test-threads=1
}

run_three_websocket_soak_filter() {
  local filter="$1"
  mkdir -p "$(dirname "${three_websocket_soak_artifact_pointer}")"
  rm -f "${three_websocket_soak_artifact_pointer}"

  local output_file
  output_file="$(mktemp "${TMPDIR:-/tmp}/codex-router-three-websocket-soak.XXXXXX")"
  set +e
  run_test_filter "${filter}" | tee "${output_file}"
  local status=${PIPESTATUS[0]}
  set -e
  if [[ "${status}" -eq 0 ]]; then
    local artifact_path
    artifact_path="$(
      awk '/codex_router_three_websocket_artifact=/{sub(/^.*codex_router_three_websocket_artifact=/, ""); value=$0} END{print value}' "${output_file}"
    )"
    rm -f "${output_file}"
    if [[ -z "${artifact_path}" ]]; then
      echo "three-WebSocket soak did not print an artifact path" >&2
      return 1
    fi
    printf '%s\n' "${artifact_path}" > "${three_websocket_soak_artifact_pointer}"
    return 0
  fi

  rm -f "${output_file}"
  return "${status}"
}

run_quota_reconnect_filter() {
  local filter="$1"
  mkdir -p "$(dirname "${quota_reconnect_artifact_pointer}")"
  rm -f "${quota_reconnect_artifact_pointer}"

  local output_file
  output_file="$(mktemp "${TMPDIR:-/tmp}/codex-router-quota-reconnect.XXXXXX")"
  set +e
  run_test_filter "${filter}" | tee "${output_file}"
  local status=${PIPESTATUS[0]}
  set -e
  if [[ "${status}" -eq 0 ]]; then
    local artifact_path
    artifact_path="$(
      awk '/codex_router_quota_reconnect_artifact=/{sub(/^.*codex_router_quota_reconnect_artifact=/, ""); value=$0} END{print value}' "${output_file}"
    )"
    rm -f "${output_file}"
    if [[ -z "${artifact_path}" ]]; then
      echo "quota reconnect did not print an artifact path" >&2
      return 1
    fi
    repo_relative_path "${artifact_path}" > "${quota_reconnect_artifact_pointer}"
    return 0
  fi

  rm -f "${output_file}"
  return "${status}"
}

run_s8_overlap_quota_filter() {
  local filter="$1"
  mkdir -p "$(dirname "${s8_overlap_quota_artifact_pointer}")"
  rm -f "${s8_overlap_quota_artifact_pointer}"

  local output_file
  output_file="$(mktemp "${TMPDIR:-/tmp}/codex-router-s8-overlap-quota.XXXXXX")"
  set +e
  run_test_filter "${filter}" | tee "${output_file}"
  local status=${PIPESTATUS[0]}
  set -e
  if [[ "${status}" -eq 0 ]]; then
    local artifact_path
    artifact_path="$(
      awk '/codex_router_s8_overlap_quota_artifact=/{sub(/^.*codex_router_s8_overlap_quota_artifact=/, ""); value=$0} END{print value}' "${output_file}"
    )"
    rm -f "${output_file}"
    if [[ -z "${artifact_path}" ]]; then
      echo "S8 overlap quota did not print an artifact path" >&2
      return 1
    fi
    repo_relative_path "${artifact_path}" > "${s8_overlap_quota_artifact_pointer}"
    return 0
  fi

  rm -f "${output_file}"
  return "${status}"
}

if [[ "${scenario}" == "concurrent" ]]; then
  run_test_filter "three_codex_websocket_concurrent_e2e_"
elif [[ "${scenario}" == "quota-reconnect" ]]; then
  run_quota_reconnect_filter "installed_codex_websocket_quota_reconnect_"
elif [[ "${scenario}" == "s8-overlap-quota" ]]; then
  run_s8_overlap_quota_filter "installed_codex_websocket_s8_overlap_quota_"
elif [[ "${scenario}" == "soak" ]]; then
  run_three_websocket_soak_filter "three_codex_websocket_soak_"
elif [[ "${scenario}" == "all" && "${transport}" == "websocket" ]]; then
  run_test_filter "installed_codex_websocket_"
  run_test_filter "three_codex_websocket_concurrent_e2e_"
  run_quota_reconnect_filter "installed_codex_websocket_quota_reconnect_"
  run_three_websocket_soak_filter "three_codex_websocket_soak_"
else
  run_test_filter "${test_filter}"
fi
