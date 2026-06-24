#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
transport="all"

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
    --help|-h)
      cat <<'USAGE'
Usage: tests/smoke/installed_codex_mock.sh [--transport http-sse|websocket|all]
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

export PATH="${HOME}/.cargo/bin:${PATH}"

cd "${repo_root}"

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

"${cargo_command[@]}" test \
  -p codex-router-test-support \
  "${test_filter}" \
  -- \
  --ignored \
  --nocapture \
  --test-threads=1
