#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
transport="all"
scenario="serial"

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
        echo "--scenario requires one of: serial, concurrent, soak, all" >&2
        exit 2
      fi
      scenario="$2"
      shift 2
      ;;
    --help|-h)
      cat <<'USAGE'
Usage: tests/smoke/installed_codex_mock.sh [--transport http-sse|websocket|all] [--scenario serial|concurrent|soak|all]
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
  serial|concurrent|soak|all)
    ;;
  *)
    echo "--scenario must be one of: serial, concurrent, soak, all" >&2
    exit 2
    ;;
esac

if [[ "${scenario}" =~ ^(concurrent|soak)$ && "${transport}" != "websocket" ]]; then
  echo "--scenario ${scenario} requires --transport websocket" >&2
  exit 2
fi

export PATH="${HOME}/.cargo/bin:${PATH}"

cd "${repo_root}"
three_websocket_soak_artifact_pointer="${repo_root}/tmp/smoke/installed-codex-three-websocket-soak-artifact.txt"

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
  if run_test_filter "${filter}" | tee "${output_file}"; then
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

  local status=$?
  rm -f "${output_file}"
  return "${status}"
}

if [[ "${scenario}" == "concurrent" ]]; then
  run_test_filter "three_codex_websocket_concurrent_e2e_"
elif [[ "${scenario}" == "soak" ]]; then
  run_three_websocket_soak_filter "three_codex_websocket_soak_"
elif [[ "${scenario}" == "all" && "${transport}" == "websocket" ]]; then
  run_test_filter "installed_codex_websocket_"
  run_test_filter "three_codex_websocket_concurrent_e2e_"
  run_three_websocket_soak_filter "three_codex_websocket_soak_"
else
  run_test_filter "${test_filter}"
fi
